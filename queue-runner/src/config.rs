use std::{net::SocketAddr, sync::Arc};

use clap::Parser;
use tokio::sync::RwLock;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// REST server bind
    #[clap(short, long, default_value = "[::1]:8080")]
    pub rest_bind: SocketAddr,

    /// GRPC server bind
    #[clap(short, long, default_value = "[::1]:50051")]
    pub grpc_bind: SocketAddr,

    /// Config path
    #[clap(short, long, default_value = "config.toml")]
    pub config_path: String,
}

impl Args {
    pub fn new() -> Self {
        Self::parse()
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

#[derive(Debug, Default, serde::Deserialize, Copy, Clone, PartialEq, Eq)]
pub enum MachineSortFn {
    SpeedFactorOnly,
    CpuCoreCountWithSpeedFactor,
    #[default]
    BogomipsWithSpeedFactor,
}

/// Main configuration of the application
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AppConfig {
    #[serde(default = "default_log_dir")]
    pub hydra_log_dir: std::path::PathBuf,

    #[serde(default = "default_pg_socket_url")]
    pub db_url: secrecy::SecretString,

    #[serde(default = "default_max_db_connections")]
    pub max_db_connections: u32,

    #[serde(default)]
    pub machine_sort_fn: MachineSortFn,

    pub remote_store_addr: Option<String>,
    pub signing_key_path: Option<std::path::PathBuf>,

    #[serde(default)]
    pub use_substitute: bool,
}

/// Prepared configuration of the application
#[derive(Debug)]
pub struct PreparedApp {
    pub hydra_log_dir: std::path::PathBuf,
    pub db_url: secrecy::SecretString,
    pub max_db_connections: u32,
    pub machine_sort_fn: MachineSortFn,
    remote_store_addr: Option<String>,
    signing_key_path: Option<std::path::PathBuf>,
    pub use_substitute: bool,
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

        Ok(Self {
            hydra_log_dir: val.hydra_log_dir,
            db_url: val.db_url,
            max_db_connections: val.max_db_connections,
            machine_sort_fn: val.machine_sort_fn,
            remote_store_addr,
            signing_key_path,
            use_substitute: val.use_substitute,
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
    pub fn init(filepath: &str) -> anyhow::Result<Arc<RwLock<Self>>> {
        Ok(Arc::new(RwLock::new(load_config(filepath)?)))
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

pub async fn reload(
    current_config: Arc<RwLock<PreparedApp>>,
    filepath: &str,
    state: Arc<crate::state::State>,
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

    if let Err(e) = state.reload_config_callback(&new_config).await {
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
        let mut current_config_unwrapped = current_config.write().await;
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
