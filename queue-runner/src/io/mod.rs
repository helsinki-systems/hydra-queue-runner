use std::sync::{Arc, atomic::Ordering};

use ahash::{AHashMap, HashMap};

#[derive(Debug, serde::Serialize)]
pub struct Empty {}

#[derive(Debug, serde::Serialize)]
pub struct Error {
    pub error: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct BuildPayload {
    pub drv: String,
    pub jobset_id: i32,
}

#[derive(Debug, serde::Serialize)]
pub struct Pressure {
    avg10: f32,
    avg60: f32,
    avg300: f32,
    total: u64,
}

impl From<&crate::state::Pressure> for Pressure {
    fn from(item: &crate::state::Pressure) -> Self {
        Self {
            avg10: item.get_avg10(),
            avg60: item.get_avg60(),
            avg300: item.get_avg300(),
            total: item.get_total(),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct MachineStats {
    current_jobs: u64,
    nr_steps_done: u64,
    avg_step_time_ms: u64,
    avg_step_import_time_ms: u64,
    avg_step_build_time_ms: u64,
    total_step_time_ms: u64,
    total_step_import_time_ms: u64,
    total_step_build_time_ms: u64,
    idle_since: i64,

    last_failure: i64,
    disabled_until: i64,
    consecutive_failures: u64,
    last_ping: i64,
    since_last_ping: i64,

    load1: f32,
    load5: f32,
    load15: f32,
    mem_usage: u64,
    cpu_some_psi: Pressure,
    mem_some_psi: Pressure,
    mem_full_psi: Pressure,
    io_some_psi: Pressure,
    io_full_psi: Pressure,
}

impl MachineStats {
    fn from(item: &std::sync::Arc<crate::state::MachineStats>, now: i64) -> Self {
        let last_ping = item.get_last_ping();

        let nr_steps_done = item.get_nr_steps_done();
        let total_step_time_ms = item.get_total_step_time_ms();
        let total_step_import_time_ms = item.get_total_step_import_time_ms();
        let total_step_build_time_ms = item.get_total_step_build_time_ms();
        let (avg_step_time_ms, avg_step_import_time_ms, avg_step_build_time_ms) =
            if nr_steps_done > 0 {
                (
                    total_step_time_ms / nr_steps_done,
                    total_step_import_time_ms / nr_steps_done,
                    total_step_build_time_ms / nr_steps_done,
                )
            } else {
                (0, 0, 0)
            };

        Self {
            current_jobs: item.get_current_jobs(),
            nr_steps_done,
            avg_step_time_ms,
            avg_step_import_time_ms,
            avg_step_build_time_ms,
            total_step_time_ms,
            total_step_import_time_ms,
            total_step_build_time_ms,
            idle_since: item.get_idle_since(),
            last_failure: item.get_last_failure(),
            disabled_until: item.get_disabled_until(),
            consecutive_failures: item.get_consecutive_failures(),
            last_ping,
            since_last_ping: now - last_ping,
            load1: item.get_load1(),
            load5: item.get_load5(),
            load15: item.get_load15(),
            mem_usage: item.get_mem_usage(),
            cpu_some_psi: (&item.cpu_some_psi).into(),
            mem_some_psi: (&item.mem_some_psi).into(),
            mem_full_psi: (&item.mem_full_psi).into(),
            io_some_psi: (&item.io_some_psi).into(),
            io_full_psi: (&item.io_full_psi).into(),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct Machine {
    systems: Vec<crate::state::System>,
    hostname: String,
    uptime: f64,
    cpu_count: u32,
    bogomips: f32,
    speed_factor: f32,
    max_jobs: u32,
    cpu_psi_threshold: f32,
    mem_psi_threshold: f32,
    io_psi_threshold: Option<f32>,
    score: f32,
    total_mem: u64,
    supported_features: Vec<String>,
    mandatory_features: Vec<String>,
    cgroups: bool,
    stats: MachineStats,
    jobs: Vec<nix_utils::StorePath>,

    has_dynamic_capacity: bool,
    has_static_capacity: bool,
}

impl Machine {
    pub fn from_state(
        item: &Arc<crate::state::Machine>,
        sort_fn: crate::config::MachineSortFn,
    ) -> Self {
        let jobs = { item.jobs.read().iter().map(|j| j.path.clone()).collect() };
        let time = chrono::Utc::now();
        Self {
            systems: item.systems.clone(),
            uptime: (time - item.joined_at).as_seconds_f64(),
            hostname: item.hostname.clone(),
            cpu_count: item.cpu_count,
            bogomips: item.bogomips,
            speed_factor: item.speed_factor,
            max_jobs: item.max_jobs,
            cpu_psi_threshold: item.cpu_psi_threshold,
            mem_psi_threshold: item.mem_psi_threshold,
            io_psi_threshold: item.io_psi_threshold,
            score: item.score(sort_fn),
            total_mem: item.total_mem,
            supported_features: item.supported_features.clone(),
            mandatory_features: item.mandatory_features.clone(),
            cgroups: item.cgroups,
            stats: MachineStats::from(&item.stats, time.timestamp()),
            jobs,
            has_dynamic_capacity: item.has_dynamic_capacity(),
            has_static_capacity: item.has_static_capacity(),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct BuildQueueStats {
    active_runnable: u64,
    total_runnable: u64,
    avg_runnable_time: u64,
    wait_time: u64,
}

impl From<crate::state::BuildQueueStats> for BuildQueueStats {
    fn from(v: crate::state::BuildQueueStats) -> Self {
        Self {
            active_runnable: v.active_runnable,
            total_runnable: v.total_runnable,
            avg_runnable_time: v.avg_runnable_time,
            wait_time: v.wait_time,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct Process {
    pid: i32,
    vsize_bytes: u64,
    rss_bytes: u64,
    shared_bytes: u64,
}

impl Process {
    fn new() -> Option<Self> {
        let me = procfs::process::Process::myself().ok()?;
        let page_size = procfs::page_size();
        let statm = me.statm().ok()?;
        let vsize = statm.size * page_size;
        let rss = statm.resident * page_size;
        let shared = statm.shared * page_size;
        Some(Self {
            pid: me.pid,
            vsize_bytes: vsize,
            rss_bytes: rss,
            shared_bytes: shared,
        })
    }
}

#[derive(Debug, serde::Serialize)]
pub struct QueueRunnerStats {
    status: &'static str,
    time: chrono::DateTime<chrono::Utc>,
    uptime: f64,
    proc: Option<Process>,
    supported_features: Vec<String>,

    build_count: usize,
    jobset_count: usize,
    step_count: usize,
    runnable_count: usize,
    queue_stats: HashMap<crate::state::System, BuildQueueStats>,

    queue_checks_started: u64,
    queue_build_loads: u64,
    queue_steps_created: u64,
    queue_checks_early_exits: u64,
    queue_checks_finished: u64,

    dispatcher_time_spent_running: u64,
    dispatcher_time_spent_waiting: u64,

    queue_monitor_time_spent_running: u64,
    queue_monitor_time_spent_waiting: u64,

    nr_builds_read: i64,
    build_read_time_ms: i64,
    nr_builds_unfinished: i64,
    nr_builds_done: i64,
    nr_steps_started: i64,
    nr_steps_done: i64,
    nr_steps_building: i64,
    nr_steps_waiting: i64,
    nr_steps_runnable: i64,
    nr_steps_unfinished: i64,
    nr_unsupported_steps: i64,
    nr_substitutes_started: i64,
    nr_substitutes_failed: i64,
    nr_substitutes_succeeded: i64,
    nr_retries: i64,
    max_nr_retries: i64,
    avg_step_time_ms: i64,
    avg_step_import_time_ms: i64,
    avg_step_build_time_ms: i64,
    total_step_time_ms: i64,
    total_step_import_time_ms: i64,
    total_step_build_time_ms: i64,
    nr_queue_wakeups: i64,
    nr_dispatcher_wakeups: i64,
    dispatch_time_ms: i64,
    machines_total: i64,
    machines_in_use: i64,
}

impl QueueRunnerStats {
    pub async fn new(state: Arc<crate::state::State>) -> Self {
        let build_count = state.get_nr_builds_unfinished();
        let jobset_count = { state.jobsets.read().len() };
        let step_count = state.get_nr_steps_unfinished();
        let runnable_count = state.get_nr_runnable();
        let queue_stats = {
            let queues = state.queues.read().await;
            queues
                .iter()
                .map(|(system, queue)| (system.clone(), queue.get_stats().into()))
                .collect()
        };

        state.metrics.refresh_dynamic_metrics(&state).await;

        let time = chrono::Utc::now();
        Self {
            status: "up",
            time,
            uptime: (time - state.started_at).as_seconds_f64(),
            proc: Process::new(),
            supported_features: state.machines.get_supported_features(),
            build_count,
            jobset_count,
            step_count,
            runnable_count,
            queue_stats,
            queue_checks_started: state.metrics.queue_checks_started.get(),
            queue_build_loads: state.metrics.queue_build_loads.get(),
            queue_steps_created: state.metrics.queue_steps_created.get(),
            queue_checks_early_exits: state.metrics.queue_checks_early_exits.get(),
            queue_checks_finished: state.metrics.queue_checks_finished.get(),

            dispatcher_time_spent_running: state.metrics.dispatcher_time_spent_running.get(),
            dispatcher_time_spent_waiting: state.metrics.dispatcher_time_spent_waiting.get(),

            queue_monitor_time_spent_running: state.metrics.queue_monitor_time_spent_running.get(),
            queue_monitor_time_spent_waiting: state.metrics.queue_monitor_time_spent_waiting.get(),

            nr_builds_read: state.metrics.nr_builds_read.get(),
            build_read_time_ms: state.metrics.build_read_time_ms.get(),
            nr_builds_unfinished: state.metrics.nr_builds_unfinished.get(),
            nr_builds_done: state.metrics.nr_builds_done.get(),
            nr_steps_started: state.metrics.nr_steps_started.get(),
            nr_steps_done: state.metrics.nr_steps_done.get(),
            nr_steps_building: state.metrics.nr_steps_building.get(),
            nr_steps_waiting: state.metrics.nr_steps_waiting.get(),
            nr_steps_runnable: state.metrics.nr_steps_runnable.get(),
            nr_steps_unfinished: state.metrics.nr_steps_unfinished.get(),
            nr_unsupported_steps: state.metrics.nr_unsupported_steps.get(),
            nr_substitutes_started: state.metrics.nr_substitutes_started.get(),
            nr_substitutes_failed: state.metrics.nr_substitutes_failed.get(),
            nr_substitutes_succeeded: state.metrics.nr_substitutes_succeeded.get(),
            nr_retries: state.metrics.nr_retries.get(),
            max_nr_retries: state.metrics.max_nr_retries.get(),
            avg_step_time_ms: state.metrics.avg_step_time_ms.get(),
            avg_step_import_time_ms: state.metrics.avg_step_import_time_ms.get(),
            avg_step_build_time_ms: state.metrics.avg_step_build_time_ms.get(),
            total_step_time_ms: state.metrics.total_step_time_ms.get(),
            total_step_import_time_ms: state.metrics.total_step_import_time_ms.get(),
            total_step_build_time_ms: state.metrics.total_step_build_time_ms.get(),
            nr_queue_wakeups: state.metrics.nr_queue_wakeups.get(),
            nr_dispatcher_wakeups: state.metrics.nr_dispatcher_wakeups.get(),
            dispatch_time_ms: state.metrics.dispatch_time_ms.get(),
            machines_total: state.metrics.machines_total.get(),
            machines_in_use: state.metrics.machines_in_use.get(),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct DumpResponse {
    queue_runner: QueueRunnerStats,
    machines: Vec<Machine>,
}

impl DumpResponse {
    pub fn new(queue_runner: QueueRunnerStats, machines: Vec<Machine>) -> Self {
        Self {
            queue_runner,
            machines,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct Jobset {
    id: crate::state::JobsetID,
    project_name: String,
    name: String,

    seconds: i64,
    shares: u32,
}

impl From<std::sync::Arc<crate::state::Jobset>> for Jobset {
    fn from(item: std::sync::Arc<crate::state::Jobset>) -> Self {
        Self {
            id: item.id,
            project_name: item.project_name.clone(),
            name: item.name.clone(),
            seconds: item.get_seconds(),
            shares: item.get_shares(),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct JobsetsResponse {
    jobsets: Vec<Jobset>,
    jobset_count: usize,
}

impl JobsetsResponse {
    pub fn new(jobsets: Vec<Jobset>) -> Self {
        Self {
            jobset_count: jobsets.len(),
            jobsets,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct Build {
    id: crate::state::BuildID,
    drv_path: nix_utils::StorePath,
    jobset_id: crate::state::JobsetID,
    name: String,
    timestamp: chrono::DateTime<chrono::Utc>,
    max_silent_time: i32,
    timeout: i32,
    local_priority: i32,
    global_priority: i32,
    finished_in_db: bool,
}

impl From<std::sync::Arc<crate::state::Build>> for Build {
    fn from(item: std::sync::Arc<crate::state::Build>) -> Self {
        Self {
            id: item.id,
            drv_path: item.drv_path.clone(),
            jobset_id: item.jobset_id,
            name: item.name.clone(),
            timestamp: item.timestamp,
            max_silent_time: item.max_silent_time,
            timeout: item.timeout,
            local_priority: item.local_priority,
            global_priority: item.global_priority.load(Ordering::SeqCst),
            finished_in_db: item.finished_in_db.load(Ordering::SeqCst),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct BuildsResponse {
    builds: Vec<Build>,
    build_count: usize,
}

impl BuildsResponse {
    pub fn new(builds: Vec<Build>) -> Self {
        Self {
            build_count: builds.len(),
            builds,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct Step {
    drv_path: nix_utils::StorePath,
    runnable: bool,
    finished: bool,

    created: bool,
    tries: u32,
    highest_global_priority: i32,
    highest_local_priority: i32,

    lowest_build_id: crate::state::BuildID,
}

impl From<std::sync::Arc<crate::state::Step>> for Step {
    fn from(item: std::sync::Arc<crate::state::Step>) -> Self {
        Self {
            drv_path: item.get_drv_path().clone(),
            runnable: item.get_runnable(),
            finished: item.get_finished(),
            created: item.atomic_state.created.load(Ordering::SeqCst),
            tries: item.atomic_state.tries.load(Ordering::SeqCst),
            highest_global_priority: item
                .atomic_state
                .highest_global_priority
                .load(Ordering::SeqCst),
            highest_local_priority: item
                .atomic_state
                .highest_local_priority
                .load(Ordering::SeqCst),
            lowest_build_id: item.atomic_state.lowest_build_id.load(Ordering::SeqCst),
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct StepsResponse {
    steps: Vec<Step>,
    step_count: usize,
}

impl StepsResponse {
    pub fn new(steps: Vec<Step>) -> Self {
        Self {
            step_count: steps.len(),
            steps,
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, serde::Serialize)]
pub struct StepInfo {
    drv_path: nix_utils::StorePath,
    already_scheduled: bool,
    runnable: bool,
    finished: bool,
    cancelled: bool,
    runnable_since: chrono::DateTime<chrono::Utc>,

    lowest_share_used: f64,
    highest_global_priority: i32,
    highest_local_priority: i32,
    lowest_build_id: crate::state::BuildID,
}

impl From<std::sync::Arc<crate::state::StepInfo>> for StepInfo {
    fn from(item: std::sync::Arc<crate::state::StepInfo>) -> Self {
        Self {
            drv_path: item.step.get_drv_path().clone(),
            already_scheduled: item.get_already_scheduled(),
            runnable: item.step.get_runnable(),
            finished: item.step.get_finished(),
            cancelled: item.get_cancelled(),
            runnable_since: item.runnable_since,
            lowest_share_used: item.lowest_share_used,
            highest_global_priority: item.highest_global_priority,
            highest_local_priority: item.highest_local_priority,
            lowest_build_id: item.lowest_build_id,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct QueueResponse {
    queues: AHashMap<String, Vec<StepInfo>>,
}

impl QueueResponse {
    pub fn new(queues: AHashMap<String, Vec<StepInfo>>) -> Self {
        Self { queues }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct StepInfoResponse {
    steps: Vec<StepInfo>,
    step_count: usize,
}

impl StepInfoResponse {
    pub fn new(steps: Vec<StepInfo>) -> Self {
        Self {
            step_count: steps.len(),
            steps,
        }
    }
}
