#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![allow(clippy::match_wildcard_for_single_variants)]

use std::sync::Arc;

use anyhow::Context as _;
use runner_v1::{
    BuilderRequest, builder_request, runner_request, runner_service_client::RunnerServiceClient,
};
use state::State;
use tonic::transport::Channel;

use tokio_stream::StreamExt as _;
use tonic::Request;
use tracing_subscriber::{Layer as _, Registry, layer::SubscriberExt as _};

mod config;
mod state;
mod system;
mod runner_v1 {
    // We need to allow pedantic here because of generated code
    #![allow(clippy::pedantic)]

    tonic::include_proto!("runner.v1");
}

#[tracing::instrument(skip(state, client, request), err)]
async fn handle_request(
    state: Arc<State>,
    client: &mut RunnerServiceClient<Channel>,
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
        runner_request::Message::Join(_) => (),
    }
    Ok(())
}

#[tracing::instrument(skip(client, state), err)]
async fn start_bidirectional_stream(
    client: &mut RunnerServiceClient<Channel>,
    state: Arc<State>,
) -> anyhow::Result<()> {
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

    while let Some(item) = stream.next().await {
        match item.map(|v| v.message) {
            Ok(Some(v)) => {
                if let Err(err) = handle_request(state2.clone(), client, v).await {
                    log::error!("Failed to correctly handle request: {err}");
                }
            }
            Ok(None) => (),
            Err(e) => log::error!("stream message delivery failed: {e}"),
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_log::LogTracer::init()?;
    let log_env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_filter(log_env_filter);
    let subscriber = Registry::default().with(fmt_layer);
    tracing::subscriber::set_global_default(subscriber)?;

    let args = config::Args::new();

    if !args.mtls_configured_correctly() {
        log::error!(
            "mtls configured inproperly, please pass all options: \
            server_root_ca_cert_path, client_cert_path, client_key_path and domain_name!"
        );
        return Err(anyhow::anyhow!("Configuration issue"));
    }

    log::info!("connecting to {}", args.gateway_endpoint);
    let channel = if args.mtls_enabled() {
        log::info!("mtls is enabled");
        let (server_root_ca_cert, client_identity, domain_name) = args
            .get_mtls()
            .await
            .context("Failed to get_mtls Certificate and Identity")?;
        let tls = tonic::transport::ClientTlsConfig::new()
            .domain_name(domain_name)
            .ca_certificate(server_root_ca_cert)
            .identity(client_identity);

        tonic::transport::Channel::builder(args.gateway_endpoint.parse()?)
            .tls_config(tls)?
            .connect()
            .await?
    } else {
        tonic::transport::Channel::builder(args.gateway_endpoint.parse()?)
            .connect()
            .await?
    };

    let state = State::new(args.ping_interval, args.speed_factor);
    let mut client = RunnerServiceClient::new(channel)
        .send_compressed(tonic::codec::CompressionEncoding::Zstd)
        .accept_compressed(tonic::codec::CompressionEncoding::Zstd);
    let srv = start_bidirectional_stream(&mut client, state);

    let _notify = sd_notify::notify(
        false,
        &[
            sd_notify::NotifyState::Status("Running"),
            sd_notify::NotifyState::Ready,
        ],
    );
    srv.await?;

    Ok(())
}
