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

fn extract_path_info(
    body: &[u8],
    path: StorePath,
    full_path: &str,
) -> Result<Option<PathInfo>, crate::Error> {
    let mut infos = serde_json::from_slice::<AHashMap<String, Option<InnerPathInfo>>>(body)?;
    Ok(infos
        .remove(full_path)
        .and_then(|i| i.map(|i| PathInfo::new(path, i))))
}

// TODO: evaluate if we need to limit memory here
#[cached(
    ty = "UnboundCache<String, Option<PathInfo>>",
    create = "{ UnboundCache::with_capacity(50) }",
    result = true,
    convert = r#"{ format!("{}", path) }"#
)]
#[tracing::instrument(fields(%path), err)]
pub async fn query_path_info(path: &StorePath) -> Result<Option<PathInfo>, crate::Error> {
    let full_path = path.get_full_path();
    let cmd = &tokio::process::Command::new("nix")
        .args([
            "path-info",
            "--json",
            "--size",
            "--closure-size",
            "--offline",
            "--option",
            "substitute",
            "false",
            "--option",
            "builders",
            "",
            &full_path,
        ])
        .output()
        .await?;
    if cmd.status.success() {
        extract_path_info(&cmd.stdout, path.to_owned(), &full_path)
    } else {
        Ok(None)
    }
}

pub async fn clear_query_path_cache() {
    let mut cache = QUERY_PATH_INFO.lock().await;
    cache.cache_clear();
}

#[tracing::instrument(err)]
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

#[tracing::instrument(skip(path))]
pub async fn check_if_storepath_exists_using_pathinfo(path: &StorePath) -> bool {
    tokio::process::Command::new("nix")
        .args([
            "path-info",
            "--offline",
            "--option",
            "substitute",
            "false",
            "--option",
            "builders",
            "",
            &path.get_full_path(),
        ])
        .output()
        .await
        .map(|cmd| cmd.status.success())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::extract_path_info;
    use crate::StorePath;

    #[test]
    fn can_extract_path_info() {
        let path = StorePath::new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-helsinki-laptop12-25.05.drv",
        );
        let body = r#"{"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-helsinki-laptop12-25.05.drv":{"ca":"text:sha256:07416qdchgddgfm02v3g01z7n63rmhjmga0glv3wf4nly3sw6y5a","closureSize":59548800,"deriver":null,"narHash":"sha256-72ema2mC9vU4ZmnbfHUrKe/jNp7P8MqX7rjgZ7nRR+w=","narSize":29240,"references":[],"registrationTime":1751964968,"signatures":[],"ultimate":false}}"#;
        let info = extract_path_info(body.as_bytes(), path.clone(), &path.get_full_path())
            .unwrap()
            .unwrap();
        assert!(info.deriver.is_none());
        assert_eq!(
            info.ca,
            Some("text:sha256:07416qdchgddgfm02v3g01z7n63rmhjmga0glv3wf4nly3sw6y5a".into())
        );
        assert_eq!(
            info.nar_hash,
            "sha256-72ema2mC9vU4ZmnbfHUrKe/jNp7P8MqX7rjgZ7nRR+w=",
        );
        assert_eq!(info.nar_size, 29240);
        assert_eq!(info.closure_size, 59548800);
    }

    #[test]
    fn can_handle_null() {
        let path = StorePath::new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-helsinki-laptop12-25.05.drv",
        );
        let body = r#"{"/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-nixos-system-helsinki-laptop12-25.05.drv":null}"#;
        let info = extract_path_info(body.as_bytes(), path.clone(), &path.get_full_path()).unwrap();
        assert!(info.is_none());
    }
}
