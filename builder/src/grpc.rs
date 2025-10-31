use std::sync::Arc;

use anyhow::Context as _;
use runner_v1::{
    BuilderRequest, builder_request, runner_request, runner_service_client::RunnerServiceClient,
};
use tonic::{Request, service::interceptor::InterceptedService, transport::Channel};

pub mod runner_v1 {
    // We need to allow pedantic here because of generated code
    #![allow(clippy::pedantic)]

    tonic::include_proto!("runner.v1");
}

#[derive(Debug, Clone)]
pub enum BuilderInterceptor {
    Token {
        token: tonic::metadata::MetadataValue<tonic::metadata::Ascii>,
    },
    Noop,
}

impl tonic::service::Interceptor for BuilderInterceptor {
    fn call(&mut self, request: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        let mut request = hydra_tracing::propagate::send_trace(request).map_err(|e| *e)?;

        if let BuilderInterceptor::Token { token } = self {
            request
                .metadata_mut()
                .insert("authorization", token.clone());
        }

        Ok(request)
    }
}

pub type BuilderClient = RunnerServiceClient<InterceptedService<Channel, BuilderInterceptor>>;

pub async fn init_client(cli: &crate::config::Cli) -> anyhow::Result<BuilderClient> {
    if !cli.mtls_configured_correctly() {
        log::error!(
            "mtls configured inproperly, please pass all options: \
            server_root_ca_cert_path, client_cert_path, client_key_path and domain_name!"
        );
        return Err(anyhow::anyhow!("Configuration issue"));
    }

    log::info!("connecting to {}", cli.gateway_endpoint);
    let channel = if cli.mtls_enabled() {
        log::info!("mtls is enabled");
        let (server_root_ca_cert, client_identity, domain_name) = cli
            .get_mtls()
            .await
            .context("Failed to get_mtls Certificate and Identity")?;
        let tls = tonic::transport::ClientTlsConfig::new()
            .domain_name(domain_name)
            .ca_certificate(server_root_ca_cert)
            .identity(client_identity);

        tonic::transport::Channel::builder(cli.gateway_endpoint.parse()?)
            .tls_config(tls)
            .context("Failed to attach tls config")?
            .connect()
            .await
            .context("Failed to establish connection with Channel")?
    } else if let Some(path) = cli.gateway_endpoint.strip_prefix("unix://") {
        let path = path.to_owned();
        tonic::transport::Endpoint::try_from("http://[::]:50051")?
            .connect_with_connector(tower::service_fn(move |_: tonic::transport::Uri| {
                let path = path.clone();
                async move {
                    Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(
                        tokio::net::UnixStream::connect(&path).await?,
                    ))
                }
            }))
            .await
            .context("Failed to establish unix socket connection with Channel")?
    } else {
        tonic::transport::Channel::builder(cli.gateway_endpoint.parse()?)
            .connect()
            .await
            .context("Failed to establish connection with Channel")?
    };

    let interceptor = if let Some(t) = cli.get_authorization_token().await? {
        BuilderInterceptor::Token {
            token: format!("Bearer {t}").parse()?,
        }
    } else {
        BuilderInterceptor::Noop
    };

    Ok(RunnerServiceClient::with_interceptor(channel, interceptor)
        .send_compressed(tonic::codec::CompressionEncoding::Zstd)
        .accept_compressed(tonic::codec::CompressionEncoding::Zstd)
        .max_decoding_message_size(50 * 1024 * 1024)
        .max_encoding_message_size(50 * 1024 * 1024))
}

#[tracing::instrument(skip(state, client, request), err)]
async fn handle_request(
    state: Arc<crate::state::State>,
    client: &mut BuilderClient,
    request: runner_request::Message,
) -> anyhow::Result<()> {
    match request {
        runner_request::Message::Build(m) => {
            let client = client.clone();
            state.schedule_build(client, m);
        }
        runner_request::Message::Abort(m) => {
            state.abort_build(&m);
        }
        runner_request::Message::Join(m) => {
            state.max_concurrent_downloads.store(
                m.max_concurrent_downloads,
                std::sync::atomic::Ordering::Relaxed,
            );
        }
        runner_request::Message::Ping(_) => (),
    }
    Ok(())
}

#[tracing::instrument(skip(client, state), err)]
pub async fn start_bidirectional_stream(
    client: &mut BuilderClient,
    state: Arc<crate::state::State>,
) -> anyhow::Result<()> {
    use tokio_stream::StreamExt as _;

    let state2 = state.clone();
    let join_msg = state.get_join_message().await?;

    let ping_stream = async_stream::stream! {
        yield BuilderRequest {
            message: Some(builder_request::Message::Join(join_msg))
        };

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(state.config.ping_interval));
        loop {
            interval.tick().await;

            let ping = match state.get_ping_message() {
                Ok(v) => builder_request::Message::Ping(v),
                Err(e) => {
                    log::error!("Failed to construct ping message: {e}");
                    continue
                },
            };
            log::debug!("sending ping: {ping:?}");

            yield BuilderRequest {
                message: Some(ping)
            };
        }
    };
    let mut stream = client
        .open_tunnel(Request::new(ping_stream))
        .await?
        .into_inner();

    let mut consecutive_failure_count = 0;
    while let Some(item) = stream.next().await {
        match item.map(|v| v.message) {
            Ok(Some(v)) => {
                consecutive_failure_count = 0;
                if let Err(err) = handle_request(state2.clone(), client, v).await {
                    log::error!("Failed to correctly handle request: {err}");
                }
            }
            Ok(None) => {
                consecutive_failure_count = 0;
            }
            Err(e) => {
                consecutive_failure_count += 1;
                log::error!("stream message delivery failed: {e}");
                if consecutive_failure_count == 10 {
                    return Err(anyhow::anyhow!(
                        "Failed to communicate {consecutive_failure_count} times over the channel. \
                        Terminating the application."
                    ));
                }
            }
        }
    }
    Ok(())
}
