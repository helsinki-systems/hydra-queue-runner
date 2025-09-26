use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio_stream::wrappers::LinesStream;

use crate::StorePath;

#[derive(Debug, Clone)]
pub struct BuildOptions {
    max_log_size: u64,
    max_silent_time: i32,
    build_timeout: i32,
}

impl BuildOptions {
    pub fn new(max_log_size: Option<u64>) -> Self {
        Self {
            max_log_size: max_log_size.unwrap_or(64u64 << 20),
            max_silent_time: 0,
            build_timeout: 0,
        }
    }

    pub fn complete(max_log_size: u64, max_silent_time: i32, build_timeout: i32) -> Self {
        Self {
            max_log_size,
            max_silent_time,
            build_timeout,
        }
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
#[tracing::instrument(skip(opts, drvs), err)]
pub async fn realise_drvs(
    drvs: &[&StorePath],
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
            "--quiet", // we want to always set this
            "--max-silent-time",
            &opts.max_silent_time.to_string(),
            "--timeout",
            &opts.build_timeout.to_string(),
            "--option",
            "max-build-log-size",
            &opts.max_log_size.to_string(),
            "--option",
            "fallback",
            "true",
            "--option",
            "substitute",
            "false",
            "--option",
            "builders",
            "",
        ])
        .args(drvs.iter().map(|v| v.get_full_path()))
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
    realise_drvs(&[drv], opts, kill_on_drop).await
}
