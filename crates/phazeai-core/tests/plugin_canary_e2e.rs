//! End-to-end test for the native plugin loader.
//!
//! Builds `phazeai-plugin-canary` (if not already built), stages its `plugin.toml`
//! and `cdylib` into a temp directory matching the loader's naming convention,
//! then drives the full `ExtensionManager` lifecycle against it: scan → load →
//! activate → command dispatch → event delivery → unload.
//!
//! Run with `cargo test -p phazeai-core --test plugin_canary_e2e`.

use phazeai_core::ext_host::{DummyDelegate, ExtensionManager, IdeDelegateHost, PluginEvent};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

/// Locate (or build) the canary `cdylib`. Returns the path to the shared
/// object on the current platform.
fn build_and_locate_canary() -> PathBuf {
    // Build with the same profile this test runs under (debug by default).
    let status = Command::new(env!("CARGO"))
        .args(["build", "-p", "phazeai-plugin-canary", "--quiet"])
        .status()
        .expect("spawning cargo build for canary must succeed");
    assert!(status.success(), "canary build failed");

    // `target/<profile>/<libname>` — we discover the target dir via CARGO_MANIFEST_DIR.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("target")
        .join("debug");

    let lib_name = if cfg!(target_os = "windows") {
        "phazeai_plugin_canary.dll"
    } else if cfg!(target_os = "macos") {
        "libphazeai_plugin_canary.dylib"
    } else {
        "libphazeai_plugin_canary.so"
    };

    let candidate = target.join(lib_name);
    assert!(
        candidate.exists(),
        "expected cdylib at {} — build output layout changed?",
        candidate.display()
    );
    candidate
}

/// Stage a plugin directory with the exact names `ExtensionManager` looks for:
///   <dir>/plugin.toml
///   <dir>/lib<name>.so   (Linux)  /  <name>.dll  (Windows)  /  lib<name>.dylib (macOS)
///
/// The manifest's `name` field drives the derived library file name, so we
/// must keep them in sync with whatever the cdylib exports as.
fn stage_plugin_dir(lib_src: &std::path::Path) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plugin_dir = tmp.path().join("canary");
    std::fs::create_dir_all(&plugin_dir).unwrap();

    // Manifest — keep `name` equal to the cdylib's base name so the loader
    // finds `libphazeai_plugin_canary.so` automatically on Linux.
    let manifest = r#"name = "phazeai_plugin_canary"
version = "0.1.0"
description = "End-to-end canary for the PhazePlugin ABI."
author = "PhazeAI"
min_api_version = 1
"#;
    std::fs::write(plugin_dir.join("plugin.toml"), manifest).unwrap();

    let dest_name = if cfg!(target_os = "windows") {
        "phazeai_plugin_canary.dll"
    } else if cfg!(target_os = "macos") {
        "libphazeai_plugin_canary.dylib"
    } else {
        "libphazeai_plugin_canary.so"
    };
    std::fs::copy(lib_src, plugin_dir.join(dest_name)).expect("copy cdylib into staging dir");

    tmp
}

#[test]
fn canary_plugin_loads_activates_and_executes_end_to_end() {
    let lib = build_and_locate_canary();
    let staged = stage_plugin_dir(&lib);

    let host = IdeDelegateHost::new(Arc::new(DummyDelegate));
    let mut mgr = ExtensionManager::with_plugin_dir(staged.path());

    // Scan should discover exactly one plugin.
    mgr.scan_plugins(&host);
    let loaded = mgr.get_plugins();
    assert_eq!(
        loaded.len(),
        1,
        "expected 1 loaded plugin, got {:?}",
        loaded
    );
    let info = &loaded[0];
    assert_eq!(info.name, "phazeai_plugin_canary");
    assert!(info.active, "plugin must be active after scan");

    // The plugin registers one command.
    assert_eq!(info.commands.len(), 1);
    assert_eq!(info.commands[0].id, "canary.echo");

    // Execute the command and check the echo round-trips the args verbatim.
    let out = mgr
        .execute_command("canary.echo", r#"{"msg":"hi"}"#)
        .expect("canary.echo must succeed");
    assert_eq!(out, r#"{"msg":"hi"}"#);

    // Unknown command on a loaded plugin should not panic; it returns Err
    // from somewhere in the dispatch chain.
    let unknown = mgr.execute_command("canary.does_not_exist", "{}");
    assert!(unknown.is_err(), "unknown command must return Err");

    // Deliver an event — should not panic and should leave the plugin loaded.
    mgr.broadcast_event(&PluginEvent::FileSaved {
        path: "/tmp/canary.rs".to_string(),
    });
    assert_eq!(mgr.get_plugins().len(), 1);

    // Unload cleanly — the canary's on_deactivate + _phazeai_plugin_destroy
    // run as the NativePlugin is dropped inside `unload_plugin`.
    mgr.unload_plugin("phazeai_plugin_canary");
    assert!(mgr.get_plugins().is_empty(), "plugin must be unloaded");
}
