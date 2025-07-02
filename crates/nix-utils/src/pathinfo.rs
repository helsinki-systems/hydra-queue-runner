use ahash::AHashMap;
use cached::proc_macro::cached;
use cached::{Cached, UnboundCache};

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
    pub path: String,
    pub ca: Option<String>,
    pub deriver: Option<String>,
    pub nar_hash: String,
    pub nar_size: u64,
    pub closure_size: u64,
}

impl PathInfo {
    fn new(p: String, inner: InnerPathInfo) -> Self {
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
pub async fn query_path_info(path: &str) -> Result<Option<PathInfo>, crate::Error> {
    let path = if path.starts_with("/nix/store/") {
        path
    } else {
        &format!("/nix/store/{path}")
    };

    let cmd = &tokio::process::Command::new("nix")
        .args(["path-info", "--json", "--size", "--closure-size", path])
        .output()
        .await?;
    if cmd.status.success() {
        let mut infos = serde_json::from_slice::<AHashMap<String, InnerPathInfo>>(&cmd.stdout)?;
        Ok(infos
            .remove(path)
            .map(|i| PathInfo::new(path.to_string(), i)))
    } else {
        Ok(None)
    }
}

pub async fn clear_query_path_cache() {
    let mut cache = QUERY_PATH_INFO.lock().await;
    cache.cache_clear();
}

pub async fn query_path_infos(paths: &[&str]) -> Result<AHashMap<String, PathInfo>, crate::Error> {
    let mut res = AHashMap::new();
    for p in paths {
        if let Some(info) = query_path_info(p).await? {
            res.insert(p.to_string(), info);
        }
    }
    Ok(res)
}
