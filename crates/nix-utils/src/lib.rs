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

    #[error("tokio join error: `{0}`")]
    TokioJoin(#[from] tokio::task::JoinError),

    #[error("utf8 error: `{0}`")]
    Utf8(#[from] std::str::Utf8Error),

    #[error("Failed to get tokio stdout stream")]
    Stream,

    #[error("regex error: `{0}`")]
    Regex(#[from] regex::Error),

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
pub use nar::{export_nar, export_nars, import_nar};
pub use nix_support::{BuildMetric, BuildProduct, NixSupport, parse_nix_support_from_outputs};
pub use pathinfo::{PathInfo, clear_query_path_cache, query_path_info, query_path_infos};
pub use remote::RemoteStore;

pub const HASH_LEN: usize = 32;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct StorePath {
    base_name: String,
}

impl StorePath {
    pub fn new(p: &str) -> Self {
        if let Some(postfix) = p.strip_prefix("/nix/store/") {
            debug_assert!(postfix.len() > HASH_LEN + 1);
            Self {
                base_name: postfix.to_string(),
            }
        } else {
            debug_assert!(p.len() > HASH_LEN + 1);
            Self {
                base_name: p.to_string(),
            }
        }
    }

    pub fn base_name(&self) -> &str {
        &self.base_name
    }

    pub fn name(&self) -> &str {
        &self.base_name[HASH_LEN + 1..]
    }

    pub fn hash_part(&self) -> &str {
        &self.base_name[..HASH_LEN]
    }

    pub fn get_full_path(&self) -> String {
        format!("/nix/store/{}", self.base_name)
    }
}
impl serde::Serialize for StorePath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.base_name())
    }
}

impl std::fmt::Display for StorePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        writeln!(f, "{}", self.base_name)
    }
}

#[tracing::instrument(skip(path))]
pub fn check_if_storepath_exists(path: &StorePath) -> bool {
    std::path::Path::new(&path.get_full_path()).exists()
}

pub fn validate_statuscode(status: std::process::ExitStatus) -> Result<(), Error> {
    if status.success() {
        Ok(())
    } else {
        Err(Error::Exit(status))
    }
}

pub fn add_root(root_dir: &std::path::Path, store_path: &StorePath) {
    let path = root_dir.join(store_path.base_name());
    if !path.exists() {
        let _ = std::os::unix::fs::symlink(store_path.get_full_path(), path);
    }
}
