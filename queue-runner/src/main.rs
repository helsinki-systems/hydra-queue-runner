#![deny(clippy::all)]
#![deny(clippy::pedantic)]

use tracing_subscriber::{Layer as _, Registry, layer::SubscriberExt as _};

use state::State;

mod config;
mod db;
mod server;
mod state;
mod utils;

fn start_task_loops(state: std::sync::Arc<State>) {
    log::info!("QueueRunner starting task loops");

    spawn_config_reloader(state.clone(), state.config.clone(), &state.args.config_path);
    tokio::task::spawn({
        let state = state.clone();
        async move {
            if let Err(e) = state.start_queue_monitor_loop().await {
                log::error!("Failed to spawn queue monitor loop. e={e}");
            }
        }
    });
    tokio::task::spawn(async move { state.start_dispatch_loop().await });
}

fn spawn_config_reloader(
    state: std::sync::Arc<State>,
    current_config: std::sync::Arc<tokio::sync::RwLock<config::PreparedApp>>,
    filepath: &str,
) {
    let filepath = filepath.to_owned();
    tokio::spawn(async move {
        loop {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .unwrap()
                .recv()
                .await
                .unwrap();
            log::info!("Reloading...");
            let current_config = current_config.clone();
            config::reload(current_config.clone(), &filepath, state.clone()).await;
        }
    });
}

#[cfg(debug_assertions)]
fn init_tracing() -> anyhow::Result<()> {
    tracing_log::LogTracer::init()?;
    let log_env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_filter(log_env_filter);
    let console_layer = console_subscriber::spawn();

    let subscriber = Registry::default().with(fmt_layer).with(console_layer);
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}

#[cfg(not(debug_assertions))]
fn init_tracing() -> anyhow::Result<()> {
    tracing_log::LogTracer::init()?;
    let log_env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_filter(log_env_filter);
    let subscriber = Registry::default().with(fmt_layer);
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing()?;

    let state = State::new().await?;

    start_task_loops(state.clone());
    log::info!(
        "QueueRunner listening on grpc: {} and rest: {}",
        state.args.grpc_bind,
        state.args.rest_bind
    );
    let srv1 = server::grpc::Server::run(state.args.grpc_bind, state.clone());
    let srv2 = server::http::Server::run(state.args.rest_bind, state.clone());

    let handle = futures_util::future::join(srv1, srv2);

    let _notify = sd_notify::notify(
        false,
        &[
            sd_notify::NotifyState::Status("Running"),
            sd_notify::NotifyState::Ready,
        ],
    );

    match handle.await {
        (Ok(()), Ok(())) => (),
        (Ok(()), Err(e)) => {
            log::error!("hyper error while awaiting handle: {e}");
            std::process::exit(1);
        }
        (Err(e), Ok(())) => {
            log::error!("tonic error while awaiting handle: {e}");
            std::process::exit(1);
        }
        (Err(e1), Err(e2)) => {
            log::error!("tonic and hyper error while awaiting handle: {e1} | {e2}");
            std::process::exit(1);
        }
    }

    Ok(())
}
