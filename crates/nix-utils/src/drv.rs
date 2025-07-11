use ahash::AHashMap;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio_stream::wrappers::LinesStream;

use crate::StorePath;

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

#[derive(Debug, Clone)]
pub struct Output {
    pub name: String,
    pub path: Option<StorePath>,
}

fn deserialize_outputs<'de, D>(deserializer: D) -> Result<Vec<Output>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let x: AHashMap<String, OutputPath> = serde::Deserialize::deserialize(deserializer)?;
    Ok(x.into_iter()
        .map(|(k, v)| Output {
            name: k,
            path: v.path.map(|v| StorePath::new(&v)),
        })
        .collect())
}

#[derive(Debug, serde::Deserialize)]
pub struct Derivation {
    // Missing `args`, `builder`, `inputSrcs`,
    // we dont need env right now, so we dont need to extract it and keep it in memory
    // pub env: AHashMap<String, String>,
    #[serde(rename = "inputDrvs", deserialize_with = "deserialize_input_drvs")]
    pub input_drvs: Vec<String>,
    #[serde(deserialize_with = "deserialize_outputs")]
    pub outputs: Vec<Output>,
    pub name: String,
    pub system: String,
}

#[tracing::instrument(err)]
pub async fn query_drvs(drvs: &[&StorePath]) -> Result<Vec<Derivation>, crate::Error> {
    let cmd = &tokio::process::Command::new("nix")
        .args(["derivation", "show"])
        .args(drvs.iter().map(|v| v.get_full_path()))
        .output()
        .await?;
    if cmd.status.success() {
        let drvs = serde_json::from_slice::<AHashMap<String, Option<Derivation>>>(&cmd.stdout)?;
        Ok(drvs.into_values().flatten().collect())
    } else {
        log::warn!(
            "nix derivation show returned exit={} stdout={:?} stderr={:?}",
            cmd.status,
            std::str::from_utf8(&cmd.stdout),
            std::str::from_utf8(&cmd.stderr),
        );
        Ok(vec![])
    }
}

#[tracing::instrument(fields(%drv), err)]
pub async fn query_drv(drv: &StorePath) -> Result<Option<Derivation>, crate::Error> {
    Ok(query_drvs(&[drv]).await?.into_iter().next())
}

#[tracing::instrument(fields(%drv), err)]
pub async fn topo_sort_drvs(drv: &StorePath) -> Result<Vec<String>, crate::Error> {
    use std::io::BufRead as _;

    let cmd = &tokio::process::Command::new("nix-store")
        .args(["-qR", &drv.get_full_path()])
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
#[tracing::instrument(skip(opts), fields(%drv), err)]
pub async fn realise_drv(
    drv: &StorePath,
    opts: &BuildOptions,
    kill_on_drop: bool,
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
    use tokio_stream::StreamExt;

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
            &drv.get_full_path(),
        ])
        .kill_on_drop(kill_on_drop)
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
pub fn query_missing_outputs(outputs: &[Output]) -> Vec<Output> {
    let mut missing = vec![];
    for o in outputs {
        if let Some(path) = &o.path {
            if !super::check_if_storepath_exists(path) {
                missing.push(o.clone());
            }
        }
    }
    missing
}

#[tracing::instrument(skip(outputs, remote_store_url))]
pub async fn query_missing_remote_outputs(
    outputs: Vec<Output>,
    remote_store_url: &str,
) -> Vec<Output> {
    use futures::stream::StreamExt as _;

    tokio_stream::iter(outputs)
        .map(|o| async move {
            let Some(path) = &o.path else {
                return None;
            };
            let remote_store = crate::RemoteStore::new(remote_store_url);
            if !remote_store.check_if_storepath_exists(path).await {
                Some(o)
            } else {
                None
            }
        })
        .buffered(50)
        .filter_map(|o| async { o })
        .collect()
        .await
}
