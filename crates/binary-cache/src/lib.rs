#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![allow(clippy::missing_errors_doc)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use moka::future::Cache;
use nix_utils::BaseStore as _;
use nix_utils::RealisationOperations as _;
use object_store::ObjectStore as _;
use secrecy::ExposeSecret;

mod cfg;
mod compression;
mod narinfo;
mod streaming_hash;

pub use crate::cfg::{S3CacheConfig, S3ClientConfig, S3CredentialsConfig, S3Scheme};
pub use crate::compression::Compression;
pub use crate::narinfo::NarInfo;
use crate::narinfo::NarInfoError;

#[derive(Debug, Default)]
struct AtomicS3Stats {
    put: AtomicU64,
    put_bytes: AtomicU64,
    put_time_ms: AtomicU64,
    get: AtomicU64,
    get_bytes: AtomicU64,
    get_time_ms: AtomicU64,
    head: AtomicU64,
}

#[derive(Debug, Default)]
pub struct S3Stats {
    pub put: u64,
    pub put_bytes: u64,
    pub put_time_ms: u64,
    pub get: u64,
    pub get_bytes: u64,
    pub get_time_ms: u64,
    pub head: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("Object store error: {0}")]
    ObjectStore(#[from] object_store::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Signing error: {0}")]
    Signing(String),
    #[error(transparent)]
    NarInfoParseError(#[from] NarInfoError),
    #[error(transparent)]
    NixStoreError(#[from] nix_utils::Error),
    #[error("cannot add '{0}' to the binary cache because the reference '{1}' is not valid")]
    ReferenceVerifyError(nix_utils::StorePath, nix_utils::StorePath),
    #[error("Hash error: {0}")]
    HashingError(#[from] crate::streaming_hash::Error),
}

#[derive(Clone)]
pub struct S3BinaryCacheClient {
    s3: object_store::aws::AmazonS3,
    pub cfg: cfg::S3CacheConfig,
    s3_stats: Arc<AtomicS3Stats>,
    signing_keys: Vec<secrecy::SecretString>,
    narinfo_cache: Cache<nix_utils::StorePath, NarInfo>,
}

#[tracing::instrument(skip(stream, chunk), err)]
async fn read_chunk_async<S: tokio::io::AsyncRead + Unpin + Send>(
    stream: &mut S,
    mut chunk: bytes::BytesMut,
) -> std::io::Result<bytes::Bytes> {
    use tokio::io::AsyncReadExt as _;

    while chunk.len() < chunk.capacity() {
        let read = stream.read_buf(&mut chunk).await?;

        if read == 0 {
            break;
        }
    }

    Ok(chunk.freeze())
}

#[tracing::instrument(skip(upload_item, first_chunk, stream), err)]
async fn run_multipart_upload(
    upload_item: &mut Box<dyn object_store::MultipartUpload>,
    first_chunk: Bytes,
    mut stream: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
    buffer_size: usize,
) -> Result<usize, CacheError> {
    let mut part_number = 1;
    let mut first_chunk_opt = Some(first_chunk);
    let mut file_size = 0;

    loop {
        let chunk = if part_number == 1 {
            first_chunk_opt.take().unwrap()
        } else {
            let buf = bytes::BytesMut::with_capacity(buffer_size);
            read_chunk_async(&mut stream, buf).await?
        };
        file_size += chunk.len();

        if chunk.is_empty() {
            break;
        }

        log::debug!("Uploading part {} with size {}", part_number, chunk.len());
        upload_item.put_part(chunk.into()).await?;
        part_number += 1;
    }

    log::debug!(
        "Completing multipart upload with {} parts, total size: {}",
        part_number,
        file_size
    );
    upload_item.complete().await?;
    Ok(file_size)
}

impl S3BinaryCacheClient {
    fn construct_client(
        cfg: &cfg::S3ClientConfig,
    ) -> Result<object_store::aws::AmazonS3, object_store::Error> {
        let mut builder = object_store::aws::AmazonS3Builder::from_env()
            .with_region(&cfg.region)
            .with_bucket_name(&cfg.bucket)
            .with_imdsv1_fallback();

        if let Some(credentials) = &cfg.credentials {
            builder = builder
                .with_access_key_id(&credentials.access_key_id)
                .with_secret_access_key(&credentials.secret_access_key);
        } else if std::env::var("AWS_ACCESS_KEY_ID").ok().is_none()
            && std::env::var("AWS_SECRET_ACCESS_KEY").ok().is_none()
        {
            let profile = cfg.profile.as_deref().unwrap_or("default");
            if let Ok((access_key, secret_key)) = crate::cfg::read_aws_credentials_file(profile) {
                log::info!("Using AWS credentials from credentials file for profile: {profile}",);
                builder = builder
                    .with_access_key_id(&access_key)
                    .with_secret_access_key(&secret_key);
            } else {
                log::warn!(
                    "AWS credentials not found in environment variables or credentials file for profile: {profile}",
                );
            }
        }

        if let Some(endpoint) = &cfg.endpoint {
            builder = builder.with_endpoint(endpoint);
            builder = builder.with_virtual_hosted_style_request(false);
        }

        if cfg.scheme == cfg::S3Scheme::HTTP {
            builder = builder.with_allow_http(true);
        }

        builder.build()
    }

    #[tracing::instrument(skip(cfg), err)]
    pub async fn new(cfg: cfg::S3CacheConfig) -> Result<Self, CacheError> {
        let mut signing_keys = vec![];
        for p in &cfg.secret_key_files {
            signing_keys.push(secrecy::SecretString::new(
                tokio::fs::read_to_string(p).await?.into(),
            ));
        }

        Ok(Self {
            s3: Self::construct_client(&cfg.client_config)?,
            cfg,
            s3_stats: Arc::new(AtomicS3Stats::default()),
            signing_keys,
            narinfo_cache: Cache::builder()
                .time_to_live(Duration::from_secs(3600)) // 1 hour TTL
                .build(),
        })
    }

    #[must_use]
    pub fn s3_stats(&self) -> S3Stats {
        S3Stats {
            put: self.s3_stats.put.load(Ordering::Relaxed),
            put_bytes: self.s3_stats.put_bytes.load(Ordering::Relaxed),
            put_time_ms: self.s3_stats.put_time_ms.load(Ordering::Relaxed),
            get: self.s3_stats.get.load(Ordering::Relaxed),
            get_bytes: self.s3_stats.get_bytes.load(Ordering::Relaxed),
            get_time_ms: self.s3_stats.get_time_ms.load(Ordering::Relaxed),
            head: self.s3_stats.head.load(Ordering::Relaxed),
        }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn head_object(&self, key: &str) -> Result<bool, CacheError> {
        let res = self.s3.head(&object_store::path::Path::from(key)).await;
        self.s3_stats.head.fetch_add(1, Ordering::Relaxed);
        match res {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(CacheError::ObjectStore(e)),
        }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_object(&self, key: &str) -> Result<Option<Bytes>, CacheError> {
        let start = Instant::now();
        let get_result = match self.s3.get(&object_store::path::Path::from(key)).await {
            Ok(v) => v,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(CacheError::ObjectStore(e)),
        };
        let bs = get_result.bytes().await?;
        let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or_default();

        self.s3_stats.get.fetch_add(1, Ordering::Relaxed);
        self.s3_stats.get_bytes.fetch_add(
            u64::try_from(bs.len()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.s3_stats
            .get_time_ms
            .fetch_add(elapsed, Ordering::Relaxed);

        Ok(Some(bs))
    }

    #[tracing::instrument(skip(self, content, content_type), err)]
    pub async fn upsert_file(
        &self,
        name: &str,
        content: String,
        content_type: &str,
    ) -> Result<(), CacheError> {
        let stream = Box::new(std::io::Cursor::new(Bytes::from(content)));
        self.upsert_file_stream(name, stream, content_type).await
    }

    #[tracing::instrument(skip(self, stream, content_type), err)]
    pub async fn upsert_file_stream(
        &self,
        name: &str,
        mut stream: Box<(dyn tokio::io::AsyncBufRead + Unpin + Send)>,
        content_type: &str,
    ) -> Result<(), CacheError> {
        if name.starts_with("log/") {
            let compressor = self.cfg.log_compression.get_compression_fn(
                self.cfg.get_compression_level(),
                self.cfg.parallel_compression,
            );
            let mut stream = compressor(stream);
            self.upload_file(
                name,
                &mut stream,
                content_type,
                self.cfg.log_compression.content_encoding(),
            )
            .await
        } else if std::path::Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ls"))
        {
            let compressor = self.cfg.ls_compression.get_compression_fn(
                self.cfg.get_compression_level(),
                self.cfg.parallel_compression,
            );
            let mut stream = compressor(stream);
            self.upload_file(
                name,
                &mut stream,
                content_type,
                self.cfg.ls_compression.content_encoding(),
            )
            .await
        } else if std::path::Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("narinfo"))
        {
            let compressor = self.cfg.narinfo_compression.get_compression_fn(
                self.cfg.get_compression_level(),
                self.cfg.parallel_compression,
            );
            let mut stream = compressor(stream);
            self.upload_file(
                name,
                &mut stream,
                content_type,
                self.cfg.narinfo_compression.content_encoding(),
            )
            .await
        } else {
            self.upload_file(name, &mut stream, content_type, "").await
        }
    }

    #[tracing::instrument(skip(self, stream, content_type), err)]
    async fn upload_file(
        &self,
        name: &str,
        mut stream: &mut (dyn tokio::io::AsyncRead + Unpin + Send),
        content_type: &str,
        content_encoding: &str,
    ) -> Result<(), CacheError> {
        let start = Instant::now();
        let buf = bytes::BytesMut::with_capacity(self.cfg.buffer_size);
        let first_chunk = read_chunk_async(&mut stream, buf).await?;
        let first_chunk_len = first_chunk.len();

        if first_chunk_len < self.cfg.buffer_size {
            self.s3
                .put_opts(
                    &object_store::path::Path::from(name),
                    object_store::PutPayload::from_bytes(first_chunk.clone()),
                    object_store::PutOptions {
                        attributes: {
                            let mut attrs = object_store::Attributes::new();
                            attrs.insert(
                                object_store::Attribute::ContentType,
                                content_type.to_owned().into(),
                            );
                            if !content_encoding.is_empty() {
                                attrs.insert(
                                    object_store::Attribute::ContentEncoding,
                                    content_encoding.to_owned().into(),
                                );
                            }
                            attrs
                        },
                        ..Default::default()
                    },
                )
                .await?;

            log::debug!("put_object for small file -> done, size: {first_chunk_len}",);

            let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or_default();
            self.s3_stats
                .put_time_ms
                .fetch_add(elapsed, Ordering::Relaxed);
            self.s3_stats.put.fetch_add(1, Ordering::Relaxed);
            self.s3_stats
                .put_bytes
                .fetch_add(first_chunk_len as u64, Ordering::Relaxed);

            return Ok(());
        }

        log::debug!(
            "Starting multipart upload for large file, first chunk size: {}",
            first_chunk_len
        );
        let mut multipart_upload = self
            .s3
            .put_multipart_opts(
                &object_store::path::Path::from(name),
                object_store::PutMultipartOptions {
                    attributes: {
                        let mut attrs = object_store::Attributes::new();
                        attrs.insert(
                            object_store::Attribute::ContentType,
                            content_type.to_owned().into(),
                        );
                        if !content_encoding.is_empty() {
                            attrs.insert(
                                object_store::Attribute::ContentEncoding,
                                content_encoding.to_owned().into(),
                            );
                        }
                        attrs
                    },
                    ..Default::default()
                },
            )
            .await?;
        match run_multipart_upload(
            &mut multipart_upload,
            first_chunk,
            stream,
            self.cfg.buffer_size,
        )
        .await
        {
            Ok(file_size) => {
                let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or_default();
                self.s3_stats
                    .put_time_ms
                    .fetch_add(elapsed, Ordering::Relaxed);
                self.s3_stats.put.fetch_add(1, Ordering::Relaxed);
                self.s3_stats
                    .put_bytes
                    .fetch_add(file_size as u64, Ordering::Relaxed);
            }
            Err(e) => {
                log::warn!("Upload was interrupted - Aborting multipart upload: {e}");

                if let Err(e) = multipart_upload.abort().await {
                    log::warn!("Failed to abort multipart upload: {e}");
                }
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip(self, listing), err)]
    async fn upload_listing(
        &self,
        path: &nix_utils::StorePath,
        listing: String,
    ) -> Result<String, CacheError> {
        let base = path.hash_part();
        let info_key = format!("{base}.ls");
        self.upsert_file(&info_key, listing, "application/json")
            .await?;
        Ok(info_key)
    }

    #[tracing::instrument(skip(self, narinfo), err)]
    async fn upload_narinfo(&self, narinfo: NarInfo) -> Result<String, CacheError> {
        let base = narinfo.store_path.hash_part();
        let info_key = format!("{base}.narinfo");
        self.upsert_file(&info_key, narinfo.to_string(), "text/x-nix-narinfo")
            .await?;
        Ok(info_key)
    }

    #[tracing::instrument(skip(self, store), err)]
    pub async fn copy_path(
        &self,
        store: &nix_utils::LocalStore,
        path: &nix_utils::StorePath,
        repair: bool,
    ) -> Result<(), CacheError> {
        if !repair && self.has_narinfo(path).await? {
            return Ok(());
        }

        let Some(path_info) = store.query_path_info(path).await else {
            return Ok(()); // TODO: not found error?
        };
        let mut narinfo = NarInfo::new(path, path_info, self.cfg.compression, &self.signing_keys);
        let queried_references = store
            .query_path_infos(&narinfo.references.iter().collect::<Vec<_>>())
            .await;
        for r in &narinfo.references {
            if !queried_references.contains_key(r) {
                return Err(CacheError::ReferenceVerifyError(
                    narinfo.store_path,
                    r.to_owned(),
                ));
            }
        }

        if self.cfg.write_nar_listing {
            let ls = store.list_nar(path, true).await?;
            self.upload_listing(path, ls).await?;
        }

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Result<Bytes, std::io::Error>>();
        let closure = move |data: &[u8]| {
            let data = Bytes::copy_from_slice(data);
            tx.send(Ok(data)).is_ok()
        };

        tokio::task::spawn({
            let path = narinfo.store_path.clone();
            let store = store.clone();
            async move {
                let _ = store.nar_from_path(&path, closure);
            }
        });
        let stream = tokio_util::io::StreamReader::new(
            tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
        );
        let compressor = narinfo.compression.get_compression_fn(
            self.cfg.get_compression_level(),
            self.cfg.parallel_compression,
        );
        let compressed_stream = compressor(stream);
        let mut hashing_reader = crate::streaming_hash::HashingReader::new(compressed_stream);
        self.upload_file(
            &narinfo.url,
            &mut hashing_reader,
            narinfo.compression.content_type(),
            narinfo.compression.content_encoding(),
        )
        .await?;

        let (file_hash, file_size) = hashing_reader.finalize()?;

        if let Ok(file_hash) = nix_utils::convert_hash(
            &format!("{file_hash:x}"),
            Some(nix_utils::HashAlgorithm::SHA256),
            nix_utils::HashFormat::Nix32,
        ) {
            narinfo.file_hash = Some(format!("sha256:{file_hash}"));
            narinfo.file_size = Some(file_size as u64);
        }

        self.upload_narinfo(narinfo).await?;

        Ok(())
    }

    #[tracing::instrument(skip(self, store, paths), err)]
    pub async fn copy_paths(
        &self,
        store: &nix_utils::LocalStore,
        paths: Vec<nix_utils::StorePath>,
        repair: bool,
    ) -> Result<(), CacheError> {
        use futures::stream::StreamExt as _;

        let mut stream = tokio_stream::iter(paths)
            .map(|p| async move {
                log::debug!("copying path {p} to s3 binary cache.");
                self.copy_path(store, &p, repair).await
            })
            .buffered(10);

        while let Some(v) = tokio_stream::StreamExt::next(&mut stream).await {
            v?;
        }

        Ok(())
    }

    #[tracing::instrument(skip(self, store, id), err)]
    pub async fn copy_realisation(
        &self,
        store: &nix_utils::LocalStore,
        id: &nix_utils::DrvOutput,
        repair: bool,
    ) -> Result<(), CacheError> {
        if !repair && self.has_realisation(id).await? {
            return Ok(());
        }

        let mut raw_realisation = store.query_raw_realisation(&id.drv_hash, &id.output_name)?;
        if !self.signing_keys.is_empty() {
            for s in &self.signing_keys {
                raw_realisation.sign(s.expose_secret())?;
            }
        }

        self.upsert_file(
            &format!("realisations/{id}.doi"),
            raw_realisation.as_json(),
            "application/json",
        )
        .await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn download_narinfo(
        &self,
        store_path: &nix_utils::StorePath,
    ) -> Result<Option<NarInfo>, CacheError> {
        if let Some(narinfo) = self.narinfo_cache.get(store_path).await {
            return Ok(Some(narinfo));
        }

        match self
            .get_object(&format!("{}.narinfo", store_path.hash_part()))
            .await?
        {
            Some(v) => {
                let narinfo: NarInfo = String::from_utf8_lossy(&v).parse()?;
                self.narinfo_cache
                    .insert(store_path.to_owned(), narinfo.clone())
                    .await;
                Ok(Some(narinfo))
            }
            None => Ok(None),
        }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn download_nar(&self, nar_url: &str) -> Result<Option<Bytes>, CacheError> {
        self.get_object(nar_url).await
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn has_narinfo(&self, store_path: &nix_utils::StorePath) -> Result<bool, CacheError> {
        if self.narinfo_cache.contains_key(store_path) {
            return Ok(true);
        }
        Ok(self.download_narinfo(store_path).await?.is_some())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn download_realisation(
        &self,
        id: &nix_utils::DrvOutput,
    ) -> Result<Option<String>, CacheError> {
        match self.get_object(&format!("realisations/{id}.doi")).await? {
            Some(v) => Ok(Some(String::from_utf8_lossy(&v).to_string())),
            None => Ok(None),
        }
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn has_realisation(&self, id: &nix_utils::DrvOutput) -> Result<bool, CacheError> {
        Ok(self.download_realisation(id).await?.is_some())
    }

    #[tracing::instrument(skip(self, paths))]
    pub async fn query_missing_paths(
        &self,
        paths: Vec<nix_utils::StorePath>,
    ) -> Vec<nix_utils::StorePath> {
        use futures::stream::StreamExt as _;

        tokio_stream::iter(paths)
            .map(|p| async move {
                if self.has_narinfo(&p).await.unwrap_or_default() {
                    None
                } else {
                    Some(p)
                }
            })
            .buffered(50)
            .filter_map(|p| async { p })
            .collect()
            .await
    }

    #[tracing::instrument(skip(self, outputs))]
    pub async fn query_missing_remote_outputs(
        &self,
        outputs: Vec<nix_utils::DerivationOutput>,
    ) -> Vec<nix_utils::DerivationOutput> {
        use futures::stream::StreamExt as _;

        tokio_stream::iter(outputs)
            .map(|o| async move {
                let Some(path) = &o.path else {
                    return None;
                };
                if self.has_narinfo(path).await.unwrap_or_default() {
                    None
                } else {
                    Some(o)
                }
            })
            .buffered(50)
            .filter_map(|o| async { o })
            .collect()
            .await
    }
}
