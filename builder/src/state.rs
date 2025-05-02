use std::{sync::Arc, time::Instant};

use ahash::AHashMap;
use futures::TryFutureExt;
use nix_utils::{Derivation, query_path_infos};
use tokio::io::{AsyncBufReadExt, BufReader};
use tonic::Request;

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
}

pub struct State {
    id: uuid::Uuid,

    active_builds: parking_lot::RwLock<AHashMap<String, Arc<BuildInfo>>>,

    pub config: Config,
}

impl State {
    pub fn new(ping_interval: u64, speed_factor: f32) -> Arc<Self> {
        Arc::new(Self {
            id: uuid::Uuid::new_v4(),
            active_builds: parking_lot::RwLock::new(AHashMap::new()),
            config: Config {
                ping_interval,
                speed_factor,
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
        let drv = m.drv.clone();

        let task_handle = tokio::spawn({
            let self_ = self.clone();
            async move {
                let drv = m.drv.clone();
                match self_.process_build(client.clone(), m).await {
                    Ok(()) => {
                        let mut active = self_.active_builds.write();
                        active.remove(&drv);
                    }
                    Err(e) => {
                        log::error!("Build of {drv} failed with {e}");
                        if let Err(e) = client
                            .complete_build_with_failure(crate::runner_v1::FailResultInfo {
                                machine_id: self_.id.to_string(),
                                drv,
                            })
                            .await
                        {
                            log::error!("Failed to submit build failure info: {e}");
                        }
                    }
                }
            }
        });

        {
            let mut active = self.active_builds.write();
            active.insert(
                drv,
                Arc::new(BuildInfo {
                    handle: task_handle,
                }),
            );
        }
    }

    #[tracing::instrument(skip(self, drv))]
    pub fn abort_build(&self, drv: &str) {
        let mut active = self.active_builds.write();
        if let Some(b) = active.remove(drv) {
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
        let drv = m.drv.clone();

        let before_import = Instant::now();
        import_requisites(&mut client, m.requisites).await?;
        let import_elapsed = before_import.elapsed();

        let before_build = Instant::now();
        let (mut child, mut log_output) = nix_utils::realise_drv(
            &m.drv,
            &nix_utils::BuildOptions::complete(m.max_log_size, m.max_silent_time, m.build_timeout),
        )
        .await?;
        let log_stream = async_stream::stream! {
            while let Some(chunk) = log_output.next().await {
                match chunk {
                    Ok(chunk) => yield crate::runner_v1::LogChunk {
                        drv: m.drv.clone(),
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
        nix_utils::validate_statuscode(child.wait().await?)?;

        let build_elapsed = before_build.elapsed();
        log::info!("Finished building {drv}");

        if let Some(drv_info) = nix_utils::query_drv(&drv).await? {
            upload_nars(
                client.clone(),
                drv_info
                    .outputs
                    .iter()
                    .filter_map(|o| o.path.clone())
                    .collect::<Vec<_>>(),
            )
            .await?;

            let build_results =
                new_build_result_info(machine_id, &drv, drv_info, import_elapsed, build_elapsed)
                    .await?;
            client.complete_build_with_success(build_results).await?;
        }
        Ok(())
    }
}

#[tracing::instrument(skip(client), err)]
async fn import_path(
    mut client: crate::runner_v1::runner_service_client::RunnerServiceClient<
        tonic::transport::Channel,
    >,
    path: String,
) -> anyhow::Result<()> {
    use tokio_stream::StreamExt as _;

    if !nix_utils::check_if_storepath_exists(&path).await {
        log::debug!("Importing {path}");
        let input_stream = client
            .stream_file(crate::runner_v1::StorePath { path })
            .await?
            .into_inner();
        nix_utils::import_nar(input_stream.map_while(|s| s.map(|m| m.chunk.into()).ok())).await?;
    }
    Ok(())
}

#[tracing::instrument(skip(client, requisites), err)]
async fn import_requisites(
    client: &mut crate::runner_v1::runner_service_client::RunnerServiceClient<
        tonic::transport::Channel,
    >,
    requisites: Vec<String>,
) -> anyhow::Result<()> {
    use futures::stream::StreamExt as _;

    let mut stream = futures::StreamExt::map(tokio_stream::iter(requisites), |p| {
        import_path(client.clone(), p)
    })
    .buffer_unordered(50);

    while let Some(r) = tokio_stream::StreamExt::next(&mut stream).await {
        r?;
    }
    Ok(())
}

#[tracing::instrument(skip(client, nars), err)]
async fn upload_nars(
    client: crate::runner_v1::runner_service_client::RunnerServiceClient<tonic::transport::Channel>,
    nars: Vec<String>,
) -> anyhow::Result<()> {
    use futures::stream::StreamExt as _;

    let mut stream = futures::StreamExt::map(tokio_stream::iter(nars), |p| {
        let mut client = client.clone();
        async move {
            let (mut child, s) = nix_utils::export_nar(&p).await?;
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
    drv: &str,
    drv_info: Derivation,
    import_elapsed: std::time::Duration,
    build_elapsed: std::time::Duration,
) -> anyhow::Result<crate::runner_v1::BuildResultInfo> {
    let outputs = &drv_info
        .outputs
        .iter()
        .filter_map(|o| o.path.as_deref())
        .collect::<Vec<_>>();
    let pathinfos = query_path_infos(outputs).await?;

    let mut metrics = Vec::new();
    let mut failed = false;
    let mut hydra_release_name = None;

    for output in outputs {
        let file_path = std::path::Path::new(&output).join("nix-support/hydra-metrics");

        if let Ok(file) = tokio::fs::File::open(&file_path).await {
            let reader = BufReader::new(file);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await? {
                let fields: Vec<String> = line.split_whitespace().map(ToOwned::to_owned).collect();
                if fields.len() < 2 {
                    continue;
                }

                metrics.push(crate::runner_v1::BuildMetric {
                    path: (*output).to_owned(),
                    name: fields[0].clone(),
                    value: fields[1].parse::<f64>().unwrap_or(0.0),
                    unit: if fields.len() >= 3 {
                        Some(fields[2].clone())
                    } else {
                        None
                    },
                });
            }
        }
    }

    for output in outputs {
        let file_path = std::path::Path::new(&output).join("nix-support/failed");
        if tokio::fs::try_exists(file_path).await.unwrap_or_default() {
            failed = true;
            break;
        }
    }

    for output in outputs {
        let file_path = std::path::Path::new(&output).join("nix-support/hydra-release-name");
        if let Ok(v) = tokio::fs::read_to_string(file_path).await {
            let v = v.trim();
            if !v.is_empty() {
                hydra_release_name = Some(v.to_owned());
                break;
            }
        }
    }

    Ok(crate::runner_v1::BuildResultInfo {
        machine_id: machine_id.to_string(),
        drv: drv.to_owned(),
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
                                    path: p,
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
            metrics,
            failed,
            hydra_release_name,
            products: vec![],
        }),
    })
}
