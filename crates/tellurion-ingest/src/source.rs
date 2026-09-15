//! Resolves a `load` source argument (local path or http(s) URL) to a local
//! file. Remote sources are downloaded by shelling out to `curl` rather than
//! adding an HTTP client dependency.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

pub struct ResolvedSource {
    pub path: PathBuf,
    temp: Option<tempfile::TempPath>,
}

impl ResolvedSource {
    /// Removes the downloaded temp file, if any. Best-effort: a failed
    /// cleanup must not mask the load's own result.
    pub async fn cleanup(&self) {
        if self.temp.is_some() {
            let _ = tokio::fs::remove_file(&self.path).await;
        }
    }
}

pub async fn resolve(source: &str) -> anyhow::Result<ResolvedSource> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let temp = download(source).await?;
        Ok(ResolvedSource {
            path: temp.to_path_buf(),
            temp: Some(temp),
        })
    } else {
        let path = PathBuf::from(source);
        if !path.exists() {
            anyhow::bail!("source path '{source}' does not exist");
        }
        Ok(ResolvedSource { path, temp: None })
    }
}

async fn download(url: &str) -> anyhow::Result<tempfile::TempPath> {
    download_with_limits(
        url,
        MAX_DOWNLOAD_BYTES,
        DOWNLOAD_TIMEOUT,
        &std::env::temp_dir(),
    )
    .await
}

async fn download_with_limits(
    url: &str,
    max_bytes: u64,
    deadline: Duration,
    temp_dir: &Path,
) -> anyhow::Result<tempfile::TempPath> {
    let expires = tokio::time::Instant::now() + deadline;
    // Keep only known vector-format suffixes for ogr2ogr's format detection.
    // The URL basename, query, and fragment never become local filenames.
    let url_path = url.split(['?', '#']).next().unwrap_or(url);
    let extension = Path::new(url_path)
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    let suffix = match extension.as_deref() {
        Some("csv") => ".csv",
        Some("tsv") => ".tsv",
        Some("geojson") => ".geojson",
        Some("json") => ".json",
        Some("gpkg") => ".gpkg",
        Some("fgb") => ".fgb",
        Some("gml") => ".gml",
        Some("kml") => ".kml",
        Some("zip") => ".zip",
        Some("parquet") => ".parquet",
        _ => "",
    };
    let (file, dest) = tempfile::Builder::new()
        .prefix("tellurion-ingest-")
        .suffix(suffix)
        .tempfile_in(temp_dir)?
        .into_parts();
    let mut output = tokio::fs::File::from_std(file);

    tracing::info!(path = %dest.display(), "downloading dataset");

    let mut child = tokio::process::Command::new("curl")
        // Ignore curlrc overrides and URL globbing: one URL, one bounded stream.
        .args([
            "-q",
            "-fsSL",
            "--globoff",
            "--proto",
            "=http,https",
            "--proto-redir",
            "=http,https",
            "--",
        ])
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| {
            anyhow::anyhow!("failed to invoke 'curl': {err}. Is curl installed and on PATH?")
        })?;

    let transfer = async {
        let mut input = child.stdout.take().expect("curl stdout is piped");
        let mut buffer = [0u8; 64 * 1024];
        let mut downloaded = 0u64;
        loop {
            let count = input.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            anyhow::ensure!(
                count as u64 <= max_bytes - downloaded,
                "download exceeds maximum size of {max_bytes} bytes"
            );
            output.write_all(&buffer[..count]).await?;
            downloaded += count as u64;
        }
        output.flush().await?;
        let status = child.wait().await?;
        anyhow::ensure!(
            status.success(),
            "curl download failed (exit status: {status})"
        );
        Ok::<(), anyhow::Error>(())
    };
    let result = tokio::time::timeout_at(expires, transfer)
        .await
        .unwrap_or_else(|_| {
            Err(anyhow::anyhow!(
                "download exceeded overall timeout of {deadline:?}"
            ))
        });
    if result.is_err() {
        // Reap failures promptly; kill_on_drop also covers future cancellation.
        let _ = child.kill().await;
    }
    result?;
    drop(output);
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve(response: &'static [u8]) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/data.geojson", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket.write_all(response).await.unwrap();
        });
        (url, task)
    }

    #[tokio::test]
    async fn dropping_remote_source_removes_download() {
        let (url, server) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").await;
        let resolved = resolve(&url).await.unwrap();
        let path = resolved.path.clone();
        assert_eq!(std::fs::read(&path).unwrap(), b"{}");
        drop(resolved);
        let exists = path.exists();
        // Clean the legacy implementation's leaked file even on the red run.
        let _ = std::fs::remove_file(path);
        server.await.unwrap();
        assert!(
            !exists,
            "download must be removed when its owner is dropped"
        );
    }

    #[tokio::test]
    async fn local_sources_are_never_removed() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let resolved = resolve(file.path().to_str().unwrap()).await.unwrap();
        resolved.cleanup().await;
        drop(resolved);
        assert!(file.path().exists());
    }

    #[tokio::test]
    async fn url_query_is_not_part_of_temporary_filename() {
        let dir = tempfile::tempdir().unwrap();
        let (url, server) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").await;
        let path = download_with_limits(
            &format!("{url}?token=private"),
            2,
            std::time::Duration::from_secs(2),
            dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(path.extension().unwrap(), "geojson");
        assert!(!path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("private"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn uppercase_format_suffix_is_preserved_without_url_basename() {
        let dir = tempfile::tempdir().unwrap();
        let (url, server) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").await;
        let path = download_with_limits(
            &url.replace("data.geojson", "data.CSV"),
            2,
            Duration::from_secs(2),
            dir.path(),
        )
        .await
        .unwrap();
        assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("csv"));
        assert!(!path.file_name().unwrap().to_str().unwrap().contains("data"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_unknown_length_body_over_limit_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let (url, server) = serve(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n123456789").await;
        let result =
            download_with_limits(&url, 8, std::time::Duration::from_secs(2), dir.path()).await;
        server.await.unwrap();
        assert!(
            result.is_err(),
            "unknown-length body must respect byte limit"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn partial_transfer_failure_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let (url, server) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial").await;
        let result =
            download_with_limits(&url, 100, std::time::Duration::from_secs(2), dir.path()).await;
        server.await.unwrap();
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn http_error_is_not_a_successful_source() {
        let dir = tempfile::tempdir().unwrap();
        let (url, server) = serve(b"HTTP/1.1 404 Not Found\r\nContent-Length: 4\r\n\r\noops").await;
        let result = download_with_limits(&url, 100, Duration::from_secs(2), dir.path()).await;
        server.await.unwrap();
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn cancellation_closes_transfer_and_removes_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/data", listener.local_addr().unwrap());
        let (started, ready) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\npartial")
                .await
                .unwrap();
            started.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), socket.read(&mut request)).await
        });
        let temp_dir = dir.path().to_path_buf();
        let download = tokio::spawn(async move {
            download_with_limits(&url, 100, std::time::Duration::from_secs(5), &temp_dir).await
        });
        ready.await.unwrap();
        download.abort();
        assert!(download.await.unwrap_err().is_cancelled());
        let disconnected = server.await.unwrap();
        assert!(
            matches!(disconnected, Ok(Ok(0)) | Ok(Err(_))),
            "curl must disconnect after cancellation"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn stalled_body_hits_overall_deadline_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/data", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\na")
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(600)).await;
            let _ = socket.write_all(b"b").await;
        });
        let result =
            download_with_limits(&url, 100, std::time::Duration::from_millis(100), dir.path())
                .await;
        server.await.unwrap();
        assert!(
            result.is_err(),
            "stalled body must time out before fixture completes"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn same_url_downloads_have_independent_private_files() {
        let dir = tempfile::tempdir().unwrap();
        let (url, server) = serve(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").await;
        let first = download_with_limits(&url, 2, std::time::Duration::from_secs(2), dir.path())
            .await
            .unwrap();
        server.await.unwrap();
        // Reuse the exact URL, including port, after the first listener closed.
        let address = url
            .strip_prefix("http://")
            .unwrap()
            .split('/')
            .next()
            .unwrap();
        let listener = tokio::net::TcpListener::bind(address).await.unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n[]")
                .await
                .unwrap();
        });
        let second = download_with_limits(&url, 2, std::time::Duration::from_secs(2), dir.path())
            .await
            .unwrap();
        assert_ne!(first.to_path_buf(), second.to_path_buf());
        assert_eq!(std::fs::read(&first).unwrap(), b"{}");
        assert_eq!(std::fs::read(&second).unwrap(), b"[]");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&first).unwrap().permissions().mode() & 0o077,
                0
            );
        }
        drop((first, second));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        server.await.unwrap();
    }
}
