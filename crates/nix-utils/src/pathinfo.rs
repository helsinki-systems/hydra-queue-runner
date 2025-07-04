use ahash::AHashMap;
use cached::proc_macro::cached;
use cached::{Cached, UnboundCache};

use crate::StorePath;

#[derive(Debug, serde::Deserialize)]
struct InnerPathInfo {
    ca: Option<String>,
    deriver: Option<String>,
    #[serde(rename = "narHash")]
    nar_hash: String,
    #[serde(rename = "narSize")]
    nar_size: u64,
    #[serde(rename = "closureSize")]
    closure_size: u64,
}

#[derive(Debug, Clone)]
pub struct PathInfo {
    pub path: StorePath,
    pub ca: Option<String>,
    pub deriver: Option<String>,
    pub nar_hash: String,
    pub nar_size: u64,
    pub closure_size: u64,
}

impl PathInfo {
    fn new(p: StorePath, inner: InnerPathInfo) -> Self {
        Self {
            path: p,
            ca: inner.ca,
            deriver: inner.deriver,
            nar_hash: inner.nar_hash,
            nar_size: inner.nar_size,
            closure_size: inner.closure_size,
        }
    }
}

// TODO: evaluate if we need to limit memory here
#[cached(
    ty = "UnboundCache<String, Option<PathInfo>>",
    create = "{ UnboundCache::with_capacity(50) }",
    result = true,
    convert = r#"{ format!("{}", path) }"#
)]
pub async fn query_path_info(path: &StorePath) -> Result<Option<PathInfo>, crate::Error> {
    let full_path = path.get_full_path();
    let cmd = &tokio::process::Command::new("nix")
        .args([
            "path-info",
            "--json",
            "--size",
            "--closure-size",
            &full_path,
        ])
        .output()
        .await?;
    if cmd.status.success() {
        let mut infos = serde_json::from_slice::<AHashMap<String, InnerPathInfo>>(&cmd.stdout)?;
        Ok(infos
            .remove(&full_path)
            .map(|i| PathInfo::new(path.to_owned(), i)))
    } else {
        Ok(None)
    }
}

pub async fn clear_query_path_cache() {
    let mut cache = QUERY_PATH_INFO.lock().await;
    cache.cache_clear();
}

pub async fn query_path_infos(
    paths: &[&StorePath],
) -> Result<AHashMap<StorePath, PathInfo>, crate::Error> {
    let mut res = AHashMap::new();
    for p in paths {
        if let Some(info) = query_path_info(p).await? {
            res.insert((*p).to_owned(), info);
        }
    }
    Ok(res)
}
