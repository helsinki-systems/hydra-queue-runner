use ahash::AHashMap;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::LinesStream;

fn deserialize_input_drvs<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let x: AHashMap<String, serde_json::Value> = serde::Deserialize::deserialize(deserializer)?;
    Ok(x.into_keys().collect())
}

#[derive(Debug, serde::Deserialize)]
pub struct OutputPath {
    path: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Output {
    pub name: String,
    pub path: Option<String>,
}

fn deserialize_outputs<'de, D>(deserializer: D) -> Result<Vec<Output>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let x: AHashMap<String, OutputPath> = serde::Deserialize::deserialize(deserializer)?;
    Ok(x.into_iter()
        .map(|(k, v)| Output {
            name: k,
            path: v.path,
        })
        .collect())
}

#[derive(Debug, serde::Deserialize)]
pub struct Derivation {
    // Missing `args`, `builder`, `inputSrcs`,
    pub env: AHashMap<String, String>,
    #[serde(rename = "inputDrvs", deserialize_with = "deserialize_input_drvs")]
    pub input_drvs: Vec<String>,
    #[serde(deserialize_with = "deserialize_outputs")]
    pub outputs: Vec<Output>,
    pub name: String,
    pub system: String,
}

#[tracing::instrument(skip(drvs), err)]
pub async fn query_drvs(drvs: &[&str]) -> Result<Vec<Derivation>, crate::Error> {
    let cmd = &tokio::process::Command::new("nix")
        .args(["derivation", "show"])
        .args(drvs)
        .output()
        .await?;
    if cmd.status.success() {
        let drvs = serde_json::from_slice::<AHashMap<String, Derivation>>(&cmd.stdout)?;
        Ok(drvs.into_values().collect())
    } else {
        Ok(vec![])
    }
}

#[tracing::instrument(skip(drv), err)]
pub async fn query_drv(drv: &str) -> Result<Option<Derivation>, crate::Error> {
    // should only return 1 drv, so we can pop first
    let drv = if drv.starts_with("/nix/store/") {
        drv
    } else {
        &format!("/nix/store/{drv}")
    };

    Ok(query_drvs(&[drv]).await?.into_iter().next())
}

#[tracing::instrument(skip(drv), err)]
pub async fn topo_sort_drvs(drv: &str) -> Result<Vec<String>, crate::Error> {
    use std::io::BufRead as _;

    // should only return 1 drv, so we can pop first
    let drv = if drv.starts_with("/nix/store/") {
        drv
    } else {
        &format!("/nix/store/{drv}")
    };

    let cmd = &tokio::process::Command::new("nix-store")
        .args(["-qR", drv])
        .output()
        .await?;
    if cmd.status.success() {
        let cursor = std::io::Cursor::new(&cmd.stdout);
        Ok(std::io::BufReader::new(cursor)
            .lines()
            .map_while(Result::ok)
            .collect::<Vec<String>>())
    } else {
        Ok(vec![])
    }
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    max_log_size: u64,
    max_silent_time: i32,
    build_timeout: i32,
    substitute: bool,
    build: bool,
}

fn format_bool(v: bool) -> &'static str {
    if v { "true" } else { "false" }
}

impl BuildOptions {
    pub fn new(max_log_size: Option<u64>) -> Self {
        Self {
            max_log_size: max_log_size.unwrap_or(64u64 << 20),
            max_silent_time: 0,
            build_timeout: 0,
            substitute: false,
            build: true,
        }
    }

    pub fn complete(max_log_size: u64, max_silent_time: i32, build_timeout: i32) -> Self {
        Self {
            max_log_size,
            max_silent_time,
            build_timeout,
            substitute: false,
            build: true,
        }
    }

    pub fn substitute_only() -> Self {
        let mut o = Self::new(None);
        o.build = false;
        o.substitute = true;
        o
    }

    pub fn set_max_silent_time(&mut self, max_silent_time: i32) {
        self.max_silent_time = max_silent_time;
    }

    pub fn set_build_timeout(&mut self, build_timeout: i32) {
        self.build_timeout = build_timeout;
    }

    pub fn get_max_log_size(&self) -> u64 {
        self.max_log_size
    }

    pub fn get_max_silent_time(&self) -> i32 {
        self.max_silent_time
    }

    pub fn get_build_timeout(&self) -> i32 {
        self.build_timeout
    }
}

#[allow(clippy::type_complexity)]
#[tracing::instrument(skip(drv, opts), err)]
pub async fn realise_drv(
    drv: &str,
    opts: &BuildOptions,
) -> Result<
    (
        tokio::process::Child,
        tokio_stream::adapters::Merge<
            LinesStream<BufReader<tokio::process::ChildStdout>>,
            LinesStream<BufReader<tokio::process::ChildStderr>>,
        >,
    ),
    crate::Error,
> {
    let drv = if drv.starts_with("/nix/store/") {
        drv
    } else {
        &format!("/nix/store/{drv}")
    };

    let mut child = tokio::process::Command::new("nix-store")
        .args([
            "-r",
            "--max-silent-time",
            &opts.max_silent_time.to_string(),
            "--timeout",
            &opts.build_timeout.to_string(),
            "--option",
            "max-build-log-size",
            &opts.max_log_size.to_string(),
            "--option",
            "fallback",
            format_bool(opts.build),
            "--option",
            "substitute",
            format_bool(opts.substitute),
            drv,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let stdout = child.stdout.take().ok_or(crate::Error::Stream)?;
    let stderr = child.stderr.take().ok_or(crate::Error::Stream)?;

    let stdout = LinesStream::new(BufReader::new(stdout).lines());
    let stderr = LinesStream::new(BufReader::new(stderr).lines());

    Ok((child, StreamExt::merge(stdout, stderr)))
}

#[tracing::instrument(skip(outputs))]
pub async fn query_missing_outputs(outputs: &[Output]) -> Vec<Output> {
    let mut missing = vec![];
    for o in outputs {
        if let Some(path) = &o.path {
            if !super::check_if_storepath_exists(path).await {
                missing.push(o.clone());
            }
        }
    }
    missing
}
