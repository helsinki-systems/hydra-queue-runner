#![deny(clippy::all)]
#![deny(clippy::pedantic)]
#![allow(clippy::missing_errors_doc)]
#![recursion_limit = "256"]

pub mod config;
pub mod io;
pub mod server;
pub mod state;
pub mod utils;

use state::State;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn start_task_loops(state: &std::sync::Arc<State>) -> Vec<tokio::task::AbortHandle> {
    tracing::info!("QueueRunner starting task loops");

    let mut service_list = vec![
        spawn_config_reloader(state.clone(), state.config.clone(), &state.cli.config_path),
        state.clone().start_queue_monitor_loop(),
        state.clone().start_dispatch_loop(),
        state.clone().start_dump_status_loop(),
        state.clone().start_uploader_queue(),
    ];
    if let Some(fod_checker) = &state.fod_checker {
        service_list.push(fod_checker.clone().start_traverse_loop());
    }

    service_list
}

fn spawn_config_reloader(
    state: std::sync::Arc<State>,
    current_config: config::App,
    filepath: &str,
) -> tokio::task::AbortHandle {
    let filepath = filepath.to_owned();
    let task = tokio::spawn(async move {
        loop {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .unwrap()
                .recv()
                .await
                .unwrap();
            tracing::info!("Reloading...");
            config::reload(&current_config, &filepath, &state).await;
        }
    });
    task.abort_handle()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let tracing_guard = hydra_tracing::init()?;
    nix_utils::init_nix();

    let state = State::new(&tracing_guard).await?;
    if state.cli.status {
        state.get_status_from_main_process().await?;
        return Ok(());
    }

    if !state.cli.mtls_configured_correctly() {
        tracing::error!(
            "mtls configured inproperly, please pass all options: server_cert_path, server_key_path and client_ca_cert_path!"
        );
        return Err(anyhow::anyhow!("Configuration issue"));
    }

    let lockfile_path = state.config.get_lockfile();
    let _lock = lockfile::Lockfile::create_with_parents(&lockfile_path).map_err(|e| {
        anyhow::anyhow!(
            "Another instance is already running. Path={} Internal Error: {e}",
            lockfile_path.display()
        )
    })?;

    let task_abort_handles = start_task_loops(&state);
    tracing::info!(
        "QueueRunner listening on grpc: {:?} and rest: {}",
        state.cli.grpc_bind,
        state.cli.rest_bind
    );
    let srv1 = server::grpc::Server::run(state.cli.grpc_bind.clone(), state.clone());
    let srv2 = server::http::Server::run(state.cli.rest_bind, state.clone());

    let task = tokio::spawn(async move {
        match futures_util::future::join(srv1, srv2).await {
            (Ok(()), Ok(())) => Ok(()),
            (Ok(()), Err(e)) => Err(anyhow::anyhow!("hyper error while awaiting handle: {e}")),
            (Err(e), Ok(())) => Err(anyhow::anyhow!("tonic error while awaiting handle: {e}")),
            (Err(e1), Err(e2)) => Err(anyhow::anyhow!(
                "tonic and hyper error while awaiting handle: {e1} | {e2}"
            )),
        }
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
            tracing::info!("Received sigint - shutting down gracefully");
            let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Stopping]);
            abort_handle.abort();
            for h in task_abort_handles {
                h.abort();
            }
            // removing all machines will also mark all currently running jobs as cancelled
            state.remove_all_machines().await;
            let _ = state.clear_busy().await;
            Ok(())
        }
        _ = sigterm.recv() => {
            tracing::info!("Received sigterm - shutting down gracefully");
            let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Stopping]);
            abort_handle.abort();
            for h in task_abort_handles {
                h.abort();
            }
            // removing all machines will also mark all currently running jobs as cancelled
            state.remove_all_machines().await;
            let _ = state.clear_busy().await;
            Ok(())
        }
        r = task => {
            r??;
            Ok(())
        }
    }
}
