#[derive(Debug, serde::Deserialize)]
pub struct NixConfig {
    #[serde(deserialize_with = "crate::deserialize_inner_value")]
    system: String,
    #[serde(
        rename = "extra-platforms",
        deserialize_with = "crate::deserialize_inner_value"
    )]
    extra_platforms: Vec<String>,
    #[serde(
        rename = "system-features",
        deserialize_with = "crate::deserialize_inner_value"
    )]
    pub system_features: Vec<String>,
    #[serde(
        rename = "use-cgroups",
        deserialize_with = "crate::deserialize_inner_value"
    )]
    pub cgroups: bool,
}

impl NixConfig {
    pub fn systems(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(4);
        out.push(self.system.clone());
        out.extend_from_slice(&self.extra_platforms);
        out
    }
}

#[tracing::instrument(err)]
pub async fn get_nix_config() -> Result<NixConfig, crate::Error> {
    let o = &tokio::process::Command::new("nix")
        .args(["config", "show", "--json"])
        .output()
        .await?;
    if let Err(e) = crate::validate_statuscode(o.status) {
        log::error!(
            "{e} stdout={:?} stderr={:?}",
            std::str::from_utf8(&o.stdout),
            std::str::from_utf8(&o.stderr)
        );
        return Err(e);
    };

    Ok(serde_json::from_slice::<NixConfig>(&o.stdout)?)
}
