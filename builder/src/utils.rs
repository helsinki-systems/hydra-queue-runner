#![allow(dead_code)]

use nix_utils::BaseStore as _;

static MAX_LOG_SIZE: u64 = 64u64 << 20;
static MAX_SILENT_TIME: i32 = 600; // 10min
static MAX_BUILD_TIMEOUT: i32 = 600; // 10min

#[allow(clippy::expect_used)]
static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"got:\s*([A-Za-z0-9-/+=]+)").expect("Failed to compile regex")
});

enum State {
    WaitingForHash,
    FindingRegex,
    Done,
}

#[derive(Debug)]
pub enum FodResult {
    Ok {
        actual_sri: String,
        output: String,
    },
    HashMismatch {
        expected_sri: String,
        actual_sri: String,
        output: String,
    },
    BuildFailure {
        expected_sri: String,
        output: String,
    },
}

async fn collect_log_output<S>(mut stream: S) -> anyhow::Result<String>
where
    S: tokio_stream::Stream<Item = std::io::Result<String>> + Unpin,
{
    use tokio_stream::StreamExt as _;

    let mut output = String::new();
    while let Some(line_result) = stream.next().await {
        let line = line_result?; // propagate line read error if any
        output.push_str(&line);
        output.push('\n');
    }
    Ok(output)
}

async fn detect_hash_mismatch<S>(mut stream: S) -> anyhow::Result<(String, Option<String>)>
where
    S: tokio_stream::Stream<Item = std::io::Result<String>> + Unpin,
{
    use tokio_stream::StreamExt as _;

    let mut state = State::WaitingForHash;
    let mut missmatch = None;
    let mut output = String::new();
    while let Some(line_result) = stream.next().await {
        let line = line_result?; // propagate line read error if any
        output.push_str(&line);
        output.push('\n');

        match state {
            State::Done => (),
            State::WaitingForHash => {
                if line.contains("hash mismatch") {
                    state = State::FindingRegex;
                }
            }
            State::FindingRegex => {
                if let Some(caps) = RE.captures(&line)
                    && let Some(m) = caps.get(1)
                {
                    missmatch = Some(m.as_str().to_string());
                    state = State::Done;
                }
            }
        }
    }
    Ok((output, missmatch))
}

#[tracing::instrument(skip(store, drv), fields(drv=%drv.name), err)]
pub async fn rebuild_fod(
    store: &nix_utils::LocalStore,
    drv: &nix_utils::Derivation,
) -> anyhow::Result<FodResult> {
    let Some(o) = drv.get_ca_output() else {
        return Err(anyhow::anyhow!("Not a FOD"));
    };
    let sri_hash = o.get_sri_hash()?;

    if !std::path::Path::new(&store.print_store_path(&o.path)).exists() {
        let (mut child, log_output) = nix_utils::realise_drv(
            store,
            &drv.name,
            &nix_utils::BuildOptions::complete(MAX_LOG_SIZE, MAX_SILENT_TIME, MAX_BUILD_TIMEOUT),
            true,
        )
        .await?;
        let status = child.wait().await?;
        if !status.success() {
            let (output, hash) = detect_hash_mismatch(log_output).await?;
            if let Some(hash) = hash {
                return Ok(FodResult::HashMismatch {
                    expected_sri: sri_hash,
                    actual_sri: hash,
                    output,
                });
            }
            return Ok(FodResult::BuildFailure {
                expected_sri: sri_hash,
                output: String::new(),
            });
        }
    }

    let (mut child, log_output) = nix_utils::realise_drv(
        store,
        &drv.name,
        &nix_utils::BuildOptions::complete(MAX_LOG_SIZE, MAX_SILENT_TIME, MAX_BUILD_TIMEOUT)
            .enable_check_build(),
        true,
    )
    .await?;
    let status = child.wait().await?;
    if !status.success() {
        let (output, hash) = detect_hash_mismatch(log_output).await?;
        if let Some(hash) = hash {
            return Ok(FodResult::HashMismatch {
                expected_sri: sri_hash,
                actual_sri: hash,
                output,
            });
        }
        return Ok(FodResult::BuildFailure {
            expected_sri: sri_hash,
            output,
        });
    }

    Ok(FodResult::Ok {
        actual_sri: sri_hash,
        output: collect_log_output(log_output).await?,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use futures::stream;

    #[tokio::test]
    async fn test_extract_got_after_hash_mismatch() {
        let lines = vec![
            "resolved derivation: '/nix/store/xzy.drv' -> '/nix/store/abc-source.drv'...",
            "building '/nix/store/abc-source.drv'...",
            "",
            "trying https://github.com/getsentry/vroom/archive/25.10.0.tar.gz",
            "  % Total    % Received % Xferd  Average Speed   Time    Time     Time  Current",
            "                                 Dload  Upload   Total   Spent    Left  Speed",
            "  0     0    0     0    0     0      0      0 --:--:-- --:--:-- --:--:--     0",
            "100  516k    0  516k    0     0   615k      0 --:--:-- --:--:-- --:--:-- 1560k",
            "unpacking source archive /build/download.tar.gz",
            "error: hash mismatch in fixed-output derivation '/nix/store/abc-source.drv':",
            "         specified: sha256-XmnsgfKxIDej8W1byFHDM9KzgRgq7xyNGxrKO3iz340=",
            "            got:    sha256-XOzS1E0jLzqIkdMTPQodtzlA16xE0Xm0EgUM7fhqIpc=",
            "error: 1 dependencies of derivation '/nix/store/zyx-vroom-25.10.0.drv' failed to build",
        ];
        let s = stream::iter(lines.into_iter().map(|l| Ok(l.to_string())));
        let (_, got) = detect_hash_mismatch(s).await.unwrap();
        assert_eq!(
            got,
            Some("sha256-XOzS1E0jLzqIkdMTPQodtzlA16xE0Xm0EgUM7fhqIpc=".to_string())
        );
    }

    #[tokio::test]
    async fn test_no_hash_mismatch_returns_none() {
        let lines = vec!["line one", "line two", "got: should_not_match"];
        let s = stream::iter(lines.into_iter().map(|l| Ok(l.to_string())));
        let (_, got) = detect_hash_mismatch(s).await.unwrap();
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn test_hash_mismatch_no_got_returns_none() {
        let lines = vec!["hash mismatch", "no match here"];
        let s = stream::iter(lines.into_iter().map(|l| Ok(l.to_string())));
        let (_, got) = detect_hash_mismatch(s).await.unwrap();
        assert_eq!(got, None);
    }
}
