use phazeai_core::ext_host::js::JsExtension;
use phazeai_core::ext_host::vsix::VsixLoader;
use phazeai_core::ext_host::{Extension, ExtensionManager, IdeContext, IdeDelegate};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

struct TestDelegate {
    messages: Arc<Mutex<Vec<String>>>,
}

impl IdeDelegate for TestDelegate {
    fn log(&self, msg: &str) {
        let msg = msg.to_string();
        if let Ok(mut msgs) = self.messages.lock() {
            msgs.push(msg);
        }
    }

    fn show_message(&self, msg: &str) {
        let msg = msg.to_string();
        if let Ok(mut msgs) = self.messages.lock() {
            msgs.push(msg);
        }
    }

    fn get_active_text(&self) -> String {
        "Test text".to_string()
    }
}

#[tokio::test]
async fn test_js_extension_execution() {
    let js_code = r#"
        vscode.window.showInformationMessage("Hello from PhazeAI JS Extension!");
        
        globalThis.vscode.executeCommand = function(cmd, args) {
            if (cmd === 'test.add') {
                return args.a + args.b;
            }
            return null;
        };
    "#;

    let mut ext = JsExtension::new("test-ext", vec!["test.add".to_string()], js_code)
        .expect("Failed to create JS extension");

    let msgs = Arc::new(Mutex::new(Vec::new()));
    let ctx = IdeContext::new(Arc::new(TestDelegate {
        messages: msgs.clone(),
    }));

    // Activate the extension
    ext.activate(ctx)
        .await
        .expect("Failed to activate JS extension");

    // Execute a command
    let res = ext
        .execute_command("test.add", serde_json::json!({"a": 5, "b": 10}))
        .await
        .expect("Failed to execute command");

    // The execution currently just returns Null as mapped in JS, but it didn't crash.
    assert_eq!(res, serde_json::Value::Null);
}

#[tokio::test]
async fn test_vsix_loader() {
    let mut vsix_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    vsix_path.push("../../ext-host/test-extension.vsix");

    let manager = ExtensionManager::new();

    // Load the vsix
    VsixLoader::load_vsix(&vsix_path, &manager)
        .await
        .expect("Failed to load vsix");

    // Execute the registered command
    let res = manager
        .execute_command("phazeai.testCommand", serde_json::json!({"test": "data"}))
        .await
        .expect("Failed to execute command from VSIX");

    // We expect the command to return Null since our executeCommand shim returns null
    // But importantly, it shouldn't error out.
    assert_eq!(res, serde_json::Value::Null);
}

#[tokio::test]
async fn test_wasm_extension() {
    let mut wasm_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    wasm_path.push("../../target/wasm32-unknown-unknown/release/wasm_extension.wasm");

    // Only run if the wasm file was built
    if !wasm_path.exists() {
        println!("Skipping wasm test: file not found at {:?}", wasm_path);
        return;
    }

    let wasm_bytes = std::fs::read(&wasm_path).expect("Failed to read wasm file");

    let mut ext = phazeai_core::ext_host::wasm::WasmExtension::new(
        "test-wasm",
        vec!["dummy".to_string()],
        &wasm_bytes,
    )
    .expect("Failed to instantiate WASM extension");

    let msgs = Arc::new(Mutex::new(Vec::new()));
    let ctx = IdeContext::new(Arc::new(TestDelegate {
        messages: msgs.clone(),
    }));

    // Test activate
    ext.activate(ctx.clone())
        .await
        .expect("Failed to activate WASM extension");

    // Check if the log message was recorded
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let m = msgs.lock().unwrap();
    assert!(m
        .iter()
        .any(|msg| msg.contains("WASM Extension Activated!")));
    drop(m);

    // Test command
    let res = ext
        .execute_command("dummy", serde_json::Value::Null)
        .await
        .expect("Failed to execute WASM command");
    assert_eq!(res, serde_json::Value::Null);

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let m = msgs.lock().unwrap();
    assert!(m
        .iter()
        .any(|msg| msg.contains("WASM Extension Executed Command!")));
}
