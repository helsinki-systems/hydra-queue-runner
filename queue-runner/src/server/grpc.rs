use std::{net::SocketAddr, sync::Arc};

use anyhow::Context as _;
use tokio::{io::AsyncWriteExt, sync::mpsc};

use crate::{
    server::grpc::runner_v1::StepUpdate,
    state::{Machine, State},
};
use runner_v1::{
    BuildResultInfo, BuilderRequest, FailResultInfo, JoinResponse, LogChunk, NarData,
    RunnerRequest, SimplePingMessage, StorePath, builder_request,
    runner_service_server::{RunnerService, RunnerServiceServer},
};

type BuilderResult<T> = Result<tonic::Response<T>, tonic::Status>;
type OpenTunnelResponseStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<RunnerRequest, tonic::Status>> + Send>>;
type StreamFileResponseStream =
    std::pin::Pin<Box<dyn futures::Stream<Item = Result<NarData, tonic::Status>> + Send>>;

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
        .accept_compressed(tonic::codec::CompressionEncoding::Zstd);

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

    #[tracing::instrument(skip(self, req), err)]
    async fn open_tunnel(
        &self,
        req: tonic::Request<tonic::Streaming<BuilderRequest>>,
    ) -> BuilderResult<Self::OpenTunnelStream> {
        use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

        let mut stream = req.into_inner();
        let (input_tx, mut input_rx) = mpsc::channel::<runner_v1::runner_request::Message>(128);
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
        if output_tx
            .send(Ok(RunnerRequest {
                message: Some(runner_v1::runner_request::Message::Join(JoinResponse {
                    machine_id: machine_id.to_string(),
                })),
            }))
            .await
            .is_err()
        {
            return Err(tonic::Status::internal("Failed to send join Response."));
        }

        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = ping_interval.tick() => {
                        let msg = RunnerRequest {
                            message: Some(runner_v1::runner_request::Message::Ping(SimplePingMessage {
                                message: "ping".into(),
                            }))
                        };
                        if output_tx.send(Ok(msg)).await.is_err() {
                            state.remove_machine(machine_id).await;
                            break
                        }
                    },
                    msg = input_rx.recv() => {
                        if let Some(msg) = msg {
                            if output_tx.send(Ok(RunnerRequest { message: Some(msg) })).await.is_err() {
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
                                    log::error!("client disconnected: broken pipe");
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

    #[tracing::instrument(skip(self, req), err)]
    async fn build_step_update(
        &self,
        req: tonic::Request<StepUpdate>,
    ) -> BuilderResult<runner_v1::Empty> {
        let state = self.state.clone();

        let req = req.into_inner();
        let drv = req.drv.clone();
        let machine_id = uuid::Uuid::parse_str(&req.machine_id);
        let step_status = crate::db::models::StepStatus::from(req.step_status());

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

        Ok(tonic::Response::new(runner_v1::Empty {}))
    }

    #[tracing::instrument(skip(self, req), err)]
    async fn complete_build_with_success(
        &self,
        req: tonic::Request<BuildResultInfo>,
    ) -> BuilderResult<runner_v1::Empty> {
        let state = self.state.clone();

        let req = req.into_inner();
        let drv = req.drv.clone();
        let machine_id = uuid::Uuid::parse_str(&req.machine_id);

        let build_output = crate::state::BuildOutput::from(req);
        if let Err(e) = state
            .mark_step_done(
                machine_id.ok(),
                &nix_utils::StorePath::new(&drv),
                build_output,
            )
            .await
        {
            log::error!("Failed to mark step with drv={drv} as done: {e}");
        }

        Ok(tonic::Response::new(runner_v1::Empty {}))
    }

    #[tracing::instrument(skip(self, req), err)]
    async fn complete_build_with_failure(
        &self,
        req: tonic::Request<FailResultInfo>,
    ) -> BuilderResult<runner_v1::Empty> {
        let state = self.state.clone();

        let req = req.into_inner();
        let drv = req.drv;
        let machine_id = uuid::Uuid::parse_str(&req.machine_id);

        if let Err(e) = state
            .fail_step(
                machine_id.ok(),
                &nix_utils::StorePath::new(&drv),
                std::time::Duration::from_millis(req.build_time_ms),
            )
            .await
        {
            log::error!("Failed to fail step with drv={drv}: {e}");
        }

        Ok(tonic::Response::new(runner_v1::Empty {}))
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
}
