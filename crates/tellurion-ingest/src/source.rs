//! Resolves a `load` source argument (local path or http(s) URL) to a local
//! file. Remote sources are downloaded by shelling out to `curl` rather than
//! adding an HTTP client dependency.

use std::path::PathBuf;

pub struct ResolvedSource {
    pub path: PathBuf,
    is_temp: bool,
}

impl ResolvedSource {
    /// Removes the downloaded temp file, if any. Best-effort: a failed
    /// cleanup must not mask the load's own result.
    pub async fn cleanup(&self) {
        if self.is_temp {
            let _ = tokio::fs::remove_file(&self.path).await;
        }
    }
}

pub async fn resolve(source: &str) -> anyhow::Result<ResolvedSource> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let path = download(source).await?;
        Ok(ResolvedSource {
            path,
            is_temp: true,
        })
    } else {
        let path = PathBuf::from(source);
        if !path.exists() {
            anyhow::bail!("source path '{source}' does not exist");
        }
        Ok(ResolvedSource {
            path,
            is_temp: false,
        })
    }
}

async fn download(url: &str) -> anyhow::Result<PathBuf> {
    let filename = url
        .rsplit('/')
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or("download.dat");
    let dest = std::env::temp_dir().join(format!(
        "tellurion-ingest-{}-{filename}",
        std::process::id()
    ));

    tracing::info!(%url, path = %dest.display(), "downloading dataset");

    let status = tokio::process::Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&dest)
        .arg(url)
        .status()
        .await
        .map_err(|err| {
            anyhow::anyhow!("failed to invoke 'curl': {err}. Is curl installed and on PATH?")
        })?;

    if !status.success() {
        anyhow::bail!("curl failed to download '{url}' (exit status: {status})");
    }

    Ok(dest)
}
