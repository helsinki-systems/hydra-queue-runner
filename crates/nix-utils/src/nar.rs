use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_stream::StreamExt as _;

use crate::StorePath;

#[tracing::instrument(skip(path), err)]
pub async fn export_nar(
    path: &StorePath,
    kill_on_drop: bool,
) -> Result<
    (
        tokio::process::Child,
        tokio_util::io::ReaderStream<tokio::process::ChildStdout>,
    ),
    crate::Error,
> {
    let mut child = tokio::process::Command::new("nix-store")
        .args(["--export", &path.get_full_path()])
        .kill_on_drop(kill_on_drop)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    let stdout = child.stdout.take().ok_or(crate::Error::Stream)?;
    Ok((child, tokio_util::io::ReaderStream::new(stdout)))
}

#[tracing::instrument(skip(paths), err)]
pub async fn export_nars(
    paths: &[StorePath],
    kill_on_drop: bool,
) -> Result<
    (
        tokio::process::Child,
        tokio_util::io::ReaderStream<tokio::process::ChildStdout>,
    ),
    crate::Error,
> {
    let mut child = tokio::process::Command::new("nix-store")
        .args(["--export"])
        .args(paths.iter().map(|v| v.get_full_path()))
        .kill_on_drop(kill_on_drop)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    let stdout = child.stdout.take().ok_or(crate::Error::Stream)?;
    Ok((child, tokio_util::io::ReaderStream::new(stdout)))
}

#[tracing::instrument(skip(input_stream), err)]
pub async fn import_nar<S>(mut input_stream: S, kill_on_drop: bool) -> Result<(), crate::Error>
where
    S: tokio_stream::Stream<Item = tokio_util::bytes::Bytes> + Send + Unpin + 'static,
{
    let mut child = tokio::process::Command::new("nix-store")
        .arg("--import")
        .kill_on_drop(kill_on_drop)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().ok_or(crate::Error::Stream)?;
    let mut stdout = child.stdout.take().ok_or(crate::Error::Stream)?;
    let mut stderr = child.stderr.take().ok_or(crate::Error::Stream)?;

    // Spawn a task to write to stdin
    tokio::spawn(async move {
        while let Some(chunk) = input_stream.next().await {
            if let Err(e) = stdin.write_all(&chunk).await {
                log::error!("Error writing to child stdin: {e}");
                return;
            }
        }

        // Close stdin to signal EOF
        if let Err(e) = stdin.shutdown().await {
            log::error!("Failed to shutdown stdin: {e}");
        }
    });

    let out = child.wait().await?;
    if out.success() {
        Ok(())
    } else {
        let mut stdout_str = String::new();
        let _ = stdout.read_to_string(&mut stdout_str).await;
        let mut stderr_str = String::new();
        let _ = stderr.read_to_string(&mut stderr_str).await;
        log::error!(
            "nix import failed with status={} stdout={:?} stderr={:?}",
            out,
            stdout_str,
            stderr_str
        );
        Err(super::Error::Exit(out))
    }
}
