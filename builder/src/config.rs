use clap::Parser;

#[derive(Parser, Debug)]
#[clap(
    author,
    version,
    about,
    long_about = None,
)]
pub struct Args {
    /// Gateway endpoint
    #[clap(short, long, default_value = "http://[::1]:50051")]
    pub gateway_endpoint: String,

    /// Ping interval in seconds
    #[clap(short, long, default_value_t = 30)]
    pub ping_interval: u64,

    /// Speed factor that is used when joining the queue-runner
    #[clap(short, long, default_value_t = 1.0)]
    pub speed_factor: f32,

    /// Path to Server root ca cert
    #[clap(long)]
    pub server_root_ca_cert_path: Option<std::path::PathBuf>,

    /// Path to Client cert
    #[clap(long)]
    pub client_cert_path: Option<std::path::PathBuf>,

    /// Path to Client key
    #[clap(long)]
    pub client_key_path: Option<std::path::PathBuf>,
}

impl Args {
    pub fn new() -> Self {
        Self::parse()
    }

    pub fn mtls_enabled(&self) -> bool {
        self.server_root_ca_cert_path.is_some()
            && self.client_cert_path.is_some()
            && self.client_key_path.is_some()
    }

    pub fn mtls_configured_correctly(&self) -> bool {
        self.mtls_enabled()
            || (self.server_root_ca_cert_path.is_none()
                && self.client_cert_path.is_none()
                && self.client_key_path.is_none())
    }

    pub async fn get_mtls(
        &self,
    ) -> anyhow::Result<(tonic::transport::Certificate, tonic::transport::Identity)> {
        let server_root_ca_cert_path = self
            .server_root_ca_cert_path
            .as_deref()
            .ok_or(anyhow::anyhow!("server_root_ca_cert_path not provided"))?;
        let client_cert_path = self
            .client_cert_path
            .as_deref()
            .ok_or(anyhow::anyhow!("client_cert_path not provided"))?;
        let client_key_path = self
            .client_key_path
            .as_deref()
            .ok_or(anyhow::anyhow!("client_key_path not provided"))?;

        let server_root_ca_cert = tokio::fs::read_to_string(server_root_ca_cert_path).await?;
        let server_root_ca_cert = tonic::transport::Certificate::from_pem(server_root_ca_cert);

        let client_cert = tokio::fs::read_to_string(client_cert_path).await?;
        let client_key = tokio::fs::read_to_string(client_key_path).await?;
        let client_identity = tonic::transport::Identity::from_pem(client_cert, client_key);
        Ok((server_root_ca_cert, client_identity))
    }
}
