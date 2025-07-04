use tokio::io::AsyncWriteExt as _;
use tokio_stream::StreamExt as _;

use crate::StorePath;

#[tracing::instrument(skip(path), err)]
pub async fn export_nar(
    path: &StorePath,
) -> Result<
    (
        tokio::process::Child,
        tokio_util::io::ReaderStream<tokio::process::ChildStdout>,
    ),
    crate::Error,
> {
    let mut child = tokio::process::Command::new("nix-store")
        .args(["--export", &path.get_full_path()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    let stdout = child.stdout.take().ok_or(crate::Error::Stream)?;
    Ok((child, tokio_util::io::ReaderStream::new(stdout)))
}

#[tracing::instrument(skip(input_stream), err)]
pub async fn import_nar<S>(mut input_stream: S) -> Result<(), crate::Error>
where
    S: tokio_stream::Stream<Item = tokio_util::bytes::Bytes> + Send + Unpin + 'static,
{
    let mut child = tokio::process::Command::new("nix-store")
        .arg("--import")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().ok_or(crate::Error::Stream)?;
    let stdout = child.stdout.take().ok_or(crate::Error::Stream)?;
    let stderr = child.stderr.take().ok_or(crate::Error::Stream)?;

    // Spawn a task to write to stdin
    tokio::spawn(async move {
        while let Some(chunk) = input_stream.next().await {
            if let Err(e) = stdin.write_all(&chunk).await {
                log::error!("Error writing to child stdin: {}", e);
                break;
            }
        }

        // Close stdin to signal EOF
        if let Err(e) = stdin.shutdown().await {
            log::error!("Failed to shutdown stdin: {}", e);
        }
    });

    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt as _, BufReader};
        use tokio_stream::StreamExt;
        use tokio_stream::wrappers::LinesStream;

        let stdout = LinesStream::new(BufReader::new(stdout).lines());
        let stderr = LinesStream::new(BufReader::new(stderr).lines());

        let mut stream = StreamExt::merge(stdout, stderr);

        while let Some(Ok(line)) = stream.next().await {
            log::debug!("[nix-store --import] {}", line);
        }
    });

    crate::validate_statuscode(child.wait().await?)?;
    Ok(())
}
