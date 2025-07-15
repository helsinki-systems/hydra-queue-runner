use std::{net::SocketAddr, sync::Arc};

use clap::Parser;
use tracing_subscriber::{Layer as _, Registry, layer::SubscriberExt as _};

pub fn init_tracing() -> anyhow::Result<
    tracing_subscriber::reload::Handle<tracing_subscriber::EnvFilter, tracing_subscriber::Registry>,
> {
    tracing_log::LogTracer::init()?;
    let (log_env_filter, reload_handle) = tracing_subscriber::reload::Layer::new(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    );
    let fmt_layer = tracing_subscriber::fmt::layer()
        .compact()
        .with_filter(log_env_filter);
    let subscriber = Registry::default().with(fmt_layer);
    tracing::subscriber::set_global_default(subscriber)?;
    Ok(reload_handle)
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Query the queue runner status
    #[clap(long)]
    pub status: bool,

    /// REST server bind
    #[clap(short, long, default_value = "[::1]:8080")]
    pub rest_bind: SocketAddr,

    /// GRPC server bind
    #[clap(short, long, default_value = "[::1]:50051")]
    pub grpc_bind: SocketAddr,

    /// Config path
    #[clap(short, long, default_value = "config.toml")]
    pub config_path: String,

    /// Path to Server cert
    #[clap(long)]
    pub server_cert_path: Option<std::path::PathBuf>,

    /// Path to Server key
    #[clap(long)]
    pub server_key_path: Option<std::path::PathBuf>,

    /// Path to Client ca cert
    #[clap(long)]
    pub client_ca_cert_path: Option<std::path::PathBuf>,
}

impl Args {
    pub fn new() -> Self {
        Self::parse()
    }

    pub fn mtls_enabled(&self) -> bool {
        self.server_cert_path.is_some()
            && self.server_key_path.is_some()
            && self.client_ca_cert_path.is_some()
    }

    pub fn mtls_configured_correctly(&self) -> bool {
        self.mtls_enabled()
            || (self.server_cert_path.is_none()
                && self.server_key_path.is_none()
                && self.client_ca_cert_path.is_none())
    }

    pub async fn get_mtls(
        &self,
    ) -> anyhow::Result<(tonic::transport::Certificate, tonic::transport::Identity)> {
        let server_cert_path = self
            .server_cert_path
            .as_deref()
            .ok_or(anyhow::anyhow!("server_cert_path not provided"))?;
        let server_key_path = self
            .server_key_path
            .as_deref()
            .ok_or(anyhow::anyhow!("server_key_path not provided"))?;

        let client_ca_cert_path = self
            .client_ca_cert_path
            .as_deref()
            .ok_or(anyhow::anyhow!("client_ca_cert_path not provided"))?;
        let client_ca_cert = tokio::fs::read_to_string(client_ca_cert_path).await?;
        let client_ca_cert = tonic::transport::Certificate::from_pem(client_ca_cert);

        let server_cert = tokio::fs::read_to_string(server_cert_path).await?;
        let server_key = tokio::fs::read_to_string(server_key_path).await?;
        let server_identity = tonic::transport::Identity::from_pem(server_cert, server_key);
        Ok((client_ca_cert, server_identity))
    }
}

fn default_log_dir() -> std::path::PathBuf {
    "/tmp/hydra".into()
}

fn default_pg_socket_url() -> secrecy::SecretString {
    "postgres://hydra@%2Frun%2Fpostgresql:5432/hydra".into()
}

fn default_max_db_connections() -> u32 {
    128
}

fn default_dispatch_trigger_timer_in_s() -> i64 {
    120
}

fn default_queue_trigger_timer_in_s() -> i64 {
    -1
}

fn default_max_tries() -> u32 {
    5
}

fn default_retry_interval() -> u32 {
    60
}

fn default_retry_backoff() -> f32 {
    3.0
}

fn default_stop_queue_run_after_in_s() -> i64 {
    120
}

#[derive(Debug, Default, serde::Deserialize, Copy, Clone, PartialEq, Eq)]
pub enum MachineSortFn {
    SpeedFactorOnly,
    CpuCoreCountWithSpeedFactor,
    #[default]
    BogomipsWithSpeedFactor,
}

#[derive(Debug, Default, serde::Deserialize, Copy, Clone, PartialEq, Eq)]
pub enum MachineFreeFn {
    Dynamic,
    #[default]
    Static,
}

/// Main configuration of the application
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
struct AppConfig {
    #[serde(default = "default_log_dir")]
    hydra_log_dir: std::path::PathBuf,

    #[serde(default = "default_pg_socket_url")]
    db_url: secrecy::SecretString,

    #[serde(default = "default_max_db_connections")]
    max_db_connections: u32,

    #[serde(default)]
    machine_sort_fn: MachineSortFn,

    #[serde(default)]
    machine_free_fn: MachineFreeFn,

    // setting this to -1, will disable the timer
    #[serde(default = "default_dispatch_trigger_timer_in_s")]
    dispatch_trigger_timer_in_s: i64,

    // setting this to -1, will disable the timer
    #[serde(default = "default_queue_trigger_timer_in_s")]
    queue_trigger_timer_in_s: i64,

    remote_store_addr: Option<String>,
    signing_key_path: Option<std::path::PathBuf>,

    #[serde(default)]
    use_substitutes: bool,

    roots_dir: Option<std::path::PathBuf>,

    #[serde(default = "default_max_tries")]
    max_retries: u32,

    #[serde(default = "default_retry_interval")]
    retry_interval: u32,

    #[serde(default = "default_retry_backoff")]
    retry_backoff: f32,

    #[serde(default = "default_stop_queue_run_after_in_s")]
    stop_queue_run_after_in_s: i64,
}

/// Prepared configuration of the application
#[derive(Debug)]
pub struct PreparedApp {
    pub hydra_log_dir: std::path::PathBuf,
    pub db_url: secrecy::SecretString,
    pub max_db_connections: u32,
    pub machine_sort_fn: MachineSortFn,
    pub machine_free_fn: MachineFreeFn,
    pub dispatch_trigger_timer: Option<tokio::time::Duration>,
    pub queue_trigger_timer: Option<tokio::time::Duration>,
    remote_store_addr: Option<String>,
    signing_key_path: Option<std::path::PathBuf>,
    pub use_substitutes: bool,
    pub roots_dir: std::path::PathBuf,
    pub max_retries: u32,
    pub retry_interval: f32,
    pub retry_backoff: f32,
    pub stop_queue_run_after: Option<chrono::Duration>,
}

impl TryFrom<AppConfig> for PreparedApp {
    type Error = anyhow::Error;

    fn try_from(val: AppConfig) -> Result<Self, Self::Error> {
        let signing_key_path = val.signing_key_path.and_then(|v| {
            if std::fs::exists(&v).unwrap_or_default() {
                Some(v)
            } else {
                None
            }
        });
        let remote_store_addr = val.remote_store_addr.and_then(|v| {
            if v.starts_with("file://")
                || v.starts_with("s3://")
                || v.starts_with("ssh://")
                || v.starts_with('/')
            {
                Some(v)
            } else {
                None
            }
        });

        let logname = std::env::var("LOGNAME").expect("LOGNAME not set");
        let nix_state_dir = std::env::var("NIX_STATE_DIR").unwrap_or("/nix/var/nix/".to_owned());
        let roots_dir = if let Some(roots_dir) = val.roots_dir {
            roots_dir
        } else {
            std::path::PathBuf::from(nix_state_dir)
                .join("gcroots/per-user")
                .join(logname)
                .join("hydra-roots")
        };
        std::fs::create_dir_all(&roots_dir)?;

        Ok(Self {
            hydra_log_dir: val.hydra_log_dir,
            db_url: val.db_url,
            max_db_connections: val.max_db_connections,
            machine_sort_fn: val.machine_sort_fn,
            machine_free_fn: val.machine_free_fn,
            dispatch_trigger_timer: u64::try_from(val.dispatch_trigger_timer_in_s)
                .ok()
                .and_then(|v| {
                    if v == 0 {
                        None
                    } else {
                        Some(tokio::time::Duration::from_secs(v))
                    }
                }),
            queue_trigger_timer: u64::try_from(val.queue_trigger_timer_in_s)
                .ok()
                .and_then(|v| {
                    if v == 0 {
                        None
                    } else {
                        Some(tokio::time::Duration::from_secs(v))
                    }
                }),
            remote_store_addr,
            signing_key_path,
            use_substitutes: val.use_substitutes,
            roots_dir,
            max_retries: val.max_retries,
            #[allow(clippy::cast_precision_loss)]
            retry_interval: val.retry_interval as f32,
            retry_backoff: val.retry_backoff,
            stop_queue_run_after: if val.stop_queue_run_after_in_s <= 0 {
                None
            } else {
                Some(chrono::Duration::seconds(val.stop_queue_run_after_in_s))
            },
        })
    }
}

/// Loads the config from specified path
fn load_config(filepath: &str) -> anyhow::Result<PreparedApp> {
    log::info!("Trying to loading file: {filepath}");
    let toml: AppConfig = if let Ok(content) = std::fs::read_to_string(filepath) {
        toml::from_str(&content).map_err(|e| anyhow::anyhow!("Failed to load '{filepath}': {e}"))?
    } else {
        log::warn!("no config file found! Using default config");
        toml::from_str("").map_err(|e| anyhow::anyhow!("Failed to parse \"\": {e}"))?
    };
    log::info!("Loaded config: {toml:?}");

    toml.try_into()
        .map_err(|e| anyhow::anyhow!("Failed to prepare configuration: {e}"))
}

impl PreparedApp {
    pub fn init(filepath: &str) -> anyhow::Result<Arc<parking_lot::RwLock<Self>>> {
        Ok(Arc::new(parking_lot::RwLock::new(load_config(filepath)?)))
    }

    pub fn get_remote_store_addr(&self) -> Option<String> {
        if let Some(url) = &self.remote_store_addr {
            if let Some(secret_key) = &self.signing_key_path {
                Some(format!("{url}?secret-key={}", secret_key.to_string_lossy()))
            } else {
                Some(url.clone())
            }
        } else {
            None
        }
    }
}

pub fn reload(
    current_config: &Arc<parking_lot::RwLock<PreparedApp>>,
    filepath: &str,
    state: &Arc<crate::state::State>,
) {
    let new_config = match load_config(filepath) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("Failed to load new config: {e}");
            let _notify = sd_notify::notify(
                false,
                &[
                    sd_notify::NotifyState::Status("Reload failed"),
                    sd_notify::NotifyState::Errno(1),
                ],
            );

            return;
        }
    };

    if let Err(e) = state.reload_config_callback(&new_config) {
        log::error!("Config reload failed with {e}");
        let _notify = sd_notify::notify(
            false,
            &[
                sd_notify::NotifyState::Status("Configuration reloaded failed - Running"),
                sd_notify::NotifyState::Errno(1),
            ],
        );
        return;
    }

    {
        let mut current_config_unwrapped = current_config.write();
        *current_config_unwrapped = new_config;
    }

    let _notify = sd_notify::notify(
        false,
        &[
            sd_notify::NotifyState::Status("Configuration reloaded - Running"),
            sd_notify::NotifyState::Ready,
        ],
    );
}
