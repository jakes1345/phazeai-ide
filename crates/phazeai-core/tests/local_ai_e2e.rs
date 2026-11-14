//! End-to-end check of the built-in local AI against the real world: downloads
//! the smallest catalog model from Hugging Face (≈1.1 GB, with a deliberate
//! cancel + resume), verifies it, runs a real `llama-server`, and chats with it
//! through `Settings::build_llm_client()` exactly like the IDE does.
//!
//! Ignored by default (network + ~1.1 GB). Run with:
//!
//! ```text
//! PHAZEAI_E2E_LLAMA_SERVER=/path/to/llama-server \
//! PHAZEAI_E2E_MODELS_DIR=/some/dir \
//!   cargo test -p phazeai-core --test local_ai_e2e -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use futures::StreamExt;
use phazeai_core::config::LlmProvider;
use phazeai_core::llm::{Message, StreamEvent};
use phazeai_core::local_ai::{
    download::{download_model, is_downloaded, partial_bytes},
    server::resolve_launch,
    LocalAiError, LocalAiSettings, ServerStatus, CATALOG, LOCAL_AI,
};
use phazeai_core::Settings;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "downloads ~1.1 GB and needs a llama-server binary"]
async fn download_run_and_chat_with_the_smallest_model() {
    let server_bin = PathBuf::from(
        std::env::var("PHAZEAI_E2E_LLAMA_SERVER").expect("set PHAZEAI_E2E_LLAMA_SERVER"),
    );
    let models_dir =
        PathBuf::from(std::env::var("PHAZEAI_E2E_MODELS_DIR").expect("set PHAZEAI_E2E_MODELS_DIR"));
    let entry = &CATALOG[0];

    if !is_downloaded(entry, &models_dir) {
        // 1. Start, cancel after ~64 MB, check the partial survives.
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let first = download_model(entry, &models_dir, cancel, move |p| {
            if p.downloaded > 64_000_000 {
                c2.store(true, Ordering::Relaxed);
            }
        })
        .await;
        assert!(matches!(first, Err(LocalAiError::Cancelled)), "{first:?}");
        let partial = partial_bytes(entry, &models_dir).expect("partial file kept");
        eprintln!("cancelled with {partial} bytes on disk; resuming");

        // 2. Resume to the end; size + SHA-256 are verified inside.
        let mut last = None;
        let path = download_model(entry, &models_dir, Arc::new(AtomicBool::new(false)), |p| {
            last = Some(p)
        })
        .await
        .expect("resumed download verifies");
        let last = last.unwrap();
        eprintln!(
            "downloaded {} ({} bytes, last rate {:.1} MB/s)",
            path.display(),
            last.downloaded,
            last.bytes_per_sec / 1e6
        );
        assert!(is_downloaded(entry, &models_dir));
        assert_eq!(partial_bytes(entry, &models_dir), None);
    }

    // 3. Start the real server through the shared handle.
    let local = LocalAiSettings {
        model: Some(entry.id.to_string()),
        server_path: Some(server_bin),
        ctx_size: Some(4096),
        auto_start: true,
    };
    let spec = resolve_launch(&local, &models_dir, &models_dir.join("no-install"), None).unwrap();
    let url = LOCAL_AI
        .start_spec(spec)
        .await
        .expect("server becomes healthy");
    eprintln!("llama-server ready at {url}");
    assert!(matches!(LOCAL_AI.status(), ServerStatus::Running { .. }));

    // 4. Chat through the same path every IDE panel uses.
    let mut settings = Settings::default();
    settings.llm.provider = LlmProvider::BuiltIn;
    settings.llm.model = entry.id.to_string();
    settings.local_ai = local;
    let client = settings.build_llm_client().expect("BuiltIn client builds");
    let resp = client
        .chat(
            &[Message::user(
                "Write a Rust function `add(a: i32, b: i32) -> i32`. Reply with code only.",
            )],
            &[],
        )
        .await
        .expect("chat completes");
    eprintln!("chat reply:\n{}", resp.message.content);
    assert!(
        resp.message.content.contains("fn add"),
        "{}",
        resp.message.content
    );

    let mut rx = client
        .chat_stream(&[Message::user("Say hello in one word.")], &[])
        .await
        .expect("stream opens");
    let mut text = String::new();
    while let Some(ev) = rx.next().await {
        match ev {
            StreamEvent::TextDelta(t) => text.push_str(&t),
            StreamEvent::Done => break,
            StreamEvent::Error(e) => panic!("stream error: {e}"),
            _ => {}
        }
    }
    eprintln!("streamed: {text}");
    assert!(!text.trim().is_empty());

    // 5. Tool definitions are accepted (needs --jinja).
    let tool = phazeai_core::tools::ToolDefinition {
        name: "read_file".into(),
        description: "Read a file from the project".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }),
    };
    let resp = client
        .chat(
            &[Message::user(
                "Open the file src/main.rs and show it to me.",
            )],
            &[tool],
        )
        .await
        .expect("chat with tools completes");
    eprintln!(
        "tool-call reply: content={:?} tool_calls={:?}",
        resp.message.content, resp.message.tool_calls
    );

    // 6. Stop.
    LOCAL_AI.stop();
    assert_eq!(LOCAL_AI.status(), ServerStatus::Stopped);
    assert!(settings.build_llm_client().is_err());
}

/// Reads GitHub's live llama.cpp release list (no download) and checks that a
/// verified build is found for every platform PhazeAI installs on. Catches
/// llama.cpp changing how it publishes builds, as it did on 2026-11-13.
///
/// ```text
/// cargo test -p phazeai-core --test local_ai_e2e github_lists_an_installable_build -- --ignored
/// ```
#[tokio::test]
#[ignore = "network: reads api.github.com"]
async fn github_lists_an_installable_build() {
    use phazeai_core::local_ai::install::{choose_release, Release, ReleaseChoice, RELEASES_API};
    let releases: Vec<Release> = reqwest::Client::new()
        .get(RELEASES_API)
        .header(reqwest::header::USER_AGENT, "phazeai-e2e-test")
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .expect("reach api.github.com")
        .error_for_status()
        .expect("release list")
        .json()
        .await
        .expect("release JSON");
    for (os, arch) in [
        ("linux", "x86_64"),
        ("windows", "x86_64"),
        ("macos", "aarch64"),
    ] {
        match choose_release(&releases, os, arch, true) {
            ReleaseChoice::Ready {
                release,
                asset,
                build,
                ..
            } => {
                println!(
                    "{os}/{arch}: {} → {} ({build:?})",
                    release.tag_name, asset.name
                );
                assert!(build.uses_gpu(), "{os}/{arch}: no verified GPU build");
            }
            other => panic!("{os}/{arch}: {other:?}"),
        }
    }
}

/// Installs this platform's GPU build from GitHub into a scratch folder with
/// PhazeAI's own installer, then asks it which GPUs it can use. On a machine
/// with a GPU and drivers, at least one must be listed.
///
/// ```text
/// PHAZEAI_E2E_INSTALL_ROOT=/some/empty/dir \
///   cargo test -p phazeai-core --test local_ai_e2e installs_the_gpu_build -- --ignored --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "network: downloads llama.cpp (~50 MB) and runs it"]
async fn installs_the_gpu_build_and_lists_this_machines_gpus() {
    use phazeai_core::local_ai::{hardware, install};
    let root = PathBuf::from(
        std::env::var("PHAZEAI_E2E_INSTALL_ROOT").expect("set PHAZEAI_E2E_INSTALL_ROOT"),
    );
    let found = install::install_latest(&root, true, Arc::new(AtomicBool::new(false)), |_| {})
        .await
        .expect("install");
    println!("installed: {}", found.describe());
    assert!(found.build.is_some_and(|b| b.uses_gpu()), "{found:?}");
    let gpus = hardware::list_devices(&found.path).expect("--list-devices");
    for g in &gpus {
        println!(
            "GPU: {} {} ({} MiB, {} MiB free)",
            g.name,
            g.description,
            g.vram_total >> 20,
            g.vram_free >> 20
        );
    }
    assert!(!gpus.is_empty(), "the GPU build sees no GPU");
}

/// Starts a real `llama-server` on a GGUF and checks that the GPU offload it
/// logs is what PhazeAI reports.
///
/// ```text
/// PHAZEAI_E2E_LLAMA_SERVER=/path/to/llama-server PHAZEAI_E2E_GGUF=/path/to/model.gguf \
///   cargo test -p phazeai-core --test local_ai_e2e reports_gpu_offload -- --ignored --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a llama-server binary and a GGUF model"]
async fn reports_gpu_offload_from_a_real_server() {
    use phazeai_core::local_ai::server::{free_port, LaunchSpec, LlamaServer};
    let spec = LaunchSpec {
        binary: PathBuf::from(
            std::env::var("PHAZEAI_E2E_LLAMA_SERVER").expect("set PHAZEAI_E2E_LLAMA_SERVER"),
        ),
        model_path: PathBuf::from(std::env::var("PHAZEAI_E2E_GGUF").expect("set PHAZEAI_E2E_GGUF")),
        model_id: "e2e".into(),
        ctx_size: 4096,
    };
    let mut server = LlamaServer::spawn(&spec, free_port().unwrap(), None).expect("spawn");
    server
        .wait_healthy(std::time::Duration::from_secs(180))
        .await
        .expect("healthy");
    let Some(offload) = server.offload() else {
        panic!(
            "llama-server logged no offload line; its last lines:\n{}",
            server.stderr_tail(40)
        );
    };
    println!("offload: {}", offload.describe());
    assert!(offload.gpu_layers > 0, "{}", offload.describe());
}

/// `/btw` end to end: PhazeAI Local runs a real model, and a side question
/// about a session snapshot is answered through `btw::ask`, as the IDE and
/// the CLI call it, while the configured main provider stays a paid one.
///
/// ```text
/// PHAZEAI_E2E_LLAMA_SERVER=/path/to/llama-server PHAZEAI_E2E_GGUF=/path/to/model.gguf \
///   cargo test -p phazeai-core --test local_ai_e2e btw_answers -- --ignored --nocapture
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a llama-server binary and a GGUF model"]
async fn btw_answers_a_side_question_on_the_local_model() {
    use phazeai_core::btw::{self, SessionLine, Who};
    use phazeai_core::local_ai::server::LaunchSpec;
    let spec = LaunchSpec {
        binary: PathBuf::from(
            std::env::var("PHAZEAI_E2E_LLAMA_SERVER").expect("set PHAZEAI_E2E_LLAMA_SERVER"),
        ),
        model_path: PathBuf::from(std::env::var("PHAZEAI_E2E_GGUF").expect("set PHAZEAI_E2E_GGUF")),
        model_id: "e2e-btw".into(),
        ctx_size: 8192,
    };
    LOCAL_AI.start_spec(spec).await.expect("server starts");

    let mut settings = Settings::default();
    settings.set_provider(LlmProvider::Claude); // the main chat is a paid provider
    settings.local_ai.ctx_size = Some(8192);
    let session = vec![
        SessionLine::new(Who::User, "Fix the bug in stock_value in src/lib.rs"),
        SessionLine::new(Who::Tool, "read_file src/lib.rs: 20 lines"),
        SessionLine::new(
            Who::Assistant,
            "stock_value adds price_cents + qty for each item; it should multiply \
             price_cents * qty. I'll change the + to *.",
        ),
    ];
    let prompt = btw::prompt(
        &session,
        &[],
        "In one sentence: what operator is wrong in stock_value?",
        btw::budget_chars(&settings),
    );
    let started = std::time::Instant::now();
    let mut pieces = 0;
    let answer = btw::ask(&settings, &prompt, |_| pieces += 1)
        .await
        .expect("answer");
    println!(
        "btw answer in {:.1}s, {pieces} streamed pieces: {answer}",
        started.elapsed().as_secs_f64()
    );
    LOCAL_AI.stop();
    assert!(pieces > 1, "the answer should stream");
    let lower = answer.to_lowercase();
    assert!(
        lower.contains('+') || lower.contains("plus") || lower.contains("add"),
        "{answer}"
    );
}
