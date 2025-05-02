use ahash::AHashMap;

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

#[derive(Debug)]
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

#[tracing::instrument(skip(paths), err)]
pub async fn query_path_infos(paths: &[&str]) -> Result<AHashMap<String, PathInfo>, crate::Error> {
    let cmd = &tokio::process::Command::new("nix")
        .args(["path-info", "--json", "--size", "--closure-size"])
        .args(paths)
        .output()
        .await?;
    if cmd.status.success() {
        let infos = serde_json::from_slice::<AHashMap<String, InnerPathInfo>>(&cmd.stdout)?;
        Ok(infos
            .into_iter()
            .map(|(p, i)| (p.clone(), PathInfo::new(p, i)))
            .collect())
    } else {
        Ok(AHashMap::new())
    }
}

#[tracing::instrument(skip(path), err)]
pub async fn query_path_info(path: &str) -> Result<Option<PathInfo>, crate::Error> {
    let path = if path.starts_with("/nix/store/") {
        path
    } else {
        &format!("/nix/store/{path}")
    };

    Ok(query_path_infos(&[path]).await?.remove(path))
}
