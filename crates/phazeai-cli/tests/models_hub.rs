//! `phazeai models` against a fake Hugging Face (`HF_ENDPOINT`): search,
//! list a repository's files, pull one, see it selected, delete it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const SHA: &str = "89abcdef0123456789abcdef0123456789abcdef";

/// A GGUF header for a small llama-shaped model: 4 layers, 4 heads, 2 KV
/// heads, 256 wide, 8192 tokens of trained context.
fn gguf() -> Vec<u8> {
    fn s(t: &str) -> Vec<u8> {
        let mut v = (t.len() as u64).to_le_bytes().to_vec();
        v.extend(t.as_bytes());
        v
    }
    let kv: [(&str, u32, Vec<u8>); 7] = [
        ("general.architecture", 8, s("llama")),
        ("general.name", 8, s("Tiny Coder")),
        ("llama.block_count", 4, 4u32.to_le_bytes().to_vec()),
        ("llama.context_length", 4, 8192u32.to_le_bytes().to_vec()),
        ("llama.embedding_length", 4, 256u32.to_le_bytes().to_vec()),
        ("llama.attention.head_count", 4, 4u32.to_le_bytes().to_vec()),
        (
            "llama.attention.head_count_kv",
            4,
            2u32.to_le_bytes().to_vec(),
        ),
    ];
    let mut b = b"GGUF".to_vec();
    b.extend(3u32.to_le_bytes());
    b.extend(0u64.to_le_bytes());
    b.extend((kv.len() as u64).to_le_bytes());
    for (k, ty, v) in kv {
        b.extend(s(k));
        b.extend(ty.to_le_bytes());
        b.extend(v);
    }
    b.extend(vec![9u8; 5000]);
    b
}

fn sha256(dir: &Path, bytes: &[u8]) -> String {
    let p = dir.join("hash-me");
    std::fs::write(&p, bytes).unwrap();
    phazeai_core::local_ai::download::sha256_file(&p).unwrap()
}

/// A Hub with `o/tiny-GGUF`: a Q4_K_M file and a larger Q8_0 file.
fn serve(scratch: &Path) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let q4 = gguf();
    let mut q8 = gguf();
    q8.extend(vec![8u8; 4000]);
    let tree = serde_json::json!([
        {"type": "file", "path": "README.md", "size": 3},
        {"type": "file", "path": "tiny-Q4_K_M.gguf", "size": q4.len(),
         "lfs": {"oid": sha256(scratch, &q4), "size": q4.len()}},
        {"type": "file", "path": "tiny-Q8_0.gguf", "size": q8.len(),
         "lfs": {"oid": sha256(scratch, &q8), "size": q8.len()}},
    ])
    .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => req.extend_from_slice(&buf[..n]),
                }
            }
            let req = String::from_utf8_lossy(&req).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or("");
            let body: Option<Vec<u8>> = if path.starts_with("/api/models?") {
                Some(br#"[{"id":"o/tiny-GGUF","downloads":42,"likes":1}]"#.to_vec())
            } else if path == "/api/models/o/tiny-GGUF" {
                Some(
                    serde_json::json!({"sha": SHA, "cardData": {"license": "mit"},
                        "gguf": {"total": 50_000_000u64, "context_length": 8192}})
                    .to_string()
                    .into_bytes(),
                )
            } else if path == format!("/api/models/o/tiny-GGUF/tree/{SHA}?recursive=true") {
                Some(tree.clone().into_bytes())
            } else if path == format!("/o/tiny-GGUF/resolve/{SHA}/tiny-Q4_K_M.gguf") {
                Some(q4.clone())
            } else if path == format!("/o/tiny-GGUF/resolve/{SHA}/tiny-Q8_0.gguf") {
                Some(q8.clone())
            } else {
                None
            };
            let (status, body) = match body {
                Some(b) => ("200 OK", b),
                None => ("404 Not Found", b"{}".to_vec()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    base
}

fn phazeai(home: &Path, hub: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_phazeai"))
        .arg("models")
        .args(args)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_CACHE_HOME", home.join("cache"))
        .env("XDG_DATA_HOME", home.join("data"))
        .env("HF_ENDPOINT", hub)
        .env_remove("HF_TOKEN")
        .env_remove("PHAZEAI_TELEMETRY")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(out: Output) -> String {
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

fn selected(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join("config/phazeai/config.toml")).ok()?;
    let v: toml::Value = toml::from_str(&text).unwrap();
    v.get("local_ai")?
        .get("model")?
        .as_str()
        .map(str::to_string)
}

#[test]
fn search_pull_list_and_remove_a_hugging_face_model() {
    let home = tempfile::tempdir().unwrap();
    let hub = serve(home.path());

    let found = ok(phazeai(home.path(), &hub, &["search", "tiny", "coder"]));
    assert!(
        found.contains("o/tiny-GGUF") && found.contains("42"),
        "{found}"
    );

    let files = ok(phazeai(home.path(), &hub, &["files", "o/tiny-GGUF"]));
    assert!(files.contains("License: mit"), "{files}");
    assert!(
        files.contains("Q4_K_M") && files.contains("Q8_0"),
        "{files}"
    );
    // Both fit anywhere; the higher quality one is the pick.
    let pick = files.lines().find(|l| l.starts_with('*')).unwrap();
    assert!(pick.contains("tiny-Q8_0.gguf"), "{files}");

    let err = phazeai(
        home.path(),
        &hub,
        &["pull", "o/tiny-GGUF", "--quant", "Q3_K_M"],
    );
    assert!(!err.status.success());
    assert!(
        String::from_utf8_lossy(&err.stderr).contains("has no Q3_K_M file"),
        "{}",
        String::from_utf8_lossy(&err.stderr)
    );

    let pulled = ok(phazeai(
        home.path(),
        &hub,
        &["pull", "o/tiny-GGUF", "--quant", "q4_k_m"],
    ));
    assert!(
        pulled.contains("Tiny Coder (Q4_K_M) downloaded and verified"),
        "{pulled}"
    );
    assert!(pulled.contains("8192 tokens of context"), "{pulled}");
    let id = "hf:o/tiny-GGUF:tiny-Q4_K_M.gguf";
    assert_eq!(selected(home.path()).as_deref(), Some(id));
    let dir = home.path().join("data/phazeai/models/hub/o__tiny-GGUF");
    assert_eq!(std::fs::read(dir.join("tiny-Q4_K_M.gguf")).unwrap(), gguf());
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("tiny-Q4_K_M.gguf.phaze.json")).unwrap())
            .unwrap();
    // 4 layers × 2 KV heads × (64 + 64) × 2 bytes, read from the header.
    assert_eq!(manifest["kv_bytes_per_token"], 2048);
    assert_eq!(manifest["revision"], SHA);

    let list = ok(phazeai(home.path(), &hub, &["list"]));
    let line = list.lines().find(|l| l.contains(id)).unwrap();
    assert!(
        line.starts_with('*') && line.contains("Tiny Coder"),
        "{list}"
    );

    let err = phazeai(home.path(), &hub, &["use", "hf:o/tiny-GGUF:tiny-Q8_0.gguf"]);
    assert!(!err.status.success());

    let removed = ok(phazeai(home.path(), &hub, &["rm", id]));
    assert!(removed.contains("Deleted Tiny Coder (Q4_K_M)"), "{removed}");
    assert!(!dir.exists());
    assert_eq!(selected(home.path()), None);
    let list = ok(phazeai(home.path(), &hub, &["list"]));
    assert!(list.contains("No models downloaded"), "{list}");
}
