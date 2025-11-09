use std::sync::Arc;

use prometheus::Encoder;

#[derive(Debug)]
pub struct PromMetrics {
    pub queue_checks_started: prometheus::IntCounter,
    pub queue_build_loads: prometheus::IntCounter,
    pub queue_steps_created: prometheus::IntCounter,
    pub queue_checks_early_exits: prometheus::IntCounter,
    pub queue_checks_finished: prometheus::IntCounter, // TODO

    pub dispatcher_time_spent_running: prometheus::IntCounter,
    pub dispatcher_time_spent_waiting: prometheus::IntCounter,

    pub queue_monitor_time_spent_running: prometheus::IntCounter,
    pub queue_monitor_time_spent_waiting: prometheus::IntCounter,

    pub nr_builds_read: prometheus::IntGauge, // hydra_queue_builds_read
    pub build_read_time_ms: prometheus::IntGauge, // hydra_queue_builds_time
    pub nr_builds_unfinished: prometheus::IntGauge, // hydra_queue_builds_unfinished
    pub nr_builds_done: prometheus::IntGauge, // hydra_queue_builds_finished
    pub nr_steps_started: prometheus::IntGauge, // hydra_queue_steps_started
    pub nr_steps_done: prometheus::IntGauge,  // hydra_queue_steps_finished
    pub nr_steps_building: prometheus::IntGauge, // hydra_queue_steps_building
    pub nr_steps_waiting: prometheus::IntGauge, // hydra_queue_steps_waiting
    pub nr_steps_runnable: prometheus::IntGauge, // hydra_queue_steps_runnable
    pub nr_steps_unfinished: prometheus::IntGauge, // hydra_queue_steps_unfinished
    pub nr_unsupported_steps: prometheus::IntGauge, // hydra_queue_steps_unsupported
    pub nr_unsupported_steps_aborted: prometheus::IntGauge, // hydra_queue_steps_unsupported_aborted
    pub nr_substitutes_started: prometheus::IntGauge, // hydra_queue_substitutes_started
    pub nr_substitutes_failed: prometheus::IntGauge, // hydra_queue_substitutes_failed
    pub nr_substitutes_succeeded: prometheus::IntGauge, // hydra_queue_substitutes_succeeded
    pub nr_retries: prometheus::IntGauge,     // hydra_queue_steps_retries
    pub max_nr_retries: prometheus::IntGauge, // hydra_queue_steps_max_retries
    pub avg_step_time_ms: prometheus::IntGauge, // hydra_queue_steps_avg_total_time
    pub avg_step_import_time_ms: prometheus::IntGauge, // hydra_queue_steps_avg_import_time
    pub avg_step_build_time_ms: prometheus::IntGauge, // hydra_queue_steps_avg_build_time
    pub total_step_time_ms: prometheus::IntGauge, // hydra_queue_steps_total_time
    pub total_step_import_time_ms: prometheus::IntGauge, // hydra_queue_steps_total_import_time
    pub total_step_build_time_ms: prometheus::IntGauge, // hydra_queue_steps_total_build_time
    pub nr_queue_wakeups: prometheus::IntGauge, //hydra_queue_checks
    pub nr_dispatcher_wakeups: prometheus::IntGauge, // hydra_queue_dispatch_wakeup
    pub dispatch_time_ms: prometheus::IntGauge, // hydra_queue_dispatch_time
    pub machines_total: prometheus::IntGauge, // hydra_queue_machines_total
    pub machines_in_use: prometheus::IntGauge, // hydra_queue_machines_in_use
    pub runnable_per_machine_type: prometheus::IntGaugeVec, // hydra_queue_machines_runnable
    pub running_per_machine_type: prometheus::IntGaugeVec, // hydra_queue_machines_running
}

impl PromMetrics {
    #[allow(clippy::too_many_lines)]
    pub fn new() -> anyhow::Result<Self> {
        let queue_checks_started = prometheus::IntCounter::with_opts(prometheus::Opts::new(
            "hydraqueuerunner_queue_checks_started_total",
            "Number of times State::get_queued_builds() was started",
        ))?;
        let queue_build_loads = prometheus::IntCounter::with_opts(prometheus::Opts::new(
            "hydraqueuerunner_queue_build_loads_total",
            "Number of builds loaded",
        ))?;
        let queue_steps_created = prometheus::IntCounter::with_opts(prometheus::Opts::new(
            "hydraqueuerunner_queue_steps_created_total",
            "Number of steps created",
        ))?;
        let queue_checks_early_exits = prometheus::IntCounter::with_opts(prometheus::Opts::new(
            "hydraqueuerunner_queue_checks_early_exits_total",
            "Number of times State::get_queued_builds() yielded to potential bumps",
        ))?;
        let queue_checks_finished = prometheus::IntCounter::with_opts(prometheus::Opts::new(
            "hydraqueuerunner_queue_checks_finished_total",
            "Number of times State::get_queued_builds() was completed",
        ))?;
        let dispatcher_time_spent_running =
            prometheus::IntCounter::with_opts(prometheus::Opts::new(
                "hydraqueuerunner_dispatcher_time_spent_running",
                "Time (in micros) spent running the dispatcher",
            ))?;
        let dispatcher_time_spent_waiting =
            prometheus::IntCounter::with_opts(prometheus::Opts::new(
                "hydraqueuerunner_dispatcher_time_spent_waiting",
                "Time (in micros) spent waiting for the dispatcher to obtain work",
            ))?;
        let queue_monitor_time_spent_running =
            prometheus::IntCounter::with_opts(prometheus::Opts::new(
                "hydraqueuerunner_queue_monitor_time_spent_running",
                "Time (in micros) spent running the queue monitor",
            ))?;
        let queue_monitor_time_spent_waiting =
            prometheus::IntCounter::with_opts(prometheus::Opts::new(
                "hydraqueuerunner_queue_monitor_time_spent_waiting",
                "Time (in micros) spent waiting for the queue monitor to obtain work",
            ))?;

        let nr_builds_read = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_builds_read",
            "Number of builds read from the database",
        ))?;
        let build_read_time_ms = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_builds_time",
            "Time in milliseconds spent reading builds from the database",
        ))?;
        let nr_builds_unfinished = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_builds_unfinished",
            "Number of unfinished builds in the queue",
        ))?;
        let nr_builds_done = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_builds_finished",
            "Number of finished builds in the queue",
        ))?;
        let nr_steps_started = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_started",
            "Number of build steps that have been started",
        ))?;
        let nr_steps_done = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_finished",
            "Number of build steps that have been completed",
        ))?;
        let nr_steps_building = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_building",
            "Number of build steps currently being built",
        ))?;
        let nr_steps_waiting = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_waiting",
            "Number of build steps waiting to be built",
        ))?;
        let nr_steps_runnable = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_runnable",
            "Number of build steps that are ready to run",
        ))?;
        let nr_steps_unfinished = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_unfinished",
            "Number of unfinished build steps",
        ))?;
        let nr_unsupported_steps = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_unsupported",
            "Number of unsupported build steps",
        ))?;
        let nr_unsupported_steps_aborted = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_unsupported_aborted",
            "Number of unsupported build steps that were aborted",
        ))?;
        let nr_substitutes_started = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_substitutes_started",
            "Number of substitute downloads that have been started",
        ))?;
        let nr_substitutes_failed = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_substitutes_failed",
            "Number of substitute downloads that have failed",
        ))?;
        let nr_substitutes_succeeded = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_substitutes_succeeded",
            "Number of substitute downloads that have succeeded",
        ))?;
        let nr_retries = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_retries",
            "Number of retries for build steps",
        ))?;
        let max_nr_retries = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_max_retries",
            "Maximum number of retries allowed for build steps",
        ))?;
        let avg_step_time_ms = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_avg_time_ms",
            "Average time in milliseconds for build steps to complete",
        ))?;
        let avg_step_import_time_ms = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_avg_import_time_ms",
            "Average time in milliseconds for importing build steps",
        ))?;
        let avg_step_build_time_ms = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_avg_build_time_ms",
            "Average time in milliseconds for building build steps",
        ))?;
        let total_step_time_ms = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_total_time_ms",
            "Total time in milliseconds spent on all build steps",
        ))?;
        let total_step_import_time_ms = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_total_import_time_ms",
            "Total time in milliseconds spent importing all build steps",
        ))?;
        let total_step_build_time_ms = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_steps_total_build_time_ms",
            "Total time in milliseconds spent building all build steps",
        ))?;
        let nr_queue_wakeups = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_checks",
            "Number of times the queue monitor has been woken up",
        ))?;
        let nr_dispatcher_wakeups = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_dispatch_wakeup",
            "Number of times the dispatcher has been woken up",
        ))?;
        let dispatch_time_ms = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_dispatch_time",
            "Time in milliseconds spent dispatching build steps",
        ))?;
        let machines_total = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_machines_total",
            "Total number of machines available for building",
        ))?;
        let machines_in_use = prometheus::IntGauge::with_opts(prometheus::Opts::new(
            "hydra_queue_machines_in_use",
            "Number of machines currently in use for building",
        ))?;
        let runnable_per_machine_type = prometheus::IntGaugeVec::new(
            prometheus::Opts::new(
                "hydra_queue_machines_runnable",
                "Number of runnable build steps per machine type",
            ),
            &["machine_type"],
        )?;
        let running_per_machine_type = prometheus::IntGaugeVec::new(
            prometheus::Opts::new(
                "hydra_queue_machines_running",
                "Number of running build steps per machine type",
            ),
            &["machine_type"],
        )?;

        // TODO: per machine metrics

        let r = prometheus::default_registry();
        r.register(Box::new(queue_checks_started.clone()))?;
        r.register(Box::new(queue_build_loads.clone()))?;
        r.register(Box::new(queue_steps_created.clone()))?;
        r.register(Box::new(queue_checks_early_exits.clone()))?;
        r.register(Box::new(queue_checks_finished.clone()))?;
        r.register(Box::new(dispatcher_time_spent_running.clone()))?;
        r.register(Box::new(dispatcher_time_spent_waiting.clone()))?;
        r.register(Box::new(queue_monitor_time_spent_running.clone()))?;
        r.register(Box::new(queue_monitor_time_spent_waiting.clone()))?;
        r.register(Box::new(nr_builds_read.clone()))?;
        r.register(Box::new(build_read_time_ms.clone()))?;
        r.register(Box::new(nr_builds_unfinished.clone()))?;
        r.register(Box::new(nr_builds_done.clone()))?;
        r.register(Box::new(nr_steps_started.clone()))?;
        r.register(Box::new(nr_steps_done.clone()))?;
        r.register(Box::new(nr_steps_building.clone()))?;
        r.register(Box::new(nr_steps_waiting.clone()))?;
        r.register(Box::new(nr_steps_runnable.clone()))?;
        r.register(Box::new(nr_steps_unfinished.clone()))?;
        r.register(Box::new(nr_unsupported_steps.clone()))?;
        r.register(Box::new(nr_unsupported_steps_aborted.clone()))?;
        r.register(Box::new(nr_substitutes_started.clone()))?;
        r.register(Box::new(nr_substitutes_failed.clone()))?;
        r.register(Box::new(nr_substitutes_succeeded.clone()))?;
        r.register(Box::new(nr_retries.clone()))?;
        r.register(Box::new(max_nr_retries.clone()))?;
        r.register(Box::new(avg_step_time_ms.clone()))?;
        r.register(Box::new(avg_step_import_time_ms.clone()))?;
        r.register(Box::new(avg_step_build_time_ms.clone()))?;
        r.register(Box::new(total_step_time_ms.clone()))?;
        r.register(Box::new(total_step_import_time_ms.clone()))?;
        r.register(Box::new(total_step_build_time_ms.clone()))?;
        r.register(Box::new(nr_queue_wakeups.clone()))?;
        r.register(Box::new(nr_dispatcher_wakeups.clone()))?;
        r.register(Box::new(dispatch_time_ms.clone()))?;
        r.register(Box::new(machines_total.clone()))?;
        r.register(Box::new(machines_in_use.clone()))?;
        r.register(Box::new(runnable_per_machine_type.clone()))?;
        r.register(Box::new(running_per_machine_type.clone()))?;

        Ok(Self {
            queue_checks_started,
            queue_build_loads,
            queue_steps_created,
            queue_checks_early_exits,
            queue_checks_finished,
            dispatcher_time_spent_running,
            dispatcher_time_spent_waiting,
            queue_monitor_time_spent_running,
            queue_monitor_time_spent_waiting,
            nr_builds_read,
            build_read_time_ms,
            nr_builds_unfinished,
            nr_builds_done,
            nr_steps_started,
            nr_steps_done,
            nr_steps_building,
            nr_steps_waiting,
            nr_steps_runnable,
            nr_steps_unfinished,
            nr_unsupported_steps,
            nr_unsupported_steps_aborted,
            nr_substitutes_started,
            nr_substitutes_failed,
            nr_substitutes_succeeded,
            nr_retries,
            max_nr_retries,
            avg_step_time_ms,
            avg_step_import_time_ms,
            avg_step_build_time_ms,
            total_step_time_ms,
            total_step_import_time_ms,
            total_step_build_time_ms,
            nr_queue_wakeups,
            nr_dispatcher_wakeups,
            dispatch_time_ms,
            machines_total,
            machines_in_use,
            runnable_per_machine_type,
            running_per_machine_type,
        })
    }

    pub async fn refresh_dynamic_metrics(&self, state: &Arc<super::State>) {
        let nr_steps_done = self.nr_steps_done.get();
        if nr_steps_done > 0 {
            self.avg_step_time_ms
                .set(self.total_step_time_ms.get() / nr_steps_done);
            self.avg_step_import_time_ms
                .set(self.total_step_import_time_ms.get() / nr_steps_done);
            self.avg_step_build_time_ms
                .set(self.total_step_build_time_ms.get() / nr_steps_done);
        }

        if let Ok(v) = i64::try_from(state.get_nr_builds_unfinished()) {
            self.nr_builds_unfinished.set(v);
        }
        if let Ok(v) = i64::try_from(state.get_nr_steps_unfinished()) {
            self.nr_steps_unfinished.set(v);
        }
        if let Ok(v) = i64::try_from(state.get_nr_runnable()) {
            self.nr_steps_runnable.set(v);
        }
        if let Ok(v) = i64::try_from(state.machines.get_machine_count()) {
            self.machines_total.set(v);
        }
        if let Ok(v) = i64::try_from(state.machines.get_machine_count_in_use()) {
            self.machines_in_use.set(v);
        }

        {
            let queue_stats = state.queues.read().await.get_stats_per_queue();
            self.runnable_per_machine_type.reset();
            self.running_per_machine_type.reset();
            for (t, s) in queue_stats {
                if let Ok(v) = i64::try_from(s.total_runnable) {
                    self.runnable_per_machine_type
                        .with_label_values(std::slice::from_ref(&t))
                        .set(v);
                }
                if let Ok(v) = i64::try_from(s.active_runnable) {
                    self.running_per_machine_type.with_label_values(&[t]).set(v);
                }
            }
        }
    }

    pub async fn gather_metrics(&self, state: &Arc<super::State>) -> anyhow::Result<Vec<u8>> {
        self.refresh_dynamic_metrics(state).await;

        let mut buffer = Vec::new();
        let encoder = prometheus::TextEncoder::new();
        let metric_families = prometheus::gather();
        encoder.encode(&metric_families, &mut buffer)?;

        Ok(buffer)
    }

    pub fn add_to_total_step_time_ms(&self, v: u128) {
        if let Ok(v) = i64::try_from(v) {
            self.total_step_time_ms.add(v);
        }
    }

    pub fn add_to_total_step_import_time_ms(&self, v: u128) {
        if let Ok(v) = i64::try_from(v) {
            self.total_step_import_time_ms.add(v);
        }
    }

    pub fn add_to_total_step_build_time_ms(&self, v: u128) {
        if let Ok(v) = i64::try_from(v) {
            self.total_step_build_time_ms.add(v);
        }
    }
}
