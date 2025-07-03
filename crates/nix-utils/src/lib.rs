mod config;
mod drv;
mod nar;
mod nix_support;
mod pathinfo;
mod remote;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("serde json error: `{0}`")]
    SerdeJson(#[from] serde_json::Error),

    #[error("std io error: `{0}`")]
    Io(#[from] std::io::Error),

    #[error("utf8 error: `{0}`")]
    Utf8(#[from] std::str::Utf8Error),

    #[error("Failed to get tokio stdout stream")]
    Stream,

    #[error("Command failed with `{0}`")]
    Exit(std::process::ExitStatus),
}

#[derive(Debug, serde::Deserialize)]
pub struct InnerValue<T> {
    pub value: T,
}

fn deserialize_inner_value<'de, T, D>(deserializer: D) -> Result<T, D::Error>
where
    T: serde::Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    let x: InnerValue<T> = serde::Deserialize::deserialize(deserializer)?;
    Ok(x.value)
}

pub use config::{NixConfig, get_nix_config};
pub use drv::{
    BuildOptions, Derivation, Output as DerivationOutput, query_drv, query_drvs,
    query_missing_outputs, query_missing_remote_outputs, realise_drv, topo_sort_drvs,
};
pub use nar::{export_nar, import_nar};
pub use nix_support::{BuildMetric, BuildProduct, NixSupport, parse_nix_support_from_outputs};
pub use pathinfo::{PathInfo, clear_query_path_cache, query_path_info, query_path_infos};
pub use remote::RemoteStore;

#[tracing::instrument(skip(path))]
pub fn check_if_storepath_exists(path: &str) -> bool {
    let path = if path.starts_with("/nix/store/") {
        path
    } else {
        &format!("/nix/store/{path}")
    };

    std::path::Path::new(path).exists()
}

pub fn validate_statuscode(status: std::process::ExitStatus) -> Result<(), Error> {
    if status.success() {
        Ok(())
    } else {
        Err(Error::Exit(status))
    }
}

pub fn add_root(root_dir: &std::path::Path, store_path: &str) {
    let store_path = if store_path.starts_with("/nix/store/") {
        store_path
    } else {
        &format!("/nix/store/{store_path}")
    };
    let store_path_no_prefix = store_path.strip_prefix("/nix/store/").unwrap_or(store_path);

    let path = root_dir.join(store_path_no_prefix);

    if !path.exists() {
        let _ = std::os::unix::fs::symlink(store_path, path);
    }
}
