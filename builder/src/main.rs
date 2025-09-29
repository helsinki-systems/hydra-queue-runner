#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![allow(clippy::match_wildcard_for_single_variants)]

use tracing_subscriber::{Layer as _, Registry, layer::SubscriberExt as _};

mod config;
mod grpc;
mod state;
mod system;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

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

    let mut client = grpc::init_client(&args).await?;
    let state = state::State::new(args)?;
    let task = tokio::spawn({
        let state = state.clone();
        async move { crate::grpc::start_bidirectional_stream(&mut client, state.clone()).await }
    });

    let _notify = sd_notify::notify(
        false,
        &[
            sd_notify::NotifyState::Status("Running"),
            sd_notify::NotifyState::Ready,
        ],
    );

    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    let abort_handle = task.abort_handle();

    tokio::select! {
        _ = sigint.recv() => {
            log::info!("Received sigint - shutting down gracefully");
            let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Stopping]);
            abort_handle.abort();
            state.abort_all_active_builds();
            let _ = state.clear_gcroots();
        }
        _ = sigterm.recv() => {
            log::info!("Received sigterm - shutting down gracefully");
            let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Stopping]);
            abort_handle.abort();
            state.abort_all_active_builds();
            let _ = state.clear_gcroots();
        }
        r = task => {
            let _ = state.clear_gcroots();
            r??;
        }
    };
    Ok(())
}
