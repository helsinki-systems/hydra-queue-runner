#![deny(clippy::all)]
#![deny(clippy::pedantic)]

use state::State;

mod config;
mod db;
mod io;
mod server;
mod state;
mod utils;

fn start_task_loops(state: std::sync::Arc<State>) {
    log::info!("QueueRunner starting task loops");

    spawn_config_reloader(state.clone(), state.config.clone(), &state.args.config_path);
    state.clone().start_queue_monitor_loop();
    state.clone().start_dispatch_loop();
    state.start_dump_status_loop();
}

fn spawn_config_reloader(
    state: std::sync::Arc<State>,
    current_config: std::sync::Arc<parking_lot::RwLock<config::PreparedApp>>,
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
            config::reload(&current_config, &filepath, &state);
        }
    });
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let reload_handle = config::init_tracing()?;

    let state = State::new(reload_handle).await?;
    if state.args.status {
        state.get_status_from_main_process().await?;
        return Ok(());
    }

    if !state.args.mtls_configured_correctly() {
        log::error!(
            "mtls configured inproperly, please pass all options: server_cert_path, server_key_path and client_ca_cert_path!"
        );
        return Err(anyhow::anyhow!("Configuration issue"));
    }

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
