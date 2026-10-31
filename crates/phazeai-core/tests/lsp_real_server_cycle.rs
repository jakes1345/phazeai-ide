//! End-to-end watchdog cycle against a real `rust-analyzer`.
//!
//! Marked `#[ignore]` so a plain `cargo test` does not require a
//! `rust-analyzer` install. Run with:
//!
//!     rustup component add rust-analyzer    # once, from the repo root
//!     cargo test -p phazeai-core --test lsp_real_server_cycle -- --include-ignored
//!
//! A missing or broken `rust-analyzer` FAILS the test with those
//! instructions; it never skips. Being on PATH is not enough: rustup puts a
//! `rust-analyzer` proxy in `~/.cargo/bin` even when the component is not
//! installed, so the test runs `rust-analyzer --version` the way LspManager
//! will start the server and fails if that does not work.
//!
//! Proves the full watchdog contract end-to-end:
//! 1. LspManager spawns rust-analyzer for an open .rs file.
//! 2. An unsaved edit reaches the server: documentSymbol lists a function
//!    that exists only in the editor buffer, never on disk.
//! 3. We SIGKILL the language-server PID externally.
//! 4. is_alive() flips false within 3s (reader thread sees stdout EOF).
//! 5. health_check() respawns the server: the new client is a different
//!    instance (`Arc::ptr_eq`, checked while we still hold the dead client,
//!    so its allocation cannot have been reused), has a different PID, and
//!    is_alive==true.
//! 6. The replayed did_open/did_change carried the unsaved edit: the new
//!    server lists the buffer-only function too.

use phazeai_core::{LspClient, LspEvent, LspManager};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// How to get a working rust-analyzer; part of every failure about it.
const INSTALL_HINT: &str = "install it with `rustup component add rust-analyzer` \
     (run from the repo root so rust-toolchain.toml picks the pinned toolchain), \
     then check that `rust-analyzer --version` prints a version";

/// A function the editor buffer has and the file on disk does not, so a
/// server can only list it if it was sent our did_open/did_change.
const BUFFER_ONLY_FN: &str = "only_in_unsaved_buffer";

/// Panics (never skips) unless `rust-analyzer --version` runs from `root`,
/// the directory LspManager starts the server in: rustup picks the
/// toolchain from there, so this checks the binary the test will really get.
fn require_rust_analyzer(root: &Path) -> String {
    match Command::new("rust-analyzer")
        .arg("--version")
        .current_dir(root)
        .output()
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Ok(out) => panic!(
            "rust-analyzer is on PATH but does not run ({}): {}\n{INSTALL_HINT}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .next()
                .unwrap_or_default()
        ),
        Err(e) => panic!("rust-analyzer is not on PATH ({e}); {INSTALL_HINT}"),
    }
}

fn write_minimal_crate(root: &Path) {
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

/// Waits until `client` lists [`BUFFER_ONLY_FN`] among the symbols of `path`.
/// rust-analyzer loads the workspace in the background and can cancel a
/// request meanwhile (a ContentModified error), so any other answer is
/// retried until the deadline; the panic names the last one.
async fn wait_for_buffer_only_fn(client: &LspClient, path: &Path, server: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut last = String::from("no answer");
    while Instant::now() < deadline {
        match client.document_symbols(path).await {
            Ok(symbols) => {
                let names: Vec<String> = symbols.into_iter().map(|s| s.name).collect();
                if names.iter().any(|n| n == BUFFER_ONLY_FN) {
                    return;
                }
                last = format!("symbols {names:?}");
            }
            Err(e) => last = e,
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!(
        "{server} never listed `{BUFFER_ONLY_FN}` from the unsaved buffer \
         within 60s; last answer: {last}"
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs a working rust-analyzer; fails (never skips) without one"]
async fn rust_analyzer_dies_then_health_check_replaces_it() {
    let tmp = tempfile::tempdir().expect("create tmpdir");
    let root: PathBuf = tmp.path().to_path_buf();
    write_minimal_crate(&root);
    let version = require_rust_analyzer(&root);
    eprintln!("using {version}");

    let (tx, mut rx) = mpsc::unbounded_channel::<LspEvent>();
    let mut manager = LspManager::new(root.clone(), tx);

    // Drain LSP events into a discard task so the unbounded channel doesn't
    // grow unbounded (rust-analyzer is chatty during indexing).
    tokio::spawn(async move { while rx.recv().await.is_some() {} });

    let lib = root.join("src/lib.rs");
    let saved = std::fs::read_to_string(&lib).unwrap();
    manager
        .ensure_server_for_file(&lib)
        .await
        .expect("spawn rust-analyzer");
    manager.did_open(&lib, &saved);
    // Type a function into the buffer without saving it. The manager has to
    // remember this edit, or a restarted server only knows the disk version.
    let edited = format!("{saved}pub fn {BUFFER_ONLY_FN}() {{}}\n");
    manager.did_change(&lib, 1, &edited);

    // Snapshot the original client before killing it.
    let original = manager
        .client_for_language("rust")
        .expect("rust client present after ensure_server_for_file")
        .clone();
    let original_pid = original.child_pid().expect("rust-analyzer pid");
    assert!(
        original.is_alive(),
        "freshly-spawned client should be alive"
    );
    // The live server sees the edit, so the same check after the restart
    // can only pass if the replay delivered it again.
    wait_for_buffer_only_fn(&original, &lib, "the original server").await;

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

    // Now the watchdog should respawn rust-analyzer and replay did_open.
    // We keep `original` until the comparison below: once freed, the dead
    // client's allocation can be handed to the replacement, and then equal
    // addresses would not mean the same client.
    manager.health_check().await;

    let replacement = manager
        .client_for_language("rust")
        .expect("manager should have respawned a rust client")
        .clone();
    assert!(
        !Arc::ptr_eq(&original, &replacement),
        "replacement client should be a NEW Arc, not the dead one"
    );
    drop(original);
    assert!(
        replacement.is_alive(),
        "replacement client should be alive after restart"
    );
    assert_ne!(
        replacement.child_pid().unwrap(),
        original_pid,
        "replacement should have a different PID than the killed process"
    );
    // The disk still has only `add`: the new server knows the unsaved
    // function only through the manager's replay of the open buffer.
    wait_for_buffer_only_fn(&replacement, &lib, "the restarted server").await;

    // Clean up so tempdir's Drop doesn't trip on a still-running child.
    drop(replacement);
    manager.shutdown_all().await;
}
