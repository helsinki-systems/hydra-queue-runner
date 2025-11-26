use std::sync::Arc;

use hashbrown::{HashMap, HashSet};

use nix_utils::{BaseStore, Derivation, LocalStore, StorePath};

use crate::state::{AtomicDateTime, Jobsets, QueueType, Queues, Step, StepInfo, System};

#[derive(Clone, Debug)]
struct FodItem {
    drv: nix_utils::Derivation,
    build_id: Option<i32>,
}

pub struct FodChecker {
    db: Option<db::Database>,
    store: nix_utils::LocalStore,
    config: crate::config::PreparedFodConfig,

    ca_derivations: parking_lot::RwLock<HashMap<StorePath, FodItem>>,
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
) -> HashMap<StorePath, FodItem> {
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
        let item = FodItem {
            drv: parsed,
            build_id: None,
        };
        out.insert(drv.clone(), item);
    }

    out
}

impl FodChecker {
    #[must_use]
    pub fn new(
        db: Option<db::Database>,
        store: LocalStore,
        config: crate::config::PreparedFodConfig,
        traverse_done_notifier: Option<tokio::sync::mpsc::Sender<()>>,
    ) -> Self {
        // TODO: load not done steps

        Self {
            db,
            store,
            config,

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
            if !ca.contains_key(drv) {
                let item = FodItem {
                    drv: parsed.clone(),
                    build_id: None,
                };
                ca.insert(drv.clone(), item);
            }
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

        let mut drvs = {
            let mut tt = self.to_traverse.write();
            tt.drain().collect::<HashSet<_>>()
        };
        {
            let ca_derivations = self.ca_derivations.read();
            drvs.retain(|p| !ca_derivations.contains_key(p));
        }
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
            for (k, v) in out {
                // dont update any existing value
                let _ = ca_derivations.try_insert(k, v);
            }
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

    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(self, jobsets, queues, app_config), err)]
    async fn dispatch_once(
        &self,
        jobsets: Jobsets,
        queues: Queues,
        app_config: crate::config::App,
    ) -> anyhow::Result<()> {
        let db = self
            .db
            .as_ref()
            .ok_or(anyhow::anyhow!("No db passed. Can not dispatch"))?;

        let now = jiff::Timestamp::now();

        // only do this every 1 hours as the FOD-checker has almost no priority
        if now <= (self.last_full_dispatch.load() + jiff::SignedDuration::from_hours(1)) {
            return Ok(());
        }

        let mut new_queues = HashMap::<System, Vec<StepInfo>>::with_capacity(10);
        let mut new_ca_derivations = {
            let mut ca_derivations = self.ca_derivations.write();
            ca_derivations.drain().collect::<HashMap<_, _>>()
        };
        if new_ca_derivations.is_empty() {
            // early return, because we dont want to reset last_full_dispatch in this case.
            return Ok(());
        }

        let mut conn = db.get().await?;
        let mut conn2 = db.get().await?;
        let not_done = conn
            .get_not_finished_fod_checks()
            .await?
            .into_iter()
            .map(|b| nix_utils::StorePath::new(&b.drvpath))
            .collect::<HashSet<_>>();
        new_ca_derivations.retain(|s, item| item.build_id.is_some() || !not_done.contains(s));
        if new_ca_derivations.is_empty() {
            // early return, because we dont want to reset last_full_dispatch in this case.
            return Ok(());
        }

        tracing::info!("starting fod dispatch");
        let mut tx = conn.begin_transaction().await?;
        let timestamp = i32::try_from(now.as_second())?; // TODO
        for (store_path, item) in new_ca_derivations {
            let drv = item.drv;
            if !drv.is_ca() {
                continue;
            }
            let system = drv.system.clone();

            let printed_store_path = self.store.print_store_path(&store_path);
            // if from_database is true we already have this build in the database
            let b = if let Some(build_id) = item.build_id {
                tx.load_build_by_id(build_id).await?
            } else {
                if let Some(prev_build) = tx.get_last_fod_check(&printed_store_path).await? {
                    let ts = jiff::Timestamp::from_second(
                        prev_build.stoptime.unwrap_or(prev_build.timestamp).into(),
                    )?;
                    if (ts + self.config.duration_between_fod_checks) >= now {
                        tracing::debug!(
                            "We had a fod check in the last {}h, not checking {}",
                            self.config.duration_between_fod_checks.as_hours(),
                            store_path,
                        );
                        continue;
                    }
                }
                let build_id = tx
                    .create_fod_check(db::models::InsertFodCheck {
                        timestamp,
                        jobset_id: self.config.jobset_id,
                        job: "fod-check",
                        nixname: drv.env.get_name(),
                        drv_path: &printed_store_path,
                        system: &drv.system,
                    })
                    .await?;
                tx.load_build_by_id(build_id).await?
            };
            let Some(b) = b else {
                continue;
            };
            let jobset = jobsets
                // TODO: allow tx here as well
                .create(&mut conn2, b.jobset_id, &b.project, &b.jobset)
                .await?;

            let build = super::Build::new(b, jobset)?;

            // TODO: Do we need to ensure that its actually runnable?
            let step = Step::with_build_hint(store_path, build.clone());
            step.atomic_state.set_created(true);
            step.set_drv(drv);
            step.make_runnable();
            step.add_referring_data(Some(&build), None);

            let mut indirect = HashSet::new();
            let mut steps = HashSet::new();
            step.get_dependents(&mut indirect, &mut steps);
            tracing::warn!("indirect for {}: {indirect:?}", step.get_drv_path());
            tracing::warn!("steps for {}: {steps:?}", step.get_drv_path());
            tracing::warn!(
                "direct for {}: {:?}",
                step.get_drv_path(),
                step.get_direct_builds()
            );

            let step_info = StepInfo::new(&self.store, step).await;

            new_queues
                .entry(system)
                .or_insert_with(|| Vec::with_capacity(100))
                .push(step_info);
        }
        tx.commit().await?;

        for (system, jobs) in new_queues {
            tracing::info!("Inserting new jobs into fod queue: {system} {}", jobs.len());
            queues
                .insert_new_jobs_into_fod(system, jobs, &now, app_config.get_step_sort_fn())
                .await;
        }
        queues.remove_all_weak_pointer(Some(QueueType::Fod)).await;
        self.last_full_dispatch.store(jiff::Timestamp::now());
        Ok(())
    }

    #[tracing::instrument(name = "fod_dispatch_loop", skip(self, jobsets, queues, app_config))]
    async fn dispatch_loop(
        &self,
        jobsets: Jobsets,
        queues: Queues,
        app_config: crate::config::App,
    ) {
        loop {
            tokio::select! {
                () = self.notify_dispatch.notified() => {},
                () = tokio::time::sleep(tokio::time::Duration::from_secs(3600)) => {},
            };
            if let Err(e) = self
                .dispatch_once(jobsets.clone(), queues.clone(), app_config.clone())
                .await
            {
                tracing::error!("Failed to run dispatch: {e}");
            }
        }
    }

    async fn load_vals_from_db(&self) -> anyhow::Result<()> {
        let db = self
            .db
            .as_ref()
            .ok_or(anyhow::anyhow!("No db passed. Can not dispatch"))?;
        let mut conn = db.get().await?;
        let not_done = conn.get_not_finished_fod_checks().await?;

        let mut c = 0;
        for b in not_done {
            let drvpath = nix_utils::StorePath::new(&b.drvpath);
            let Ok(drv) = nix_utils::query_drv(&self.store, &drvpath).await else {
                continue;
            };

            if let Some(drv) = drv {
                let mut ca = self.ca_derivations.write();
                ca.insert(
                    drvpath,
                    FodItem {
                        drv,
                        build_id: Some(b.id),
                    },
                );
                c += 1;
            } else {
                tracing::error!("aborting GC'ed fod check {}", b.id);
                if let Err(e) = conn.abort_build(b.id).await {
                    tracing::error!("Failed to abort the build={} e={}", b.id, e);
                }
            }
        }
        tracing::info!("Found {c} items in db");

        Ok(())
    }

    pub async fn start_dispatch_loop(
        self: Arc<Self>,
        jobsets: Jobsets,
        queues: Queues,
        app_config: crate::config::App,
    ) -> tokio::task::AbortHandle {
        // on the start of the dispatch loop we first load all current fod checks not yet finished.
        if let Err(e) = self.load_vals_from_db().await {
            tracing::error!("Failed to load old values from database: {e}");
        }

        let task = tokio::task::spawn(async move {
            Box::pin(self.dispatch_loop(jobsets, queues, app_config)).await;
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
        for (path, item) in drvs {
            processor(path, item.drv).await;
            c += 1;
        }

        c
    }

    pub fn force_dispatch_sync(&self) {
        self.last_full_dispatch.store(jiff::Timestamp::MIN);
        self.trigger_traverse();
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

        let fod = FodChecker::new(
            None,
            store,
            crate::config::PreparedFodConfig::init(1, jiff::SignedDuration::from_secs(60)),
            None,
        );
        fod.to_traverse(&hello_drv);
        fod.traverse().await;
        assert_eq!(fod.ca_derivations.read().len(), 59);
    }
}
