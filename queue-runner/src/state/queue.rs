use std::sync::Weak;
use std::sync::atomic::Ordering;
use std::sync::{Arc, atomic::AtomicBool};

use ahash::{AHashMap, AHashSet};

use super::System;
use super::build::{BuildID, Step, StepState};

type Counter = std::sync::atomic::AtomicU64;

pub struct StepInfo {
    pub step: Arc<Step>,
    already_scheduled: AtomicBool,
    cancelled: AtomicBool,
    pub runnable_since: chrono::DateTime<chrono::Utc>,

    pub lowest_share_used: f64,
    pub highest_global_priority: i32,
    pub highest_local_priority: i32,
    pub lowest_build_id: BuildID,
}

impl PartialEq for StepInfo {
    fn eq(&self, other: &Self) -> bool {
        self.step == other.step
    }
}

impl Eq for StepInfo {}

impl std::hash::Hash for StepInfo {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.step.hash(state);
    }
}

impl StepInfo {
    pub fn new(step: Arc<Step>, state: &StepState) -> Self {
        let lowest_share_used = state
            .jobsets
            .iter()
            .map(|v| v.share_used())
            .min_by(f64::total_cmp)
            .unwrap_or(1e9);

        Self {
            already_scheduled: false.into(),
            cancelled: false.into(),
            runnable_since: state.runnable_since,
            lowest_share_used,
            highest_global_priority: step
                .atomic_state
                .highest_global_priority
                .load(Ordering::SeqCst),
            highest_local_priority: step
                .atomic_state
                .highest_local_priority
                .load(Ordering::SeqCst),
            lowest_build_id: step.atomic_state.lowest_build_id.load(Ordering::SeqCst),
            step,
        }
    }

    pub fn get_already_scheduled(&self) -> bool {
        self.already_scheduled.load(Ordering::SeqCst)
    }

    pub fn set_already_scheduled(&self, v: bool) {
        self.already_scheduled.store(v, Ordering::SeqCst);
    }

    pub fn get_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

pub struct BuildQueue {
    // Note: ensure that this stays private
    jobs: parking_lot::RwLock<Vec<Weak<StepInfo>>>,

    active_runnable: Counter,
    total_runnable: Counter,
    avg_runnable_time: Counter,
    wait_time: Counter,
}

pub struct BuildQueueStats {
    pub active_runnable: u64,
    pub total_runnable: u64,
    pub avg_runnable_time: u64,
    pub wait_time: u64,
}

impl BuildQueue {
    fn new() -> Self {
        Self {
            jobs: parking_lot::RwLock::new(Vec::new()),
            active_runnable: 0.into(),
            total_runnable: 0.into(),
            avg_runnable_time: 0.into(),
            wait_time: 0.into(),
        }
    }

    pub fn incr_active(&self) {
        self.active_runnable.fetch_add(1, Ordering::SeqCst);
    }

    pub fn decr_active(&self) {
        self.active_runnable.fetch_sub(1, Ordering::SeqCst);
    }

    #[tracing::instrument(skip(self, jobs))]
    pub fn insert_new_jobs(&self, jobs: Vec<Weak<StepInfo>>, now: &chrono::DateTime<chrono::Utc>) {
        let mut current_jobs = self.jobs.write();
        let mut wait_time = 0u64;

        // this ensures we only ever have each step once
        // so ensure that current_jobs is never written anywhere else
        for j in jobs {
            if let Some(owned) = j.upgrade() {
                // runnable since is always > now
                wait_time += (*now - owned.runnable_since).num_seconds().unsigned_abs();
                current_jobs.push(j);
            }
        }
        self.wait_time.fetch_add(wait_time, Ordering::SeqCst);

        // only keep valid pointers
        current_jobs.retain(|v| v.upgrade().is_some());
        self.total_runnable
            .store(current_jobs.len() as u64, Ordering::SeqCst);

        let delta = 0.00001;
        current_jobs.sort_by(|a, b| {
            let a = a.upgrade();
            let b = b.upgrade();
            match (a, b) {
                (Some(a), Some(b)) => {
                    if a.highest_global_priority != b.highest_global_priority {
                        a.highest_global_priority.cmp(&b.highest_global_priority)
                    } else if (a.lowest_share_used - b.lowest_share_used).abs() > delta {
                        b.lowest_share_used.total_cmp(&a.lowest_share_used)
                    } else if a.highest_local_priority != b.highest_local_priority {
                        a.highest_local_priority.cmp(&b.highest_local_priority)
                    } else {
                        b.lowest_build_id.cmp(&a.lowest_build_id)
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
    }

    #[tracing::instrument(skip(self))]
    pub fn scrube_jobs(&self) {
        let mut current_jobs = self.jobs.write();
        current_jobs.retain(|v| v.upgrade().is_some());
        self.total_runnable
            .store(current_jobs.len() as u64, Ordering::SeqCst);
    }

    pub fn clone_inner(&self) -> Vec<Weak<StepInfo>> {
        (*self.jobs.read()).clone()
    }

    pub fn get_stats(&self) -> BuildQueueStats {
        BuildQueueStats {
            active_runnable: self.active_runnable.load(Ordering::SeqCst),
            total_runnable: self.total_runnable.load(Ordering::SeqCst),
            avg_runnable_time: self.avg_runnable_time.load(Ordering::SeqCst),
            wait_time: self.wait_time.load(Ordering::SeqCst),
        }
    }
}

pub struct Queues {
    // flat list of all step infos in queues, owning those steps inner queue dont own them
    jobs: AHashSet<Arc<StepInfo>>,
    inner: AHashMap<System, Arc<BuildQueue>>,
    #[allow(clippy::type_complexity)]
    scheduled: parking_lot::RwLock<
        AHashMap<nix_utils::StorePath, (Arc<StepInfo>, Arc<BuildQueue>, Arc<super::Machine>)>,
    >,
}

impl Queues {
    pub fn new() -> Self {
        Self {
            jobs: AHashSet::new(),
            inner: AHashMap::new(),
            scheduled: parking_lot::RwLock::new(AHashMap::new()),
        }
    }

    #[tracing::instrument(skip(self, jobs))]
    pub fn insert_new_jobs<S: Into<String> + std::fmt::Debug>(
        &mut self,
        system: S,
        jobs: Vec<StepInfo>,
        now: &chrono::DateTime<chrono::Utc>,
    ) {
        let mut submit_jobs: Vec<Weak<StepInfo>> = Vec::new();
        for j in jobs {
            let j = Arc::new(j);
            if !self.jobs.contains(&j) {
                self.jobs.insert(j.clone());
                submit_jobs.push(Arc::downgrade(&j));
            }
        }

        let queue = self
            .inner
            .entry(system.into())
            .or_insert_with(|| Arc::new(BuildQueue::new()));
        // queues are sorted afterwards
        queue.insert_new_jobs(submit_jobs, now);
    }

    #[tracing::instrument(skip(self))]
    pub fn remove_all_weak_pointer(&mut self) {
        for queue in self.inner.values() {
            queue.scrube_jobs();
        }
    }

    pub fn iter(&self) -> std::collections::hash_map::Iter<'_, System, Arc<BuildQueue>> {
        self.inner.iter()
    }

    #[tracing::instrument(skip(self, step, queue))]
    pub fn add_job_to_scheduled(
        &self,
        step: &Arc<StepInfo>,
        queue: &Arc<BuildQueue>,
        machine: Arc<super::Machine>,
    ) {
        let mut scheduled = self.scheduled.write();

        let drv = step.step.get_drv_path();
        scheduled.insert(drv.to_owned(), (step.clone(), queue.clone(), machine));
        step.already_scheduled.store(true, Ordering::SeqCst);
        queue.incr_active();
    }

    #[tracing::instrument(skip(self), fields(%drv))]
    pub fn remove_job_from_scheduled(
        &self,
        drv: &nix_utils::StorePath,
    ) -> Option<(Arc<StepInfo>, Arc<BuildQueue>, Arc<super::Machine>)> {
        let mut scheduled = self.scheduled.write();

        let (step_info, queue, machine) = scheduled.remove(drv)?;
        step_info.already_scheduled.store(false, Ordering::SeqCst);
        queue.decr_active();
        Some((step_info, queue, machine))
    }

    #[tracing::instrument(skip(self, stepinfo, queue))]
    pub fn remove_job(&mut self, stepinfo: &Arc<StepInfo>, queue: &Arc<BuildQueue>) {
        self.jobs.remove(stepinfo);
        // active should be removed
        queue.scrube_jobs();
    }

    #[tracing::instrument(skip(self), fields(%drv))]
    pub fn mark_job_done(&mut self, drv: &nix_utils::StorePath) {
        let Some((stepinfo, queue, _)) = ({
            let mut scheduled = self.scheduled.write();
            scheduled.remove(drv)
        }) else {
            return;
        };

        self.jobs.remove(&stepinfo);
        drop(stepinfo);
        queue.decr_active();
        queue.scrube_jobs();
    }

    #[tracing::instrument(skip(self))]
    pub async fn kill_active_steps(&self) {
        let active = {
            let scheduled = self.scheduled.read();
            scheduled.clone()
        };
        for (drv_path, (step_info, _, machine)) in &active {
            if step_info.cancelled.load(Ordering::SeqCst) {
                continue;
            }

            let mut dependents = AHashSet::new();
            let mut steps = AHashSet::new();
            step_info.step.get_dependents(&mut dependents, &mut steps);
            if !dependents.is_empty() {
                continue;
            }

            {
                step_info.cancelled.store(true, Ordering::SeqCst);
                machine.abort_build(drv_path).await;
                // TODO fail job
            }
        }
    }

    #[tracing::instrument(skip(self))]
    pub fn get_stats_per_queue(&self) -> AHashMap<System, BuildQueueStats> {
        self.inner
            .iter()
            .map(|(k, v)| (k.clone(), v.get_stats()))
            .collect()
    }

    pub fn get_jobs(&self) -> Vec<Arc<StepInfo>> {
        self.jobs.iter().map(Clone::clone).collect()
    }

    pub fn get_scheduled(&self) -> Vec<Arc<StepInfo>> {
        let s = self.scheduled.read();
        s.iter().map(|(_, (s, _, _))| s.clone()).collect()
    }
}
