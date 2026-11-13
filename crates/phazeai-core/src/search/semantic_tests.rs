//! Matching by meaning, end to end: a real HTTP endpoint on a local socket
//! that speaks the Ollama and OpenAI embedding formats, the engine, the disk
//! cache and the agent tool.
//!
//! The endpoint's "model" maps words to a handful of concepts (retrying,
//! networking, colour, databases), so two texts that share a concept but no
//! word are close, which is the property a real embedding model has and the
//! one the engine relies on.

use super::semantic::{load_cache, Semantic};
use super::*;
use crate::privacy;
use crate::tools::Tool;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const CONCEPTS: &[&[&str]] = &[
    &[
        "retry", "retries", "retried", "backoff", "attempt", "attempts", "again",
    ],
    &[
        "http", "request", "requests", "network", "socket", "failed", "send",
    ],
    &["color", "colour", "paint", "rgb", "hue", "shade"],
    &["sql", "database", "table", "rows", "query", "record"],
];

/// The fake model: one dimension per concept plus a constant one.
fn vector_of(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; CONCEPTS.len() + 1];
    v[CONCEPTS.len()] = 0.05;
    for word in text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .flat_map(|w| w.split('_'))
        .filter(|w| !w.is_empty())
    {
        let w = word.to_ascii_lowercase();
        for (i, group) in CONCEPTS.iter().enumerate() {
            if group.contains(&w.as_str()) {
                v[i] += 1.0;
            }
        }
    }
    v
}

#[derive(Default)]
struct Served {
    /// Requests answered.
    requests: AtomicUsize,
    /// Texts embedded, over all requests.
    texts: AtomicUsize,
    /// Every input text seen, in order.
    inputs: Mutex<Vec<String>>,
}

/// Serve embeddings on 127.0.0.1 until the test ends. Requests to
/// `/api/embed` get Ollama's answer shape, `/v1/embeddings` OpenAI's (in
/// reverse order on the wire, with `index`), anything else a 404.
async fn serve() -> (String, Arc<Served>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let served = Arc::new(Served::default());
    let s = Arc::clone(&served);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let s = Arc::clone(&s);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let (head_end, len) = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_ascii_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (i + 4, len);
                    }
                };
                while buf.len() < head_end + len {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                let body: Value =
                    serde_json::from_slice(&buf[head_end..head_end + len]).unwrap_or(Value::Null);
                let inputs: Vec<String> = body["input"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let (status, answer) = match path.as_str() {
                    "/api/embed" => (
                        200,
                        json!({"embeddings": inputs.iter().map(|t| vector_of(t)).collect::<Vec<_>>()}),
                    ),
                    "/v1/embeddings" => (
                        200,
                        json!({"data": inputs.iter().enumerate().rev()
                            .map(|(i, t)| json!({"index": i, "embedding": vector_of(t)}))
                            .collect::<Vec<_>>()}),
                    ),
                    _ => (404, json!({"error": "not found"})),
                };
                if status == 200 {
                    s.requests.fetch_add(1, Ordering::SeqCst);
                    s.texts.fetch_add(inputs.len(), Ordering::SeqCst);
                    s.inputs.lock().unwrap().extend(inputs);
                }
                let text = answer.to_string();
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), served)
}

/// Serialises tests that set the process-wide privacy project, and starts
/// each with none.
struct Active {
    _guard: MutexGuard<'static, ()>,
}

fn activate(root: Option<&Path>) -> Active {
    let guard = crate::tools::sandbox::tests::SERIAL
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match root {
        Some(r) => privacy::set_active_project(r),
        None => privacy::clear_active_project(),
    }
    Active { _guard: guard }
}

impl Drop for Active {
    fn drop(&mut self) {
        privacy::clear_active_project();
    }
}

fn write(root: &Path, rel: &str, text: &str) -> PathBuf {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, text).unwrap();
    p
}

fn repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        "src/net/client.rs",
        "pub fn send_with_backoff(req: &Request) -> Response {\n    \
         for attempt in 0..5 {\n        if let Ok(r) = req.send() { return r; }\n        \
         sleep(delay(attempt));\n    }\n    panic!(\"gave up\")\n}\n",
    );
    write(
        &root,
        "src/theme.rs",
        "pub fn paint(rgb: Rgb) -> Shade {\n    Shade::from_hue(rgb.hue())\n}\n",
    );
    write(
        &root,
        "src/store.rs",
        "pub fn load_rows(table: &str) -> Vec<Row> {\n    sql(\"select * from \", table)\n}\n",
    );
    (dir, root)
}

fn rel(root: &Path, hit: &SearchHit) -> String {
    Path::new(&hit.file)
        .strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn with_model(cs: &CodeSearch, url: &str, cache: &Path) {
    let cfg = EmbeddingConfig::new("concepts", Some(url), None).unwrap();
    cs.set_semantic(Some(Semantic::with_cache_dir(
        cfg,
        Some(cache.to_path_buf()),
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_query_that_shares_no_word_with_the_code_finds_it_by_meaning() {
    let _p = activate(None);
    let (_d, root) = repo();
    let cache = tempfile::tempdir().unwrap();
    let (url, served) = serve().await;

    // Keywords alone: nothing in the code says "retry" or "policy".
    let cs = CodeSearch::new(&root);
    let plain = cs.search("retry policy", 3).await.unwrap();
    assert!(plain.matches.is_empty(), "{:?}", plain.matches);

    with_model(&cs, &url, cache.path());
    cs.sync_embeddings().await.unwrap();
    let status = cs.embedding_status().unwrap();
    assert_eq!((status.ready, status.total), (3, 3));
    assert!(status.error.is_none() && status.note().is_none());
    // Every chunk was sent with its path, once, in Ollama's format.
    assert_eq!(served.texts.load(Ordering::SeqCst), 3);
    assert!(served
        .inputs
        .lock()
        .unwrap()
        .iter()
        .any(|t| t.starts_with("src/net/client.rs\n")));

    let res = cs.search("retry policy", 3).await.unwrap();
    assert_eq!(rel(&root, &res.matches[0]), "src/net/client.rs");
    assert_eq!(res.matches[0].start_line, 1);
    assert!(res.matches[0].snippet.contains("send_with_backoff"));

    // An exact identifier still comes first.
    let res = cs.search("paint", 3).await.unwrap();
    assert_eq!(rel(&root, &res.matches[0]), "src/theme.rs");

    // The blocking search, from a thread outside any runtime (the IDE's
    // Search panel), matches by meaning too.
    let cs2 = cs.clone();
    let panel = std::thread::spawn(move || cs2.search_blocking("which database records", 3));
    let res = tokio::task::spawn_blocking(move || panel.join())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(rel(&root, &res.matches[0]), "src/store.rs");
}

#[tokio::test]
async fn openai_compatible_endpoints_work_the_same() {
    let _p = activate(None);
    let (_d, root) = repo();
    let cache = tempfile::tempdir().unwrap();
    let (url, served) = serve().await;
    let cs = CodeSearch::new(&root);
    with_model(&cs, &format!("{url}/v1"), cache.path());
    cs.sync_embeddings().await.unwrap();
    assert_eq!(served.texts.load(Ordering::SeqCst), 3);
    let res = cs.search("try it again after a failure", 2).await.unwrap();
    assert_eq!(rel(&root, &res.matches[0]), "src/net/client.rs");
}

#[tokio::test]
async fn vectors_are_kept_on_disk_and_only_changed_chunks_are_sent_again() {
    let _p = activate(None);
    let (_d, root) = repo();
    let cache = tempfile::tempdir().unwrap();
    let (url, served) = serve().await;

    let cs = CodeSearch::new(&root);
    with_model(&cs, &url, cache.path());
    cs.sync_embeddings().await.unwrap();
    assert_eq!(served.texts.load(Ordering::SeqCst), 3);
    let files: Vec<_> = std::fs::read_dir(cache.path()).unwrap().collect();
    assert_eq!(files.len(), 1, "one cache file per project and model");
    let (dim, saved) = load_cache(&files[0].as_ref().unwrap().path()).unwrap();
    assert_eq!((dim, saved.len()), (CONCEPTS.len() + 1, 3));

    // A new session (the IDE restarted): everything comes from the disk.
    let again = CodeSearch::new(&root);
    with_model(&again, &url, cache.path());
    again.sync_embeddings().await.unwrap();
    assert_eq!(served.texts.load(Ordering::SeqCst), 3, "nothing re-sent");
    assert_eq!(again.embedding_status().unwrap().ready, 3);

    // One file changes: only its chunk goes out.
    let p = write(
        &root,
        "src/theme.rs",
        "pub fn paint(rgb: Rgb) -> Shade {\n    Shade::from_hue(rgb.hue()).darker()\n}\n",
    );
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    again.build(&[]).await.unwrap();
    again.sync_embeddings().await.unwrap();
    assert_eq!(served.texts.load(Ordering::SeqCst), 4);

    // A deleted file leaves the meaning ranking too.
    std::fs::remove_file(root.join("src/store.rs")).unwrap();
    again.build(&[]).await.unwrap();
    again.sync_embeddings().await.unwrap();
    assert_eq!(again.embedding_status().unwrap().ready, 2);
    let res = again.search("database rows", 3).await.unwrap();
    assert!(res.matches.iter().all(|h| rel(&root, h) != "src/store.rs"));
}

#[tokio::test]
async fn the_same_model_set_twice_keeps_its_vectors() {
    let _p = activate(None);
    let (_d, root) = repo();
    let cache = tempfile::tempdir().unwrap();
    let (url, served) = serve().await;
    let cs = CodeSearch::new(&root);
    with_model(&cs, &url, cache.path());
    cs.sync_embeddings().await.unwrap();
    with_model(&cs, &url, cache.path());
    assert_eq!(cs.embedding_status().unwrap().ready, 3);
    cs.set_embeddings(None);
    assert!(cs.embedding_status().is_none());
    assert_eq!(served.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_unreachable_endpoint_leaves_keyword_search_working_and_says_why() {
    let _p = activate(None);
    let (_d, root) = repo();
    let cache = tempfile::tempdir().unwrap();
    // A port nothing listens on.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", free.local_addr().unwrap());
    drop(free);

    let cs = CodeSearch::new(&root);
    with_model(&cs, &url, cache.path());
    assert!(cs.sync_embeddings().await.is_err());
    let status = cs.embedding_status().unwrap();
    assert_eq!(status.ready, 0);
    assert!(status.note().unwrap().contains("keywords only"));

    let res = cs.search("paint", 3).await.unwrap();
    assert_eq!(rel(&root, &res.matches[0]), "src/theme.rs");

    // The agent is told, in one line, why matching is keyword-only.
    let tool = CodeSearchTool::new(cs.clone());
    assert!(tool.description().contains("by meaning"));
    let out = tool.execute(json!({"query": "paint"})).await.unwrap();
    assert!(out["note"].as_str().unwrap().contains("unavailable"));
}

#[tokio::test]
async fn a_local_only_project_sends_no_code_to_an_endpoint_elsewhere() {
    let (_d, root) = repo();
    std::fs::create_dir_all(root.join(".phazeai")).unwrap();
    std::fs::write(root.join(privacy::POLICY_FILE), "local_only = true\n").unwrap();
    let _p = activate(Some(&root));
    let cache = tempfile::tempdir().unwrap();

    let cs = CodeSearch::new(&root);
    let cfg =
        EmbeddingConfig::new("concepts", Some("https://embeddings.example.com/v1"), None).unwrap();
    assert!(cfg.refused().is_some());
    cs.set_semantic(Some(Semantic::with_cache_dir(
        cfg,
        Some(cache.path().to_path_buf()),
    )));
    let err = cs.sync_embeddings().await.unwrap_err().to_string();
    assert!(err.contains("local_only"), "{err}");
    assert!(std::fs::read_dir(cache.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn files_the_privacy_policy_denies_are_never_embedded() {
    let (_d, root) = repo();
    write(
        &root,
        "secrets/keys.rs",
        "pub fn retry_token() -> &'static str { \"x\" }\n",
    );
    std::fs::create_dir_all(root.join(".phazeai")).unwrap();
    std::fs::write(root.join(privacy::POLICY_FILE), "deny = [\"secrets/**\"]\n").unwrap();
    let _p = activate(Some(&root));
    let cache = tempfile::tempdir().unwrap();
    let (url, served) = serve().await;

    let cs = CodeSearch::new(&root);
    with_model(&cs, &url, cache.path());
    cs.sync_embeddings().await.unwrap();
    assert!(served
        .inputs
        .lock()
        .unwrap()
        .iter()
        .all(|t| !t.contains("secrets/")));
    let res = cs.search("retry", 5).await.unwrap();
    assert!(res.matches.iter().all(|h| !h.file.contains("secrets")));
}

#[tokio::test]
async fn a_new_workspace_starts_from_its_own_vectors() {
    let _p = activate(None);
    let (_d, root) = repo();
    let other = tempfile::tempdir().unwrap();
    let other_root = other.path().canonicalize().unwrap();
    write(&other_root, "lib.rs", "fn mix_colour(hue: f32) {}\n");
    let cache = tempfile::tempdir().unwrap();
    let (url, _served) = serve().await;

    let cs = CodeSearch::new(&root);
    with_model(&cs, &url, cache.path());
    cs.sync_embeddings().await.unwrap();
    cs.reset_root(&other_root);
    assert_eq!(cs.embedding_status().unwrap().ready, 0);
    cs.sync_embeddings().await.unwrap();
    assert_eq!(cs.embedding_status().unwrap().ready, 1);
    let res = cs.search("paint shade", 3).await.unwrap();
    assert_eq!(res.matches.len(), 1);
    assert!(res.matches[0].file.ends_with("lib.rs"));
}
