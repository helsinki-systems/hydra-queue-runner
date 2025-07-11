use tokio::io::AsyncWriteExt as _;
use tokio_stream::StreamExt as _;

use crate::StorePath;

#[tracing::instrument(fields(%path), err)]
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

#[tracing::instrument(fields(paths), err)]
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
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    let mut stdin = child.stdin.take().ok_or(crate::Error::Stream)?;

    tokio::spawn(async move {
        while let Some(chunk) = input_stream.next().await {
            if let Err(e) = stdin.write_all(&chunk).await {
                log::error!("Error writing to stdin: {}", e);
                return Err(e);
            }
        }

        if let Err(e) = stdin.shutdown().await {
            log::error!("Error closing stdin: {}", e);
            return Err(e);
        }
        drop(stdin);

        Ok::<(), tokio::io::Error>(())
    })
    .await??;

    let out = child.wait().await?;
    if out.success() {
        Ok(())
    } else {
        Err(super::Error::Exit(out))
    }
}
