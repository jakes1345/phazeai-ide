//! Open Folder moves the agent's tools to the new folder. Its own test binary:
//! it changes the process's current directory.

use phazeai_core::tools::Tool;
use phazeai_core::tools::{BashTool, ListFilesTool, WriteFileTool};
use serde_json::json;

#[tokio::test(flavor = "current_thread")]
async fn after_open_folder_the_agents_tools_work_in_the_new_folder() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    std::fs::write(second.path().join("in_second.txt"), "second").unwrap();
    // The IDE started in `first`.
    phazeai_ui::app::use_workspace_for_tools(first.path());

    // The user opens `second`.
    phazeai_ui::app::use_workspace_for_tools(second.path());

    // A relative path is a file of the new folder, and writing there works.
    let write = WriteFileTool;
    write
        .execute(json!({ "path": "notes.md", "content": "hi" }))
        .await
        .expect("a relative write lands in the opened folder");
    assert!(second.path().join("notes.md").exists());
    assert!(!first.path().join("notes.md").exists());

    // Listing without a path lists the new folder.
    let listing = ListFilesTool.execute(json!({})).await.expect("list");
    assert!(listing.to_string().contains("in_second.txt"), "{listing}");

    // The shell starts there.
    let pwd = BashTool::default()
        .execute(json!({ "command": "pwd" }))
        .await
        .expect("pwd");
    let canonical = second.path().canonicalize().unwrap();
    assert!(
        pwd.to_string().contains(&canonical.display().to_string()),
        "{pwd}"
    );

    // The old folder is outside the sandbox now.
    let outside = first.path().join("x.txt");
    assert!(write
        .execute(json!({ "path": outside.to_string_lossy(), "content": "no" }))
        .await
        .is_err());
}
