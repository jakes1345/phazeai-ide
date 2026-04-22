use phazeai_core::git::{FileState, GitOps};
use phazeai_core::project::{FileChangeKind, FileWatcher};
use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use tokio::time::{sleep, timeout, Duration};

// ============================================================================
// Git Test Helpers
// ============================================================================

/// Initialize a git repository in the given directory with proper config
fn init_git_repo(dir: &Path) {
    Command::new("git")
        .args(["init"])
        .current_dir(dir)
        .output()
        .expect("Failed to init git repo");

    Command::new("git")
        .args(["config", "user.email", "test@test.com"])
        .current_dir(dir)
        .output()
        .expect("Failed to set git user.email");

    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(dir)
        .output()
        .expect("Failed to set git user.name");
}

/// Create a file with content in the given directory
fn create_file(dir: &Path, name: &str, content: &str) {
    let file_path = dir.join(name);
    fs::write(&file_path, content).expect("Failed to create file");
}

// ============================================================================
// GitOps::find_root() Tests
// ============================================================================

#[test]
fn test_find_root_finds_git_directory() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    // Initialize git repo
    init_git_repo(repo_path);

    // Create a nested directory
    let nested = repo_path.join("src").join("modules");
    fs::create_dir_all(&nested).unwrap();

    // find_root should find the repo from nested directory
    let found = GitOps::find_root(&nested);
    assert!(found.is_some());
    assert_eq!(found.unwrap(), repo_path);
}

#[test]
fn test_find_root_returns_none_when_no_git() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    // Don't initialize git - just a regular directory
    let found = GitOps::find_root(repo_path);
    assert!(found.is_none());
}

#[test]
fn test_find_root_from_exact_git_location() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);

    // Should find root even when starting at root
    let found = GitOps::find_root(repo_path);
    assert!(found.is_some());
    assert_eq!(found.unwrap(), repo_path);
}

// ============================================================================
// GitOps::status() Tests
// ============================================================================

#[tokio::test]
async fn test_status_clean_repo() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);

    let git_ops = GitOps::new(repo_path);
    let status = git_ops.status().await.unwrap();

    assert!(status.is_clean);
    assert_eq!(status.files.len(), 0);
}

#[tokio::test]
async fn test_status_detects_untracked_files() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);
    create_file(repo_path, "new_file.txt", "Hello world");

    let git_ops = GitOps::new(repo_path);
    let status = git_ops.status().await.unwrap();

    assert!(!status.is_clean);
    assert_eq!(status.files.len(), 1);
    assert_eq!(status.files[0].path, "new_file.txt");
    assert_eq!(status.files[0].status, FileState::Untracked);
}

#[tokio::test]
async fn test_status_detects_modified_files() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);
    create_file(repo_path, "tracked.txt", "Initial content");

    // Add and commit the file first
    let git_ops = GitOps::new(repo_path);
    git_ops.add(&["tracked.txt"]).await.unwrap();
    git_ops.commit("Initial commit").await.unwrap();

    // Now modify the file
    create_file(repo_path, "tracked.txt", "Modified content");

    let status = git_ops.status().await.unwrap();

    assert!(!status.is_clean);
    assert_eq!(status.files.len(), 1);
    assert_eq!(status.files[0].path, "tracked.txt");
    assert_eq!(status.files[0].status, FileState::Modified);
}

#[tokio::test]
async fn test_status_multiple_files_different_states() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);
    create_file(repo_path, "tracked.txt", "Tracked");

    let git_ops = GitOps::new(repo_path);
    git_ops.add(&["tracked.txt"]).await.unwrap();
    git_ops.commit("Initial commit").await.unwrap();

    // Modify tracked file and add new untracked file
    create_file(repo_path, "tracked.txt", "Modified");
    create_file(repo_path, "untracked.txt", "New file");

    let status = git_ops.status().await.unwrap();

    assert!(!status.is_clean);
    assert_eq!(status.files.len(), 2);

    // Find files by name since order isn't guaranteed
    let modified = status
        .files
        .iter()
        .find(|f| f.path == "tracked.txt")
        .unwrap();
    let untracked = status
        .files
        .iter()
        .find(|f| f.path == "untracked.txt")
        .unwrap();

    assert_eq!(modified.status, FileState::Modified);
    assert_eq!(untracked.status, FileState::Untracked);
}

// ============================================================================
// GitOps::add() and GitOps::commit() Tests
// ============================================================================

#[tokio::test]
async fn test_add_and_commit_file() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);
    create_file(repo_path, "test.txt", "Test content");

    let git_ops = GitOps::new(repo_path);

    // Add the file
    git_ops.add(&["test.txt"]).await.unwrap();

    // Verify file is staged (status should show Added)
    let status = git_ops.status().await.unwrap();
    assert!(!status.is_clean);
    assert_eq!(status.files[0].status, FileState::Added);

    // Commit the file
    let result = git_ops.commit("Add test file").await;
    assert!(result.is_ok());

    // After commit, repo should be clean
    let status = git_ops.status().await.unwrap();
    assert!(status.is_clean);
}

#[tokio::test]
async fn test_add_multiple_files() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);
    create_file(repo_path, "file1.txt", "Content 1");
    create_file(repo_path, "file2.txt", "Content 2");
    create_file(repo_path, "file3.txt", "Content 3");

    let git_ops = GitOps::new(repo_path);

    // Add multiple files at once
    git_ops
        .add(&["file1.txt", "file2.txt", "file3.txt"])
        .await
        .unwrap();

    let status = git_ops.status().await.unwrap();
    assert_eq!(status.files.len(), 3);
    assert!(status.files.iter().all(|f| f.status == FileState::Added));
}

// ============================================================================
// GitOps::log() Tests
// ============================================================================

#[tokio::test]
async fn test_log_shows_commit_history() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);

    let git_ops = GitOps::new(repo_path);

    // Create multiple commits
    create_file(repo_path, "file1.txt", "First");
    git_ops.add(&["file1.txt"]).await.unwrap();
    git_ops.commit("First commit").await.unwrap();

    create_file(repo_path, "file2.txt", "Second");
    git_ops.add(&["file2.txt"]).await.unwrap();
    git_ops.commit("Second commit").await.unwrap();

    create_file(repo_path, "file3.txt", "Third");
    git_ops.add(&["file3.txt"]).await.unwrap();
    git_ops.commit("Third commit").await.unwrap();

    // Get log with last 2 commits
    let log = git_ops.log(2).await.unwrap();
    let lines: Vec<&str> = log.lines().collect();

    assert_eq!(lines.len(), 2);
    assert!(log.contains("Third commit"));
    assert!(log.contains("Second commit"));
    assert!(!log.contains("First commit")); // Should not include first commit
}

#[tokio::test]
async fn test_log_empty_repo() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);

    let git_ops = GitOps::new(repo_path);

    // Log on empty repo should fail or return empty
    let result = git_ops.log(5).await;
    // Either error or empty string is acceptable
    assert!(result.is_err() || result.unwrap().is_empty());
}

// ============================================================================
// GitOps::diff() Tests
// ============================================================================

#[tokio::test]
async fn test_diff_shows_unstaged_changes() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);
    create_file(repo_path, "test.txt", "Original content");

    let git_ops = GitOps::new(repo_path);
    git_ops.add(&["test.txt"]).await.unwrap();
    git_ops.commit("Initial commit").await.unwrap();

    // Modify the file
    create_file(repo_path, "test.txt", "Modified content");

    // Get unstaged diff
    let diff = git_ops.diff(false).await.unwrap();

    assert!(diff.contains("test.txt"));
    assert!(diff.contains("-Original content") || diff.contains("Original content"));
    assert!(diff.contains("+Modified content") || diff.contains("Modified content"));
}

#[tokio::test]
async fn test_diff_staged_shows_staged_changes() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);
    create_file(repo_path, "test.txt", "Original content");

    let git_ops = GitOps::new(repo_path);
    git_ops.add(&["test.txt"]).await.unwrap();
    git_ops.commit("Initial commit").await.unwrap();

    // Modify and stage the file
    create_file(repo_path, "test.txt", "Modified content");
    git_ops.add(&["test.txt"]).await.unwrap();

    // Get staged diff
    let diff = git_ops.diff(true).await.unwrap();

    assert!(diff.contains("test.txt"));
    assert!(diff.contains("-Original content") || diff.contains("Original content"));
    assert!(diff.contains("+Modified content") || diff.contains("Modified content"));
}

#[tokio::test]
async fn test_diff_unstaged_empty_when_staged() {
    let temp_dir = TempDir::new().unwrap();
    let repo_path = temp_dir.path();

    init_git_repo(repo_path);
    create_file(repo_path, "test.txt", "Original content");

    let git_ops = GitOps::new(repo_path);
    git_ops.add(&["test.txt"]).await.unwrap();
    git_ops.commit("Initial commit").await.unwrap();

    // Modify and stage the file
    create_file(repo_path, "test.txt", "Modified content");
    git_ops.add(&["test.txt"]).await.unwrap();

    // Unstaged diff should be empty since changes are staged
    let diff = git_ops.diff(false).await.unwrap();
    assert!(diff.is_empty());
}

// ============================================================================
// FileState Enum Tests
// ============================================================================

#[test]
fn test_file_state_equality() {
    assert_eq!(FileState::Modified, FileState::Modified);
    assert_eq!(FileState::Added, FileState::Added);
    assert_eq!(FileState::Deleted, FileState::Deleted);
    assert_eq!(FileState::Renamed, FileState::Renamed);
    assert_eq!(FileState::Untracked, FileState::Untracked);
    assert_eq!(FileState::Conflicted, FileState::Conflicted);

    assert_ne!(FileState::Modified, FileState::Added);
    assert_ne!(FileState::Deleted, FileState::Untracked);
}

#[test]
fn test_file_state_clone() {
    let state = FileState::Modified;
    let cloned = state.clone();
    assert_eq!(state, cloned);
}

// ============================================================================
// FileWatcher Tests
// ============================================================================

#[tokio::test]
async fn test_watcher_creates_successfully() {
    let temp_dir = TempDir::new().unwrap();
    let watch_path = temp_dir.path();

    let result = FileWatcher::watch(watch_path);
    assert!(result.is_ok());

    let (_watcher, _rx) = result.unwrap();
    // Watcher created successfully
}

#[tokio::test]
async fn test_watcher_detects_file_creation() {
    let temp_dir = TempDir::new().unwrap();
    let watch_path = temp_dir.path();

    let (_watcher, mut rx) = FileWatcher::watch(watch_path).unwrap();

    // Give watcher time to initialize
    sleep(Duration::from_millis(100)).await;

    // Create a file
    create_file(watch_path, "new_file.txt", "Hello");

    // Wait for event with timeout
    let event = timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("Timeout waiting for event")
        .expect("No event received");

    assert_eq!(event.kind, FileChangeKind::Created);
    assert!(event.path.ends_with("new_file.txt"));
}

#[tokio::test]
async fn test_watcher_detects_file_modification() {
    let temp_dir = TempDir::new().unwrap();
    let watch_path = temp_dir.path();

    // Create file before starting watcher
    create_file(watch_path, "test_file.txt", "Initial");

    let (_watcher, mut rx) = FileWatcher::watch(watch_path).unwrap();

    // Give watcher time to initialize
    sleep(Duration::from_millis(100)).await;

    // Modify the file
    create_file(watch_path, "test_file.txt", "Modified");

    // Wait for event with timeout
    let event = timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("Timeout waiting for event")
        .expect("No event received");

    assert_eq!(event.kind, FileChangeKind::Modified);
    assert!(event.path.ends_with("test_file.txt"));
}

#[tokio::test]
async fn test_watcher_detects_file_deletion() {
    let temp_dir = TempDir::new().unwrap();
    let watch_path = temp_dir.path();

    // Create file before starting watcher
    let file_path = watch_path.join("delete_me.txt");
    create_file(watch_path, "delete_me.txt", "Content");

    let (_watcher, mut rx) = FileWatcher::watch(watch_path).unwrap();

    // Give watcher time to initialize
    sleep(Duration::from_millis(100)).await;

    // Delete the file
    fs::remove_file(&file_path).expect("Failed to delete file");

    // Wait for event with timeout
    let event = timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("Timeout waiting for event")
        .expect("No event received");

    assert_eq!(event.kind, FileChangeKind::Removed);
    assert!(event.path.ends_with("delete_me.txt"));
}

#[tokio::test]
async fn test_watcher_multiple_events() {
    let temp_dir = TempDir::new().unwrap();
    let watch_path = temp_dir.path();

    let (_watcher, mut rx) = FileWatcher::watch(watch_path).unwrap();

    // Give watcher time to initialize
    sleep(Duration::from_millis(100)).await;

    // Create multiple files
    create_file(watch_path, "file1.txt", "Content 1");
    sleep(Duration::from_millis(50)).await;
    create_file(watch_path, "file2.txt", "Content 2");
    sleep(Duration::from_millis(50)).await;
    create_file(watch_path, "file3.txt", "Content 3");

    // Collect events (may get multiple events per file due to OS behavior)
    let mut events = Vec::new();
    for _ in 0..5 {
        if let Ok(Some(event)) = timeout(Duration::from_millis(200), rx.recv()).await {
            events.push(event);
        } else {
            break;
        }
    }

    // Should have received at least some events
    assert!(!events.is_empty());

    // Check that we got events for our files
    let file_names: Vec<String> = events
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().to_string())
        .collect();

    assert!(
        file_names.contains(&"file1.txt".to_string())
            || file_names.contains(&"file2.txt".to_string())
            || file_names.contains(&"file3.txt".to_string())
    );
}

// ============================================================================
// FileChangeKind Enum Tests
// ============================================================================

#[test]
fn test_file_change_kind_equality() {
    assert_eq!(FileChangeKind::Created, FileChangeKind::Created);
    assert_eq!(FileChangeKind::Modified, FileChangeKind::Modified);
    assert_eq!(FileChangeKind::Removed, FileChangeKind::Removed);

    assert_ne!(FileChangeKind::Created, FileChangeKind::Modified);
    assert_ne!(FileChangeKind::Modified, FileChangeKind::Removed);
}

#[test]
fn test_file_change_kind_clone() {
    let kind = FileChangeKind::Created;
    let cloned = kind.clone();
    assert_eq!(kind, cloned);
}
