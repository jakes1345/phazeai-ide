use phazeai_sidecar::SidecarClient;
use serde_json::json;
use std::sync::Arc;
use std::{fs, process::Stdio};
use tempfile::tempdir;
use tokio::process::Command;

#[tokio::test]
async fn concurrent_calls_handle_out_of_order_sidecar_responses() {
    let temp_dir = tempdir().expect("create temp dir");
    let script_path = temp_dir.path().join("out_of_order_sidecar.py");
    fs::write(
        &script_path,
        r#"
import json
import select
import sys

def send_response(request):
    response = {
        "jsonrpc": "2.0",
        "id": request["id"],
        "result": {"method": request["method"]},
    }
    sys.stdout.write(json.dumps(response) + "\n")
    sys.stdout.flush()

first_line = sys.stdin.readline()
if not first_line:
    sys.exit(1)
first_request = json.loads(first_line)

# If a second request appears quickly, force out-of-order responses.
ready, _, _ = select.select([sys.stdin], [], [], 0.2)
if ready:
    second_line = sys.stdin.readline()
    if not second_line:
        sys.exit(1)
    second_request = json.loads(second_line)
    send_response(second_request)
    send_response(first_request)
else:
    send_response(first_request)
    second_line = sys.stdin.readline()
    if second_line:
        second_request = json.loads(second_line)
        send_response(second_request)
"#,
    )
    .expect("write test sidecar");

    let process = Command::new("python3")
        .arg(&script_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn sidecar fixture");

    let client = Arc::new(SidecarClient::from_process(process).expect("construct sidecar client"));
    let first = {
        let client = client.clone();
        tokio::spawn(async move { client.call("first", Some(json!({ "n": 1 }))).await })
    };
    let second = {
        let client = client.clone();
        tokio::spawn(async move { client.call("second", Some(json!({ "n": 2 }))).await })
    };

    let (first_result, second_result) = tokio::join!(first, second);
    let first_result = first_result.expect("first call join");
    let second_result = second_result.expect("second call join");

    assert!(first_result.is_ok(), "first call failed: {first_result:?}");
    assert!(
        second_result.is_ok(),
        "second call failed: {second_result:?}"
    );
}
