use std::{
    sync::{
        Arc,
        atomic::{self, AtomicBool},
    },
    time::Instant,
};

use ahash::AHashMap;
use anyhow::Context;
use backon::RetryableWithContext as _;
use futures::TryFutureExt as _;
use tonic::Request;
use tracing::Instrument;

use crate::grpc::BuilderClient;
use crate::grpc::runner_v1::{
    AbortMessage, BuildMessage, BuildMetric, BuildProduct, BuildResultInfo, BuildResultState,
    FetchRequisitesRequest, JoinMessage, LogChunk, NarData, NixSupport, Output, OutputNameOnly,
    OutputWithPath, PingMessage, PressureState, StepStatus, StepUpdate, StorePaths, output,
};
use nix_utils::BaseStore as _;

const RETRY_MIN_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(3);
const RETRY_MAX_DELAY: tokio::time::Duration = tokio::time::Duration::from_secs(90);

fn retry_strategy() -> backon::ExponentialBuilder {
    backon::ExponentialBuilder::default()
        .with_jitter()
        .with_min_delay(RETRY_MIN_DELAY)
        .with_max_delay(RETRY_MAX_DELAY)
}

#[derive(thiserror::Error, Debug)]
#[allow(clippy::enum_variant_names)]
pub enum JobFailure {
    #[error("Build failure: `{0}`")]
    Build(anyhow::Error),
    #[error("Preparing failure: `{0}`")]
    Preparing(anyhow::Error),
    #[error("Import failure: `{0}`")]
    Import(anyhow::Error),
    #[error("Upload failure: `{0}`")]
    Upload(anyhow::Error),
    #[error("Post processing failure: `{0}`")]
    PostProcessing(anyhow::Error),
}

impl From<JobFailure> for BuildResultState {
    fn from(item: JobFailure) -> Self {
        match item {
            JobFailure::Build(_) => Self::BuildFailure,
            JobFailure::Preparing(_) => Self::PreparingFailure,
            JobFailure::Import(_) => Self::ImportFailure,
            JobFailure::Upload(_) => Self::UploadFailure,
            JobFailure::PostProcessing(_) => Self::PostProcessingFailure,
        }
    }
}

pub struct BuildInfo {
    handle: tokio::task::JoinHandle<()>,
    was_cancelled: Arc<AtomicBool>,
}

impl BuildInfo {
    fn abort(&self) {
        self.was_cancelled.store(true, atomic::Ordering::SeqCst);
        self.handle.abort();
    }
}

pub struct Config {
    pub ping_interval: u64,
    pub speed_factor: f32,
    pub max_jobs: u32,
    pub tmp_avail_threshold: f32,
    pub store_avail_threshold: f32,
    pub load1_threshold: f32,
    pub cpu_psi_threshold: f32,
    pub mem_psi_threshold: f32,
    pub io_psi_threshold: Option<f32>,
    pub gcroots: std::path::PathBuf,
    pub systems: Vec<String>,
    pub supported_features: Vec<String>,
    pub mandatory_features: Vec<String>,
    pub cgroups: bool,
    pub use_substitutes: bool,
}

pub struct State {
    id: uuid::Uuid,

    active_builds: parking_lot::RwLock<AHashMap<nix_utils::StorePath, Arc<BuildInfo>>>,

    pub config: Config,

    pub max_concurrent_downloads: atomic::AtomicU32,
}

#[derive(Debug)]
struct Gcroot {
    root: std::path::PathBuf,
}

impl Gcroot {
    pub fn new(path: std::path::PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&path)?;
        Ok(Self { root: path })
    }
}

impl std::fmt::Display for Gcroot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "{}", self.root.display())
    }
}

impl Drop for Gcroot {
    fn drop(&mut self) {
        if self.root.exists() {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

impl State {
    pub fn new(cli: &super::config::Cli) -> anyhow::Result<Arc<Self>> {
        nix_utils::set_verbosity(1);

        let logname = std::env::var("LOGNAME").context("LOGNAME not set")?;
        let nix_state_dir = std::env::var("NIX_STATE_DIR").unwrap_or("/nix/var/nix/".to_owned());
        let gcroots = std::path::PathBuf::from(nix_state_dir)
            .join("gcroots/per-user")
            .join(logname)
            .join("hydra-roots");
        std::fs::create_dir_all(&gcroots)?;

        let state = Arc::new(Self {
            id: uuid::Uuid::new_v4(),
            active_builds: parking_lot::RwLock::new(AHashMap::new()),
            config: Config {
                ping_interval: cli.ping_interval,
                speed_factor: cli.speed_factor,
                max_jobs: cli.max_jobs,
                tmp_avail_threshold: cli.tmp_avail_threshold,
                store_avail_threshold: cli.store_avail_threshold,
                load1_threshold: cli.load1_threshold,
                cpu_psi_threshold: cli.cpu_psi_threshold,
                mem_psi_threshold: cli.mem_psi_threshold,
                io_psi_threshold: cli.io_psi_threshold,
                gcroots,
                systems: if let Some(s) = &cli.systems {
                    s.clone()
                } else {
                    let mut out = Vec::with_capacity(8);
                    out.push(nix_utils::get_this_system());
                    out.extend(nix_utils::get_extra_platforms());
                    out
                },
                supported_features: if let Some(s) = &cli.supported_features {
                    s.clone()
                } else {
                    nix_utils::get_system_features()
                },
                mandatory_features: cli.mandatory_features.clone().unwrap_or_default(),
                cgroups: nix_utils::get_use_cgroups(),
                use_substitutes: cli.use_substitutes,
            },
            max_concurrent_downloads: 5.into(),
        });
        log::info!("Builder systems={:?}", state.config.systems);
        log::info!(
            "Builder supported_features={:?}",
            state.config.supported_features
        );
        log::info!(
            "Builder mandatory_features={:?}",
            state.config.mandatory_features
        );
        log::info!("Builder use_cgroups={:?}", state.config.cgroups);

        Ok(state)
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_join_message(&self) -> anyhow::Result<JoinMessage> {
        let sys = crate::system::BaseSystemInfo::new()?;

        Ok(JoinMessage {
            machine_id: self.id.to_string(),
            systems: self.config.systems.clone(),
            hostname: gethostname::gethostname()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Couldn't convert hostname to string"))?,
            cpu_count: u32::try_from(sys.cpu_count)?,
            bogomips: sys.bogomips,
            speed_factor: self.config.speed_factor,
            max_jobs: self.config.max_jobs,
            tmp_avail_threshold: self.config.tmp_avail_threshold,
            store_avail_threshold: self.config.store_avail_threshold,
            load1_threshold: self.config.load1_threshold,
            cpu_psi_threshold: self.config.cpu_psi_threshold,
            mem_psi_threshold: self.config.mem_psi_threshold,
            io_psi_threshold: self.config.io_psi_threshold,
            total_mem: sys.total_memory,
            supported_features: self.config.supported_features.clone(),
            mandatory_features: self.config.mandatory_features.clone(),
            cgroups: self.config.cgroups,
        })
    }

    #[tracing::instrument(skip(self), err)]
    pub fn get_ping_message(&self) -> anyhow::Result<PingMessage> {
        let sysinfo = crate::system::SystemLoad::new()?;

        Ok(PingMessage {
            machine_id: self.id.to_string(),
            load1: sysinfo.load_avg_1,
            load5: sysinfo.load_avg_5,
            load15: sysinfo.load_avg_15,
            mem_usage: sysinfo.mem_usage,
            pressure: sysinfo.pressure.map(|p| PressureState {
                cpu_some: p.cpu_some.map(Into::into),
                mem_some: p.mem_some.map(Into::into),
                mem_full: p.mem_full.map(Into::into),
                io_some: p.io_some.map(Into::into),
                io_full: p.io_full.map(Into::into),
                irq_full: p.irq_full.map(Into::into),
            }),
            tmp_free_percent: sysinfo.tmp_free_percent,
            store_free_percent: sysinfo.store_free_percent,
        })
    }

    #[tracing::instrument(skip(self, client, m), fields(drv=%m.drv))]
    pub fn schedule_build(self: Arc<Self>, client: BuilderClient, m: BuildMessage) {
        let drv = nix_utils::StorePath::new(&m.drv);
        if self.contains_build(&drv) {
            return;
        }
        log::info!("Building {drv}");

        let was_cancelled = Arc::new(AtomicBool::new(false));
        let task_handle = tokio::spawn({
            let self_ = self.clone();
            let drv = drv.clone();
            let was_cancelled = was_cancelled.clone();
            async move {
                let mut import_elapsed = std::time::Duration::from_millis(0);
                let mut build_elapsed = std::time::Duration::from_millis(0);
                match Box::pin(self_.process_build(
                    client.clone(),
                    m,
                    &mut import_elapsed,
                    &mut build_elapsed,
                ))
                .await
                {
                    Ok(()) => {
                        log::info!("Successfully completed build process for {drv}");
                        self_.remove_build(&drv);
                    }
                    Err(e) => {
                        if was_cancelled.load(atomic::Ordering::SeqCst) {
                            log::error!("Build of {drv} was cancelled {e}, not reporting Error");
                            return;
                        }

                        log::error!("Build of {drv} failed with {e}");
                        self_.remove_build(&drv);
                        let failed_build = BuildResultInfo {
                            machine_id: self_.id.to_string(),
                            drv: drv.into_base_name(),
                            import_time_ms: u64::try_from(import_elapsed.as_millis())
                                .unwrap_or_default(),
                            build_time_ms: u64::try_from(build_elapsed.as_millis())
                                .unwrap_or_default(),
                            result_state: BuildResultState::from(e) as i32,
                            outputs: vec![],
                            nix_support: None,
                        };

                        if let (_, Err(e)) = (|tuple: (BuilderClient, BuildResultInfo)| async {
                            let (mut client, body) = tuple;
                            let res = client.complete_build(body.clone()).await;
                            ((client, body), res)
                        })
                        .retry(retry_strategy())
                        .sleep(tokio::time::sleep)
                        .context((client.clone(), failed_build))
                        .notify(|err: &tonic::Status, dur: core::time::Duration| {
                            log::error!("Failed to submit build failure info: err={err}, retrying in={dur:?}");
                        })
                        .await
                        {
                            log::error!("Failed to submit build failure info: {e}");
                        }
                    }
                }
            }
        });

        self.insert_new_build(
            drv,
            BuildInfo {
                handle: task_handle,
                was_cancelled,
            },
        );
    }

    fn contains_build(&self, drv: &nix_utils::StorePath) -> bool {
        let active = self.active_builds.read();
        active.contains_key(drv)
    }

    fn insert_new_build(&self, drv: nix_utils::StorePath, b: BuildInfo) {
        {
            let mut active = self.active_builds.write();
            active.insert(drv, Arc::new(b));
        }
        self.publish_builds_to_sd_notify();
    }

    fn remove_build(&self, drv: &nix_utils::StorePath) -> Option<Arc<BuildInfo>> {
        let b = {
            let mut active = self.active_builds.write();
            active.remove(drv)
        };
        self.publish_builds_to_sd_notify();
        b
    }

    #[tracing::instrument(skip(self, m), fields(drv=%m.drv))]
    pub fn abort_build(&self, m: &AbortMessage) {
        log::info!("Try cancelling build");
        if let Some(b) = self.remove_build(&nix_utils::StorePath::new(&m.drv)) {
            b.abort();
        }
    }

    pub fn abort_all_active_builds(&self) {
        let mut active = self.active_builds.write();
        for b in active.values() {
            b.abort();
        }
        active.clear();
    }

    #[tracing::instrument(skip(self, client, m), fields(drv=%m.drv), err)]
    #[allow(clippy::too_many_lines)]
    async fn process_build(
        &self,
        mut client: BuilderClient,
        m: BuildMessage,
        import_elapsed: &mut std::time::Duration,
        build_elapsed: &mut std::time::Duration,
    ) -> Result<(), JobFailure> {
        // we dont use anyhow here because we manually need to write the correct build status
        // to the queue runner.
        use tokio_stream::StreamExt as _;

        let store = nix_utils::LocalStore::init();

        let machine_id = self.id;
        let drv = nix_utils::StorePath::new(&m.drv);
        let resolved_drv = m
            .resolved_drv
            .as_ref()
            .map(|v| nix_utils::StorePath::new(v));

        let before_import = Instant::now();
        let gcroot_prefix = uuid::Uuid::new_v4().to_string();
        let gcroot = self
            .get_gcroot(&gcroot_prefix)
            .map_err(|e| JobFailure::Preparing(e.into()))?;

        let _ = client // we ignore the error here, as this step status has no prio
            .build_step_update(StepUpdate {
                machine_id: machine_id.to_string(),
                drv: drv.base_name().to_owned(),
                step_status: StepStatus::SeningInputs as i32,
            })
            .await;
        let requisites = client
            .fetch_drv_requisites(FetchRequisitesRequest {
                path: resolved_drv.as_ref().unwrap_or(&drv).base_name().to_owned(),
                include_outputs: false,
            })
            .await
            .map_err(|e| JobFailure::Import(e.into()))?
            .into_inner()
            .requisites;

        import_requisites(
            &mut client,
            store.clone(),
            &gcroot,
            resolved_drv.as_ref().unwrap_or(&drv),
            requisites
                .into_iter()
                .map(|s| nix_utils::StorePath::new(&s)),
            usize::try_from(
                self.max_concurrent_downloads
                    .load(atomic::Ordering::Relaxed),
            )
            .unwrap_or(5),
            self.config.use_substitutes,
        )
        .await
        .map_err(JobFailure::Import)?;
        *import_elapsed = before_import.elapsed();

        // Resolved drv and drv output paths are the same
        let drv_info = nix_utils::query_drv(&drv)
            .await
            .map_err(|e| JobFailure::Import(e.into()))?
            .ok_or(JobFailure::Import(anyhow::anyhow!("drv not found")))?;

        let _ = client // we ignore the error here, as this step status has no prio
            .build_step_update(StepUpdate {
                machine_id: machine_id.to_string(),
                drv: drv.base_name().to_owned(),
                step_status: StepStatus::Building as i32,
            })
            .await;
        let before_build = Instant::now();
        let (mut child, mut log_output) = nix_utils::realise_drv(
            resolved_drv.as_ref().unwrap_or(&drv),
            &nix_utils::BuildOptions::complete(m.max_log_size, m.max_silent_time, m.build_timeout),
            true,
        )
        .await
        .map_err(|e| JobFailure::Build(e.into()))?;
        let drv2 = drv.clone();
        let log_stream = async_stream::stream! {
            while let Some(chunk) = log_output.next().await {
                match chunk {
                    Ok(chunk) => yield LogChunk {
                        drv: drv2.base_name().to_owned(),
                        data: format!("{chunk}\n").into(),
                    },
                    Err(e) => {
                        log::error!("Failed to write log chunk to queue-runner: {e}");
                        break
                    }
                }
            }
        };
        client
            .build_log(Request::new(log_stream))
            .await
            .map_err(|e| JobFailure::Build(e.into()))?;
        let output_paths = drv_info
            .outputs
            .iter()
            .filter_map(|o| o.path.clone())
            .collect::<Vec<_>>();
        nix_utils::validate_statuscode(
            child
                .wait()
                .await
                .map_err(|e| JobFailure::Build(e.into()))?,
        )
        .map_err(|e| JobFailure::Build(e.into()))?;
        for o in &output_paths {
            nix_utils::add_root(&gcroot.root, o);
        }

        *build_elapsed = before_build.elapsed();
        log::info!("Finished building {drv}");

        let _ = client // we ignore the error here, as this step status has no prio
            .build_step_update(StepUpdate {
                machine_id: machine_id.to_string(),
                drv: drv.base_name().to_owned(),
                step_status: StepStatus::ReceivingOutputs as i32,
            })
            .await;
        upload_nars(client.clone(), store.clone(), output_paths)
            .await
            .map_err(JobFailure::Upload)?;

        let _ = client // we ignore the error here, as this step status has no prio
            .build_step_update(StepUpdate {
                machine_id: machine_id.to_string(),
                drv: drv.base_name().to_owned(),
                step_status: StepStatus::PostProcessing as i32,
            })
            .await;
        let build_results = Box::pin(new_success_build_result_info(
            store.clone(),
            machine_id,
            &drv,
            drv_info,
            *import_elapsed,
            *build_elapsed,
        ))
        .await
        .map_err(JobFailure::PostProcessing)?;

        // This part is stupid, if writing doesnt work, we try to write a failure, maybe that works.
        // We retry to ensure that this almost never happens.
        (|tuple: (BuilderClient, BuildResultInfo)| async {
            let (mut client, body) = tuple;
            let res = client.complete_build(body.clone()).await;
            ((client, body), res)
        })
        .retry(retry_strategy())
        .sleep(tokio::time::sleep)
        .context((client.clone(), build_results))
        .notify(|err: &tonic::Status, dur: core::time::Duration| {
            log::error!("Failed to submit build success info: err={err}, retrying in={dur:?}");
        })
        .await
        .1
        .map_err(|e| {
            log::error!("Failed to submit build success info. Will fail build: err={e}");
            JobFailure::PostProcessing(e.into())
        })?;
        Ok(())
    }

    #[tracing::instrument(skip(self), err)]
    fn get_gcroot(&self, prefix: &str) -> std::io::Result<Gcroot> {
        Gcroot::new(self.config.gcroots.join(prefix))
    }

    #[tracing::instrument(skip(self))]
    fn publish_builds_to_sd_notify(&self) {
        let active = {
            let builds = self.active_builds.read();
            builds
                .keys()
                .map(|b| b.base_name().to_owned())
                .collect::<Vec<_>>()
        };

        let _notify = sd_notify::notify(
            false,
            &[
                sd_notify::NotifyState::Status(&if active.is_empty() {
                    "Building 0 drvs".into()
                } else {
                    format!("Building {} drvs: {}", active.len(), active.join(", "))
                }),
                sd_notify::NotifyState::Ready,
            ],
        );
    }

    pub fn clear_gcroots(&self) -> std::io::Result<()> {
        std::fs::remove_dir_all(&self.config.gcroots)?;
        std::fs::create_dir_all(&self.config.gcroots)?;
        Ok(())
    }
}

#[tracing::instrument(skip(store), fields(%gcroot, %path))]
async fn filter_missing(
    store: &nix_utils::LocalStore,
    gcroot: &Gcroot,
    path: nix_utils::StorePath,
) -> Option<nix_utils::StorePath> {
    if store.is_valid_path(&path).await {
        nix_utils::add_root(&gcroot.root, &path);
        None
    } else {
        Some(path)
    }
}

async fn substitute_paths(
    store: &nix_utils::LocalStore,
    paths: &[nix_utils::StorePath],
) -> anyhow::Result<()> {
    for p in paths {
        store.ensure_path(p).await?;
    }
    Ok(())
}

#[tracing::instrument(skip(client, store), fields(%gcroot), err)]
async fn import_paths(
    mut client: BuilderClient,
    store: nix_utils::LocalStore,
    gcroot: &Gcroot,
    paths: Vec<nix_utils::StorePath>,
    filter: bool,
    use_substitutes: bool,
) -> anyhow::Result<()> {
    use futures::StreamExt as _;

    let paths = if filter {
        futures::StreamExt::map(tokio_stream::iter(paths), |p| {
            filter_missing(&store, gcroot, p)
        })
        .buffered(10)
        .filter_map(|o| async { o })
        .collect::<Vec<_>>()
        .await
    } else {
        paths
    };
    let paths = if use_substitutes {
        // we can ignore the error
        let _ = substitute_paths(&store, &paths).await;
        let paths = futures::StreamExt::map(tokio_stream::iter(paths), |p| {
            filter_missing(&store, gcroot, p)
        })
        .buffered(10)
        .filter_map(|o| async { o })
        .collect::<Vec<_>>()
        .await;
        if paths.is_empty() {
            return Ok(());
        }
        paths
    } else {
        paths
    };

    log::debug!("Start importing paths");
    let stream = client
        .stream_files(StorePaths {
            paths: paths.iter().map(|p| p.base_name().to_owned()).collect(),
        })
        .await?
        .into_inner();

    store
        .import_paths(
            tokio_stream::StreamExt::map(stream, |s| {
                s.map(|v| v.chunk.into())
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, e))
            }),
            false,
        )
        .await?;
    log::debug!("Finished importing paths");

    for p in paths {
        nix_utils::add_root(&gcroot.root, &p);
    }
    Ok(())
}

#[tracing::instrument(skip(client, store, requisites), fields(%gcroot, %drv), err)]
async fn import_requisites<T: IntoIterator<Item = nix_utils::StorePath>>(
    client: &mut BuilderClient,
    store: nix_utils::LocalStore,
    gcroot: &Gcroot,
    drv: &nix_utils::StorePath,
    requisites: T,
    max_concurrent_downloads: usize,
    use_substitutes: bool,
) -> anyhow::Result<()> {
    use futures::stream::StreamExt as _;

    let requisites = futures::StreamExt::map(tokio_stream::iter(requisites), |p| {
        filter_missing(&store, gcroot, p)
    })
    .buffered(50)
    .filter_map(|o| async { o })
    .collect::<Vec<_>>()
    .await;

    let (input_drvs, input_srcs): (Vec<_>, Vec<_>) = requisites
        .into_iter()
        .partition(nix_utils::StorePath::is_drv);

    for srcs in input_srcs.chunks(max_concurrent_downloads) {
        import_paths(
            client.clone(),
            store.clone(),
            gcroot,
            srcs.to_vec(),
            true,
            use_substitutes,
        )
        .await?;
    }

    for drvs in input_drvs.chunks(max_concurrent_downloads) {
        import_paths(
            client.clone(),
            store.clone(),
            gcroot,
            drvs.to_vec(),
            true,
            false, // never use substitute for drvs
        )
        .await?;
    }

    let full_requisites = client
        .clone()
        .fetch_drv_requisites(FetchRequisitesRequest {
            path: drv.base_name().to_owned(),
            include_outputs: true,
        })
        .await?
        .into_inner()
        .requisites
        .into_iter()
        .map(|s| nix_utils::StorePath::new(&s))
        .collect::<Vec<_>>();
    let full_requisites = futures::StreamExt::map(tokio_stream::iter(full_requisites), |p| {
        filter_missing(&store, gcroot, p)
    })
    .buffered(50)
    .filter_map(|o| async { o })
    .collect::<Vec<_>>()
    .await;

    for other in full_requisites.chunks(max_concurrent_downloads) {
        // we can skip filtering here as we already done that
        import_paths(
            client.clone(),
            store.clone(),
            gcroot,
            other.to_vec(),
            false,
            use_substitutes,
        )
        .await?;
    }

    Ok(())
}

#[tracing::instrument(skip(client, store), err)]
async fn upload_nars(
    mut client: BuilderClient,
    store: nix_utils::LocalStore,
    nars: Vec<nix_utils::StorePath>,
) -> anyhow::Result<()> {
    log::debug!("Start uploading paths");
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<NarData>();
    let closure = move |data: &[u8]| {
        let data = Vec::from(data);
        tx.send(NarData { chunk: data }).is_ok()
    };
    let a = client
        .build_result(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
        .map_err(Into::<anyhow::Error>::into);

    let b = tokio::task::spawn_blocking(move || {
        async move {
            store.export_paths(&nars, closure)?;
            log::debug!("Finished exporting paths");
            Ok::<(), anyhow::Error>(())
        }
        .in_current_span()
    })
    .await?
    .map_err(Into::<anyhow::Error>::into);
    futures::future::try_join(a, b).await?;
    log::debug!("Finished uploading paths");
    Ok(())
}

#[tracing::instrument(skip(store, drv_info), fields(%drv), ret(level = tracing::Level::DEBUG), err)]
async fn new_success_build_result_info(
    store: nix_utils::LocalStore,
    machine_id: uuid::Uuid,
    drv: &nix_utils::StorePath,
    drv_info: nix_utils::Derivation,
    import_elapsed: std::time::Duration,
    build_elapsed: std::time::Duration,
) -> anyhow::Result<BuildResultInfo> {
    let outputs = &drv_info
        .outputs
        .iter()
        .filter_map(|o| o.path.as_ref())
        .collect::<Vec<_>>();
    let pathinfos = store.query_path_infos(outputs).await;
    let nix_support = Box::pin(shared::parse_nix_support_from_outputs(&drv_info.outputs)).await?;

    let mut build_outputs = vec![];
    for o in drv_info.outputs {
        build_outputs.push(Output {
            output: Some(match o.path {
                Some(p) => {
                    if let Some(info) = pathinfos.get(&p) {
                        output::Output::Withpath(OutputWithPath {
                            name: o.name,
                            closure_size: store.compute_closure_size(&p).await,
                            path: p.into_base_name(),
                            nar_size: info.nar_size,
                            nar_hash: info.nar_hash.clone(),
                        })
                    } else {
                        output::Output::Nameonly(OutputNameOnly { name: o.name })
                    }
                }
                None => output::Output::Nameonly(OutputNameOnly { name: o.name }),
            }),
        });
    }

    Ok(BuildResultInfo {
        machine_id: machine_id.to_string(),
        drv: drv.base_name().to_owned(),
        import_time_ms: u64::try_from(import_elapsed.as_millis())?,
        build_time_ms: u64::try_from(build_elapsed.as_millis())?,
        result_state: BuildResultState::Success as i32,
        outputs: build_outputs,
        nix_support: Some(NixSupport {
            metrics: nix_support
                .metrics
                .into_iter()
                .map(|m| BuildMetric {
                    path: m.path,
                    name: m.name,
                    unit: m.unit,
                    value: m.value,
                })
                .collect(),
            failed: nix_support.failed,
            hydra_release_name: nix_support.hydra_release_name,
            products: nix_support
                .products
                .into_iter()
                .map(|p| BuildProduct {
                    path: p.path,
                    default_path: p.default_path,
                    r#type: p.r#type,
                    subtype: p.subtype,
                    name: p.name,
                    is_regular: p.is_regular,
                    sha256hash: p.sha256hash,
                    file_size: p.file_size,
                })
                .collect(),
        }),
    })
}
