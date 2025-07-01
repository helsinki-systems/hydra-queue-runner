use ahash::AHashMap;

#[derive(Clone)]
pub struct RemoteStore<'a> {
    remote_addr: &'a str,
}

impl<'a> RemoteStore<'a> {
    pub fn new(remote_addr: &'a str) -> Self {
        Self { remote_addr }
    }

    #[tracing::instrument(skip(self, path), err)]
    pub async fn copy_path(&self, path: String) -> Result<(), crate::Error> {
        let path = if path.starts_with("/nix/store/") {
            &path
        } else {
            &format!("/nix/store/{path}")
        };

        let mut child = tokio::process::Command::new("nix")
            .args(["copy", "--to", self.remote_addr, path])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        crate::validate_statuscode(child.wait().await?)?;
        Ok(())
    }

    #[tracing::instrument(skip(self, path))]
    pub async fn check_if_storepath_exists(&self, path: &str) -> bool {
        let path = if path.starts_with("/nix/store/") {
            path
        } else {
            &format!("/nix/store/{path}")
        };

        let Ok(cmd) = tokio::process::Command::new("nix")
            .args(["path-info", "--json", "--store", self.remote_addr, path])
            .output()
            .await
        else {
            return false;
        };
        if cmd.status.success() {
            let Ok(infos) =
                serde_json::from_slice::<AHashMap<String, Option<serde_json::Value>>>(&cmd.stdout)
            else {
                return false;
            };

            infos.get(path).map(|v| v.is_some()).unwrap_or_default()
        } else {
            false
        }
    }
}
