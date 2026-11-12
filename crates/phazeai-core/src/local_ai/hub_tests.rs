use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use super::*;
use crate::local_ai::catalog::Fit;
use crate::local_ai::gguf::tests::qwen7b;
use crate::local_ai::server::selected_model;
use crate::local_ai::LocalAiSettings;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn hw(total_gib: u64, free_gib: u64) -> HardwareInfo {
    HardwareInfo {
        total_ram: total_gib * GIB,
        available_ram: free_gib * GIB,
        physical_cores: Some(8),
        logical_cores: 16,
        free_disk: Some(500 * GIB),
        os: "linux".into(),
        arch: "x86_64".into(),
    }
}

fn file(path: &str, size: u64) -> HubFile {
    HubFile {
        path: path.into(),
        quant: quant_of(path),
        shards: vec![HubShard {
            path: path.into(),
            size,
            sha256: "0".repeat(64),
        }],
    }
}

fn sha(bytes: &[u8]) -> String {
    crate::local_ai::download::hex(&Sha256::digest(bytes))
}

#[test]
fn quantization_comes_from_the_file_name() {
    assert_eq!(
        quant_of("Qwen2.5-7B-Instruct-Q4_K_M.gguf").as_deref(),
        Some("Q4_K_M")
    );
    assert_eq!(quant_of("sub/model.IQ4_XS.gguf").as_deref(), Some("IQ4_XS"));
    assert_eq!(
        quant_of("big-Q8_0-00001-of-00002.gguf").as_deref(),
        Some("Q8_0")
    );
    assert_eq!(
        quant_of("phi-3-mini-4k-instruct-f16.gguf").as_deref(),
        Some("F16")
    );
    assert_eq!(quant_of("model-q5_k_s.gguf").as_deref(), Some("Q5_K_S"));
    assert_eq!(quant_of("Q4_K_Mixer.gguf"), None);
    assert_eq!(quant_of("model.gguf"), None);
}

#[test]
fn files_are_grouped_into_quantizations() {
    let entry = |path: &str, size: u64, lfs: bool| TreeEntry {
        kind: "file".into(),
        path: path.into(),
        size,
        lfs: lfs.then(|| Lfs {
            oid: format!("{size:064x}"),
            size,
        }),
    };
    let files = group_files(vec![
        TreeEntry {
            kind: "directory".into(),
            path: "Q8_0".into(),
            size: 0,
            lfs: None,
        },
        entry("README.md", 10, false),
        entry("m-Q4_K_M.gguf", 400, true),
        entry("mmproj-m-f16.gguf", 50, true),
        entry("Q8_0/m-Q8_0-00002-of-00002.gguf", 300, true),
        entry("Q8_0/m-Q8_0-00001-of-00002.gguf", 500, true),
        // Half of a split model is not a model.
        entry("F16/m-F16-00001-of-00002.gguf", 900, true),
        // Not stored in LFS: no hash to verify against.
        entry("tiny-Q2_K.gguf", 5, false),
    ]);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "m-Q4_K_M.gguf");
    assert_eq!(files[0].quant.as_deref(), Some("Q4_K_M"));
    assert_eq!(files[1].path, "Q8_0/m-Q8_0-00001-of-00002.gguf");
    assert_eq!(files[1].quant.as_deref(), Some("Q8_0"));
    assert_eq!(files[1].size(), 800);
    assert_eq!(
        files[1]
            .shards
            .iter()
            .map(|s| s.path.as_str())
            .collect::<Vec<_>>(),
        [
            "Q8_0/m-Q8_0-00001-of-00002.gguf",
            "Q8_0/m-Q8_0-00002-of-00002.gguf"
        ]
    );
}

#[test]
fn names_and_ids_are_checked() {
    assert!(valid_repo("bartowski/Qwen2.5-Coder-7B-Instruct-GGUF"));
    assert!(!valid_repo("bartowski"));
    assert!(!valid_repo("a/b/c"));
    assert!(!valid_repo("../etc"));
    assert!(!valid_repo("a/.."));
    assert!(!valid_repo("a/b c"));
    let id = model_id("o/m", "Q8_0/m-Q8_0-00001-of-00002.gguf");
    assert_eq!(
        parse_id(&id),
        Some(("o/m", "Q8_0/m-Q8_0-00001-of-00002.gguf"))
    );
    assert_eq!(parse_id("qwen2.5-coder-7b"), None);
    assert_eq!(parse_id("hf:o/m:../../x.gguf"), None);
    assert_eq!(parse_id("hf:o/m:a\\..\\x.gguf"), None);
    assert_eq!(
        rel_path("Q8_0/m.gguf"),
        Some(PathBuf::from("Q8_0").join("m.gguf"))
    );
    for bad in [
        "",
        "/m.gguf",
        "a//m.gguf",
        "./m.gguf",
        "../m.gguf",
        "C:m.gguf",
        "a\\m.gguf",
    ] {
        assert_eq!(rel_path(bad), None, "{bad}");
    }
    assert_eq!(
        resolve_url("http://h:1/mirror/", "o/m", "abc", "Q8 0/m#1.gguf")
            .unwrap()
            .as_str(),
        "http://h:1/mirror/o/m/resolve/abc/Q8%200/m%231.gguf"
    );
    assert_eq!(
        next_link("<https://huggingface.co/api/models/o/m/tree/x?cursor=abc>; rel=\"next\""),
        Some("https://huggingface.co/api/models/o/m/tree/x?cursor=abc".into())
    );
    assert_eq!(next_link("<https://x>; rel=\"prev\""), None);
}

/// The quantizations a typical 7B repository offers.
fn seven_b() -> HubListing {
    HubListing {
        repo: "o/m".into(),
        revision: SHA.into(),
        files: vec![
            file("m-Q2_K.gguf", 3_020_000_000),
            file("m-Q4_K_M.gguf", 4_680_000_000),
            file("m-Q5_K_M.gguf", 5_440_000_000),
            file("m-Q8_0.gguf", 8_100_000_000),
            file("m-f16.gguf", 15_200_000_000),
        ],
        params: Some(7_600_000_000),
        context_length: Some(32_768),
        license: Some("apache-2.0".into()),
    }
}

#[test]
fn the_best_quantization_that_fits_is_chosen() {
    let l = seven_b();
    // Plenty of RAM: Q8_0, not F16 (twice the size for almost nothing).
    assert_eq!(choose(&l, &hw(64, 48)).unwrap().path, "m-Q8_0.gguf");
    assert_eq!(choose(&l, &hw(16, 12)).unwrap().path, "m-Q8_0.gguf");
    // 12 GiB installed: Q8_0 would leave the OS and IDE too little.
    assert_eq!(choose(&l, &hw(12, 9)).unwrap().path, "m-Q5_K_M.gguf");
    // 8 GiB: only the 2-bit file runs, and only tightly.
    let small = choose(&l, &hw(8, 6)).unwrap();
    assert_eq!(small.path, "m-Q2_K.gguf");
    assert_eq!(fit_file(&l, small, &hw(8, 6), 16_384).fit, Fit::Tight);
    // 4 GiB: nothing.
    assert!(choose(&l, &hw(4, 3)).is_none());
    // A repository that only has F16 still gets an answer.
    let only = HubListing {
        files: vec![file("m-f16.gguf", 15_200_000_000)],
        ..seven_b()
    };
    assert_eq!(choose(&only, &hw(64, 48)).unwrap().path, "m-f16.gguf");
}

#[test]
fn kv_cache_estimates_and_context() {
    let l = seven_b();
    // 16 KiB per billion parameters.
    assert_eq!(estimated_kv_bytes(&l.files[1], l.params), 124_518);
    // With no parameter count, from the file size and quantization.
    let guess = estimated_kv_bytes(&l.files[1], None);
    assert!((100_000..150_000).contains(&guess), "{guess}");
    assert_eq!(
        estimated_kv_bytes(&file("tiny.gguf", 1000), None),
        32 * 1024
    );
    assert_eq!(default_ctx(Some(131_072)), 16_384);
    assert_eq!(default_ctx(Some(4096)), 4096);
    assert_eq!(default_ctx(Some(512)), DEFAULT_CTX_SIZE);
    assert_eq!(default_ctx(None), DEFAULT_CTX_SIZE);
}

/// A Hub with one repository, `o/m`: a Q4 file and a two-shard Q8 model,
/// listed over two pages, plus a gated repository. With `bad`, the Q4 file
/// comes back corrupt.
struct FakeHub {
    base: String,
    auth: Arc<Mutex<Vec<String>>>,
    q4: Vec<u8>,
    q8: (Vec<u8>, Vec<u8>),
}

fn fake_hub(bad: bool) -> FakeHub {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let auth = Arc::new(Mutex::new(Vec::new()));
    let mut q4 = qwen7b();
    q4.extend(vec![7u8; 4096]);
    let mut q8a = qwen7b();
    q8a.extend(vec![1u8; 2048]);
    let q8b = vec![2u8; 3000];
    let hub = FakeHub {
        base: base.clone(),
        auth: auth.clone(),
        q4: q4.clone(),
        q8: (q8a.clone(), q8b.clone()),
    };
    let lfs = |path: &str, bytes: &[u8]| {
        serde_json::json!({"type": "file", "path": path, "size": bytes.len(),
            "lfs": {"oid": sha(bytes), "size": bytes.len()}})
    };
    let page1 = serde_json::json!([
        {"type": "file", "path": "README.md", "size": 12},
        lfs("m-Q4_K_M.gguf", &q4),
        {"type": "directory", "path": "Q8_0", "size": 0},
    ])
    .to_string();
    let page2 = serde_json::json!([
        lfs("Q8_0/m-Q8_0-00001-of-00002.gguf", &q8a),
        lfs("Q8_0/m-Q8_0-00002-of-00002.gguf", &q8b),
        lfs("mmproj-m-f16.gguf", b"vision"),
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
            let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
            if let Some(line) = req
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
            {
                auth.lock().unwrap().push(line[14..].trim().to_string());
            }
            let tree = format!("/api/models/o/m/tree/{SHA}?recursive=true");
            let resolve = |f: &str| format!("/o/m/resolve/{SHA}/{f}");
            let ok = |b: Vec<u8>| ("200 OK", String::new(), b);
            let (status, extra, body): (&str, String, Vec<u8>) = if path == "/api/models/o/m" {
                ok(serde_json::json!({"sha": SHA,
                    "gguf": {"total": 7_600_000_000u64, "context_length": 32768},
                    "cardData": {"license": "apache-2.0"}})
                .to_string()
                .into_bytes())
            } else if path == tree {
                (
                    "200 OK",
                    format!("Link: <{base}{tree}&cursor=2>; rel=\"next\"\r\n"),
                    page1.clone().into_bytes(),
                )
            } else if path == format!("{tree}&cursor=2") {
                ok(page2.clone().into_bytes())
            } else if path.starts_with("/api/models?") && path.contains("filter=gguf") {
                ok(br#"[{"id":"o/m","downloads":1234,"likes":5}]"#.to_vec())
            } else if path == resolve("m-Q4_K_M.gguf") {
                let mut b = q4.clone();
                if bad {
                    b[100] ^= 0xff;
                }
                ok(b)
            } else if path == resolve("Q8_0/m-Q8_0-00001-of-00002.gguf") {
                ok(q8a.clone())
            } else if path == resolve("Q8_0/m-Q8_0-00002-of-00002.gguf") {
                ok(q8b.clone())
            } else if path == "/api/models/gated/m" {
                ("401 Unauthorized", String::new(), b"{}".to_vec())
            } else {
                ("404 Not Found", String::new(), b"{}".to_vec())
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    hub
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn a_hub_model_is_listed_pulled_selected_and_removed() {
    let fake = fake_hub(false);
    let hub = Hub::new(&fake.base, Some("hf_secret")).unwrap();
    let models = tempfile::tempdir().unwrap();
    let rt = runtime();

    let found = rt.block_on(search(&hub, "coder", 5)).unwrap();
    assert_eq!(found[0].id, "o/m");
    assert_eq!(found[0].downloads, 1234);

    let listing = rt.block_on(listing(&hub, "o/m")).unwrap();
    assert_eq!(listing.revision, SHA);
    assert_eq!(listing.params, Some(7_600_000_000));
    assert_eq!(listing.license.as_deref(), Some("apache-2.0"));
    // Both pages read; README and vision projector left out.
    let paths: Vec<_> = listing.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["m-Q4_K_M.gguf", "Q8_0/m-Q8_0-00001-of-00002.gguf"]);

    // On a big machine, the two-shard Q8_0.
    let pick = choose(&listing, &hw(64, 48)).unwrap().clone();
    assert_eq!(pick.shards.len(), 2);
    let mut last = None;
    let model = rt
        .block_on(pull(
            &hub,
            &listing,
            &pick,
            models.path(),
            Arc::new(AtomicBool::new(false)),
            |p| last = Some(p),
        ))
        .unwrap();
    let total = (fake.q8.0.len() + fake.q8.1.len()) as u64;
    let last = last.unwrap();
    assert_eq!((last.downloaded, last.total), (total, total));
    assert_eq!(model.id, "hf:o/m:Q8_0/m-Q8_0-00001-of-00002.gguf");
    assert_eq!(model.display_name, "Qwen2.5 7B Instruct (Q8_0)");
    // The exact KV size from the header, not the estimate.
    assert_eq!(model.kv_bytes_per_token, 2 * 28 * 4 * 128 * 2);
    assert_eq!(model.ctx_size, 16_384);
    assert_eq!(model.size(), total);
    let dir = repo_dir(models.path(), "o/m");
    assert_eq!(
        std::fs::read(dir.join("Q8_0/m-Q8_0-00001-of-00002.gguf")).unwrap(),
        fake.q8.0
    );
    assert_eq!(
        std::fs::read(dir.join("Q8_0/m-Q8_0-00002-of-00002.gguf")).unwrap(),
        fake.q8.1
    );
    // The token reached the Hub on every request.
    let auth = fake.auth.lock().unwrap().clone();
    assert!(
        auth.len() >= 6 && auth.iter().all(|a| a == "Bearer hf_secret"),
        "{auth:?}"
    );

    assert_eq!(installed(models.path()), vec![model.clone()]);

    // Named in settings, or picked when no model is named.
    for named in [Some(model.id.clone()), None] {
        let settings = LocalAiSettings {
            model: named,
            ..Default::default()
        };
        let choice = selected_model(&settings, models.path()).unwrap();
        assert_eq!(choice.id, model.id);
        assert!(choice.downloaded);
        assert_eq!(choice.path, dir.join("Q8_0/m-Q8_0-00001-of-00002.gguf"));
        assert_eq!(choice.ctx_size, 16_384);
    }

    // A missing shard means it is not installed.
    std::fs::remove_file(dir.join("Q8_0/m-Q8_0-00002-of-00002.gguf")).unwrap();
    assert!(installed(models.path()).is_empty());
    let settings = LocalAiSettings {
        model: Some(model.id.clone()),
        ..Default::default()
    };
    let err = selected_model(&settings, models.path()).unwrap_err();
    assert!(err.contains("phazeai models pull o/m"), "{err}");

    // Removing deletes the files, the manifest and the emptied folders.
    remove(models.path(), &model).unwrap();
    assert!(!dir.exists());
    assert!(models.path().join("hub").exists());
}

#[test]
fn a_corrupt_download_is_not_installed() {
    let fake = fake_hub(true);
    let hub = Hub::new(&fake.base, None).unwrap();
    let models = tempfile::tempdir().unwrap();
    let rt = runtime();
    let listing = rt.block_on(listing(&hub, "o/m")).unwrap();
    let q4 = listing.files[0].clone();
    assert_eq!(q4.size(), fake.q4.len() as u64);
    let err = rt
        .block_on(pull(
            &hub,
            &listing,
            &q4,
            models.path(),
            Arc::new(AtomicBool::new(false)),
            |_| {},
        ))
        .unwrap_err();
    assert!(matches!(err, LocalAiError::HashMismatch { .. }), "{err}");
    assert!(installed(models.path()).is_empty());
    // No token configured, none sent.
    assert!(fake.auth.lock().unwrap().is_empty());
}

#[test]
fn gated_and_missing_repositories_say_why() {
    let fake = fake_hub(false);
    let hub = Hub::new(&fake.base, None).unwrap();
    let rt = runtime();
    let err = rt
        .block_on(listing(&hub, "gated/m"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("HF_TOKEN"), "{err}");
    let err = rt
        .block_on(listing(&hub, "no/such"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("not found"), "{err}");
    let err = rt
        .block_on(listing(&hub, "not-a-repo"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("owner/name"), "{err}");
}
