use std::{net::SocketAddr, sync::Arc};

use anyhow::Context as _;
use tokio::{io::AsyncWriteExt, sync::mpsc};
use tracing::Instrument as _;

use crate::{
    server::grpc::runner_v1::{BuildResultState, StepUpdate},
    state::{Machine, MachineMessage, State},
};
use runner_v1::{
    BuildResultInfo, BuilderRequest, FetchRequisitesRequest, JoinResponse, LogChunk, NarData,
    RunnerRequest, SimplePingMessage, StorePath, StorePaths, builder_request,
    runner_service_server::{RunnerService, RunnerServiceServer},
};

type BuilderResult<T> = Result<tonic::Response<T>, tonic::Status>;
type OpenTunnelResponseStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<RunnerRequest, tonic::Status>> + Send>>;
type StreamFileResponseStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<NarData, tonic::Status>> + Send>>;

// there is no reason to make this configurable, it only exists so we ensure the channel is not
// closed. we dont use this to write any actual information.
const BACKWARDS_PING_INTERVAL: u64 = 30;

pub mod runner_v1 {
    // We need to allow pedantic here because of generated code
    #![allow(clippy::pedantic)]

    tonic::include_proto!("runner.v1");
}

fn match_for_io_error(err_status: &tonic::Status) -> Option<&std::io::Error> {
    let mut err: &(dyn std::error::Error + 'static) = err_status;

    loop {
        if let Some(io_err) = err.downcast_ref::<std::io::Error>() {
            return Some(io_err);
        }

        // h2::Error do not expose std::io::Error with `source()`
        // https://github.com/hyperium/h2/pull/462
        if let Some(h2_err) = err.downcast_ref::<h2::Error>() {
            if let Some(io_err) = h2_err.get_io() {
                return Some(io_err);
            }
        }

        err = err.source()?;
    }
}

#[tracing::instrument(skip(state, msg))]
fn handle_message(state: &Arc<State>, msg: builder_request::Message) {
    match msg {
        // at this point in time, builder already joined, so this message can be ignored
        builder_request::Message::Join(_) => (),
        builder_request::Message::Ping(msg) => {
            log::debug!("new ping: {msg:?}");
            let Ok(machine_id) = uuid::Uuid::parse_str(&msg.machine_id) else {
                return;
            };
            if let Some(m) = state.machines.get_machine_by_id(machine_id) {
                m.stats.store_ping(&msg);
            }
        }
        #[allow(unreachable_patterns)]
        _ => log::warn!("unhandled message: {msg:?}"),
    }
}

pub struct Server {
    state: Arc<State>,
}

impl Server {
    pub async fn run(addr: SocketAddr, state: Arc<State>) -> anyhow::Result<()> {
        let service = RunnerServiceServer::new(Self {
            state: state.clone(),
        })
        .send_compressed(tonic::codec::CompressionEncoding::Zstd)
        .accept_compressed(tonic::codec::CompressionEncoding::Zstd)
        .max_decoding_message_size(50 * 1024 * 1024)
        .max_encoding_message_size(50 * 1024 * 1024);

        if state.args.mtls_enabled() {
            log::info!("Using mtls");
            let (client_ca_cert, server_identity) = state
                .args
                .get_mtls()
                .await
                .context("Failed to get_mtls Certificate and Identity")?;

            let tls = tonic::transport::ServerTlsConfig::new()
                .identity(server_identity)
                .client_ca_root(client_ca_cert);

            tonic::transport::Server::builder()
                .tls_config(tls)?
                .trace_fn(|_| tracing::info_span!("grpc_server"))
                .add_service(service)
                .serve(addr)
                .await?;
        } else {
            tonic::transport::Server::builder()
                .trace_fn(|_| tracing::info_span!("grpc_server"))
                .add_service(service)
                .serve(addr)
                .await?;
        }

        Ok(())
    }
}

#[tonic::async_trait]
impl RunnerService for Server {
    type OpenTunnelStream = OpenTunnelResponseStream;
    type StreamFileStream = StreamFileResponseStream;
    type StreamFilesStream = StreamFileResponseStream;

    #[tracing::instrument(skip(self, req), err)]
    async fn open_tunnel(
        &self,
        req: tonic::Request<tonic::Streaming<BuilderRequest>>,
    ) -> BuilderResult<Self::OpenTunnelStream> {
        use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

        let mut stream = req.into_inner();
        let (input_tx, mut input_rx) = mpsc::channel::<MachineMessage>(128);
        let machine = match stream.next().await {
            Some(Ok(m)) => match m.message {
                Some(runner_v1::builder_request::Message::Join(v)) => {
                    Machine::new(v, input_tx).ok()
                }
                _ => None,
            },
            _ => None,
        };
        let Some(machine) = machine else {
            return Err(tonic::Status::invalid_argument("No Ping message was sent"));
        };

        let state = self.state.clone();
        let machine_id = state.insert_machine(machine.clone()).await;
        log::info!("Registered new machine: machine_id={machine_id} machine={machine}",);

        let (output_tx, output_rx) = mpsc::channel(128);
        if let Err(e) = output_tx
            .send(Ok(RunnerRequest {
                message: Some(runner_v1::runner_request::Message::Join(JoinResponse {
                    machine_id: machine_id.to_string(),
                })),
            }))
            .await
        {
            log::error!("Failed to send join response machine_id={machine_id} e={e}");
            return Err(tonic::Status::internal("Failed to send join Response."));
        }

        let mut ping_interval =
            tokio::time::interval(std::time::Duration::from_secs(BACKWARDS_PING_INTERVAL));
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = ping_interval.tick() => {
                        let msg = RunnerRequest {
                            message: Some(runner_v1::runner_request::Message::Ping(SimplePingMessage {
                                message: "ping".into(),
                            }))
                        };
                        if let Err(e) = output_tx.send(Ok(msg)).await {
                            log::error!("Failed to send message to machine={machine_id} e={e}");
                            state.remove_machine(machine_id).await;
                            break
                        }
                    },
                    msg = input_rx.recv() => {
                        if let Some(msg) = msg {
                            if let Err(e) = output_tx.send(Ok(msg.into_request())).await {
                                log::error!("Failed to send message to machine={machine_id} e={e}");
                                state.remove_machine(machine_id).await;
                                break
                            }
                        } else {
                            state.remove_machine(machine_id).await;
                            break
                        }
                    },
                    msg = stream.next() => match msg.map(|v| v.map(|v| v.message)) {
                        Some(Ok(Some(msg))) => handle_message(&state, msg),
                        Some(Ok(None)) => (), // empty meesage can be ignored
                        Some(Err(err)) => {
                            if let Some(io_err) = match_for_io_error(&err) {
                                if io_err.kind() == std::io::ErrorKind::BrokenPipe {
                                    log::error!("client disconnected: broken pipe: machine={machine_id}");
                                    state.remove_machine(machine_id).await;
                                    break;
                                }
                            }

                            match output_tx.send(Err(err)).await {
                                Ok(()) => (),
                                Err(_err) => {
                                    state.remove_machine(machine_id).await;
                                    break
                                }
                            }
                        },
                        None => {
                            state.remove_machine(machine_id).await;
                            break
                        }
                    }
                }
            }
        });

        Ok(tonic::Response::new(
            Box::pin(ReceiverStream::new(output_rx)) as Self::OpenTunnelStream,
        ))
    }

    #[tracing::instrument(skip(self, req), err)]
    async fn build_log(
        &self,
        req: tonic::Request<tonic::Streaming<LogChunk>>,
    ) -> BuilderResult<runner_v1::Empty> {
        use tokio_stream::StreamExt as _;

        let mut stream = req.into_inner();
        let state = self.state.clone();

        let mut out_file: Option<tokio::fs::File> = None;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;

            if let Some(ref mut file) = out_file {
                file.write_all(&chunk.data).await?;
            } else {
                let mut file = state
                    .new_log_file(&nix_utils::StorePath::new(&chunk.drv))
                    .await
                    .map_err(|_| tonic::Status::internal("Failed to create log file."))?;
                file.write_all(&chunk.data).await?;
                out_file = Some(file);
            }
        }

        Ok(tonic::Response::new(runner_v1::Empty {}))
    }

    #[tracing::instrument(skip(self, req), err)]
    async fn build_result(
        &self,
        req: tonic::Request<tonic::Streaming<NarData>>,
    ) -> BuilderResult<runner_v1::Empty> {
        use tokio_stream::StreamExt as _;

        let mut stream = req.into_inner();

        if let Some(Ok(first_item)) = stream.next().await {
            let new_stream = tokio_stream::once(first_item.chunk.into())
                .chain(stream.map_while(|s| s.map(|m| m.chunk.into()).ok()));
            nix_utils::import_nar(new_stream, false)
                .await
                .map_err(|_| tonic::Status::internal("Failed to import nar"))?;
        }

        Ok(tonic::Response::new(runner_v1::Empty {}))
    }

    #[tracing::instrument(skip(self), err)]
    async fn build_step_update(
        &self,
        req: tonic::Request<StepUpdate>,
    ) -> BuilderResult<runner_v1::Empty> {
        let state = self.state.clone();

        let req = req.into_inner();
        let drv = req.drv.clone();
        let machine_id = uuid::Uuid::parse_str(&req.machine_id);
        let step_status = crate::db::models::StepStatus::from(req.step_status());

        tokio::spawn({
            async move {
                if let Err(e) = state
                    .update_build_step(
                        machine_id.ok(),
                        &nix_utils::StorePath::new(&drv),
                        step_status,
                    )
                    .await
                {
                    log::error!(
                        "Failed to update build step with drv={drv} step_status={step_status:?}: {e}"
                    );
                }
            }.in_current_span()
        });

        Ok(tonic::Response::new(runner_v1::Empty {}))
    }

    #[tracing::instrument(skip(self, req), fields(machine_id=req.get_ref().machine_id, drv=req.get_ref().drv), err)]
    async fn complete_build(
        &self,
        req: tonic::Request<BuildResultInfo>,
    ) -> BuilderResult<runner_v1::Empty> {
        let state = self.state.clone();

        let req = req.into_inner();
        let drv = req.drv.clone();
        let machine_id = uuid::Uuid::parse_str(&req.machine_id);

        tokio::spawn({
            async move {
                if req.result_state() == BuildResultState::Success {
                    let build_output = crate::state::BuildOutput::from(req);
                    if let Err(e) = state
                        .succeed_step(
                            machine_id.ok(),
                            &nix_utils::StorePath::new(&drv),
                            build_output,
                        )
                        .await
                    {
                        log::error!("Failed to mark step with drv={drv} as done: {e}");
                    }
                } else if let Err(e) = state
                    .fail_step(
                        machine_id.ok(),
                        &nix_utils::StorePath::new(&drv),
                        req.result_state().into(),
                        std::time::Duration::from_millis(req.import_time_ms),
                        std::time::Duration::from_millis(req.build_time_ms),
                    )
                    .await
                {
                    log::error!("Failed to fail step with drv={drv}: {e}");
                }
            }
            .in_current_span()
        });

        Ok(tonic::Response::new(runner_v1::Empty {}))
    }

    #[tracing::instrument(skip(self, req), err)]
    async fn fetch_drv_requisites(
        &self,
        req: tonic::Request<FetchRequisitesRequest>,
    ) -> BuilderResult<runner_v1::DrvRequisitesMessage> {
        let req = req.into_inner();
        let drv = req.path;
        let include_outputs = req.include_outputs;

        let requisites =
            nix_utils::topo_sort_drvs(&nix_utils::StorePath::new(&drv), include_outputs)
                .await
                .map_err(|e| {
                    log::error!("failed to toposort drv e={e}");
                    tonic::Status::internal("failed to toposort drv.")
                })?;

        Ok(tonic::Response::new(runner_v1::DrvRequisitesMessage {
            requisites,
        }))
    }

    #[tracing::instrument(skip(self, req), err)]
    async fn stream_file(
        &self,
        req: tonic::Request<StorePath>,
    ) -> BuilderResult<Self::StreamFileStream> {
        use tokio_stream::StreamExt as _;

        let path = nix_utils::StorePath::new(&req.into_inner().path);
        let (mut child, mut bytes_stream) = nix_utils::export_nar(&path, false)
            .await
            .map_err(|_| tonic::Status::internal("failed to export paths"))?;

        let output = async_stream::try_stream! {
            while let Some(chunk) = bytes_stream.next().await {
                let chunk = chunk?;
                yield NarData {
                    chunk: chunk.into()
                }
            }
            nix_utils::validate_statuscode(
                child
                    .wait()
                    .await
                    .map_err(|_| tonic::Status::internal("failed to export paths"))?,
            )
            .map_err(|_| tonic::Status::internal("failed to export paths"))?;
        };

        Ok(tonic::Response::new(
            Box::pin(output) as Self::StreamFileStream
        ))
    }

    #[tracing::instrument(skip(self, req), err)]
    async fn stream_files(
        &self,
        req: tonic::Request<StorePaths>,
    ) -> BuilderResult<Self::StreamFilesStream> {
        use tokio_stream::StreamExt as _;

        let req = req.into_inner();
        let paths = req
            .paths
            .into_iter()
            .map(|p| nix_utils::StorePath::new(&p))
            .collect::<Vec<_>>();

        let (mut child, mut bytes_stream) = nix_utils::export_nars(&paths, false)
            .await
            .map_err(|_| tonic::Status::internal("failed to export paths"))?;

        let output = async_stream::try_stream! {
            while let Some(chunk) = bytes_stream.next().await {
                let chunk = chunk?;
                yield NarData {
                    chunk: chunk.into()
                }
            }
            nix_utils::validate_statuscode(
                child
                    .wait()
                    .await
                    .map_err(|_| tonic::Status::internal("failed to export paths"))?,
            )
            .map_err(|_| tonic::Status::internal("failed to export paths"))?;
        };

        Ok(tonic::Response::new(
            Box::pin(output) as Self::StreamFilesStream
        ))
    }
}
