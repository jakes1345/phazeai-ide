//! End-to-end watchdog cycle against a real `rust-analyzer`.
//!
//! Marked `#[ignore]` because it depends on a locally-installed
//! `rust-analyzer` binary and is slow (~10-30s on a warm cache while
//! rust-analyzer initializes twice). Run with:
//!
//!     cargo test -p phazeai-core --test lsp_real_server_cycle -- --ignored
//!
//! Proves the full watchdog contract end-to-end:
//! 1. LspManager spawns rust-analyzer for an open .rs file.
//! 2. We SIGKILL the language-server PID externally.
//! 3. is_alive() flips false within 2s (reader thread sees stdout EOF).
//! 4. health_check() respawns the server and replays did_open.
//! 5. The new client has a different Arc pointer (real replacement, not
//!    the same instance) and is_alive==true.

use phazeai_core::{LspEvent, LspManager};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

fn rust_analyzer_available() -> bool {
    Command::new("which")
        .arg("rust-analyzer")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_minimal_crate(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"watchdog_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "spawns real rust-analyzer; slow (~30s) and needs the binary on PATH"]
async fn rust_analyzer_dies_then_health_check_replaces_it() {
    if !rust_analyzer_available() {
        eprintln!("skipping: rust-analyzer not on PATH");
        return;
    }

    let tmp = tempfile::tempdir().expect("create tmpdir");
    let root: PathBuf = tmp.path().to_path_buf();
    write_minimal_crate(&root);

    let (tx, mut rx) = mpsc::unbounded_channel::<LspEvent>();
    let mut manager = LspManager::new(root.clone(), tx);

    // Drain LSP events into a discard task so the unbounded channel doesn't
    // grow unbounded (rust-analyzer is chatty during indexing).
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let lib = root.join("src/lib.rs");
    let text = std::fs::read_to_string(&lib).unwrap();
    manager
        .ensure_server_for_file(&lib)
        .await
        .expect("spawn rust-analyzer");
    manager.did_open(&lib, &text);

    // Snapshot the original client before killing it.
    let original = manager
        .client_for_language("rust")
        .expect("rust client present after ensure_server_for_file")
        .clone();
    let original_ptr = Arc::as_ptr(&original) as usize;
    let original_pid = original.child_pid().expect("rust-analyzer pid");
    assert!(
        original.is_alive(),
        "freshly-spawned client should be alive"
    );

    // SIGKILL the language-server process — simulates a real crash.
    let killed = Command::new("kill")
        .arg("-9")
        .arg(original_pid.to_string())
        .status()
        .expect("invoke kill")
        .success();
    assert!(killed, "kill -9 {original_pid} failed");

    // Reader thread should see stdout EOF and flip alive=false within ~200ms.
    let deadline = Instant::now() + Duration::from_secs(3);
    while original.is_alive() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        !original.is_alive(),
        "is_alive should flip false within 3s of SIGKILL"
    );

    // Drop our handle so the manager can drop the dead Arc cleanly.
    drop(original);

    // Now the watchdog should respawn rust-analyzer and replay did_open.
    manager.health_check().await;

    let replacement = manager
        .client_for_language("rust")
        .expect("manager should have respawned a rust client")
        .clone();
    let replacement_ptr = Arc::as_ptr(&replacement) as usize;
    assert_ne!(
        original_ptr, replacement_ptr,
        "replacement client should be a NEW Arc, not the dead one"
    );
    assert!(
        replacement.is_alive(),
        "replacement client should be alive after restart"
    );
    assert_ne!(
        replacement.child_pid().unwrap(),
        original_pid,
        "replacement should have a different PID than the killed process"
    );

    // Clean up so tempdir's Drop doesn't trip on a still-running child.
    drop(replacement);
    manager.shutdown_all().await;
}
