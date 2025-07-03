#![allow(dead_code)]

mod build;
mod jobset;
mod machine;
mod metrics;
mod queue;

pub use build::{Build, BuildID, BuildOutput, RemoteBuild, Step, StorePath};
pub use jobset::{Jobset, JobsetID, SCHEDULING_WINDOW};
pub use machine::{Machine, Pressure, Stats as MachineStats};
pub use queue::{BuildQueueStats, StepInfo};

use std::sync::atomic::Ordering;
use std::time::Instant;
use std::{sync::Arc, sync::Weak};

use ahash::{AHashMap, AHashSet};
use futures::TryStreamExt;
use multimap::MultiMap;
use secrecy::ExposeSecret as _;

use crate::config::{Args, PreparedApp};
use crate::db::models::BuildStatus;
use crate::utils::finish_build_step;
use machine::Machines;

enum CreateStepResult {
    None,
    Valid(Arc<Step>),
    PreviousFailure(Arc<Step>),
}

pub struct State {
    pub config: Arc<tokio::sync::RwLock<PreparedApp>>,
    pub args: Args,
    pub db: crate::db::Database,

    pub machines: Machines,

    pub log_dir: std::path::PathBuf,

    // hardcoded values fromold queue runner
    // pub maxTries: u32 = 5;
    // pub retryInterval: u32 = 30;
    // pub retryBackoff: f32 = 3.0;
    // pub maxParallelCopyClosure: u32 = 4;
    // pub maxUnsupportedTime: u32 = 0;
    pub builds: parking_lot::RwLock<AHashMap<BuildID, Arc<Build>>>,
    // Projectname, Jobsetname
    pub jobsets: parking_lot::RwLock<AHashMap<(String, String), Arc<Jobset>>>,
    pub steps: parking_lot::RwLock<AHashMap<StorePath, Weak<Step>>>,
    pub runnable: parking_lot::RwLock<Vec<Weak<Step>>>,
    pub queues: tokio::sync::RwLock<queue::Queues>,

    pub started_at: chrono::DateTime<chrono::Utc>,

    pub metrics: metrics::PromMetrics,
    pub notify_dispatch: tokio::sync::Notify,
}

impl State {
    pub async fn new() -> anyhow::Result<Arc<Self>> {
        let args = Args::new();
        let config = PreparedApp::init(&args.config_path)?;
        let (log_dir, db) = {
            let config = config.read().await;
            let log_dir = config.hydra_log_dir.clone();
            let db =
                crate::db::Database::new(config.db_url.expose_secret(), config.max_db_connections)
                    .await?;
            (log_dir, db)
        };

        let _ = tokio::fs::create_dir_all(&log_dir).await;
        Ok(Arc::new(Self {
            config,
            args,
            db,
            machines: Machines::new(),
            log_dir,
            builds: parking_lot::RwLock::new(AHashMap::new()),
            jobsets: parking_lot::RwLock::new(AHashMap::new()),
            steps: parking_lot::RwLock::new(AHashMap::new()),
            runnable: parking_lot::RwLock::new(Vec::new()),
            queues: tokio::sync::RwLock::new(queue::Queues::new()),
            started_at: chrono::Utc::now(),
            metrics: metrics::PromMetrics::new()?,
            notify_dispatch: tokio::sync::Notify::new(),
        }))
    }

    pub async fn reload_config_callback(&self, new_config: &PreparedApp) -> anyhow::Result<()> {
        // IF this gets more complex we need a way to trap the state and revert.
        // right now it doesnt matter because only reconfigure_pool can fail and this is the first
        // thing we do.

        let curr_config = self.config.read().await;
        if curr_config.db_url.expose_secret() != new_config.db_url.expose_secret() {
            self.db
                .reconfigure_pool(new_config.db_url.expose_secret())?;
        }
        if curr_config.machine_sort_fn != new_config.machine_sort_fn {
            self.machines.sort(new_config.machine_sort_fn);
        }
        Ok(())
    }

    pub fn get_nr_builds_unfinished(&self) -> usize {
        self.builds.read().len()
    }

    pub fn get_nr_steps_unfinished(&self) -> usize {
        let mut steps = self.steps.write();
        steps.retain(|_, s| s.upgrade().is_some());
        steps.len()
    }

    pub fn get_nr_runnables(&self) -> usize {
        let mut runnable = self.runnable.write();
        runnable.retain(|r| r.upgrade().is_some());
        runnable.len()
    }

    #[tracing::instrument(skip(self, machine))]
    pub async fn insert_machine(&self, machine: Machine) -> uuid::Uuid {
        let sort_fn = {
            let config = self.config.read().await;
            config.machine_sort_fn
        };
        let machine_id = self.machines.insert_machine(machine, sort_fn);
        self.trigger_dispatch();
        machine_id
    }

    #[tracing::instrument(skip(self))]
    pub async fn remove_machine(&self, machine_id: uuid::Uuid) {
        if let Some(m) = self.machines.remove_machine(machine_id) {
            let queues = self.queues.read().await;
            let jobs = m.jobs.read();
            for job in jobs.iter() {
                queues.remove_job_from_scheduled(&job.path);
            }
        }
    }

    #[tracing::instrument(skip(self, step, system), err, ret)]
    async fn realise_drv_on_valid_machine(
        &self,
        step: Arc<Step>,
        system: &str,
    ) -> anyhow::Result<bool> {
        let drv = step.get_drv_path();
        let Some(machine) = self.machines.get_machine_for_system(system) else {
            log::warn!("No free machine found for system={system} drv={drv}");
            return Ok(false);
        };

        let mut build_options = nix_utils::BuildOptions::new(None);
        let build_id = {
            let mut dependents = AHashSet::new();
            let mut steps = AHashSet::new();
            step.get_dependents(&mut dependents, &mut steps);

            if dependents.is_empty() {
                // Apparently all builds that depend on this derivation are gone (e.g. cancelled). So
                // don't bother. This is very unlikely to happen, because normally Steps are only kept
                // alive by being reachable from a Build. However, it's possible that a new Build just
                // created a reference to this step. So to handle that possibility, we retry this step
                // (putting it back in the runnable queue). If there are really no strong pointers to
                // the step, it will be deleted.
                log::info!("maybe cancelling build step {}", step.get_drv_path());
                return Ok(false);
            }

            let Some(build) = dependents
                .iter()
                .find(|b| &b.drv_path == drv)
                .or(dependents.iter().next())
            else {
                // this should never happen, as we checked is_empty above and fallback is just any build
                return Ok(false);
            };

            build_options.set_max_silent_time(build.max_silent_time);
            build_options.set_build_timeout(build.timeout);
            build.id
        };

        let mut job = machine::Job::new(build_id, drv.to_owned());
        job.result.start_time = chrono::Utc::now().timestamp();
        if self.check_cached_failure(step.clone()).await {
            job.result.step_status = BuildStatus::CachedFailure;
            self.inner_fail_job(drv, None, job, step.clone()).await?;
            return Ok(false);
        }

        self.construct_log_file_path(drv)
            .await?
            .to_str()
            .ok_or(anyhow::anyhow!("failed to construct log path string."))?
            .clone_into(&mut job.result.log_file);
        let step_nr = {
            let mut db = self.db.get().await?;
            let mut tx = db.begin_transaction().await?;

            let step_nr = tx
                .create_build_step(
                    Some(job.result.start_time),
                    build_id,
                    step.clone(),
                    machine.hostname.clone(),
                    BuildStatus::Busy,
                    None,
                    None,
                )
                .await?;
            tx.commit().await?;
            step_nr
        };
        job.step_nr = step_nr;

        log::info!(
            "Submitting build drv={drv} on machine={} hostname={} build_id={build_id} step_nr={step_nr}",
            machine.id,
            machine.hostname
        );
        self.db
            .get()
            .await?
            .update_build_step(crate::db::models::UpdateBuildStep {
                build_id,
                step_nr,
                status: crate::db::models::StepStatus::Building,
            })
            .await?;
        machine.build_drv(job, &build_options).await;
        self.metrics.nr_steps_started.add(1);
        self.metrics.nr_steps_building.add(1);
        Ok(true)
    }

    #[tracing::instrument(skip(self, drv), err)]
    async fn construct_log_file_path(&self, drv: &str) -> anyhow::Result<std::path::PathBuf> {
        let mut log_file = self.log_dir.clone();
        let drv_filename = std::path::Path::new(drv)
            .file_name()
            .ok_or(anyhow::anyhow!("Not a valid drv"))?
            .to_str()
            .ok_or(anyhow::anyhow!("Not a valid utf-8 str."))?;
        let (dir, file) = drv_filename.split_at(2);
        log_file.push(format!("{dir}/"));
        let _ = tokio::fs::create_dir_all(&log_file).await; // create dir
        log_file.push(file);
        Ok(log_file)
    }

    #[tracing::instrument(skip(self, drv), err)]
    pub async fn new_log_file(&self, drv: &str) -> anyhow::Result<tokio::fs::File> {
        let log_file = self.construct_log_file_path(drv).await?;
        log::debug!("opening {log_file:?}");

        Ok(tokio::fs::File::options()
            .create(true)
            .truncate(true)
            .write(true)
            .read(false)
            .mode(0o666)
            .open(log_file)
            .await?)
    }

    #[tracing::instrument(skip(self, new_ids, new_builds_by_id, new_builds_by_path))]
    async fn process_new_builds(
        &self,
        new_ids: Vec<BuildID>,
        mut new_builds_by_id: AHashMap<BuildID, Arc<Build>>,
        new_builds_by_path: MultiMap<StorePath, BuildID, ahash::RandomState>,
    ) {
        let mut finished_drvs = AHashSet::<StorePath>::new();

        let starttime = chrono::Utc::now();
        let mut new_runnable_addition = false;
        for id in new_ids {
            let Some(build) = new_builds_by_id.get(&id).cloned() else {
                continue;
            };

            let mut new_runnable = AHashSet::<Arc<Step>>::new();
            let mut nr_added = 0;
            let now = Instant::now();

            self.create_build(
                build,
                &mut nr_added,
                &mut new_builds_by_id,
                &new_builds_by_path,
                &mut finished_drvs,
                &mut new_runnable,
            )
            .await;

            // we should never run into this issue
            #[allow(clippy::cast_possible_truncation)]
            self.metrics
                .build_read_time_ms
                .add(now.elapsed().as_millis() as i64);

            log::info!(
                "got {} new runnable steps from {} new builds",
                new_runnable.len(),
                nr_added
            );
            for r in new_runnable {
                self.make_runnable(&r);
                new_runnable_addition = true;
            }

            self.metrics.nr_builds_read.add(nr_added);
            if chrono::Utc::now() > (starttime + chrono::Duration::seconds(60)) {
                self.metrics.queue_checks_early_exits.inc();
                break;
            }
        }

        if new_runnable_addition {
            self.trigger_dispatch();
        }
    }

    #[tracing::instrument(skip(self), err)]
    async fn process_queue_change(&self) -> anyhow::Result<()> {
        let mut db = self.db.get().await?;
        let curr_ids = db
            .get_not_finished_builds_fast()
            .await?
            .into_iter()
            .map(|b| (b.id, b.globalpriority))
            .collect::<AHashMap<_, _>>();

        {
            let mut builds = self.builds.write();
            builds.retain(|k, _| curr_ids.contains_key(k));
            for (id, build) in builds.iter() {
                let Some(new_priority) = curr_ids.get(id) else {
                    // we should never get into this case because of the retain above
                    continue;
                };

                if build.global_priority.load(Ordering::SeqCst) < *new_priority {
                    log::info!("priority of build {id} increased");
                    build.global_priority.store(*new_priority, Ordering::SeqCst);
                    build.propagate_priorities();
                }
            }
        }

        // TODO: figure out why old queue-runner killed a bunch of steps at this point in time
        Ok(())
    }

    #[tracing::instrument(skip(self, drv_path))]
    pub async fn queue_one_build(
        &self,
        jobset_id: i32,
        drv_path: &StorePath,
    ) -> anyhow::Result<()> {
        let mut db = self.db.get().await?;
        let drv = nix_utils::query_drv(drv_path)
            .await?
            .ok_or(anyhow::anyhow!("drv not found"))?;
        db.insert_debug_build(jobset_id, drv_path, &drv.system)
            .await?;

        let mut tx = db.begin_transaction().await?;
        tx.notify_builds_added().await?;
        tx.commit().await?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_queued_builds(&self) -> anyhow::Result<()> {
        self.metrics.queue_checks_started.inc();

        let mut new_ids = Vec::<BuildID>::new();
        let mut new_builds_by_id = AHashMap::<BuildID, Arc<Build>>::new();
        let mut new_builds_by_path = MultiMap::<StorePath, BuildID, ahash::RandomState>::default();

        {
            let mut conn = self.db.get().await?;
            for b in conn.get_not_finished_builds().await? {
                let jobset = self
                    .create_jobset(&mut conn, b.jobset_id, &b.project, &b.jobset)
                    .await?;
                let build = Build::new(b, jobset)?;
                new_ids.push(build.id);
                new_builds_by_id.insert(build.id, build.clone());
                new_builds_by_path.insert(build.drv_path.clone(), build.id);
            }
        }
        log::debug!("new_ids: {new_ids:?}");
        log::debug!("new_builds_by_id: {new_builds_by_id:?}");
        log::debug!("new_builds_by_path: {new_builds_by_path:?}");

        self.process_new_builds(new_ids, new_builds_by_id, new_builds_by_path)
            .await;
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    pub fn start_queue_monitor_loop(self: Arc<Self>) {
        tokio::task::spawn({
            let state = self.clone();
            async move {
                if let Err(e) = state.queue_monitor_loop().await {
                    log::error!("Failed to spawn queue monitor loop. e={e}");
                }
            }
        });
    }

    #[tracing::instrument(skip(self), err)]
    async fn queue_monitor_loop(&self) -> anyhow::Result<()> {
        let mut listener = self
            .db
            .listener(vec![
                "builds_added",
                "builds_restarted",
                "builds_cancelled",
                "builds_deleted",
                "builds_dumped",
                "jobset_shares_changed",
            ])
            .await?;

        loop {
            let before_work = Instant::now();
            nix_utils::clear_query_path_cache().await;
            if let Err(e) = self.get_queued_builds().await {
                log::error!("get_queue_builds failed inside queue monitor loop: {e}");
                continue;
            }

            #[allow(clippy::cast_possible_truncation)]
            self.metrics
                .queue_monitor_time_spent_running
                .inc_by(before_work.elapsed().as_micros() as u64);

            let before_sleep = Instant::now();
            let notification = match listener.try_next().await {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(e) => {
                    log::warn!("PgListener failed with e={e}");
                    continue;
                }
            };
            log::trace!("New notification from PgListener. notification={notification:?}");

            match notification.channel() {
                "builds_added" => log::debug!("got notification: new builds added to the queue"),
                "builds_restarted" => log::debug!("got notification: builds restarted"),
                "builds_cancelled" | "builds_deleted" | "builds_bumped" => {
                    log::debug!("got notification: builds cancelled or bumped");
                    if let Err(e) = self.process_queue_change().await {
                        log::error!("Failed to process queue change. e={e}");
                    }
                }
                "jobset_shares_changed" => {
                    log::debug!("got notification: jobset shares changed");
                    if let Err(e) = self.handle_jobset_change().await {
                        log::error!("Failed to handle jobset change. e={e}");
                    }
                }
                _ => (),
            }

            #[allow(clippy::cast_possible_truncation)]
            self.metrics
                .queue_monitor_time_spent_waiting
                .inc_by(before_sleep.elapsed().as_micros() as u64);
        }
    }

    #[tracing::instrument(skip(self))]
    pub fn start_dispatch_loop(self: Arc<Self>) {
        tokio::task::spawn({
            async move {
                loop {
                    let before_sleep = Instant::now();
                    self.notify_dispatch.notified().await;

                    #[allow(clippy::cast_possible_truncation)]
                    self.metrics
                        .dispatcher_time_spent_waiting
                        .inc_by(before_sleep.elapsed().as_micros() as u64);

                    self.metrics.nr_dispatcher_wakeups.add(1);
                    let before_work = Instant::now();
                    self.do_dispatch_once().await;

                    let elapsed = before_work.elapsed();

                    #[allow(clippy::cast_possible_truncation)]
                    self.metrics
                        .dispatcher_time_spent_running
                        .inc_by(elapsed.as_micros() as u64);

                    #[allow(clippy::cast_possible_truncation)]
                    self.metrics
                        .dispatch_time_ms
                        .add(elapsed.as_millis() as i64);
                }
            }
        });
    }

    #[tracing::instrument(skip(self))]
    pub fn trigger_dispatch(&self) {
        self.notify_dispatch.notify_one();
    }

    #[tracing::instrument(skip(self))]
    async fn do_dispatch_once(&self) {
        let mut new_runnable = Vec::new();
        {
            let mut runnable = self.runnable.write();
            runnable.retain(|r| {
                if let Some(step) = r.upgrade() {
                    new_runnable.push(step.clone());
                    true
                } else {
                    false
                }
            });
        }

        let now = chrono::Utc::now();
        let mut new_queues = MultiMap::<String, StepInfo, ahash::RandomState>::default();
        for r in new_runnable {
            let Some(system) = r.get_system() else {
                continue;
            };
            let state = r.state.read();
            if r.atomic_state.tries.load(Ordering::SeqCst) > 0 && state.after > now {
                continue;
            }

            new_queues.insert(system, StepInfo::new(r.clone(), &state));
        }

        {
            let mut queues = self.queues.write().await;
            for (system, jobs) in new_queues {
                queues.insert_new_jobs(system, jobs, &now);
            }
        }

        {
            let mut nr_steps_waiting = 0;
            let queues = self.queues.read().await;
            for (system, queue) in queues.iter() {
                for job in queue.clone_inner() {
                    let Some(job) = job.upgrade() else {
                        continue;
                    };
                    if job.get_already_scheduled() {
                        continue;
                    }

                    match self
                        .realise_drv_on_valid_machine(job.step.clone(), system)
                        .await
                    {
                        Ok(true) => queues.add_job_to_scheduled(&job, queue),
                        Ok(_) => nr_steps_waiting += 1,
                        Err(e) => {
                            log::warn!(
                                "Failed to realise drv on valid machine, will be skipped: drv={} e={e}",
                                job.step.get_drv_path(),
                            );
                        }
                    }
                }
            }
            self.metrics.nr_steps_waiting.set(nr_steps_waiting);
        }
    }

    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(self, output), err)]
    pub async fn mark_step_done(
        &self,
        machine_id: Option<uuid::Uuid>,
        drv_path: &StorePath,
        output: BuildOutput,
    ) -> anyhow::Result<()> {
        let step = {
            let steps = self.steps.write();
            let Some(step) = steps.get(drv_path).and_then(Weak::upgrade) else {
                return Ok(());
            };
            step
        };
        step.set_finished(true);
        self.metrics.nr_steps_done.add(1);
        self.metrics.nr_steps_building.sub(1);

        {
            let mut queues = self.queues.write().await;
            log::info!("marking job as done: drv_path={drv_path}");
            queues.mark_job_done(drv_path);
        }

        let machine_job_tuple = if let Some(machine_id) = machine_id {
            if let Some(m) = self.machines.get_machine_by_id(machine_id) {
                log::debug!("removing job from machine: drv_path={drv_path} m={}", m.id);
                let j = m.remove_job(drv_path);
                (Some(m), j)
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        let (Some(machine), Some(mut job)) = machine_job_tuple else {
            // Seems like we did not have a job scheduled
            // aka something went wrong.
            // We trigger dispatch and try to recover but we cant update the DB
            log::warn!("Failed to find job and machine.");
            self.trigger_dispatch();

            return Ok(());
        };
        job.result.step_status = BuildStatus::Success;
        job.result.stop_time = chrono::Utc::now().timestamp();

        {
            let mut db = self.db.get().await?;
            let mut tx = db.begin_transaction().await?;
            finish_build_step(
                &mut tx,
                job.build_id,
                job.step_nr,
                &job.result,
                Some(machine.hostname.clone()),
            )
            .await?;
            tx.commit().await?;
        }
        let remote_store_url = {
            let config = self.config.read().await;
            config.get_remote_store_addr()
        };
        if let Some(url) = remote_store_url {
            let outputs = output.outputs.clone();
            tokio::spawn(async move {
                use futures::stream::StreamExt as _;

                let remote_store = nix_utils::RemoteStore::new(&url);
                let mut stream =
                    futures::StreamExt::map(tokio_stream::iter(outputs), |(_, path)| {
                        // TODO: copy logs, currently not possible with cli interface
                        remote_store.copy_path(path)
                    })
                    .buffer_unordered(10);
                while let Some(v) = tokio_stream::StreamExt::next(&mut stream).await {
                    if let Err(e) = v {
                        log::error!("Failed to copy path to remote store: {e}");
                    }
                }
            });
        }

        let mut direct = Vec::new();
        {
            let state = step.state.read();
            for b in &state.builds {
                let Some(b) = b.upgrade() else {
                    continue;
                };
                if !b.finished_in_db.load(Ordering::SeqCst) {
                    direct.push(b);
                }
            }

            if direct.is_empty() {
                let mut steps = self.steps.write();
                steps.retain(|s, _| s != step.get_drv_path());
            }
        }

        {
            let mut db = self.db.get().await?;
            let mut tx = db.begin_transaction().await?;
            for b in &direct {
                let is_cached = job.build_id != b.id || job.result.is_cached;
                tx.mark_succeeded_build(
                    b.clone(),
                    &output,
                    is_cached,
                    i32::try_from(job.result.start_time)?,
                    i32::try_from(job.result.stop_time)?,
                )
                .await?;
                self.metrics.nr_builds_done.add(1);
            }

            tx.commit().await?;
        }

        {
            // Remove the direct dependencies from 'builds'. This will cause them to be
            // destroyed.
            let mut current_builds = self.builds.write();
            for b in &direct {
                b.finished_in_db.store(true, Ordering::SeqCst);
                current_builds.remove(&b.id);
            }
        }

        {
            let mut db = self.db.get().await?;
            let mut tx = db.begin_transaction().await?;
            for b in direct {
                tx.notify_build_finished(b.id, &[]).await?;
            }

            tx.commit().await?;
        }

        {
            let state = step.state.write();
            for rdep in &state.rdeps {
                let Some(rdep) = rdep.upgrade() else {
                    continue;
                };

                let mut runnable = false;
                {
                    let mut rdep_state = rdep.state.write();
                    rdep_state
                        .deps
                        .retain(|s| s.get_drv_path() != step.get_drv_path());
                    if rdep_state.deps.is_empty()
                        && rdep.atomic_state.created.load(Ordering::SeqCst)
                    {
                        runnable = true;
                    }
                }

                if runnable {
                    self.make_runnable(&rdep);
                }
            }
        }

        // always trigger dispatch, as we now might have a free machine again
        self.trigger_dispatch();

        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(self), err)]
    pub async fn fail_step(
        &self,
        machine_id: Option<uuid::Uuid>,
        drv_path: &StorePath,
    ) -> anyhow::Result<()> {
        let step = {
            let steps = self.steps.write();
            let Some(step) = steps.get(drv_path).and_then(Weak::upgrade) else {
                return Ok(());
            };
            step
        };
        step.set_finished(false);
        self.metrics.nr_steps_done.add(1);
        self.metrics.nr_steps_building.add(1);

        {
            let queues = self.queues.write().await;
            // TODO: max failure count
            log::info!("removing job from running in system queue: drv_path={drv_path}");
            queues.remove_job_from_scheduled(drv_path);
        }

        let machine_job_tuple = if let Some(machine_id) = machine_id {
            if let Some(m) = self.machines.get_machine_by_id(machine_id) {
                log::debug!("removing job from machine: drv_path={drv_path} m={}", m.id);
                let j = m.remove_job(drv_path);
                (Some(m), j)
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        let (Some(machine), Some(job)) = machine_job_tuple else {
            // Seems like we did not have a job scheduled
            // aka something went wrong.
            // We trigger dispatch and try to recover but we cant update the DB
            self.trigger_dispatch();

            return Ok(());
        };

        self.inner_fail_job(drv_path, Some(machine), job, step)
            .await
    }

    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(self), err)]
    async fn inner_fail_job(
        &self,
        drv_path: &StorePath,
        machine: Option<Arc<Machine>>,
        job: machine::Job,
        step: Arc<Step>,
    ) -> anyhow::Result<()> {
        // TODO: builder:415
        let mut dependent_ids = Vec::new();
        loop {
            let indirect = self.get_all_indirect_builds(&step);
            // TODO: stepFinished ?
            if indirect.is_empty() {
                break;
            }

            // Create failed build steps for every build that depends on this, except when this
            // step is cached and is the top-level of that build (since then it's redundant with
            // the build's isCachedBuild field).
            {
                let mut db = self.db.get().await?;
                let mut tx = db.begin_transaction().await?;
                for b in &indirect {
                    if (job.result.step_status == BuildStatus::CachedFailure
                        && &b.drv_path == step.get_drv_path())
                        || ((job.result.step_status != BuildStatus::CachedFailure
                            && job.result.step_status != BuildStatus::Unsupported)
                            && job.build_id == b.id)
                        || b.finished_in_db.load(Ordering::SeqCst)
                    {
                        continue;
                    }

                    tx.create_build_step(
                        None,
                        b.id,
                        step.clone(),
                        machine
                            .as_deref()
                            .map(|m| m.hostname.clone())
                            .unwrap_or_default(),
                        job.result.step_status,
                        job.result.error_msg.clone(),
                        if job.build_id == b.id {
                            None
                        } else {
                            Some(job.build_id)
                        },
                    )
                    .await?;
                }

                // Mark all builds that depend on this derivation as failed.
                for b in &indirect {
                    if b.finished_in_db.load(Ordering::SeqCst) {
                        continue;
                    }

                    log::error!("marking build {} as failed", b.id);
                    tx.update_build_after_failure(
                        b.id,
                        if &b.drv_path != step.get_drv_path()
                            && job.result.step_status == BuildStatus::Failed
                        {
                            BuildStatus::DepFailed
                        } else {
                            job.result.step_status
                        },
                        i32::try_from(job.result.start_time)?,
                        i32::try_from(job.result.stop_time)?,
                        job.result.step_status == BuildStatus::CachedFailure,
                    )
                    .await?;
                    self.metrics.nr_builds_done.add(1);
                }

                // Remember failed paths in the database so that they won't be built again.
                if job.result.step_status == BuildStatus::CachedFailure && job.result.can_cache {
                    for o in step.get_outputs().unwrap_or_default() {
                        let Some(p) = o.path else { continue };
                        tx.insert_failed_paths(&p).await?;
                    }
                }

                tx.commit().await?;
            }

            {
                // Remove the indirect dependencies from 'builds'. This will cause them to be
                // destroyed.
                let mut current_builds = self.builds.write();
                for b in indirect {
                    b.finished_in_db.store(true, Ordering::SeqCst);
                    current_builds.remove(&b.id);
                    dependent_ids.push(b.id);
                }
            }
        }
        {
            let mut db = self.db.get().await?;
            let mut tx = db.begin_transaction().await?;
            tx.notify_build_finished(job.build_id, &dependent_ids)
                .await?;
            tx.commit().await?;
        }

        // trigger dispatch, as we now have a free mashine again
        self.trigger_dispatch();

        Ok(())
    }

    #[tracing::instrument(skip(self, step))]
    fn get_all_indirect_builds(&self, step: &Arc<Step>) -> AHashSet<Arc<Build>> {
        let mut indirect = AHashSet::new();
        let mut steps = AHashSet::new();
        step.get_dependents(&mut indirect, &mut steps);

        // If there are no builds left, delete all referring
        // steps from ‘steps’. As for the success case, we can
        // be certain no new referrers can be added.
        if indirect.is_empty() {
            let mut current_steps_map = self.steps.write();
            for s in steps {
                let drv = s.get_drv_path();
                log::debug!("finishing build step '{drv}'");
                current_steps_map.retain(|path, _| path != drv);
            }
        }

        indirect
    }

    #[tracing::instrument(skip(self, conn), err)]
    async fn create_jobset(
        &self,
        conn: &mut crate::db::Connection,
        jobset_id: i32,
        project_name: &str,
        jobset_name: &str,
    ) -> anyhow::Result<Arc<Jobset>> {
        let key = (project_name.to_owned(), jobset_name.to_owned());
        {
            let jobsets = self.jobsets.read();
            if let Some(jobset) = jobsets.get(&key) {
                return Ok(jobset.clone());
            }
        }

        let shares = conn
            .get_jobset_scheduling_shares(jobset_id)
            .await?
            .ok_or(anyhow::anyhow!(
                "Scheduling Shares not found for jobset not found."
            ))?;
        let jobset = Jobset::new(jobset_id, project_name, jobset_name);
        jobset.set_shares(shares)?;

        for step in conn.get_jobset_build_steps(jobset_id).await? {
            let Some(starttime) = step.starttime else {
                continue;
            };
            let Some(stoptime) = step.stoptime else {
                continue;
            };
            jobset.add_step(i64::from(starttime), i64::from(stoptime - starttime));
        }

        let jobset = Arc::new(jobset);
        {
            let mut jobsets = self.jobsets.write();
            jobsets.insert(key, jobset.clone());
        }

        Ok(jobset.clone())
    }

    #[tracing::instrument(skip(self, build, step), err)]
    async fn handle_previous_failure(
        &self,
        build: Arc<Build>,
        step: Arc<Step>,
    ) -> anyhow::Result<()> {
        // Some step previously failed, so mark the build as failed right away.
        log::error!(
            "marking build {} as cached failure due to '{}'",
            build.id,
            step.get_drv_path()
        );
        if build.finished_in_db.load(Ordering::SeqCst) {
            return Ok(());
        }

        // if !build.finished_in_db
        let mut conn = self.db.get().await?;
        let mut tx = conn.begin_transaction().await?;

        // Find the previous build step record, first by derivation path, then by output
        // path.
        let mut propagated_from = tx
            .get_last_build_step_id(step.get_drv_path())
            .await?
            .unwrap_or_default();

        if propagated_from == 0 {
            // we can access step.drv here because the value is always set if
            // PreviousFailure is returned, so this should never yield None

            let outputs = step.get_outputs().unwrap_or_default();
            for o in outputs {
                let res = if let Some(path) = &o.path {
                    tx.get_last_build_step_id_for_output_path(path).await
                } else {
                    tx.get_last_build_step_id_for_output_with_drv(step.get_drv_path(), &o.name)
                        .await
                };
                if let Ok(Some(res)) = res {
                    propagated_from = res;
                    break;
                }
            }
        }

        tx.create_build_step(
            None,
            build.id,
            step.clone(),
            String::new(),
            BuildStatus::CachedFailure,
            None,
            Some(propagated_from),
        )
        .await?;
        tx.update_build_after_previous_failure(
            build.id,
            if step.get_drv_path() == &build.drv_path {
                BuildStatus::Failed
            } else {
                BuildStatus::DepFailed
            },
        )
        .await?;

        let _ = tx.notify_build_finished(build.id, &[]).await;
        tx.commit().await?;

        build.finished_in_db.store(true, Ordering::SeqCst);
        self.metrics.nr_builds_done.add(1);
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(
        self,
        build,
        nr_added,
        new_builds_by_id,
        new_builds_by_path,
        finished_drvs,
        new_runnable
    ))]
    async fn create_build(
        &self,
        build: Arc<Build>,
        nr_added: &mut i64,
        new_builds_by_id: &mut AHashMap<BuildID, Arc<Build>>,
        new_builds_by_path: &MultiMap<StorePath, BuildID, ahash::RandomState>,
        finished_drvs: &mut AHashSet<StorePath>,
        new_runnable: &mut AHashSet<Arc<Step>>,
    ) {
        self.metrics.queue_build_loads.inc();
        log::info!("loading build {} ({})", build.id, build.full_job_name());
        *nr_added += 1;
        new_builds_by_id.remove(&build.id);

        if !nix_utils::check_if_storepath_exists(&build.drv_path) {
            log::error!("aborting GC'ed build {}", build.id);
            if !build.finished_in_db.load(Ordering::SeqCst) {
                match self.db.get().await {
                    Ok(mut conn) => {
                        if let Err(e) = conn.abort_build(build.id).await {
                            log::error!("Failed to abort the build={} e={}", build.id, e);
                        }
                    }
                    Err(e) => log::error!(
                        "Failed to get database connection so we can abort the build={} e={}",
                        build.id,
                        e
                    ),
                }
            }

            build.finished_in_db.store(true, Ordering::SeqCst);
            self.metrics.nr_builds_done.add(1);
            return;
        }

        // Create steps for this derivation and its dependencies.
        let mut new_steps = AHashSet::<Arc<Step>>::new();
        let step = match self
            .create_step(
                // conn,
                build.clone(),
                &build.drv_path,
                Some(build.clone()),
                None,
                finished_drvs,
                &mut new_steps,
                new_runnable,
            )
            .await
        {
            CreateStepResult::None => None,
            CreateStepResult::Valid(dep) => Some(dep),
            CreateStepResult::PreviousFailure(step) => {
                if let Err(e) = self.handle_previous_failure(build, step).await {
                    log::error!("Failed to handle previous failure: {e}");
                }
                return;
            }
        };

        for r in &new_steps {
            let Some(builds) = new_builds_by_path.get_vec(r.get_drv_path()) else {
                continue;
            };
            for i in builds {
                let Some(j) = new_builds_by_id.get(i) else {
                    continue;
                };
                Box::pin(self.create_build(
                    j.clone(),
                    nr_added,
                    new_builds_by_id,
                    new_builds_by_path,
                    finished_drvs,
                    new_runnable,
                ))
                .await;
            }
        }

        if let Some(step) = step {
            if !build.finished_in_db.load(Ordering::SeqCst) {
                let mut builds = self.builds.write();
                builds.insert(build.id, build.clone());
            }

            build.set_toplevel_step(step.clone());
            build.propagate_priorities();

            log::info!(
                "added build {} (top-level step {}, {} new steps)",
                build.id,
                step.get_drv_path(),
                new_steps.len()
            );
        } else {
            // If we didn't get a step, it means the step's outputs are
            // all valid. So we mark this as a finished, cached build.
            // TODO

            // BuildOutput res = getBuildOutputCached(conn, destStore, build->drvPath);
            //
            // for (auto & i : destStore->queryDerivationOutputMap(build->drvPath, &*localStore))
            //     addRoot(i.second);
            //
            // {
            // auto mc = startDbUpdate();
            // pqxx::work txn(conn);
            // time_t now = time(0);
            // if (!buildOneDone && build->id == buildOne) buildOneDone = true;
            // printMsg(lvlInfo, "marking build %1% as succeeded (cached)", build->id);
            // markSucceededBuild(txn, build, res, true, now, now);
            // notifyBuildFinished(txn, build->id, {});
            // txn.commit();
            // }
            build.finished_in_db.store(true, Ordering::SeqCst);
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(skip(
        self,
        build,
        drv_path,
        referring_build,
        referring_step,
        finished_drvs,
        new_steps,
        new_runnable
    ))]
    async fn create_step(
        &self,
        build: Arc<Build>,
        drv_path: &StorePath,
        referring_build: Option<Arc<Build>>,
        referring_step: Option<Arc<Step>>,
        finished_drvs: &mut AHashSet<StorePath>,
        new_steps: &mut AHashSet<Arc<Step>>,
        new_runnable: &mut AHashSet<Arc<Step>>,
    ) -> CreateStepResult {
        if finished_drvs.contains(drv_path) {
            return CreateStepResult::None;
        }

        let mut is_new = false;
        let step = {
            let mut steps = self.steps.write();
            let step = if let Some(step) = steps.get(drv_path) {
                if let Some(step) = step.upgrade() {
                    step
                } else {
                    steps.remove(drv_path);
                    is_new = true;
                    Step::new(drv_path.to_owned())
                }
            } else {
                is_new = true;
                Step::new(drv_path.to_owned())
            };

            {
                let mut state = step.state.write();
                if let Some(referring_build) = referring_build {
                    state.builds.push(Arc::downgrade(&referring_build));
                }
                if let Some(referring_step) = referring_step {
                    state.rdeps.push(Arc::downgrade(&referring_step));
                }
            }

            steps.insert(drv_path.clone(), Arc::downgrade(&step));
            step
        };

        if !is_new {
            return CreateStepResult::Valid(step);
        }
        self.metrics.queue_steps_created.inc();
        log::debug!("considering derivation '{drv_path}'");

        let Some(drv) = nix_utils::query_drv(drv_path).await.ok().flatten() else {
            return CreateStepResult::None;
        };

        let (use_substitute, remote_store_url) = {
            let config = self.config.read().await;
            (config.use_substitute, config.get_remote_store_addr())
        };
        let missing_outputs = if let Some(remote_store_url) = remote_store_url.as_deref() {
            nix_utils::query_missing_remote_outputs(drv.outputs.clone(), remote_store_url).await
        } else {
            nix_utils::query_missing_outputs(&drv.outputs)
        };
        step.set_drv(drv);

        if self.check_cached_failure(step.clone()).await {
            return CreateStepResult::PreviousFailure(step);
        }

        log::debug!("missing outputs: {missing_outputs:?}");
        let mut valid = missing_outputs.is_empty();
        if !missing_outputs.is_empty() && use_substitute {
            use futures::stream::StreamExt as _;

            let mut substituted = 0;
            let missing_outputs_len = missing_outputs.len();
            let build_opts = nix_utils::BuildOptions::substitute_only();

            let remote_store = remote_store_url.as_deref().map(nix_utils::RemoteStore::new);
            let mut stream = futures::StreamExt::map(tokio_stream::iter(missing_outputs), |o| {
                crate::utils::substitute_output(
                    self.db.clone(),
                    o,
                    build.id,
                    drv_path,
                    &build_opts,
                    remote_store.as_ref(),
                )
            })
            .buffer_unordered(10);
            while let Some(v) = tokio_stream::StreamExt::next(&mut stream).await {
                match v {
                    Ok(()) => substituted += 1,
                    Err(e) => {
                        log::error!("Failed to substitute path: {e}");
                    }
                }
            }
            valid = substituted == missing_outputs_len;
        }

        if valid {
            finished_drvs.insert(drv_path.to_owned());
            return CreateStepResult::None;
        }

        log::debug!("creating build step '{drv_path}");
        let Some(input_drvs) = step.get_inputs() else {
            return CreateStepResult::None;
        };

        // TODO: paralise
        for i in &input_drvs {
            match Box::pin(self.create_step(
                // conn,
                build.clone(),
                i,
                None,
                Some(step.clone()),
                finished_drvs,
                new_steps,
                new_runnable,
            ))
            .await
            {
                CreateStepResult::None => (),
                CreateStepResult::Valid(dep) => {
                    let mut state = step.state.write();
                    state.deps.insert(dep);
                }
                CreateStepResult::PreviousFailure(step) => {
                    return CreateStepResult::PreviousFailure(step);
                }
            }
        }

        {
            let state = step.state.read();
            step.atomic_state.created.store(true, Ordering::SeqCst);
            if state.deps.is_empty() {
                new_runnable.insert(step.clone());
            }
        }

        new_steps.insert(step.clone());
        CreateStepResult::Valid(step)
    }

    #[tracing::instrument(skip(self, step), ret)]
    async fn check_cached_failure(&self, step: Arc<Step>) -> bool {
        let Some(drv_outputs) = step.get_outputs() else {
            return false;
        };

        let Ok(mut conn) = self.db.get().await else {
            return false;
        };
        for o in &drv_outputs {
            if let Some(path) = &o.path {
                if conn.check_if_path_failed(path).await.unwrap_or_default() {
                    return true;
                }
            }
        }

        false
    }

    #[tracing::instrument(skip(self, step))]
    fn make_runnable(&self, step: &Arc<Step>) {
        log::info!("step '{}' is now runnable", step.get_drv_path());

        {
            let mut state = step.state.write();
            debug_assert!(step.atomic_state.created.load(Ordering::SeqCst));
            debug_assert!(!step.get_finished());
            debug_assert!(state.deps.is_empty());
            state.runnable_since = chrono::Utc::now();
        }

        {
            let mut runnable = self.runnable.write();
            runnable.push(Arc::downgrade(step));
        }
    }

    #[tracing::instrument(skip(self), err)]
    async fn handle_jobset_change(&self) -> anyhow::Result<()> {
        let curr_jobsets_in_db = self.db.get().await?.get_jobsets().await?;

        let jobsets = self.jobsets.read();
        for row in curr_jobsets_in_db {
            if let Some(i) = jobsets.get(&(row.project.clone(), row.name.clone())) {
                if let Err(e) = i.set_shares(row.schedulingshares) {
                    log::error!(
                        "Failed to update jobset scheduling shares. project_name={} jobset_name={} e={}",
                        row.project,
                        row.name,
                        e,
                    );
                }
            }
        }

        Ok(())
    }
}
