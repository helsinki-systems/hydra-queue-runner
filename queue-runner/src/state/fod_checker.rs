use std::sync::Arc;

use hashbrown::{HashMap, HashSet};

use nix_utils::{Derivation, LocalStore, StorePath};

use crate::state::{AtomicDateTime, QueueType, Queues, Step, StepInfo, System};

pub struct FodChecker {
    store: nix_utils::LocalStore,

    ca_derivations: parking_lot::RwLock<HashMap<StorePath, nix_utils::Derivation>>,
    to_traverse: parking_lot::RwLock<HashSet<StorePath>>,

    notify_traverse: tokio::sync::Notify,
    notify_dispatch: tokio::sync::Notify,
    traverse_done_notifier: Option<tokio::sync::mpsc::Sender<()>>,

    last_full_dispatch: AtomicDateTime,
}

async fn collect_ca_derivations(
    store: &LocalStore,
    drv: &StorePath,
    processed: Arc<parking_lot::RwLock<HashSet<StorePath>>>,
) -> HashMap<StorePath, nix_utils::Derivation> {
    use futures::StreamExt as _;

    {
        let p = processed.read();
        if p.contains(drv) {
            return HashMap::new();
        }
    }
    {
        let mut p = processed.write();
        p.insert(drv.clone());
    }

    let Some(parsed) = nix_utils::query_drv(store, drv).await.ok().flatten() else {
        return HashMap::new();
    };

    let is_ca = parsed.is_ca();
    let mut out = if parsed.input_drvs.is_empty() {
        HashMap::new()
    } else {
        futures::StreamExt::map(tokio_stream::iter(parsed.input_drvs.clone()), |i| {
            let processed = processed.clone();
            async move {
                let i = StorePath::new(&i);
                Box::pin(collect_ca_derivations(store, &i, processed)).await
            }
        })
        .buffered(10) // keep this low as it has no high prio
        .flat_map(futures::stream::iter)
        .collect::<HashMap<_, _>>()
        .await
    };
    if is_ca {
        out.insert(drv.clone(), parsed);
    }

    out
}

impl FodChecker {
    #[must_use]
    pub fn new(
        store: LocalStore,
        traverse_done_notifier: Option<tokio::sync::mpsc::Sender<()>>,
    ) -> Self {
        Self {
            store,
            ca_derivations: parking_lot::RwLock::new(HashMap::with_capacity(1000)),
            to_traverse: parking_lot::RwLock::new(HashSet::new()),

            notify_traverse: tokio::sync::Notify::new(),
            notify_dispatch: tokio::sync::Notify::new(),
            traverse_done_notifier,

            last_full_dispatch: AtomicDateTime::new(jiff::Timestamp::MIN),
        }
    }

    pub(super) fn add_ca_drv_parsed(&self, drv: &StorePath, parsed: &nix_utils::Derivation) {
        if parsed.is_ca() {
            let mut ca = self.ca_derivations.write();
            ca.insert(drv.clone(), parsed.clone());
        }
    }

    pub fn to_traverse(&self, drv: &StorePath) {
        let mut tt = self.to_traverse.write();
        tt.insert(drv.clone());
    }

    pub fn clone_to_traverse(&self) -> Vec<StorePath> {
        let tt = self.to_traverse.read();
        tt.iter().map(ToOwned::to_owned).collect()
    }

    pub fn clone_ca_derivations(&self) -> Vec<StorePath> {
        let cas = self.ca_derivations.read();
        cas.keys().map(ToOwned::to_owned).collect()
    }

    async fn traverse(&self) {
        use futures::StreamExt as _;

        let drvs = {
            let mut tt = self.to_traverse.write();
            tt.drain().collect::<HashSet<_>>()
        };
        if drvs.is_empty() {
            return;
        }

        let processed = Arc::new(parking_lot::RwLock::new(HashSet::<StorePath>::new()));
        let out = futures::StreamExt::map(tokio_stream::iter(drvs), |i| {
            let processed = processed.clone();
            async move { Box::pin(collect_ca_derivations(&self.store, &i, processed)).await }
        })
        .buffered(10) // keep this low as it has no high prio
        .flat_map(futures::stream::iter)
        .collect::<HashMap<_, _>>()
        .await;

        {
            let out_len = out.len();
            let mut ca_derivations = self.ca_derivations.write();
            ca_derivations.extend(out);
            tracing::info!(
                "new ca derivations: {out_len} => resulting count: {}",
                ca_derivations.len()
            );
        }
        self.notify_dispatch.notify_one();
    }

    #[tracing::instrument(skip(self))]
    pub fn trigger_traverse(&self) {
        self.notify_traverse.notify_one();
    }

    #[tracing::instrument(skip(self))]
    async fn traverse_loop(&self) {
        loop {
            tokio::select! {
                () = self.notify_traverse.notified() => {},
                () = tokio::time::sleep(tokio::time::Duration::from_secs(60)) => {},
            };
            self.traverse().await;
            if let Some(tx) = &self.traverse_done_notifier {
                let _ = tx.send(()).await;
            }
        }
    }

    pub fn start_traverse_loop(self: Arc<Self>) -> tokio::task::AbortHandle {
        let task = tokio::task::spawn(async move {
            Box::pin(self.traverse_loop()).await;
        });
        task.abort_handle()
    }

    #[tracing::instrument(skip(self, queues, config))]
    async fn dispatch_once(&self, queues: Queues, config: crate::config::App) {
        let now = jiff::Timestamp::now();

        // only do this every 3 hours as the FOD-checker has almost no priority
        if now <= (self.last_full_dispatch.load() + jiff::SignedDuration::from_hours(3)) {
            return;
        }

        let mut new_queues = HashMap::<System, Vec<StepInfo>>::with_capacity(10);
        let new_ca_derivations = {
            let mut ca_derivations = self.ca_derivations.write();
            ca_derivations.drain().collect::<HashMap<_, _>>()
        };
        if new_ca_derivations.is_empty() {
            // early return, because we dont want to reset last_full_dispatch in this case.
            return;
        }

        tracing::info!("starting fod dispatch");
        for (store_path, drv) in new_ca_derivations {
            if !drv.is_ca() {
                continue;
            }
            let system = drv.system.clone();

            // TODO: write new step as build into db
            // TODO: check last time we checked that FOD, dont check FODs more than 1Week, configurable
            // TODO: write into db

            // TODO: Do we need to ensure that its actually runnable?
            let step = Step::new(store_path);
            step.atomic_state.set_created(true);
            step.set_drv(drv);
            step.make_runnable();
            let step_info = StepInfo::new(&self.store, step).await;

            new_queues
                .entry(system)
                .or_insert_with(|| Vec::with_capacity(100))
                .push(step_info);
        }

        for (system, jobs) in new_queues {
            tracing::info!("Inserting new jobs into fod queue: {system} {}", jobs.len());
            queues
                .insert_new_jobs_into_fod(system, jobs, &now, config.get_step_sort_fn())
                .await;
        }
        queues.remove_all_weak_pointer(Some(QueueType::Fod)).await;
        self.last_full_dispatch.store(jiff::Timestamp::now());
    }

    #[tracing::instrument(name = "fod_dispatch_loop", skip(self, queues, config))]
    async fn dispatch_loop(&self, queues: Queues, config: crate::config::App) {
        loop {
            tokio::select! {
                () = self.notify_dispatch.notified() => {},
                () = tokio::time::sleep(tokio::time::Duration::from_secs(3600)) => {},
            };
            self.dispatch_once(queues.clone(), config.clone()).await;
        }
    }

    pub fn start_dispatch_loop(
        self: Arc<Self>,
        queues: Queues,
        config: crate::config::App,
    ) -> tokio::task::AbortHandle {
        let task = tokio::task::spawn(async move {
            Box::pin(self.dispatch_loop(queues, config)).await;
        });
        task.abort_handle()
    }

    pub async fn process<F>(&self, processor: F) -> i64
    where
        F: AsyncFn(StorePath, Derivation) -> (),
    {
        let drvs = {
            let mut drvs = self.ca_derivations.write();
            let cloned = drvs.clone();
            drvs.clear();
            cloned
        };

        let mut c = 0;
        for (path, drv) in drvs {
            processor(path, drv).await;
            c += 1;
        }

        c
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use crate::state::fod_checker::FodChecker;
    use nix_utils::BaseStore as _;

    #[ignore = "Requires a valid drv in the nix-store"]
    #[tokio::test]
    async fn test_traverse() {
        let store = nix_utils::LocalStore::init();
        let hello_drv =
            nix_utils::StorePath::new("rl5m4zxd24mkysmpbp4j9ak6q7ia6vj8-hello-2.12.2.drv");
        store.ensure_path(&hello_drv).await.unwrap();

        let fod = FodChecker::new(store, None);
        fod.to_traverse(&hello_drv);
        fod.traverse().await;
        assert_eq!(fod.ca_derivations.read().len(), 59);
    }
}
