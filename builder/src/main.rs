#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![allow(clippy::match_wildcard_for_single_variants)]

mod config;
mod grpc;
mod state;
mod system;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _tracing_guard = hydra_tracing::init()?;
    nix_utils::init_nix();

    let args = config::Args::new();

    let state = state::State::new(&args)?;
    let mut client = grpc::init_client(&args).await?;
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
