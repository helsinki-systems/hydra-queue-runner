use ahash::AHashMap;

use crate::StorePath;

#[derive(Clone)]
pub struct RemoteStore<'a> {
    remote_addr: &'a str,
}

impl<'a> RemoteStore<'a> {
    pub fn new(remote_addr: &'a str) -> Self {
        Self { remote_addr }
    }

    #[tracing::instrument(skip(self, path), err)]
    pub async fn copy_path(&self, path: StorePath) -> Result<(), crate::Error> {
        let mut child = tokio::process::Command::new("nix")
            .args(["copy", "--to", self.remote_addr, &path.get_full_path()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        crate::validate_statuscode(child.wait().await?)?;
        Ok(())
    }

    #[tracing::instrument(skip(self, path))]
    pub async fn check_if_storepath_exists(&self, path: &StorePath) -> bool {
        let full_path = path.get_full_path();
        let Ok(cmd) = tokio::process::Command::new("nix")
            .args([
                "path-info",
                "--json",
                "--store",
                self.remote_addr,
                &full_path,
            ])
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

            infos
                .get(&full_path)
                .map(|v| v.is_some())
                .unwrap_or_default()
        } else {
            false
        }
    }
}
