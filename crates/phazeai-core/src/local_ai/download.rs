//! Resumable, SHA-256-verified downloads.
//!
//! Bytes stream into `<dest>.part`. An interrupted download resumes with an
//! HTTP `Range` request; the bytes already on disk are hashed first so the
//! digest still covers the whole file. Only when the size and SHA-256 match is
//! the `.part` file renamed into place (same directory, so the rename is
//! atomic) — a file at the final path is therefore always a verified one. A
//! hash mismatch deletes the partial file.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::catalog::{disk_margin, CatalogEntry};
use super::LocalAiError;

/// Reported while a download runs (and once more when it finishes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DownloadProgress {
    /// Bytes on disk so far, including any resumed prefix.
    pub downloaded: u64,
    pub total: u64,
    /// Smoothed transfer rate of this session, bytes/second.
    pub bytes_per_sec: f64,
}

impl DownloadProgress {
    pub fn fraction(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.downloaded as f64 / self.total as f64).clamp(0.0, 1.0)
        }
    }
}

/// Path of the in-progress file for `dest`.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// Final path of a catalog model inside `models_dir`.
pub fn model_path(entry: &CatalogEntry, models_dir: &Path) -> PathBuf {
    models_dir.join(entry.file)
}

/// A model counts as downloaded when its final file exists with the exact size.
/// (The file only reaches that path after its SHA-256 matched.)
pub fn is_downloaded(entry: &CatalogEntry, models_dir: &Path) -> bool {
    std::fs::metadata(model_path(entry, models_dir))
        .map(|m| m.len() == entry.size)
        .unwrap_or(false)
}

/// Bytes of an unfinished download of `entry`, if one exists.
pub fn partial_bytes(entry: &CatalogEntry, models_dir: &Path) -> Option<u64> {
    std::fs::metadata(part_path(&model_path(entry, models_dir)))
        .ok()
        .map(|m| m.len())
}

/// Remove a downloaded model and any partial download of it.
pub fn delete_model(entry: &CatalogEntry, models_dir: &Path) -> std::io::Result<()> {
    let dest = model_path(entry, models_dir);
    for p in [part_path(&dest), dest] {
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Download a catalog model into `models_dir`, verifying size and SHA-256.
pub async fn download_model(
    entry: &CatalogEntry,
    models_dir: &Path,
    cancel: Arc<AtomicBool>,
    on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf, LocalAiError> {
    let client = http_client()?;
    download_verified(
        &client,
        &entry.url(),
        &model_path(entry, models_dir),
        entry.size,
        entry.sha256,
        cancel,
        on_progress,
    )
    .await
}

/// HTTP client for large downloads. Proxies from `HTTPS_PROXY` / `HTTP_PROXY`
/// are honoured (reqwest's default).
pub fn http_client() -> Result<reqwest::Client, LocalAiError> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("PhazeAI/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Fail early when the volume can't hold the rest of a `total`-byte file
/// (`remaining` bytes still to fetch) plus a margin.
pub fn check_disk(remaining: u64, total: u64, free: Option<u64>) -> Result<(), LocalAiError> {
    let needed = remaining + disk_margin(total);
    match free {
        Some(available) if available < needed => {
            Err(LocalAiError::InsufficientDisk { needed, available })
        }
        _ => Ok(()),
    }
}

/// Stream `url` to `dest`, resuming `<dest>.part` if present, and only move it
/// into place when it is exactly `expected_size` bytes with SHA-256
/// `expected_sha256` (lower-case hex). Setting `cancel` stops the transfer and
/// keeps the partial file for a later resume.
pub async fn download_verified(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    expected_size: u64,
    expected_sha256: &str,
    cancel: Arc<AtomicBool>,
    mut on_progress: impl FnMut(DownloadProgress),
) -> Result<PathBuf, LocalAiError> {
    let expected_sha256 = expected_sha256.to_ascii_lowercase();
    let dir = dest
        .parent()
        .ok_or_else(|| LocalAiError::Other(format!("bad destination {}", dest.display())))?;
    tokio::fs::create_dir_all(dir).await?;
    let part = part_path(dest);

    // Already in place (e.g. a second click): re-verify rather than trust it.
    if tokio::fs::metadata(dest)
        .await
        .map(|m| m.len() == expected_size)
        .unwrap_or(false)
    {
        let digest = hash_file_prefix(dest.to_path_buf(), expected_size, cancel.clone()).await?;
        if hex(&digest.finalize()) == expected_sha256 {
            return Ok(dest.to_path_buf());
        }
        tokio::fs::remove_file(dest).await?;
    }

    let mut have = tokio::fs::metadata(&part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if have > expected_size {
        tokio::fs::remove_file(&part).await?;
        have = 0;
    }
    check_disk(
        expected_size - have,
        expected_size,
        super::hardware::free_disk_space(dir),
    )?;

    // Hash what is already on disk so the final digest covers the whole file.
    let mut hasher = if have > 0 {
        hash_file_prefix(part.clone(), have, cancel.clone()).await?
    } else {
        Sha256::new()
    };

    let mut req = client.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let resp = req.send().await?;
    let status = resp.status();
    let mut file = if have > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
        let start = resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(content_range_start);
        if start != Some(have) {
            return Err(LocalAiError::Other(format!(
                "server resumed at the wrong offset ({start:?} instead of {have})"
            )));
        }
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .await?
    } else if have > 0 && status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // Nothing left to fetch: the partial file is already complete.
        drop(resp);
        return finish(&part, dest, have, expected_size, hasher, &expected_sha256).await;
    } else if status.is_success() {
        // 200: the server sent the whole file (no resume support) — start over.
        have = 0;
        hasher = Sha256::new();
        tokio::fs::File::create(&part).await?
    } else {
        return Err(LocalAiError::Other(format!(
            "download failed: HTTP {status} from {url}"
        )));
    };

    let started = Instant::now();
    let mut last_report = Instant::now();
    let mut since_report: u64 = 0;
    let mut rate = 0.0_f64;
    let mut stream = resp.bytes_stream();
    on_progress(DownloadProgress {
        downloaded: have,
        total: expected_size,
        bytes_per_sec: 0.0,
    });
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::Relaxed) {
            file.flush().await?;
            return Err(LocalAiError::Cancelled);
        }
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(e) => {
                // A dropped connection: land what arrived so the retry resumes
                // from it (tokio's File finishes buffered writes in the background).
                file.flush().await?;
                return Err(e.into());
            }
        };
        if have + chunk.len() as u64 > expected_size {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            return Err(LocalAiError::SizeMismatch {
                expected: expected_size,
                actual: have + chunk.len() as u64,
            });
        }
        file.write_all(&chunk).await?;
        hasher.update(&chunk);
        have += chunk.len() as u64;
        since_report += chunk.len() as u64;
        let dt = last_report.elapsed();
        if dt >= Duration::from_millis(250) {
            let inst = since_report as f64 / dt.as_secs_f64();
            rate = if rate == 0.0 {
                inst
            } else {
                0.7 * rate + 0.3 * inst
            };
            on_progress(DownloadProgress {
                downloaded: have,
                total: expected_size,
                bytes_per_sec: rate,
            });
            last_report = Instant::now();
            since_report = 0;
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    if rate == 0.0 {
        let secs = started.elapsed().as_secs_f64().max(1e-3);
        rate = since_report as f64 / secs;
    }
    on_progress(DownloadProgress {
        downloaded: have,
        total: expected_size,
        bytes_per_sec: rate,
    });
    if have < expected_size {
        return Err(LocalAiError::Other(format!(
            "connection closed after {have} of {expected_size} bytes; try again to resume"
        )));
    }
    finish(&part, dest, have, expected_size, hasher, &expected_sha256).await
}

async fn finish(
    part: &Path,
    dest: &Path,
    have: u64,
    expected_size: u64,
    hasher: Sha256,
    expected_sha256: &str,
) -> Result<PathBuf, LocalAiError> {
    if have != expected_size {
        let _ = tokio::fs::remove_file(part).await;
        return Err(LocalAiError::SizeMismatch {
            expected: expected_size,
            actual: have,
        });
    }
    let actual = hex(&hasher.finalize());
    if actual != expected_sha256 {
        let _ = tokio::fs::remove_file(part).await;
        return Err(LocalAiError::HashMismatch {
            expected: expected_sha256.to_string(),
            actual,
        });
    }
    tokio::fs::rename(part, dest).await?;
    Ok(dest.to_path_buf())
}

/// SHA-256 state after the first `len` bytes of `path` (off the async threads).
async fn hash_file_prefix(
    path: PathBuf,
    len: u64,
    cancel: Arc<AtomicBool>,
) -> Result<Sha256, LocalAiError> {
    tokio::task::spawn_blocking(move || {
        let mut f = std::fs::File::open(&path)?.take(len);
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(LocalAiError::Cancelled);
            }
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        Ok(hasher)
    })
    .await
    .map_err(|e| LocalAiError::Other(format!("hashing task failed: {e}")))?
}

/// First byte offset of a `Content-Range: bytes <start>-<end>/<total>` header.
fn content_range_start(v: &str) -> Option<u64> {
    v.trim()
        .strip_prefix("bytes ")?
        .split('-')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Lower-case hex of a digest.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// SHA-256 of a whole file, lower-case hex (used for verifying installers too).
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut f, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// How the test server behaves on each request.
    #[derive(Clone, Copy)]
    enum Mode {
        /// Honour `Range`.
        Normal,
        /// Honour `Range`, but close the connection after this many body bytes
        /// on the *first* request only.
        CutFirstAt(usize),
        /// Ignore `Range` and always send the whole body with 200.
        NoRanges,
    }

    struct TestServer {
        url: String,
        /// The `Range` header (or None) of each request received.
        ranges: Arc<Mutex<Vec<Option<String>>>>,
    }

    /// A real HTTP/1.1 server on 127.0.0.1 serving `body`.
    async fn serve(body: Vec<u8>, mode: Mode) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = ranges.clone();
        let body = Arc::new(body);
        tokio::spawn(async move {
            let mut first = true;
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut head = Vec::new();
                let mut b = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if sock.read(&mut b).await.unwrap_or(0) == 0 {
                        break;
                    }
                    head.push(b[0]);
                }
                let head = String::from_utf8_lossy(&head).to_string();
                let range = head.lines().find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("range")
                        .then(|| v.trim().to_string())
                });
                seen.lock().unwrap().push(range.clone());
                let total = body.len();
                let start = match (mode, &range) {
                    (Mode::NoRanges, _) | (_, None) => None,
                    (_, Some(r)) => r
                        .strip_prefix("bytes=")
                        .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok()),
                };
                let (status, slice) = match start {
                    Some(s) if s >= total => {
                        let resp = format!(
                            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                        continue;
                    }
                    Some(s) => (
                        format!(
                            "206 Partial Content\r\nContent-Range: bytes {s}-{}/{total}",
                            total - 1
                        ),
                        &body[s..],
                    ),
                    None => ("200 OK".to_string(), &body[..]),
                };
                let resp = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                    slice.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let send = match mode {
                    Mode::CutFirstAt(n) if first => &slice[..n.min(slice.len())],
                    _ => slice,
                };
                first = false;
                for c in send.chunks(64 * 1024) {
                    if sock.write_all(c).await.is_err() {
                        break;
                    }
                }
                let _ = sock.shutdown().await;
            }
        });
        TestServer {
            url: format!("http://{addr}/model.gguf"),
            ranges,
        }
    }

    fn body(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn sha(b: &[u8]) -> String {
        hex(&Sha256::digest(b))
    }

    fn no_cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[tokio::test]
    async fn full_download_is_verified_and_renamed() {
        let data = body(1_000_000);
        let srv = serve(data.clone(), Mode::Normal).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        let mut reports = Vec::new();
        let out = download_verified(
            &http_client().unwrap(),
            &srv.url,
            &dest,
            data.len() as u64,
            &sha(&data),
            no_cancel(),
            |p| reports.push(p),
        )
        .await
        .unwrap();
        assert_eq!(out, dest);
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(!part_path(&dest).exists());
        let last = reports.last().unwrap();
        assert_eq!(last.downloaded, data.len() as u64);
        assert_eq!(last.fraction(), 1.0);
        assert_eq!(srv.ranges.lock().unwrap().as_slice(), &[None]);
    }

    #[tokio::test]
    async fn interrupted_download_resumes_with_range() {
        let data = body(800_000);
        let srv = serve(data.clone(), Mode::CutFirstAt(300_000)).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        let client = http_client().unwrap();
        let first = download_verified(
            &client,
            &srv.url,
            &dest,
            data.len() as u64,
            &sha(&data),
            no_cancel(),
            |_| {},
        )
        .await;
        assert!(first.is_err(), "cut connection must not succeed");
        assert_eq!(std::fs::metadata(part_path(&dest)).unwrap().len(), 300_000);
        assert!(!dest.exists());

        download_verified(
            &client,
            &srv.url,
            &dest,
            data.len() as u64,
            &sha(&data),
            no_cancel(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let ranges = srv.ranges.lock().unwrap().clone();
        assert_eq!(ranges, vec![None, Some("bytes=300000-".to_string())]);
    }

    #[tokio::test]
    async fn hash_mismatch_deletes_partial_and_errors() {
        let data = body(200_000);
        let srv = serve(data.clone(), Mode::Normal).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        let wrong = sha(b"something else");
        let err = download_verified(
            &http_client().unwrap(),
            &srv.url,
            &dest,
            data.len() as u64,
            &wrong,
            no_cancel(),
            |_| {},
        )
        .await
        .unwrap_err();
        match err {
            LocalAiError::HashMismatch { expected, actual } => {
                assert_eq!(expected, wrong);
                assert_eq!(actual, sha(&data));
            }
            other => panic!("expected HashMismatch, got {other:?}"),
        }
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
    }

    #[tokio::test]
    async fn corrupt_resumed_prefix_is_caught() {
        // A partial file whose bytes differ from the server's: the resumed tail is
        // fine, but the whole-file hash must still reject it.
        let data = body(400_000);
        let srv = serve(data.clone(), Mode::Normal).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        std::fs::write(part_path(&dest), vec![0u8; 100_000]).unwrap();
        let err = download_verified(
            &http_client().unwrap(),
            &srv.url,
            &dest,
            data.len() as u64,
            &sha(&data),
            no_cancel(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LocalAiError::HashMismatch { .. }), "{err:?}");
        assert!(!part_path(&dest).exists() && !dest.exists());
        assert_eq!(
            srv.ranges.lock().unwrap().as_slice(),
            &[Some("bytes=100000-".to_string())]
        );
    }

    #[tokio::test]
    async fn server_without_range_support_restarts_cleanly() {
        let data = body(300_000);
        let srv = serve(data.clone(), Mode::NoRanges).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        std::fs::write(part_path(&dest), &data[..1000]).unwrap();
        download_verified(
            &http_client().unwrap(),
            &srv.url,
            &dest,
            data.len() as u64,
            &sha(&data),
            no_cancel(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[tokio::test]
    async fn complete_partial_gets_416_and_is_finished() {
        let data = body(50_000);
        let srv = serve(data.clone(), Mode::Normal).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        std::fs::write(part_path(&dest), &data).unwrap();
        download_verified(
            &http_client().unwrap(),
            &srv.url,
            &dest,
            data.len() as u64,
            &sha(&data),
            no_cancel(),
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[tokio::test]
    async fn oversized_body_is_rejected() {
        let data = body(10_000);
        let srv = serve(data.clone(), Mode::Normal).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        let err = download_verified(
            &http_client().unwrap(),
            &srv.url,
            &dest,
            5_000,
            &sha(&data[..5000]),
            no_cancel(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LocalAiError::SizeMismatch { .. }), "{err:?}");
        assert!(!part_path(&dest).exists());
    }

    #[tokio::test]
    async fn cancel_keeps_partial_for_resume() {
        let data = body(4_000_000);
        let srv = serve(data.clone(), Mode::Normal).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let err = download_verified(
            &http_client().unwrap(),
            &srv.url,
            &dest,
            data.len() as u64,
            &sha(&data),
            cancel,
            move |p| {
                if p.downloaded > 0 {
                    c2.store(true, Ordering::Relaxed);
                }
            },
        )
        .await;
        // Either cancelled mid-stream, or the whole body arrived before the first
        // progress tick; both are valid outcomes for a small local transfer.
        match err {
            Err(LocalAiError::Cancelled) => {
                assert!(part_path(&dest).exists());
                assert!(!dest.exists());
            }
            Ok(_) => assert_eq!(std::fs::read(&dest).unwrap(), data),
            Err(e) => panic!("unexpected {e:?}"),
        }
    }

    #[tokio::test]
    async fn pre_cancelled_download_stops_before_writing_everything() {
        let data = body(2_000_000);
        let srv = serve(data.clone(), Mode::Normal).await;
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        let err = download_verified(
            &http_client().unwrap(),
            &srv.url,
            &dest,
            data.len() as u64,
            &sha(&data),
            Arc::new(AtomicBool::new(true)),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, LocalAiError::Cancelled));
        assert!(!dest.exists());
    }

    #[test]
    fn disk_check() {
        let gb = 1_000_000_000;
        assert!(check_disk(100, 100, None).is_ok());
        assert!(check_disk(100, 100, Some(100 + 1024 * 1024)).is_ok());
        assert!(matches!(
            check_disk(100, 100, Some(100)),
            Err(LocalAiError::InsufficientDisk { .. })
        ));
        // A resumed 4 GB download only needs the rest plus the margin.
        assert!(check_disk(gb, 4 * gb, Some(gb + 400_000_000)).is_ok());
        assert!(check_disk(gb, 4 * gb, Some(gb + 300_000_000)).is_err());
    }

    #[test]
    fn content_range_parsing() {
        assert_eq!(content_range_start("bytes 300-999/1000"), Some(300));
        assert_eq!(content_range_start("bytes */1000"), None);
        assert_eq!(content_range_start("items 1-2/3"), None);
    }

    #[test]
    fn delete_removes_final_and_partial() {
        let dir = tempfile::tempdir().unwrap();
        let e = &super::super::catalog::CATALOG[0];
        let dest = model_path(e, dir.path());
        std::fs::write(&dest, b"x").unwrap();
        std::fs::write(part_path(&dest), b"y").unwrap();
        assert_eq!(partial_bytes(e, dir.path()), Some(1));
        assert!(
            !is_downloaded(e, dir.path()),
            "wrong size is not downloaded"
        );
        delete_model(e, dir.path()).unwrap();
        assert!(!dest.exists() && !part_path(&dest).exists());
        delete_model(e, dir.path()).unwrap();
    }
}
