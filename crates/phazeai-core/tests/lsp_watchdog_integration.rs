//! End-to-end watchdog test using a real spawned child that exits on demand.
//!
//! Uses `/bin/sh -c 'sleep 60'` as a stand-in language server: the process is
//! real, has stdin/stdout/stderr pipes, and can be killed externally. We
//! bypass `LspClient::initialize` (which requires a real LSP handshake) and
//! drive only the process-liveness side of the contract: spawn → kill →
//! observe `is_alive()` flip false within a bounded time.
//!
//! This is the smallest test that proves the reader-loop-EOF →
//! `alive.store(false)` cycle actually fires on a real OS process death.

use phazeai_core::lsp::client::LspClient;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

#[test]
fn alive_flips_false_when_child_exits() {
    // `LspClient::start` spawns the command, takes stdin/stdout/stderr, and
    // launches a reader thread. is_alive() is gated on the reader thread
    // observing EOF on stdout, which fires when the child process exits
    // regardless of any LSP handshake — so a child that just exits
    // immediately is enough to drive the watchdog signal we care about.
    //
    // We can't call client.shutdown() here because it sends an LSP
    // Shutdown request and awaits the response, and a /bin/sh child never
    // sends one — that path is exercised in real-server use, not this
    // unit-level liveness test.
    let (tx, _rx) = mpsc::unbounded_channel();
    let workspace = std::env::temp_dir();

    // /bin/true exits 0 immediately; the reader thread should observe EOF
    // and flip alive=false within milliseconds.
    let client = LspClient::start("/bin/true", &[], &workspace, tx)
        .expect("spawn /bin/true as fake LSP server");

    let deadline = Instant::now() + Duration::from_secs(2);
    while client.is_alive() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }

    assert!(
        !client.is_alive(),
        "is_alive should flip false within 2s of the child exiting; \
         reader thread either didn't see EOF or didn't update the flag"
    );
}

#[test]
fn alive_stays_true_while_child_is_running() {
    // Inverse contract: a child that's still running keeps is_alive=true.
    // /bin/sleep 5 holds its stdout open and blocks; the reader thread is
    // sitting in read_line, alive should stay true throughout.
    let (tx, _rx) = mpsc::unbounded_channel();
    let workspace = std::env::temp_dir();

    let client = LspClient::start("/bin/sleep", &["5".to_string()], &workspace, tx)
        .expect("spawn /bin/sleep");

    // Wait briefly to ensure no spurious EOF flips the flag.
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        client.is_alive(),
        "is_alive should stay true while child is running"
    );

    // Drop the client; its Drop impl kills the child, freeing the test from
    // waiting out the full sleep.
    drop(client);
}
