use std::{sync::Arc, time::Instant};

use ahash::AHashMap;
use futures::TryFutureExt;
use tonic::Request;

use crate::runner_v1::{StepStatus, StepUpdate};

pub struct BuildInfo {
    handle: tokio::task::JoinHandle<()>,
}

impl BuildInfo {
    fn abort(&self) {
        self.handle.abort();
    }
}

pub struct Config {
    pub ping_interval: u64,
    pub speed_factor: f32,
    pub gcroots: std::path::PathBuf,
}

pub struct State {
    id: uuid::Uuid,

    active_builds: parking_lot::RwLock<AHashMap<nix_utils::StorePath, Arc<BuildInfo>>>,

    pub config: Config,
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
        writeln!(f, "{:?}", self.root)
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
    pub fn new(ping_interval: u64, speed_factor: f32) -> Arc<Self> {
        let logname = std::env::var("LOGNAME").expect("LOGNAME not set");

        let nix_state_dir = std::env::var("NIX_STATE_DIR").unwrap_or("/nix/var/nix/".to_owned());
        let gcroots = std::path::PathBuf::from(nix_state_dir)
            .join("gcroots/per-user")
            .join(logname)
            .join("hydra-roots");

        Arc::new(Self {
            id: uuid::Uuid::new_v4(),
            active_builds: parking_lot::RwLock::new(AHashMap::new()),
            config: Config {
                ping_interval,
                speed_factor,
                gcroots,
            },
        })
    }

    #[tracing::instrument(skip(self), err)]
    pub async fn get_join_message(&self) -> anyhow::Result<crate::runner_v1::JoinMessage> {
        let sys = crate::system::BaseSystemInfo::new()?;

        let nix_config = nix_utils::get_nix_config().await?;
        log::info!("Builder nix config: {nix_config:?}");

        Ok(crate::runner_v1::JoinMessage {
            machine_id: self.id.to_string(),
            systems: nix_config.systems(),
            hostname: gethostname::gethostname()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Couldn't convert hostname to string"))?,
            cpu_count: u32::try_from(sys.cpu_count)?,
            bogomips: sys.bogomips,
            speed_factor: self.config.speed_factor,
            total_mem: sys.total_memory,
            features: nix_config.system_features,
            cgroups: nix_config.cgroups,
        })
    }

    #[tracing::instrument(skip(self), err)]
    pub fn get_ping_message(&self) -> anyhow::Result<crate::runner_v1::PingMessage> {
        let sysinfo = crate::system::SystemLoad::new()?;

        Ok(crate::runner_v1::PingMessage {
            machine_id: self.id.to_string(),
            load1: sysinfo.load_avg_1,
            load5: sysinfo.load_avg_5,
            load15: sysinfo.load_avg_15,
            mem_usage: sysinfo.mem_usage,
            cpu_some: Some(sysinfo.cpu_some_psi.into()),
            mem_some: Some(sysinfo.mem_some_psi.into()),
            mem_full: Some(sysinfo.mem_full_psi.into()),
            io_some: Some(sysinfo.io_some_psi.into()),
            io_full: Some(sysinfo.io_full_psi.into()),
        })
    }

    #[tracing::instrument(skip(self, client, m))]
    pub fn schedule_build(
        self: Arc<Self>,
        mut client: crate::runner_v1::runner_service_client::RunnerServiceClient<
            tonic::transport::Channel,
        >,
        m: crate::runner_v1::BuildMessage,
    ) {
        let drv = nix_utils::StorePath::new(&m.drv);
        if self.contains_build(&drv) {
            return;
        }
        log::info!("Building {drv}");

        let task_handle = tokio::spawn({
            let self_ = self.clone();
            let drv = drv.clone();
            async move {
                match self_.process_build(client.clone(), m).await {
                    Ok(()) => {
                        log::info!("Successfully completed build process for {drv}");
                        self_.remove_build(&drv);
                    }
                    Err(e) => {
                        log::error!("Build of {drv} failed with {e}");
                        self_.remove_build(&drv);
                        if let Err(e) = client
                            .complete_build_with_failure(crate::runner_v1::FailResultInfo {
                                machine_id: self_.id.to_string(),
                                drv: drv.base_name().to_owned(),
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

    #[tracing::instrument(skip(self, m))]
    pub fn abort_build(&self, m: &crate::runner_v1::AbortMessage) {
        if let Some(b) = self.remove_build(&nix_utils::StorePath::new(&m.drv)) {
            b.abort();
        }
    }

    #[tracing::instrument(skip(self, client, m), err)]
    async fn process_build(
        &self,
        mut client: crate::runner_v1::runner_service_client::RunnerServiceClient<
            tonic::transport::Channel,
        >,
        m: crate::runner_v1::BuildMessage,
    ) -> anyhow::Result<()> {
        use tokio_stream::StreamExt as _;

        let machine_id = self.id;
        let drv = nix_utils::StorePath::new(&m.drv);

        let before_import = Instant::now();
        let gcroot_prefix = uuid::Uuid::new_v4().to_string();
        let gcroot = self.get_gcroot(&gcroot_prefix)?;

        let _ = client // we ignore the error here, as this step status has no prio
            .build_step_update(StepUpdate {
                machine_id: machine_id.to_string(),
                drv: drv.base_name().to_owned(),
                step_status: StepStatus::SeningInputs as i32,
            })
            .await;
        import_requisites(
            &mut client,
            &gcroot,
            &drv,
            m.requisites
                .into_iter()
                .map(|s| nix_utils::StorePath::new(&s)),
        )
        .await?;
        let import_elapsed = before_import.elapsed();

        let drv_info = nix_utils::query_drv(&drv)
            .await?
            .ok_or(anyhow::anyhow!("drv info not found"))?;

        let _ = client // we ignore the error here, as this step status has no prio
            .build_step_update(StepUpdate {
                machine_id: machine_id.to_string(),
                drv: drv.base_name().to_owned(),
                step_status: StepStatus::Building as i32,
            })
            .await;
        let before_build = Instant::now();
        let (mut child, mut log_output) = nix_utils::realise_drv(
            &drv,
            &nix_utils::BuildOptions::complete(m.max_log_size, m.max_silent_time, m.build_timeout),
            true,
        )
        .await?;
        let drv2 = drv.clone();
        let log_stream = async_stream::stream! {
            while let Some(chunk) = log_output.next().await {
                match chunk {
                    Ok(chunk) => yield crate::runner_v1::LogChunk {
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
        client.build_log(Request::new(log_stream)).await?;
        let output_paths = drv_info
            .outputs
            .iter()
            .filter_map(|o| o.path.clone())
            .collect::<Vec<_>>();
        nix_utils::validate_statuscode(child.wait().await?)?;
        for o in &output_paths {
            nix_utils::add_root(&gcroot.root, o);
        }

        let build_elapsed = before_build.elapsed();
        log::info!("Finished building {drv}");

        let _ = client // we ignore the error here, as this step status has no prio
            .build_step_update(StepUpdate {
                machine_id: machine_id.to_string(),
                drv: drv.base_name().to_owned(),
                step_status: StepStatus::ReceivingOutputs as i32,
            })
            .await;
        upload_nars(client.clone(), output_paths).await?;
        let build_results =
            new_build_result_info(machine_id, &drv, drv_info, import_elapsed, build_elapsed)
                .await?;

        let _ = client // we ignore the error here, as this step status has no prio
            .build_step_update(StepUpdate {
                machine_id: machine_id.to_string(),
                drv: drv.base_name().to_owned(),
                step_status: StepStatus::PostProcessing as i32,
            })
            .await;
        client.complete_build_with_success(build_results).await?;

        Ok(())
    }

    fn get_gcroot(&self, prefix: &str) -> std::io::Result<Gcroot> {
        Gcroot::new(self.config.gcroots.join(prefix))
    }

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
}

#[tracing::instrument(skip(client), err)]
async fn import_path(
    mut client: crate::runner_v1::runner_service_client::RunnerServiceClient<
        tonic::transport::Channel,
    >,
    gcroot: &Gcroot,
    path: nix_utils::StorePath,
) -> anyhow::Result<()> {
    use tokio_stream::StreamExt as _;

    if !nix_utils::check_if_storepath_exists(&path) {
        log::debug!("Importing {path}");
        let input_stream = client
            .stream_file(crate::runner_v1::StorePath {
                path: path.base_name().to_owned(),
            })
            .await?
            .into_inner();
        nix_utils::import_nar(
            input_stream.map_while(|s| s.map(|m| m.chunk.into()).ok()),
            true,
        )
        .await?;
    }
    nix_utils::add_root(&gcroot.root, &path);
    Ok(())
}

#[tracing::instrument(skip(client, requisites), err)]
async fn import_requisites<T: IntoIterator<Item = nix_utils::StorePath>>(
    client: &mut crate::runner_v1::runner_service_client::RunnerServiceClient<
        tonic::transport::Channel,
    >,
    gcroot: &Gcroot,
    drv: &nix_utils::StorePath,
    requisites: T,
) -> anyhow::Result<()> {
    use futures::stream::StreamExt as _;

    let (drvs, other): (Vec<_>, Vec<_>) = requisites
        .into_iter()
        .partition(nix_utils::StorePath::is_drv);

    let mut stream = futures::StreamExt::map(tokio_stream::iter(other), |p| {
        import_path(client.clone(), gcroot, p)
    })
    .buffer_unordered(50);
    while let Some(r) = tokio_stream::StreamExt::next(&mut stream).await {
        r?;
    }

    // Do this drv by drv otherwise drvs get corrupted
    for drv in drvs {
        import_path(client.clone(), gcroot, drv).await?;
    }

    Ok(())
}

#[tracing::instrument(skip(client, nars), err)]
async fn upload_nars(
    client: crate::runner_v1::runner_service_client::RunnerServiceClient<tonic::transport::Channel>,
    nars: Vec<nix_utils::StorePath>,
) -> anyhow::Result<()> {
    use futures::stream::StreamExt as _;

    let mut stream = futures::StreamExt::map(tokio_stream::iter(nars), |p| {
        let mut client = client.clone();
        async move {
            let (mut child, s) = nix_utils::export_nar(&p, true).await?;
            let wait_fut = child.wait().map_err(Into::<anyhow::Error>::into);
            let submit_fut = client
                .build_result(tokio_stream::StreamExt::map_while(s, move |s| {
                    s.map(|m| crate::runner_v1::NarData { chunk: m.into() })
                        .ok()
                }))
                .map_err(Into::<anyhow::Error>::into);
            let _ = futures::future::try_join(wait_fut, submit_fut).await?;
            Ok::<(), anyhow::Error>(())
        }
    })
    .buffer_unordered(10);

    while let Some(r) = tokio_stream::StreamExt::next(&mut stream).await {
        r?;
    }
    Ok(())
}

#[tracing::instrument(skip(drv_info), err)]
async fn new_build_result_info(
    machine_id: uuid::Uuid,
    drv: &nix_utils::StorePath,
    drv_info: nix_utils::Derivation,
    import_elapsed: std::time::Duration,
    build_elapsed: std::time::Duration,
) -> anyhow::Result<crate::runner_v1::BuildResultInfo> {
    let outputs = &drv_info
        .outputs
        .iter()
        .filter_map(|o| o.path.as_ref())
        .collect::<Vec<_>>();
    let pathinfos = nix_utils::query_path_infos(outputs).await?;

    let nix_support = nix_utils::parse_nix_support_from_outputs(&drv_info.outputs).await?;
    Ok(crate::runner_v1::BuildResultInfo {
        machine_id: machine_id.to_string(),
        drv: drv.base_name().to_owned(),
        import_time_ms: u64::try_from(import_elapsed.as_millis())?,
        build_time_ms: u64::try_from(build_elapsed.as_millis())?,
        outputs: drv_info
            .outputs
            .into_iter()
            .map(|o| crate::runner_v1::Output {
                output: Some(match o.path {
                    Some(p) => {
                        if let Some(info) = pathinfos.get(&p) {
                            crate::runner_v1::output::Output::Withpath(
                                crate::runner_v1::OutputWithPath {
                                    name: o.name,
                                    path: p.base_name().to_owned(),
                                    closure_size: info.closure_size,
                                    nar_size: info.nar_size,
                                    nar_hash: info.nar_hash.clone(),
                                },
                            )
                        } else {
                            crate::runner_v1::output::Output::Nameonly(
                                crate::runner_v1::OutputNameOnly { name: o.name },
                            )
                        }
                    }
                    None => crate::runner_v1::output::Output::Nameonly(
                        crate::runner_v1::OutputNameOnly { name: o.name },
                    ),
                }),
            })
            .collect(),
        nix_support: Some(crate::runner_v1::NixSupport {
            metrics: nix_support
                .metrics
                .into_iter()
                .map(|m| crate::runner_v1::BuildMetric {
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
                .map(|p| crate::runner_v1::BuildProduct {
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
