use nix_utils::BaseStore as _;

// TODO: scheduling is shit, because if we crash/restart we need to start again as the builds are
//       already done in the db.
//       So we need to make this persistent!

struct Message {
    store_paths: Vec<nix_utils::StorePath>,
    log_remote_path: String,
    log_local_path: String,
}

pub struct Uploader {
    upload_queue_sender: tokio::sync::mpsc::UnboundedSender<Message>,
    upload_queue_receiver: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<Message>>,
}

impl Uploader {
    pub fn new() -> Self {
        let (upload_queue_tx, upload_queue_rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        Self {
            upload_queue_sender: upload_queue_tx,
            upload_queue_receiver: tokio::sync::Mutex::new(upload_queue_rx),
        }
    }

    #[tracing::instrument(skip(self), err)]
    pub fn schedule_upload(
        &self,
        store_paths: Vec<nix_utils::StorePath>,
        log_remote_path: String,
        log_local_path: String,
    ) -> anyhow::Result<()> {
        log::info!("Scheduling new path upload: {:?}", store_paths);
        self.upload_queue_sender.send(Message {
            store_paths,
            log_remote_path,
            log_local_path,
        })?;
        Ok(())
    }

    async fn upload_msg(
        &self,
        local_store: nix_utils::LocalStore,
        remote_stores: Vec<binary_cache::S3BinaryCacheClient>,
        msg: Message,
    ) {
        // TODO: we need retries for this! We can not affored to have a failure on cache push
        log::info!("Uploading paths: {:?}", msg.store_paths);

        for remote_store in remote_stores {
            let file = tokio::fs::File::open(&msg.log_local_path).await.unwrap();
            let reader = Box::new(tokio::io::BufReader::new(file));

            if let Err(e) = remote_store
                .upsert_file_stream(&msg.log_remote_path, reader, "text/plain; charset=utf-8")
                .await
            {
                log::error!("Failed to copy path to remote store: {e}");
            }

            let paths_to_copy = local_store
                .query_requisites(&msg.store_paths.iter().collect::<Vec<_>>(), false)
                .await
                .unwrap_or_default();
            let paths_to_copy = remote_store.query_missing_paths(paths_to_copy).await;
            if let Err(e) = remote_store
                .copy_paths(&local_store, paths_to_copy, false)
                .await
            {
                log::error!(
                    "Failed to copy paths to remote store({}): {e}",
                    remote_store.cfg.client_config.bucket
                );
            }
        }

        log::info!("Finished uploading paths: {:?}", msg.store_paths);
    }

    pub async fn upload_once(
        &self,
        local_store: nix_utils::LocalStore,
        remote_stores: Vec<binary_cache::S3BinaryCacheClient>,
    ) {
        let Some(msg) = ({
            let mut rx = self.upload_queue_receiver.lock().await;
            rx.recv().await
        }) else {
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
        let mut messages: Vec<Message> = Vec::with_capacity(limit);
        {
            let mut rx = self.upload_queue_receiver.lock().await;
            rx.recv_many(&mut messages, limit).await;
        }

        let mut jobs = vec![];
        for msg in messages {
            jobs.push(self.upload_msg(local_store.clone(), remote_stores.clone(), msg));
        }
        futures::future::join_all(jobs).await;
    }
}
