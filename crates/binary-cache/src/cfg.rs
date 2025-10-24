use crate::Compression;

#[derive(Debug, Clone)]
pub struct S3CacheConfig {
    pub client_config: S3ClientConfig,

    pub compression: Compression,
    pub write_nar_listing: bool,
    pub write_debug_info: bool, // TODO
    pub secret_key_files: Vec<std::path::PathBuf>,
    pub parallel_compression: bool,
    pub compression_level: Option<i32>,

    pub narinfo_compression: Compression,
    pub ls_compression: Compression,
    pub log_compression: Compression,
    pub buffer_size: usize,
}

impl S3CacheConfig {
    #[must_use]
    pub fn new(client_config: S3ClientConfig) -> Self {
        Self {
            client_config,
            compression: Compression::Xz,
            write_nar_listing: false,
            write_debug_info: false,
            secret_key_files: Vec::default(),
            parallel_compression: false,
            compression_level: Option::default(),
            narinfo_compression: Compression::None,
            ls_compression: Compression::None,
            log_compression: Compression::None,
            buffer_size: 8 * 1024 * 1024,
        }
    }

    #[must_use]
    pub fn with_compression(mut self, compression: Option<Compression>) -> Self {
        if let Some(compression) = compression {
            self.compression = compression;
        }
        self
    }

    #[must_use]
    pub fn with_write_nar_listing(mut self, write_nar_listing: Option<&str>) -> Self {
        if let Some(write_nar_listing) = write_nar_listing {
            let s = write_nar_listing.trim().to_ascii_lowercase();
            self.write_nar_listing = s.as_str() == "1" || s.as_str() == "true";
        }
        self
    }

    #[must_use]
    pub fn with_write_debug_info(mut self, write_debug_info: Option<&str>) -> Self {
        if let Some(write_debug_info) = write_debug_info {
            let s = write_debug_info.trim().to_ascii_lowercase();
            self.write_debug_info = s.as_str() == "1" || s.as_str() == "true";
        }
        self
    }

    #[must_use]
    pub fn add_secret_key_files(mut self, secret_keys: &[std::path::PathBuf]) -> Self {
        self.secret_key_files.extend_from_slice(secret_keys);
        self
    }

    #[must_use]
    pub fn with_parallel_compression(mut self, parallel_compression: Option<&str>) -> Self {
        if let Some(parallel_compression) = parallel_compression {
            let s = parallel_compression.trim().to_ascii_lowercase();
            self.parallel_compression = s.as_str() == "1" || s.as_str() == "true";
        }
        self
    }

    #[must_use]
    pub fn with_compression_level(mut self, compression_level: Option<i32>) -> Self {
        if let Some(compression_level) = compression_level {
            self.compression_level = Some(compression_level);
        }
        self
    }

    #[must_use]
    pub fn with_narinfo_compression(mut self, compression: Option<Compression>) -> Self {
        if let Some(compression) = compression {
            self.narinfo_compression = compression;
        }
        self
    }

    #[must_use]
    pub fn with_ls_compression(mut self, compression: Option<Compression>) -> Self {
        if let Some(compression) = compression {
            self.ls_compression = compression;
        }
        self
    }

    #[must_use]
    pub fn with_log_compression(mut self, compression: Option<Compression>) -> Self {
        if let Some(compression) = compression {
            self.log_compression = compression;
        }
        self
    }

    #[must_use]
    pub fn with_buffer_size(mut self, buffer_size: Option<usize>) -> Self {
        if let Some(buffer_size) = buffer_size {
            self.buffer_size = buffer_size;
        }
        self
    }

    pub(crate) fn get_compression_level(&self) -> async_compression::Level {
        if let Some(l) = self.compression_level {
            async_compression::Level::Precise(l)
        } else {
            async_compression::Level::Default
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UrlParseError {
    #[error("Uri parse error: {0}")]
    UriParseError(#[from] url::ParseError),
    #[error("Int parse error: {0}")]
    IntParseError(#[from] std::num::ParseIntError),
    #[error("Invalid S3Scheme: {0}")]
    S3SchemeParseError(String),
    #[error("Invalid Compression: {0}")]
    CompressionParseError(String),
    #[error("Bad schema: {0}")]
    BadSchema(String),
    #[error("Bucket not defined")]
    NoBucket,
}

impl std::str::FromStr for S3CacheConfig {
    type Err = UrlParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uri = url::Url::parse(&s.trim().to_ascii_lowercase())?;
        if uri.scheme() != "s3" {
            return Err(UrlParseError::BadSchema(uri.scheme().to_owned()));
        }
        let bucket = uri.authority();
        if bucket.is_empty() {
            return Err(UrlParseError::NoBucket);
        }
        let query = uri
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        let cfg = S3ClientConfig::new(bucket.to_owned())
            .with_region(query.get("region").map(std::string::String::as_str))
            .with_scheme(
                query
                    .get("scheme")
                    .map(|x| x.parse::<S3Scheme>())
                    .transpose()
                    .map_err(UrlParseError::S3SchemeParseError)?,
            )
            .with_endpoint(query.get("endpoint").map(std::string::String::as_str))
            .with_profile(query.get("profile").map(std::string::String::as_str));

        Ok(S3CacheConfig::new(cfg)
            .with_compression(
                query
                    .get("compression")
                    .map(|x| x.parse::<Compression>())
                    .transpose()
                    .map_err(UrlParseError::CompressionParseError)?,
            )
            .with_write_nar_listing(
                query
                    .get("write-nar-listing")
                    .map(std::string::String::as_str),
            )
            .with_write_debug_info(
                query
                    .get("write-debug-info")
                    .map(std::string::String::as_str),
            )
            .add_secret_key_files(
                &query
                    .get("secret-key")
                    .map(|s| if s.is_empty() { vec![] } else { vec![s.into()] })
                    .unwrap_or_default(),
            )
            .add_secret_key_files(
                &query
                    .get("secret-keys")
                    .map(|s| {
                        s.split(',')
                            .filter(|s| !s.is_empty())
                            .map(Into::into)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            )
            .with_parallel_compression(
                query
                    .get("parallel-compression")
                    .map(std::string::String::as_str),
            )
            .with_compression_level(
                query
                    .get("compression-level")
                    .map(|x| x.parse::<i32>())
                    .transpose()?,
            )
            .with_narinfo_compression(
                query
                    .get("narinfo-compression")
                    .map(|x| x.parse::<Compression>())
                    .transpose()
                    .map_err(UrlParseError::CompressionParseError)?,
            )
            .with_ls_compression(
                query
                    .get("ls-compression")
                    .map(|x| x.parse::<Compression>())
                    .transpose()
                    .map_err(UrlParseError::CompressionParseError)?,
            )
            .with_log_compression(
                query
                    .get("log-compression")
                    .map(|x| x.parse::<Compression>())
                    .transpose()
                    .map_err(UrlParseError::CompressionParseError)?,
            )
            .with_buffer_size(
                query
                    .get("buffer-size")
                    .map(|x| x.parse::<usize>())
                    .transpose()?,
            ))
    }
}

#[derive(Debug, Clone, Default, Copy, PartialEq, Eq)]
pub enum S3Scheme {
    HTTP,
    #[default]
    HTTPS,
}

impl std::str::FromStr for S3Scheme {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "http" => Ok(S3Scheme::HTTP),
            "https" => Ok(S3Scheme::HTTPS),
            v => Err(v.to_owned()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct S3ClientConfig {
    pub region: String,
    pub scheme: S3Scheme,
    pub endpoint: Option<String>,
    pub bucket: String,
    pub profile: Option<String>, // TODO: unused
    pub(crate) credentials: Option<S3CredentialsConfig>,
}

impl S3ClientConfig {
    #[must_use]
    pub fn new(bucket: String) -> Self {
        Self {
            region: "us-east-1".into(),
            scheme: S3Scheme::default(),
            endpoint: None,
            bucket,
            profile: None,
            credentials: None,
        }
    }

    #[must_use]
    pub fn with_region(mut self, region: Option<&str>) -> Self {
        if let Some(region) = region {
            self.region = region.into();
        }
        self
    }

    #[must_use]
    pub fn with_scheme(mut self, scheme: Option<S3Scheme>) -> Self {
        if let Some(scheme) = scheme {
            self.scheme = scheme;
        }
        self
    }

    #[must_use]
    pub fn with_endpoint(mut self, endpoint: Option<&str>) -> Self {
        self.endpoint = endpoint.map(ToOwned::to_owned);
        self
    }

    #[must_use]
    pub fn with_profile(mut self, profile: Option<&str>) -> Self {
        self.profile = profile.map(ToOwned::to_owned);
        self
    }

    #[must_use]
    pub fn with_credentials(mut self, credentials: Option<S3CredentialsConfig>) -> Self {
        self.credentials = credentials;
        self
    }
}

#[derive(Debug, Clone)]
pub struct S3CredentialsConfig {
    pub access_key_id: String,
    pub secret_access_key: String,
}
