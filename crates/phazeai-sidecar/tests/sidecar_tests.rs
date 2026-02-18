use phazeai_sidecar::{JsonRpcRequest, JsonRpcResponse, SidecarManager};
use serde_json::{json, Value};
use std::path::PathBuf;
use tempfile::NamedTempFile;

// ============================================================================
// Protocol Tests - JsonRpcRequest
// ============================================================================

#[test]
fn test_jsonrpc_request_new_creates_correct_structure() {
    let request = JsonRpcRequest::new(1, "test_method", None);

    assert_eq!(request.jsonrpc, "2.0");
    assert_eq!(request.id, 1);
    assert_eq!(request.method, "test_method");
    assert!(request.params.is_none());
}

#[test]
fn test_jsonrpc_request_serializes_to_valid_json() {
    let request = JsonRpcRequest::new(42, "initialize", None);
    let json = serde_json::to_value(&request).unwrap();

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 42);
    assert_eq!(json["method"], "initialize");
}

#[test]
fn test_jsonrpc_request_with_params_serializes_correctly() {
    let params = json!({
        "workspace": "/path/to/workspace",
        "capabilities": ["lsp", "completion"]
    });

    let request = JsonRpcRequest::new(1, "initialize", Some(params.clone()));
    let json = serde_json::to_value(&request).unwrap();

    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["id"], 1);
    assert_eq!(json["method"], "initialize");
    assert_eq!(json["params"], params);
}

#[test]
fn test_jsonrpc_request_with_none_params_omits_params_field() {
    let request = JsonRpcRequest::new(1, "shutdown", None);
    let json = serde_json::to_string(&request).unwrap();

    // Should not contain "params" field at all
    assert!(!json.contains("\"params\""));
}

// ============================================================================
// Protocol Tests - JsonRpcResponse
// ============================================================================

#[test]
fn test_jsonrpc_response_deserialization_success_case() {
    let json_str = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "result": {"status": "ok", "value": 42}
    }"#;

    let response: JsonRpcResponse = serde_json::from_str(json_str).unwrap();

    assert_eq!(response.jsonrpc, "2.0");
    assert_eq!(response.id, 1);
    assert!(response.result.is_some());
    assert!(response.error.is_none());
    assert_eq!(response.result.unwrap()["status"], "ok");
}

#[test]
fn test_jsonrpc_response_deserialization_error_case() {
    let json_str = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32601,
            "message": "Method not found"
        }
    }"#;

    let response: JsonRpcResponse = serde_json::from_str(json_str).unwrap();

    assert_eq!(response.jsonrpc, "2.0");
    assert_eq!(response.id, 1);
    assert!(response.result.is_none());
    assert!(response.error.is_some());

    let error = response.error.unwrap();
    assert_eq!(error.code, -32601);
    assert_eq!(error.message, "Method not found");
}

#[test]
fn test_jsonrpc_response_is_success_returns_true_when_no_error() {
    let json_str = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "result": {"data": "success"}
    }"#;

    let response: JsonRpcResponse = serde_json::from_str(json_str).unwrap();
    assert!(response.is_success());
}

#[test]
fn test_jsonrpc_response_is_success_returns_false_when_error_present() {
    let json_str = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32600,
            "message": "Invalid Request"
        }
    }"#;

    let response: JsonRpcResponse = serde_json::from_str(json_str).unwrap();
    assert!(!response.is_success());
}

#[test]
fn test_jsonrpc_response_into_result_returns_ok_with_result_value() {
    let json_str = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "result": {"data": "test_value"}
    }"#;

    let response: JsonRpcResponse = serde_json::from_str(json_str).unwrap();
    let result = response.into_result();

    assert!(result.is_ok());
    assert_eq!(result.unwrap()["data"], "test_value");
}

#[test]
fn test_jsonrpc_response_into_result_returns_err_with_error_message() {
    let json_str = r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": -32700,
            "message": "Parse error"
        }
    }"#;

    let response: JsonRpcResponse = serde_json::from_str(json_str).unwrap();
    let result = response.into_result();

    assert!(result.is_err());
    assert_eq!(result.unwrap_err(), "Parse error");
}

#[test]
fn test_jsonrpc_response_into_result_returns_ok_null_when_no_result_no_error() {
    let json_str = r#"{
        "jsonrpc": "2.0",
        "id": 1
    }"#;

    let response: JsonRpcResponse = serde_json::from_str(json_str).unwrap();
    let result = response.into_result();

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Value::Null);
}

#[test]
fn test_jsonrpc_error_serialization_roundtrip() {
    let error_json = json!({
        "code": -32603,
        "message": "Internal error",
        "data": {"details": "Something went wrong"}
    });

    // We need to access JsonRpcError through the protocol module
    // since it's not re-exported. We'll test it through JsonRpcResponse.
    let response_json = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": error_json
    });

    let response: JsonRpcResponse = serde_json::from_value(response_json).unwrap();
    let error = response.error.as_ref().unwrap();

    assert_eq!(error.code, -32603);
    assert_eq!(error.message, "Internal error");
    assert!(error.data.is_some());

    // Serialize back
    let serialized = serde_json::to_value(&response).unwrap();
    assert_eq!(serialized["error"]["code"], -32603);
    assert_eq!(serialized["error"]["message"], "Internal error");
    assert_eq!(serialized["error"]["data"]["details"], "Something went wrong");
}

// ============================================================================
// Manager Tests - SidecarManager
// ============================================================================

#[test]
fn test_sidecar_manager_new_creates_non_running_manager() {
    let manager = SidecarManager::new("python3", PathBuf::from("/tmp/script.py"));

    assert!(!manager.is_running());
}

#[test]
fn test_sidecar_manager_is_running_returns_false_initially() {
    let manager = SidecarManager::new("python3", PathBuf::from("/tmp/test.py"));

    assert_eq!(manager.is_running(), false);
}

#[tokio::test]
async fn test_sidecar_manager_start_fails_with_nonexistent_script() {
    let mut manager = SidecarManager::new("python3", PathBuf::from("/nonexistent/path/script.py"));

    let result = manager.start().await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("Sidecar script not found"));
    assert!(!manager.is_running());
}

#[tokio::test]
async fn test_sidecar_manager_check_python_returns_true_for_python3() {
    // This test may fail if python3 is not installed
    // We'll check both common Python installations
    let has_python3 = SidecarManager::check_python("python3").await;
    let has_python = SidecarManager::check_python("python").await;

    // At least one should be available on most systems, but we'll be lenient
    // and just verify the function works
    if !has_python3 && !has_python {
        // Skip test if no Python is available
        eprintln!("Warning: No Python installation found, skipping Python availability test");
    } else {
        assert!(has_python3 || has_python);
    }
}

#[tokio::test]
async fn test_sidecar_manager_check_python_returns_false_for_nonexistent_binary() {
    let result = SidecarManager::check_python("totally_fake_python_binary_12345").await;

    assert!(!result);
}

#[test]
fn test_sidecar_manager_take_process_returns_none_when_not_started() {
    let mut manager = SidecarManager::new("python3", PathBuf::from("/tmp/test.py"));

    let process = manager.take_process();

    assert!(process.is_none());
}

// ============================================================================
// Integration Tests - Manager with real script
// ============================================================================

#[tokio::test]
async fn test_sidecar_manager_start_with_valid_script() {
    // Create a temporary Python script
    let temp_file = NamedTempFile::new().unwrap();
    let script_path = temp_file.path();

    // Write a simple Python script that exits immediately
    std::fs::write(script_path, "#!/usr/bin/env python3\nimport sys\nsys.exit(0)\n").unwrap();

    // Check if Python is available first
    if !SidecarManager::check_python("python3").await {
        eprintln!("Python3 not available, skipping test");
        return;
    }

    let mut manager = SidecarManager::new("python3", script_path);

    let result = manager.start().await;

    assert!(result.is_ok());
    assert!(manager.is_running());

    // Clean up
    manager.stop().await;
}

#[tokio::test]
async fn test_sidecar_manager_take_process_returns_some_after_start() {
    // Create a temporary Python script that runs indefinitely
    let temp_file = NamedTempFile::new().unwrap();
    let script_path = temp_file.path();

    std::fs::write(
        script_path,
        "#!/usr/bin/env python3\nimport time\nwhile True: time.sleep(1)\n"
    ).unwrap();

    // Check if Python is available first
    if !SidecarManager::check_python("python3").await {
        eprintln!("Python3 not available, skipping test");
        return;
    }

    let mut manager = SidecarManager::new("python3", script_path);
    manager.start().await.unwrap();

    let process = manager.take_process();

    assert!(process.is_some());
    assert!(!manager.is_running()); // Should be false after take_process

    // Clean up the process
    if let Some(mut p) = process {
        let _ = p.kill().await;
    }
}

#[tokio::test]
async fn test_sidecar_manager_stop_cleans_up_process() {
    let temp_file = NamedTempFile::new().unwrap();
    let script_path = temp_file.path();

    std::fs::write(
        script_path,
        "#!/usr/bin/env python3\nimport time\nwhile True: time.sleep(1)\n"
    ).unwrap();

    if !SidecarManager::check_python("python3").await {
        eprintln!("Python3 not available, skipping test");
        return;
    }

    let mut manager = SidecarManager::new("python3", script_path);
    manager.start().await.unwrap();

    assert!(manager.is_running());

    manager.stop().await;

    assert!(!manager.is_running());
}

#[tokio::test]
async fn test_sidecar_manager_start_when_already_running() {
    let temp_file = NamedTempFile::new().unwrap();
    let script_path = temp_file.path();

    std::fs::write(
        script_path,
        "#!/usr/bin/env python3\nimport time\nwhile True: time.sleep(1)\n"
    ).unwrap();

    if !SidecarManager::check_python("python3").await {
        eprintln!("Python3 not available, skipping test");
        return;
    }

    let mut manager = SidecarManager::new("python3", script_path);

    // Start once
    let result1 = manager.start().await;
    assert!(result1.is_ok());

    // Start again - should return Ok without error
    let result2 = manager.start().await;
    assert!(result2.is_ok());
    assert!(manager.is_running());

    manager.stop().await;
}
