// Tests for the native Rust plugin extension host.
//
// The tests that required VSIX, JS (Deno), and WASM backends have been
// removed because those backends no longer exist.  The tests below verify
// the plugin manager logic that does not depend on an actual .so file being
// present on disk.

use phazeai_core::ext_host::{
    DummyDelegate, ExtensionManager, IdeDelegateHost, IdeDelegate, PluginEvent,
};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Helper: a test delegate that records log and message calls
// ---------------------------------------------------------------------------

struct RecordingDelegate {
    log_lines: Arc<Mutex<Vec<String>>>,
    messages: Arc<Mutex<Vec<String>>>,
}

impl IdeDelegate for RecordingDelegate {
    fn log(&self, msg: &str) {
        self.log_lines.lock().unwrap().push(msg.to_string());
    }
    fn show_message(&self, msg: &str) {
        self.messages.lock().unwrap().push(msg.to_string());
    }
    fn get_active_text(&self) -> String {
        "selected text".to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_extension_manager_new_has_no_plugins() {
    let manager = ExtensionManager::new();
    assert!(
        manager.get_plugins().is_empty(),
        "A freshly created manager should have no plugins loaded"
    );
}

#[test]
fn test_extension_manager_with_custom_dir() {
    let tmp = tempfile::tempdir().expect("could not create temp dir");
    let manager = ExtensionManager::with_plugin_dir(tmp.path());
    assert_eq!(manager.plugin_dir, tmp.path());
    assert!(manager.get_plugins().is_empty());
}

#[test]
fn test_scan_plugins_nonexistent_dir_does_not_panic() {
    let mut manager = ExtensionManager::with_plugin_dir("/tmp/phazeai_test_nonexistent_plugins");
    let host = IdeDelegateHost::new(Arc::new(DummyDelegate));
    // Scanning a directory that does not exist must not panic — it logs a
    // message and returns without loading anything.
    manager.scan_plugins(&host);
    assert!(manager.get_plugins().is_empty());
}

#[test]
fn test_scan_plugins_empty_dir_loads_nothing() {
    let tmp = tempfile::tempdir().expect("could not create temp dir");
    let mut manager = ExtensionManager::with_plugin_dir(tmp.path());
    let host = IdeDelegateHost::new(Arc::new(DummyDelegate));
    manager.scan_plugins(&host);
    assert!(manager.get_plugins().is_empty());
}

#[test]
fn test_scan_plugins_dir_without_manifest_is_skipped() {
    let tmp = tempfile::tempdir().expect("could not create temp dir");
    // Create a subdirectory with no plugin.toml.
    std::fs::create_dir(tmp.path().join("orphan")).unwrap();
    let mut manager = ExtensionManager::with_plugin_dir(tmp.path());
    let host = IdeDelegateHost::new(Arc::new(DummyDelegate));
    manager.scan_plugins(&host);
    assert!(manager.get_plugins().is_empty());
}

#[test]
fn test_load_plugin_missing_dir_returns_error() {
    let mut manager = ExtensionManager::new();
    let host = IdeDelegateHost::new(Arc::new(DummyDelegate));
    let result = manager.load_plugin(
        std::path::Path::new("/tmp/phazeai_no_such_plugin_dir"),
        &host,
    );
    assert!(result.is_err(), "Loading from a non-existent dir should fail");
    let err = result.unwrap_err();
    assert!(
        err.contains("plugin.toml"),
        "Error should mention plugin.toml; got: {}",
        err
    );
}

#[test]
fn test_load_plugin_missing_library_returns_error() {
    let tmp = tempfile::tempdir().expect("could not create temp dir");
    // Write a valid-looking manifest but no .so file.
    let manifest = r#"
name = "test-plugin"
version = "0.1.0"
description = "A test plugin with no library"
author = "Test"
min_api_version = 1
"#;
    std::fs::write(tmp.path().join("plugin.toml"), manifest).unwrap();

    let mut manager = ExtensionManager::new();
    let host = IdeDelegateHost::new(Arc::new(DummyDelegate));
    let result = manager.load_plugin(tmp.path(), &host);
    assert!(result.is_err(), "Loading with no library file should fail");
    let err = result.unwrap_err();
    assert!(
        err.to_lowercase().contains("library") || err.to_lowercase().contains("not found"),
        "Error should mention missing library; got: {}",
        err
    );
}

#[test]
fn test_load_plugin_invalid_toml_returns_error() {
    let tmp = tempfile::tempdir().expect("could not create temp dir");
    std::fs::write(tmp.path().join("plugin.toml"), "<<< not valid toml >>>").unwrap();

    let mut manager = ExtensionManager::new();
    let host = IdeDelegateHost::new(Arc::new(DummyDelegate));
    let result = manager.load_plugin(tmp.path(), &host);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("Invalid plugin.toml"),
        "Expected TOML parse error; got: {}",
        err
    );
}

#[test]
fn test_execute_command_no_plugins_returns_error() {
    let mut manager = ExtensionManager::new();
    let result = manager.execute_command("some.command", "{}");
    assert!(result.is_err());
    assert!(
        result.unwrap_err().contains("No plugin handles command"),
        "Expected 'no plugin' error"
    );
}

#[test]
fn test_broadcast_event_with_no_plugins_does_not_panic() {
    let mut manager = ExtensionManager::new();
    // Must not panic when there are no plugins to deliver the event to.
    manager.broadcast_event(&PluginEvent::FileSaved {
        path: "/some/file.rs".to_string(),
    });
}

#[test]
fn test_unload_nonexistent_plugin_does_not_panic() {
    let mut manager = ExtensionManager::new();
    // Should silently warn and return without panicking.
    manager.unload_plugin("nonexistent-plugin");
    assert!(manager.get_plugins().is_empty());
}

#[test]
fn test_ide_delegate_host_log_routes_to_delegate() {
    use phazeai_core::ext_host::PluginHost;

    let log_lines = Arc::new(Mutex::new(Vec::new()));
    let delegate = RecordingDelegate {
        log_lines: log_lines.clone(),
        messages: Arc::new(Mutex::new(Vec::new())),
    };
    let host = IdeDelegateHost::new(Arc::new(delegate));
    host.log(2, "hello from plugin");

    let recorded = log_lines.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0], "hello from plugin");
}

#[test]
fn test_ide_delegate_host_show_message_routes_to_delegate() {
    use phazeai_core::ext_host::PluginHost;

    let messages = Arc::new(Mutex::new(Vec::new()));
    let delegate = RecordingDelegate {
        log_lines: Arc::new(Mutex::new(Vec::new())),
        messages: messages.clone(),
    };
    let host = IdeDelegateHost::new(Arc::new(delegate));
    host.show_message("toast notification");

    let recorded = messages.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0], "toast notification");
}

#[test]
fn test_ide_delegate_host_get_active_text() {
    use phazeai_core::ext_host::PluginHost;

    let delegate = RecordingDelegate {
        log_lines: Arc::new(Mutex::new(Vec::new())),
        messages: Arc::new(Mutex::new(Vec::new())),
    };
    let host = IdeDelegateHost::new(Arc::new(delegate));
    assert_eq!(host.get_active_text(), "selected text");
}

#[test]
fn test_dummy_delegate_does_not_panic() {
    let delegate = DummyDelegate;
    delegate.log("a log line");
    delegate.show_message("a message");
    let text = delegate.get_active_text();
    assert!(text.is_empty(), "DummyDelegate.get_active_text should return empty string");
}

#[test]
fn test_plugin_info_fields() {
    // Verify the PluginInfo struct is accessible and has the expected fields.
    use phazeai_core::ext_host::PluginInfo;
    let info = PluginInfo {
        name: "foo".to_string(),
        version: "1.0.0".to_string(),
        description: "A plugin".to_string(),
        author: "Author".to_string(),
        active: true,
        commands: vec![],
    };
    assert_eq!(info.name, "foo");
    assert!(info.active);
}
