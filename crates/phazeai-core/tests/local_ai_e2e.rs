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
