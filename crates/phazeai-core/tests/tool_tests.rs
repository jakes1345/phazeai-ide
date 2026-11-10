use phazeai_core::tools::{
    BashTool, EditTool, GlobTool, GrepTool, ListFilesTool, ReadFileTool, Tool, WriteFileTool,
};
use serde_json::json;
use std::path::PathBuf;
use tempfile::TempDir;

// Helper function to create test directory with files
async fn create_test_files(temp_dir: &TempDir) -> PathBuf {
    let test_dir = temp_dir.path().to_path_buf();

    // Create some test files
    tokio::fs::write(
        test_dir.join("test.txt"),
        "line 1\nline 2\nline 3\nline 4\nline 5",
    )
    .await
    .unwrap();

    tokio::fs::write(
        test_dir.join("hello.txt"),
        "Hello World\nFoo Bar\nHello Again",
    )
    .await
    .unwrap();

    tokio::fs::write(
        test_dir.join("test.rs"),
        "fn main() {\n    println!(\"Hello\");\n}",
    )
    .await
    .unwrap();

    // Create subdirectory
    tokio::fs::create_dir(test_dir.join("subdir"))
        .await
        .unwrap();

    tokio::fs::write(
        test_dir.join("subdir/nested.txt"),
        "nested content\nmore lines",
    )
    .await
    .unwrap();

    test_dir
}

// ============================================================================
// ReadFileTool Tests
// ============================================================================

#[tokio::test]
async fn test_read_file_full() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = ReadFileTool;

    let result = tool
        .execute(json!({
            "path": test_dir.join("test.txt").to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    assert_eq!(
        result["path"],
        test_dir.join("test.txt").to_string_lossy().to_string()
    );
    assert_eq!(result["total_lines"], 5);
    assert_eq!(result["lines_shown"], 5);
    assert!(result["content"].as_str().unwrap().contains("line 1"));
    assert!(result["content"].as_str().unwrap().contains("line 5"));
}

#[tokio::test]
async fn test_read_file_with_offset() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = ReadFileTool;

    let result = tool
        .execute(json!({
            "path": test_dir.join("test.txt").to_string_lossy().to_string(),
            "offset": 3
        }))
        .await
        .unwrap();

    assert_eq!(result["lines_shown"], 3);
    let content = result["content"].as_str().unwrap();
    assert!(content.contains("line 3"));
    assert!(content.contains("line 5"));
    assert!(!content.contains("line 1"));
}

#[tokio::test]
async fn test_read_file_with_limit() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = ReadFileTool;

    let result = tool
        .execute(json!({
            "path": test_dir.join("test.txt").to_string_lossy().to_string(),
            "limit": 2
        }))
        .await
        .unwrap();

    assert_eq!(result["lines_shown"], 2);
    let content = result["content"].as_str().unwrap();
    assert!(content.contains("line 1"));
    assert!(content.contains("line 2"));
    assert!(!content.contains("line 3"));
}

#[tokio::test]
async fn test_read_file_with_offset_and_limit() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = ReadFileTool;

    let result = tool
        .execute(json!({
            "path": test_dir.join("test.txt").to_string_lossy().to_string(),
            "offset": 2,
            "limit": 2
        }))
        .await
        .unwrap();

    assert_eq!(result["lines_shown"], 2);
    let content = result["content"].as_str().unwrap();
    assert!(content.contains("line 2"));
    assert!(content.contains("line 3"));
    assert!(!content.contains("line 1"));
    assert!(!content.contains("line 4"));
}

#[tokio::test]
async fn test_read_file_line_numbers() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = ReadFileTool;

    let result = tool
        .execute(json!({
            "path": test_dir.join("test.txt").to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    let content = result["content"].as_str().unwrap();
    // Each line starts with its number and a tab, without padding.
    assert!(content.starts_with("1\tline 1\n"));
    assert!(content.contains("\n5\tline 5"));
}

#[tokio::test]
async fn test_read_file_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = temp_dir.path();
    let tool = ReadFileTool;

    let result = tool
        .execute(json!({
            "path": test_dir.join("nonexistent.txt").to_string_lossy().to_string()
        }))
        .await;

    assert!(result.is_err());
}

// ============================================================================
// WriteFileTool Tests
// ============================================================================

#[tokio::test]
async fn test_write_file_new() {
    let temp_dir = TempDir::new().unwrap();
    let test_path = temp_dir.path().join("new_file.txt");
    let tool = WriteFileTool;

    let content = "Hello, World!";
    let result = tool
        .execute(json!({
            "path": test_path.to_string_lossy().to_string(),
            "content": content
        }))
        .await
        .unwrap();

    assert_eq!(result["success"], true);
    assert_eq!(result["bytes_written"], content.len());

    let written_content = tokio::fs::read_to_string(&test_path).await.unwrap();
    assert_eq!(written_content, content);
}

#[tokio::test]
async fn test_write_file_overwrite() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let test_path = test_dir.join("test.txt");
    let tool = WriteFileTool;

    let new_content = "Overwritten content";
    let result = tool
        .execute(json!({
            "path": test_path.to_string_lossy().to_string(),
            "content": new_content
        }))
        .await
        .unwrap();

    assert_eq!(result["success"], true);

    let written_content = tokio::fs::read_to_string(&test_path).await.unwrap();
    assert_eq!(written_content, new_content);
}

#[tokio::test]
async fn test_write_file_create_parent_dirs() {
    let temp_dir = TempDir::new().unwrap();
    let nested_path = temp_dir.path().join("a/b/c/test.txt");
    let tool = WriteFileTool;

    let content = "Nested file";
    let result = tool
        .execute(json!({
            "path": nested_path.to_string_lossy().to_string(),
            "content": content
        }))
        .await
        .unwrap();

    assert_eq!(result["success"], true);

    assert!(nested_path.exists());
    let written_content = tokio::fs::read_to_string(&nested_path).await.unwrap();
    assert_eq!(written_content, content);
}

#[tokio::test]
async fn test_write_file_empty() {
    let temp_dir = TempDir::new().unwrap();
    let test_path = temp_dir.path().join("empty.txt");
    let tool = WriteFileTool;

    let result = tool
        .execute(json!({
            "path": test_path.to_string_lossy().to_string(),
            "content": ""
        }))
        .await
        .unwrap();

    assert_eq!(result["success"], true);
    assert_eq!(result["bytes_written"], 0);

    let written_content = tokio::fs::read_to_string(&test_path).await.unwrap();
    assert_eq!(written_content, "");
}

// ============================================================================
// EditTool Tests
// ============================================================================

#[tokio::test]
async fn test_edit_replace_unique() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let test_path = test_dir.join("hello.txt");
    let tool = EditTool;

    let result = tool
        .execute(json!({
            "path": test_path.to_string_lossy().to_string(),
            "old_text": "World",
            "new_text": "Universe"
        }))
        .await
        .unwrap();

    assert_eq!(result["success"], true);
    assert_eq!(result["replacements"], 1);

    let content = tokio::fs::read_to_string(&test_path).await.unwrap();
    assert!(content.contains("Hello Universe"));
    assert!(!content.contains("Hello World"));
}

#[tokio::test]
async fn test_edit_replace_all() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let test_path = test_dir.join("hello.txt");
    let tool = EditTool;

    let result = tool
        .execute(json!({
            "path": test_path.to_string_lossy().to_string(),
            "old_text": "Hello",
            "new_text": "Hi",
            "replace_all": true
        }))
        .await
        .unwrap();

    assert_eq!(result["success"], true);
    assert_eq!(result["replacements"], 2);

    let content = tokio::fs::read_to_string(&test_path).await.unwrap();
    assert!(content.contains("Hi World"));
    assert!(content.contains("Hi Again"));
    assert!(!content.contains("Hello"));
}

#[tokio::test]
async fn test_edit_non_unique_error() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let test_path = test_dir.join("hello.txt");
    let tool = EditTool;

    // Try to replace "Hello" without replace_all when it appears twice
    let result = tool
        .execute(json!({
            "path": test_path.to_string_lossy().to_string(),
            "old_text": "Hello",
            "new_text": "Hi"
        }))
        .await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("matches 2 times"));
}

#[tokio::test]
async fn test_edit_missing_old_text() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let test_path = test_dir.join("hello.txt");
    let tool = EditTool;

    let result = tool
        .execute(json!({
            "path": test_path.to_string_lossy().to_string(),
            "old_text": "NonexistentText",
            "new_text": "Replacement"
        }))
        .await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("not found"));
}

// ============================================================================
// BashTool Tests
// ============================================================================

#[tokio::test]
async fn test_bash_simple_command() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = temp_dir.path();
    let tool = BashTool::new(test_dir.to_path_buf());

    let result = tool
        .execute(json!({
            "command": "echo 'Hello Bash'"
        }))
        .await
        .unwrap();

    assert_eq!(result["success"], true);
    assert_eq!(result["exit_code"], 0);
    assert!(result["stdout"].as_str().unwrap().contains("Hello Bash"));
}

#[tokio::test]
async fn test_bash_persistent_cwd() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = temp_dir.path();
    let tool = BashTool::new(test_dir.to_path_buf());

    // Create a test directory
    tokio::fs::create_dir(test_dir.join("testdir"))
        .await
        .unwrap();

    // Change directory in one command and check we're still there
    let result = tool
        .execute(json!({
            "command": "cd testdir && pwd"
        }))
        .await
        .unwrap();

    assert_eq!(result["success"], true);
    assert!(result["stdout"].as_str().unwrap().contains("testdir"));
}

#[tokio::test]
async fn test_bash_timeout() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = temp_dir.path();
    let tool = BashTool::new(test_dir.to_path_buf());

    let result = tool
        .execute(json!({
            "command": "sleep 10",
            "timeout_secs": 1
        }))
        .await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("timed out"));
}

#[tokio::test]
async fn test_bash_output_truncation() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = temp_dir.path();
    let tool = BashTool::new(test_dir.to_path_buf());

    // Generate output longer than 30000 chars
    let result = tool
        .execute(json!({
            "command": "for i in {1..5000}; do echo 'This is a long line of text for testing truncation'; done"
        }))
        .await
        .unwrap();

    let stdout = result["stdout"].as_str().unwrap();
    // Should be cut in the middle, keeping the last line
    assert!(stdout.contains(" bytes omitted ...]"));
    assert!(stdout.ends_with("This is a long line of text for testing truncation"));
}

#[tokio::test]
async fn test_bash_error_command() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = temp_dir.path();
    let tool = BashTool::new(test_dir.to_path_buf());

    let result = tool
        .execute(json!({
            "command": "nonexistent_command_xyz"
        }))
        .await
        .unwrap();

    assert_eq!(result["success"], false);
    assert_ne!(result["exit_code"], 0);
}

// ============================================================================
// GrepTool Tests
// ============================================================================

#[tokio::test]
async fn test_grep_regex_search() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = GrepTool;

    let result = tool
        .execute(json!({
            "pattern": "line [0-9]",
            "path": test_dir.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    assert_eq!(result["pattern"], "line [0-9]");
    let matches = result["matches"].as_array().unwrap();
    assert!(!matches.is_empty());
    assert!(matches
        .iter()
        .any(|m| m["content"].as_str().unwrap().contains("line 1")));
}

#[tokio::test]
async fn test_grep_case_sensitivity() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = GrepTool;

    // Case sensitive search (default)
    let result = tool
        .execute(json!({
            "pattern": "hello",
            "path": test_dir.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    let matches = result["matches"].as_array().unwrap();
    // Should not match "Hello" with capital H
    assert_eq!(matches.len(), 0);

    // Case insensitive pattern
    let result2 = tool
        .execute(json!({
            "pattern": "(?i)hello",
            "path": test_dir.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    let matches2 = result2["matches"].as_array().unwrap();
    // Should match "Hello" with case insensitive flag
    assert!(!matches2.is_empty());
}

#[tokio::test]
async fn test_grep_limit_matches() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = GrepTool;

    let result = tool
        .execute(json!({
            "pattern": ".",
            "path": test_dir.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    let matches = result["matches"].as_array().unwrap();
    // Should limit to 500 matches max (as per grep.rs line 105)
    assert!(matches.len() <= 500);
}

#[tokio::test]
async fn test_grep_single_file() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = GrepTool;

    let result = tool
        .execute(json!({
            "pattern": "Hello",
            "path": test_dir.join("hello.txt").to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    let matches = result["matches"].as_array().unwrap();
    assert!(!matches.is_empty());
    assert!(matches[0]["file"].as_str().unwrap().contains("hello.txt"));
}

// ============================================================================
// GlobTool Tests
// ============================================================================

#[tokio::test]
async fn test_glob_pattern_matching() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = GlobTool;

    let result = tool
        .execute(json!({
            "pattern": "*.txt",
            "path": test_dir.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    assert_eq!(result["pattern"], "*.txt");
    let matches = result["matches"].as_array().unwrap();
    assert!(matches.len() >= 2); // At least test.txt and hello.txt

    let match_strings: Vec<String> = matches
        .iter()
        .map(|m| m.as_str().unwrap().to_string())
        .collect();
    assert!(match_strings.iter().any(|s| s.contains("test.txt")));
    assert!(match_strings.iter().any(|s| s.contains("hello.txt")));
}

#[tokio::test]
async fn test_glob_recursive_pattern() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = GlobTool;

    let result = tool
        .execute(json!({
            "pattern": "**/*.txt",
            "path": test_dir.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    let matches = result["matches"].as_array().unwrap();
    // Should include nested.txt in subdir
    let match_strings: Vec<String> = matches
        .iter()
        .map(|m| m.as_str().unwrap().to_string())
        .collect();
    assert!(match_strings.iter().any(|s| s.contains("nested.txt")));
}

#[tokio::test]
async fn test_glob_no_matches() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = GlobTool;

    let result = tool
        .execute(json!({
            "pattern": "*.nonexistent",
            "path": test_dir.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    assert_eq!(result["count"], 0);
    let matches = result["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 0);
}

#[tokio::test]
async fn test_glob_specific_extension() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = GlobTool;

    let result = tool
        .execute(json!({
            "pattern": "*.rs",
            "path": test_dir.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    let matches = result["matches"].as_array().unwrap();
    assert!(!matches.is_empty());
    let match_strings: Vec<String> = matches
        .iter()
        .map(|m| m.as_str().unwrap().to_string())
        .collect();
    assert!(match_strings.iter().any(|s| s.contains("test.rs")));
}

// ============================================================================
// ListFilesTool Tests
// ============================================================================

#[tokio::test]
async fn test_list_files_non_recursive() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = ListFilesTool;

    let result = tool
        .execute(json!({
            "path": test_dir.to_string_lossy().to_string(),
            "recursive": false
        }))
        .await
        .unwrap();

    let files = result["files"].as_array().unwrap();

    // Should include top-level files and directories
    let file_names: Vec<String> = files
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect();

    assert!(file_names.iter().any(|n| n.contains("test.txt")));
    assert!(file_names.iter().any(|n| n.contains("subdir")));

    // Should NOT include nested files
    assert!(!file_names.iter().any(|n| n.contains("nested.txt")));
}

#[tokio::test]
async fn test_list_files_recursive() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = ListFilesTool;

    let result = tool
        .execute(json!({
            "path": test_dir.to_string_lossy().to_string(),
            "recursive": true
        }))
        .await
        .unwrap();

    let files = result["files"].as_array().unwrap();

    let file_names: Vec<String> = files
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect();

    // Should include both top-level and nested files
    assert!(file_names.iter().any(|n| n.contains("test.txt")));
    assert!(file_names.iter().any(|n| n.contains("nested.txt")));
}

#[tokio::test]
async fn test_list_files_distinguishes_types() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = ListFilesTool;

    let result = tool
        .execute(json!({
            "path": test_dir.to_string_lossy().to_string(),
            "recursive": false
        }))
        .await
        .unwrap();

    let files = result["files"].as_array().unwrap();

    // Check that we have both file and directory types
    let has_file = files.iter().any(|f| f["type"] == "file");
    let has_dir = files.iter().any(|f| f["type"] == "directory");

    assert!(has_file);
    assert!(has_dir);
}

#[tokio::test]
async fn test_list_files_default_recursive() {
    let temp_dir = TempDir::new().unwrap();
    let test_dir = create_test_files(&temp_dir).await;
    let tool = ListFilesTool;

    // Without recursive parameter (should default to false)
    let result = tool
        .execute(json!({
            "path": test_dir.to_string_lossy().to_string()
        }))
        .await
        .unwrap();

    let files = result["files"].as_array().unwrap();
    let file_names: Vec<String> = files
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect();

    // Should not include nested files with default recursive=false
    assert!(!file_names.iter().any(|n| n.contains("nested.txt")));
}

// ============================================================================
// Caps that keep one result from filling the context
// ============================================================================

#[tokio::test]
async fn a_read_without_a_limit_stops_at_2000_lines_and_says_where() {
    let temp_dir = TempDir::new().unwrap();
    let file = temp_dir.path().join("big.txt");
    let text: Vec<String> = (1..=2500).map(|i| format!("row {i}")).collect();
    tokio::fs::write(&file, text.join("\n")).await.unwrap();

    let out = ReadFileTool
        .execute(json!({ "path": file.to_string_lossy() }))
        .await
        .unwrap();
    assert_eq!(out["lines_shown"], 2000);
    assert_eq!(out["total_lines"], 2500);
    assert_eq!(out["first_line"], 1);
    assert!(out["content"].as_str().unwrap().ends_with("2000\trow 2000"));

    let told = phazeai_core::tools::render::for_model("read_file", &out);
    assert!(told.ends_with("[lines 1-2000 of 2500; read on with offset 2001]"));

    // An explicit limit is honoured past the default.
    let all = ReadFileTool
        .execute(json!({ "path": file.to_string_lossy(), "limit": 2500 }))
        .await
        .unwrap();
    assert_eq!(all["lines_shown"], 2500);
}

#[tokio::test]
async fn a_read_stops_at_50_kib_and_cuts_a_very_long_line() {
    let temp_dir = TempDir::new().unwrap();
    let file = temp_dir.path().join("wide.txt");
    let line = "x".repeat(5000);
    tokio::fs::write(&file, vec![line.as_str(); 100].join("\n"))
        .await
        .unwrap();

    let out = ReadFileTool
        .execute(json!({ "path": file.to_string_lossy() }))
        .await
        .unwrap();
    let content = out["content"].as_str().unwrap();
    assert!(content.len() <= 50 * 1024, "{}", content.len());
    assert_eq!(out["long_lines_cut"], true);
    // Each kept line is its number, a tab and 2000 characters.
    assert_eq!(content.lines().next().unwrap().len(), "1\t".len() + 2000);
    let shown = out["lines_shown"].as_u64().unwrap();
    assert!((20..100).contains(&shown), "{shown}");
}

#[tokio::test]
async fn grep_stops_at_100_hits_and_cuts_long_lines() {
    let temp_dir = TempDir::new().unwrap();
    let file = temp_dir.path().join("many.txt");
    let mut lines: Vec<String> = (0..150).map(|i| format!("hit {i}")).collect();
    lines[0] = format!("hit {}", "y".repeat(1000));
    tokio::fs::write(&file, lines.join("\n")).await.unwrap();

    let out = GrepTool
        .execute(json!({ "pattern": "hit", "path": temp_dir.path().to_string_lossy() }))
        .await
        .unwrap();
    assert_eq!(out["count"], 100);
    assert_eq!(out["truncated"], true);
    let first = out["matches"][0]["content"].as_str().unwrap();
    assert_eq!(first.chars().count(), 301, "300 characters and an ellipsis");

    let told = phazeai_core::tools::render::for_model("grep", &out);
    assert!(told.ends_with("[stopped at 100; narrow the search to see the rest]"));
}

#[tokio::test]
async fn searches_and_listings_skip_the_git_store() {
    let temp_dir = TempDir::new().unwrap();
    let root = temp_dir.path();
    tokio::fs::create_dir_all(root.join(".git/objects"))
        .await
        .unwrap();
    tokio::fs::write(root.join(".git/config"), "needle in git")
        .await
        .unwrap();
    tokio::fs::write(root.join("a.txt"), "needle in tree")
        .await
        .unwrap();
    tokio::fs::write(root.join(".env.example"), "hidden but wanted")
        .await
        .unwrap();
    let path = root.to_string_lossy();

    let grep = GrepTool
        .execute(json!({ "pattern": "needle", "path": path }))
        .await
        .unwrap();
    assert_eq!(grep["count"], 1, "{grep}");

    let listing = ListFilesTool
        .execute(json!({ "path": path, "recursive": true }))
        .await
        .unwrap();
    let names: Vec<&str> = listing["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert!(!names.iter().any(|n| n.starts_with(".git")), "{names:?}");
    // Other hidden files still show.
    assert!(names.contains(&".env.example"), "{names:?}");

    let globbed = GlobTool
        .execute(json!({ "pattern": "**/*", "path": path }))
        .await
        .unwrap();
    let found = globbed["matches"].to_string();
    assert!(!found.contains(".git/"), "{found}");
    assert!(found.contains("a.txt"), "{found}");
}
