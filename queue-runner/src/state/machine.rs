use std::sync::{Arc, atomic::Ordering};

use ahash::AHashMap;
use tokio::sync::mpsc;

use super::System;
use super::build::{BuildID, RemoteBuild};
use crate::{
    config::MachineSortFn,
    server::grpc::runner_v1::{AbortMessage, BuildMessage, JoinMessage, runner_request},
};

type Counter = std::sync::atomic::AtomicU64;

#[derive(Debug)]
pub struct Pressure {
    pub avg10: atomic_float::AtomicF32,
    pub avg60: atomic_float::AtomicF32,
    pub avg300: atomic_float::AtomicF32,
    pub total: Counter,
}

impl Pressure {
    fn new() -> Self {
        Self {
            avg10: 0.0.into(),
            avg60: 0.0.into(),
            avg300: 0.0.into(),
            total: 0.into(),
        }
    }

    pub fn store_ping_pressure(&self, msg: Option<crate::server::grpc::runner_v1::Pressure>) {
        let Some(msg) = msg else { return };
        self.avg10.store(msg.avg10, Ordering::SeqCst);
        self.avg60.store(msg.avg60, Ordering::SeqCst);
        self.avg300.store(msg.avg300, Ordering::SeqCst);
        self.total.store(msg.total, Ordering::SeqCst);
    }

    pub fn get_avg10(&self) -> f32 {
        self.avg10.load(Ordering::SeqCst)
    }

    pub fn get_avg60(&self) -> f32 {
        self.avg60.load(Ordering::SeqCst)
    }

    pub fn get_avg300(&self) -> f32 {
        self.avg300.load(Ordering::SeqCst)
    }

    pub fn get_total(&self) -> u64 {
        self.total.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
pub struct Stats {
    current_jobs: Counter,
    nr_steps_done: Counter,
    total_step_time: Counter,
    total_step_build_time: Counter,
    idle_since: std::sync::atomic::AtomicI64,

    last_failure: std::sync::atomic::AtomicI64,
    disabled_until: std::sync::atomic::AtomicI64,
    consecutive_failures: Counter,
    last_ping: std::sync::atomic::AtomicI64,

    load1: atomic_float::AtomicF32,
    load5: atomic_float::AtomicF32,
    load15: atomic_float::AtomicF32,
    mem_usage: Counter,
    pub cpu_some_psi: Pressure,
    pub mem_some_psi: Pressure,
    pub mem_full_psi: Pressure,
    pub io_some_psi: Pressure,
    pub io_full_psi: Pressure,
}

impl Stats {
    pub fn new() -> Self {
        Self {
            current_jobs: 0.into(),
            nr_steps_done: 0.into(),
            total_step_time: 0.into(),
            total_step_build_time: 0.into(),
            idle_since: (chrono::Utc::now().timestamp()).into(),
            last_failure: 0.into(),
            disabled_until: 0.into(),
            consecutive_failures: 0.into(),
            last_ping: 0.into(),

            load1: 0.0.into(),
            load5: 0.0.into(),
            load15: 0.0.into(),
            mem_usage: 0.into(),

            cpu_some_psi: Pressure::new(),
            mem_some_psi: Pressure::new(),
            mem_full_psi: Pressure::new(),
            io_some_psi: Pressure::new(),
            io_full_psi: Pressure::new(),
        }
    }

    pub fn store_current_jobs(&self, c: u64) {
        if c == 0 && self.idle_since.load(Ordering::SeqCst) == 0 {
            self.idle_since
                .store(chrono::Utc::now().timestamp(), Ordering::SeqCst);
        } else {
            self.idle_since.store(0, Ordering::SeqCst);
        }

        self.current_jobs.store(c, Ordering::SeqCst);
    }

    pub fn get_current_jobs(&self) -> u64 {
        self.current_jobs.load(Ordering::SeqCst)
    }

    pub fn get_nr_steps_done(&self) -> u64 {
        self.nr_steps_done.load(Ordering::SeqCst)
    }

    pub fn incr_nr_steps_done(&self) {
        self.nr_steps_done.fetch_add(1, Ordering::SeqCst);
    }

    pub fn get_total_step_time(&self) -> u64 {
        self.total_step_time.load(Ordering::SeqCst)
    }

    pub fn add_to_total_step_time(&self, v: u64) {
        self.total_step_time.fetch_add(v, Ordering::SeqCst);
    }

    pub fn get_total_step_build_time(&self) -> u64 {
        self.total_step_build_time.load(Ordering::SeqCst)
    }

    pub fn add_to_total_build_step_time(&self, v: u64) {
        self.total_step_build_time.fetch_add(v, Ordering::SeqCst);
    }

    pub fn get_idle_since(&self) -> i64 {
        self.idle_since.load(Ordering::SeqCst)
    }

    pub fn get_last_failure(&self) -> i64 {
        self.last_failure.load(Ordering::SeqCst)
    }

    pub fn store_last_failure_now(&self) {
        self.last_failure
            .store(chrono::Utc::now().timestamp(), Ordering::SeqCst);
    }

    pub fn get_disabled_until(&self) -> i64 {
        self.disabled_until.load(Ordering::SeqCst)
    }

    pub fn get_consecutive_failures(&self) -> u64 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }

    pub fn get_last_ping(&self) -> i64 {
        self.last_ping.load(Ordering::SeqCst)
    }

    pub fn store_ping(&self, msg: &crate::server::grpc::runner_v1::PingMessage) {
        self.last_ping
            .store(chrono::Utc::now().timestamp(), Ordering::SeqCst);

        self.load1.store(msg.load1, Ordering::SeqCst);
        self.load5.store(msg.load5, Ordering::SeqCst);
        self.load15.store(msg.load15, Ordering::SeqCst);
        self.mem_usage.store(msg.mem_usage, Ordering::SeqCst);

        self.cpu_some_psi.store_ping_pressure(msg.cpu_some);
        self.mem_some_psi.store_ping_pressure(msg.mem_some);
        self.mem_full_psi.store_ping_pressure(msg.mem_full);
        self.io_some_psi.store_ping_pressure(msg.io_some);
        self.io_full_psi.store_ping_pressure(msg.io_full);
    }

    pub fn get_load1(&self) -> f32 {
        self.load1.load(Ordering::SeqCst)
    }

    pub fn get_load5(&self) -> f32 {
        self.load5.load(Ordering::SeqCst)
    }

    pub fn get_load15(&self) -> f32 {
        self.load15.load(Ordering::SeqCst)
    }

    pub fn get_mem_usage(&self) -> u64 {
        self.mem_usage.load(Ordering::SeqCst)
    }
}

struct MachinesInner {
    by_uuid: AHashMap<uuid::Uuid, Arc<Machine>>,
    // by_system is always sorted, as we insert sorted based on cpu score
    by_system: AHashMap<System, Vec<Arc<Machine>>>,
}

impl MachinesInner {
    fn sort(&mut self, sort_fn: MachineSortFn) {
        for machines in self.by_system.values_mut() {
            machines.sort_by(|a, b| a.score(sort_fn).total_cmp(&b.score(sort_fn)));
        }
    }
}

pub struct Machines {
    inner: parking_lot::RwLock<MachinesInner>,
}

impl Machines {
    pub fn new() -> Self {
        Self {
            inner: parking_lot::RwLock::new(MachinesInner {
                by_uuid: AHashMap::new(),
                by_system: AHashMap::new(),
            }),
        }
    }

    pub fn sort(&self, sort_fn: MachineSortFn) {
        let mut inner = self.inner.write();
        inner.sort(sort_fn);
    }

    #[tracing::instrument(skip(self, machine, sort_fn))]
    pub fn insert_machine(&self, machine: Machine, sort_fn: MachineSortFn) -> uuid::Uuid {
        let machine_id = machine.id;
        let mut inner = self.inner.write();
        let machine = Arc::new(machine);
        inner.by_uuid.insert(machine_id, machine.clone());
        {
            for system in &machine.systems {
                let v = inner.by_system.entry(system.clone()).or_default();
                v.push(machine.clone());
            }
        }
        inner.sort(sort_fn);
        machine_id
    }

    #[tracing::instrument(skip(self, machine_id))]
    pub fn remove_machine(&self, machine_id: uuid::Uuid) -> Option<Arc<Machine>> {
        let mut inner = self.inner.write();
        if let Some(m) = inner.by_uuid.remove(&machine_id) {
            for system in &m.systems {
                if let Some(v) = inner.by_system.get_mut(system) {
                    v.retain(|o| o.id != machine_id);
                }
            }
            Some(m)
        } else {
            None
        }
    }

    #[tracing::instrument(skip(self, machine_id))]
    pub fn get_machine_by_id(&self, machine_id: uuid::Uuid) -> Option<Arc<Machine>> {
        let inner = self.inner.read();
        inner.by_uuid.get(&machine_id).cloned()
    }

    #[tracing::instrument(skip(self, system))]
    pub fn get_machine_for_system(&self, system: &str) -> Option<Arc<Machine>> {
        let inner = self.inner.read();
        if system == "builtin" {
            inner
                .by_uuid
                .values()
                .find(|m| m.stats.get_current_jobs() < 4)
                .cloned()
        } else {
            inner.by_system.get(system).and_then(|machines| {
                machines
                    .iter()
                    .find(|m| m.stats.get_current_jobs() < 4)
                    .cloned()
            })
        }
    }

    #[tracing::instrument(skip(self))]
    pub fn get_all_machines(&self) -> Vec<Arc<Machine>> {
        let inner = self.inner.read();
        inner.by_uuid.values().cloned().collect()
    }

    #[tracing::instrument(skip(self))]
    pub fn get_machine_count(&self) -> usize {
        self.inner.read().by_uuid.len()
    }

    #[tracing::instrument(skip(self))]
    pub fn get_machine_count_in_use(&self) -> usize {
        self.inner
            .read()
            .by_uuid
            .iter()
            .filter(|(_, v)| v.stats.get_current_jobs() > 0)
            .count()
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub path: nix_utils::StorePath,
    pub build_id: BuildID,
    pub step_nr: i32,
    pub result: RemoteBuild,
}

impl Job {
    pub fn new(build_id: BuildID, path: nix_utils::StorePath) -> Self {
        Self {
            path,
            build_id,
            step_nr: 0,
            result: RemoteBuild::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Machine {
    pub id: uuid::Uuid,
    pub systems: Vec<System>,
    pub hostname: String,
    pub cpu_count: u32,
    pub bogomips: f32,
    pub speed_factor: f32,
    pub total_mem: u64,
    pub features: Vec<String>,
    pub cgroups: bool,
    pub joined_at: chrono::DateTime<chrono::Utc>,

    msg_queue: mpsc::Sender<runner_request::Message>,
    pub stats: Arc<Stats>,
    pub jobs: Arc<parking_lot::RwLock<Vec<Job>>>,
}

impl std::fmt::Display for Machine {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "Machine: [systems={:?} hostname={} cpu_count={} bogomips={:.2} speed_factor={:.2} total_mem={:.2} features={:?} cgroups={} joined_at={}]",
            self.systems,
            self.hostname,
            self.cpu_count,
            self.bogomips,
            self.speed_factor,
            byte_unit::Byte::from_u64(self.total_mem).get_adjusted_unit(byte_unit::Unit::GB),
            self.features,
            self.cgroups,
            self.joined_at,
        )
    }
}

impl Machine {
    pub fn new(
        msg: JoinMessage,
        tx: mpsc::Sender<runner_request::Message>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            id: msg.machine_id.parse()?,
            systems: msg.systems,
            hostname: msg.hostname,
            cpu_count: msg.cpu_count,
            bogomips: msg.bogomips,
            speed_factor: msg.speed_factor,
            total_mem: msg.total_mem,
            features: msg.features,
            cgroups: msg.cgroups,
            msg_queue: tx,
            joined_at: chrono::Utc::now(),
            stats: Arc::new(Stats::new()),
            jobs: Arc::new(parking_lot::RwLock::new(Vec::new())),
        })
    }

    #[tracing::instrument(skip(self, job, opts))]
    pub async fn build_drv(&self, job: Job, opts: &nix_utils::BuildOptions) {
        let drv = job.path.clone();
        if let Err(e) = self
            .msg_queue
            .send(runner_request::Message::Build(BuildMessage {
                requisites: nix_utils::topo_sort_drvs(&drv).await.unwrap_or_default(),
                drv: drv.base_name().to_owned(),
                max_log_size: opts.get_max_log_size(),
                max_silent_time: opts.get_max_silent_time(),
                build_timeout: opts.get_build_timeout(),
            }))
            .await
        {
            log::error!("Failed to write msg to build queue! e={e}");
            return;
        }

        self.insert_job(job);
    }

    #[tracing::instrument(skip(self), fields(%drv))]
    pub async fn abort_build(&self, drv: &nix_utils::StorePath) {
        if let Err(e) = self
            .msg_queue
            .send(runner_request::Message::Abort(AbortMessage {
                drv: drv.base_name().to_owned(),
            }))
            .await
        {
            log::error!("Failed to write msg to build queue! e={e}");
            return;
        }

        self.remove_job(drv);
    }

    #[tracing::instrument(skip(self, sort_fn))]
    pub fn score(&self, sort_fn: MachineSortFn) -> f32 {
        match sort_fn {
            MachineSortFn::SpeedFactorOnly => self.speed_factor,
            MachineSortFn::CpuCoreCountWithSpeedFactor =>
            {
                #[allow(clippy::cast_precision_loss)]
                (self.speed_factor * (self.cpu_count as f32))
            }
            MachineSortFn::BogomipsWithSpeedFactor => {
                let bogomips = if self.bogomips > 1. {
                    self.bogomips
                } else {
                    1.0
                };
                #[allow(clippy::cast_precision_loss)]
                (self.speed_factor * bogomips * (self.cpu_count as f32))
            }
        }
    }

    #[tracing::instrument(skip(self), fields(%drv))]
    pub fn get_build_id_and_step_nr(&self, drv: &nix_utils::StorePath) -> Option<(i32, i32)> {
        let jobs = self.jobs.read();
        let job = jobs.iter().find(|j| &j.path == drv).cloned();
        job.map(|j| (j.build_id, j.step_nr))
    }

    #[tracing::instrument(skip(self, job))]
    fn insert_job(&self, job: Job) {
        let mut jobs = self.jobs.write();
        jobs.push(job);
        self.stats.store_current_jobs(jobs.len() as u64);
    }

    #[tracing::instrument(skip(self), fields(%drv))]
    pub fn remove_job(&self, drv: &nix_utils::StorePath) -> Option<Job> {
        let mut jobs = self.jobs.write();
        let job = jobs.iter().find(|j| &j.path == drv).cloned();
        jobs.retain(|j| &j.path != drv);
        self.stats.store_current_jobs(jobs.len() as u64);
        self.stats.incr_nr_steps_done();
        job
    }
}
