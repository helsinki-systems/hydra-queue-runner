use std::collections::VecDeque;

use backon::ExponentialBuilder;
use backon::Retryable as _;
use nix_utils::BaseStore as _;

// TODO: scheduling is shit, because if we crash/restart we need to start again as the builds are
//       already done in the db.
//       So we need to make this persistent!

fn pop_up_to<T>(q: &mut VecDeque<T>, n: usize) -> Vec<T> {
    let k = n.min(q.len());
    let mut out = Vec::with_capacity(k);
    for _ in 0..k {
        match q.pop_front() {
            Some(v) => out.push(v),
            None => break,
        }
    }
    out
}

#[derive(Debug)]
struct Message {
    store_paths: Vec<nix_utils::StorePath>,
    log_remote_path: String,
    log_local_path: String,
}

pub struct Uploader {
    queue: parking_lot::RwLock<VecDeque<Message>>,
}

impl Default for Uploader {
    fn default() -> Self {
        Self::new()
    }
}

impl Uploader {
    pub fn new() -> Self {
        Self {
            queue: parking_lot::RwLock::new(VecDeque::with_capacity(1000)),
        }
    }

    #[tracing::instrument(skip(self))]
    pub fn schedule_upload(
        &self,
        store_paths: Vec<nix_utils::StorePath>,
        log_remote_path: String,
        log_local_path: String,
    ) {
        tracing::info!("Scheduling new path upload: {:?}", store_paths);
        self.queue.write().push_back(Message {
            store_paths,
            log_remote_path,
            log_local_path,
        });
    }

    #[tracing::instrument(skip(self, local_store, remote_stores))]
    async fn upload_msg(
        &self,
        local_store: nix_utils::LocalStore,
        remote_stores: Vec<binary_cache::S3BinaryCacheClient>,
        msg: Message,
    ) {
        let span = tracing::info_span!("upload_msg", msg = ?msg);
        let _ = span.enter();
        tracing::info!("Start uploading {} paths", msg.store_paths.len());

        let paths_to_copy = match local_store
            .query_requisites(&msg.store_paths.iter().collect::<Vec<_>>(), true)
            .await
        {
            Ok(paths) => paths,
            Err(e) => {
                tracing::error!("Failed to query requisites: {e}");
                return;
            }
        };

        for remote_store in remote_stores {
            let bucket = &remote_store.cfg.client_config.bucket;

            // Upload log file with backon retry
            let log_upload_result = (|| async {
                let file = fs_err::tokio::File::open(&msg.log_local_path).await?;
                let reader = Box::new(tokio::io::BufReader::new(file));

                remote_store
                    .upsert_file_stream(&msg.log_remote_path, reader, "text/plain; charset=utf-8")
                    .await?;

                Ok::<(), anyhow::Error>(())
            })
            .retry(
                ExponentialBuilder::default()
                    .with_max_delay(std::time::Duration::from_secs(30))
                    .with_max_times(3),
            )
            .await;

            if let Err(e) = log_upload_result {
                tracing::error!("Failed to upload log file after retries: {e}");
            }
            if msg.store_paths.is_empty() {
                tracing::debug!("No NAR files to upload (presigned uploads enabled)");
            } else {
                let paths_to_copy = remote_store
                    .query_missing_paths(paths_to_copy.clone())
                    .await;

                let copy_result = (|| async {
                    remote_store
                        .copy_paths(&local_store, paths_to_copy.clone(), false)
                        .await?;

                    Ok::<(), anyhow::Error>(())
                })
                .retry(
                    ExponentialBuilder::default()
                        .with_max_delay(std::time::Duration::from_secs(60))
                        .with_max_times(5),
                )
                .await;

                if let Err(e) = copy_result {
                    tracing::error!("Failed to copy paths after retries: {e}");
                } else {
                    tracing::debug!(
                        "Successfully uploaded {} paths to bucket {bucket}",
                        msg.store_paths.len()
                    );
                }
            }
        }

        tracing::info!("Finished uploading {} paths", msg.store_paths.len());
    }

    pub async fn upload_once(
        &self,
        local_store: nix_utils::LocalStore,
        remote_stores: Vec<binary_cache::S3BinaryCacheClient>,
    ) {
        let Some(msg) = self.queue.write().pop_front() else {
            return;
        };

        self.upload_msg(local_store, remote_stores, msg).await;
    }

    pub async fn upload_many(
        &self,
        local_store: nix_utils::LocalStore,
        remote_stores: Vec<binary_cache::S3BinaryCacheClient>,
        limit: usize,
    ) {
        let messages = {
            let mut q = self.queue.write();
            pop_up_to(&mut q, limit)
        };

        let mut jobs = vec![];
        for msg in messages {
            jobs.push(self.upload_msg(local_store.clone(), remote_stores.clone(), msg));
        }
        futures::future::join_all(jobs).await;
    }

    pub fn len_of_queue(&self) -> usize {
        self.queue.read().len()
    }

    pub fn paths_in_queue(&self) -> Vec<nix_utils::StorePath> {
        self.queue
            .read()
            .iter()
            .flat_map(|m| m.store_paths.clone())
            .collect()
    }
}
