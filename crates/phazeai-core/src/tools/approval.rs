use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

/// Approval mode for tool execution
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolApprovalMode {
    /// Automatically approve all tools (dangerous, use with caution)
    AutoApprove,
    /// Always ask for user confirmation before executing
    #[default]
    AlwaysAsk,
    /// Ask once per tool, then auto-approve subsequent calls
    AskOnce,
}

/// Permission level classification for tools
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ToolPermission {
    /// Read-only operations (safe)
    ReadOnly,
    /// File write operations
    Write,
    /// Execute arbitrary commands
    Execute,
    /// Potentially destructive operations
    Destructive,
}

/// Manages tool approval and permission tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolApprovalManager {
    /// Current approval mode
    mode: ToolApprovalMode,
    /// Tools that have been approved (for AskOnce mode)
    approved_tools: HashSet<String>,
}

impl Default for ToolApprovalManager {
    fn default() -> Self {
        Self::new(ToolApprovalMode::AlwaysAsk)
    }
}

impl ToolApprovalManager {
    /// Create a new approval manager with the given mode
    pub fn new(mode: ToolApprovalMode) -> Self {
        Self {
            mode,
            approved_tools: HashSet::new(),
        }
    }

    /// Get the current approval mode
    pub fn mode(&self) -> &ToolApprovalMode {
        &self.mode
    }

    /// Set the approval mode
    pub fn set_mode(&mut self, mode: ToolApprovalMode) {
        // Clear approved tools when changing to AlwaysAsk mode
        if mode == ToolApprovalMode::AlwaysAsk {
            self.approved_tools.clear();
        }
        self.mode = mode;
    }

    /// Check if a tool needs user approval before execution
    pub fn needs_approval(&self, tool_name: &str, params: &Value) -> bool {
        match self.mode {
            ToolApprovalMode::AutoApprove => false,
            ToolApprovalMode::AlwaysAsk => {
                let permission = self.classify_tool(tool_name, params);
                // Only auto-approve truly safe read-only tools
                permission != ToolPermission::ReadOnly
            }
            ToolApprovalMode::AskOnce => {
                let permission = self.classify_tool(tool_name, params);
                // Auto-approve read-only tools
                if permission == ToolPermission::ReadOnly {
                    return false;
                }
                // Check if already approved
                !self.approved_tools.contains(tool_name)
            }
        }
    }

    /// Classify a tool's permission level based on name and parameters
    pub fn classify_tool(&self, tool_name: &str, params: &Value) -> ToolPermission {
        match tool_name {
            // Read-only tools
            "read_file" | "grep" | "glob" | "list_files" => ToolPermission::ReadOnly,

            // Write operations
            "write_file" | "edit_file" => ToolPermission::Write,

            // Bash commands need deeper inspection
            "bash" => {
                if let Some(command) = params.get("command").and_then(|v| v.as_str()) {
                    self.classify_bash_command(command)
                } else {
                    ToolPermission::Execute
                }
            }

            // Unknown tools default to Execute level
            _ => ToolPermission::Execute,
        }
    }

    /// Classify a bash command by analyzing its content for destructive patterns
    pub fn classify_bash_command(&self, command: &str) -> ToolPermission {
        let cmd_lower = command.to_lowercase();

        // Destructive patterns
        let destructive_patterns = [
            "rm -rf",
            "rm -fr",
            "rm -r",
            "rm -f",
            "mkfs",
            "dd if=",
            "format",
            "> /dev/",
            "git push --force",
            "git push -f",
            "git reset --hard",
            "git clean -fd",
            "git clean -df",
            "drop table",
            "drop database",
            "delete from",
            "truncate",
            "shutdown",
            "reboot",
            "init 0",
            "init 6",
            "kill -9",
            "killall",
            "pkill",
            ":(){:|:&};:", // fork bomb
            "chmod -r",
            "chown -r",
        ];

        for pattern in &destructive_patterns {
            if cmd_lower.contains(pattern) {
                return ToolPermission::Destructive;
            }
        }

        // Write-like operations (less destructive than above)
        let write_patterns = [
            "git commit",
            "git push",
            "npm install",
            "cargo install",
            "pip install",
            "apt install",
            "yum install",
            "brew install",
            "mkdir",
            "touch",
            "mv ",
            "cp ",
            ">>", // append
            "git add",
            "git rm",
        ];

        for pattern in &write_patterns {
            if cmd_lower.contains(pattern) {
                return ToolPermission::Write;
            }
        }

        // Check for output redirection (write)
        if cmd_lower.contains('>') && !cmd_lower.contains(">>") {
            return ToolPermission::Write;
        }

        // Read-only commands
        let readonly_patterns = [
            "ls ",
            "cat ",
            "head ",
            "tail ",
            "grep ",
            "find ",
            "echo ",
            "pwd",
            "which",
            "whereis",
            "whoami",
            "date",
            "uname",
            "git status",
            "git diff",
            "git log",
            "git show",
            "npm list",
            "cargo --version",
            "python --version",
        ];

        for pattern in &readonly_patterns {
            if cmd_lower.starts_with(pattern) || cmd_lower.contains(&format!(" {}", pattern)) {
                return ToolPermission::ReadOnly;
            }
        }

        // Default to Execute for unknown commands
        ToolPermission::Execute
    }

    /// Record that a tool has been approved (for AskOnce mode)
    pub fn record_approval(&mut self, tool_name: &str) {
        self.approved_tools.insert(tool_name.to_string());
    }

    /// Format a user-friendly approval prompt
    pub fn format_approval_prompt(&self, tool_name: &str, params: &Value) -> String {
        let permission = self.classify_tool(tool_name, params);
        let risk_level = match permission {
            ToolPermission::ReadOnly => "SAFE",
            ToolPermission::Write => "MODERATE",
            ToolPermission::Execute => "HIGH",
            ToolPermission::Destructive => "CRITICAL",
        };

        let mut prompt = format!(
            "\n=== Tool Approval Request ===\nTool: {}\nRisk Level: {}\n",
            tool_name, risk_level
        );

        // Add specific parameter details
        match tool_name {
            "read_file" => {
                if let Some(path) = params.get("path").and_then(|v| v.as_str()) {
                    prompt.push_str(&format!("Read file: {}\n", path));
                }
            }
            "write_file" => {
                if let Some(path) = params.get("path").and_then(|v| v.as_str()) {
                    prompt.push_str(&format!("Write to file: {}\n", path));
                    if let Some(content) = params.get("content").and_then(|v| v.as_str()) {
                        let preview = if content.len() > 100 {
                            format!("{}... ({} bytes)", &content[..100], content.len())
                        } else {
                            content.to_string()
                        };
                        prompt.push_str(&format!("Content preview: {}\n", preview));
                    }
                }
            }
            "edit_file" => {
                if let Some(path) = params.get("path").and_then(|v| v.as_str()) {
                    prompt.push_str(&format!("Edit file: {}\n", path));
                }
                if let Some(old) = params.get("old_string").and_then(|v| v.as_str()) {
                    let preview = if old.len() > 50 {
                        format!("{}...", &old[..50])
                    } else {
                        old.to_string()
                    };
                    prompt.push_str(&format!("Replace: {}\n", preview));
                }
            }
            "bash" => {
                if let Some(command) = params.get("command").and_then(|v| v.as_str()) {
                    prompt.push_str(&format!("Execute: {}\n", command));

                    // Add warning for destructive commands
                    if permission == ToolPermission::Destructive {
                        prompt.push_str("\nWARNING: This command may be destructive!\n");
                    }
                }
            }
            "grep" => {
                if let Some(pattern) = params.get("pattern").and_then(|v| v.as_str()) {
                    prompt.push_str(&format!("Search pattern: {}\n", pattern));
                }
                if let Some(path) = params.get("path").and_then(|v| v.as_str()) {
                    prompt.push_str(&format!("In path: {}\n", path));
                }
            }
            "glob" => {
                if let Some(pattern) = params.get("pattern").and_then(|v| v.as_str()) {
                    prompt.push_str(&format!("Find files matching: {}\n", pattern));
                }
            }
            "list_files" => {
                if let Some(path) = params.get("path").and_then(|v| v.as_str()) {
                    prompt.push_str(&format!("List directory: {}\n", path));
                }
            }
            _ => {
                prompt.push_str(&format!("Parameters: {}\n", params));
            }
        }

        prompt.push_str("\nApprove this action? (y/n): ");
        prompt
    }

    /// Clear all approved tools (reset AskOnce state)
    pub fn clear_approvals(&mut self) {
        self.approved_tools.clear();
    }

    /// Check if a specific tool has been approved
    pub fn is_approved(&self, tool_name: &str) -> bool {
        self.approved_tools.contains(tool_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_classify_readonly_tools() {
        let manager = ToolApprovalManager::default();

        assert_eq!(
            manager.classify_tool("read_file", &json!({})),
            ToolPermission::ReadOnly
        );
        assert_eq!(
            manager.classify_tool("grep", &json!({})),
            ToolPermission::ReadOnly
        );
        assert_eq!(
            manager.classify_tool("glob", &json!({})),
            ToolPermission::ReadOnly
        );
        assert_eq!(
            manager.classify_tool("list_files", &json!({})),
            ToolPermission::ReadOnly
        );
    }

    #[test]
    fn test_classify_write_tools() {
        let manager = ToolApprovalManager::default();

        assert_eq!(
            manager.classify_tool("write_file", &json!({})),
            ToolPermission::Write
        );
        assert_eq!(
            manager.classify_tool("edit_file", &json!({})),
            ToolPermission::Write
        );
    }

    #[test]
    fn test_classify_bash_commands() {
        let manager = ToolApprovalManager::default();

        // Read-only commands
        assert_eq!(
            manager.classify_bash_command("ls -la"),
            ToolPermission::ReadOnly
        );
        assert_eq!(
            manager.classify_bash_command("cat file.txt"),
            ToolPermission::ReadOnly
        );
        assert_eq!(
            manager.classify_bash_command("git status"),
            ToolPermission::ReadOnly
        );

        // Write commands
        assert_eq!(
            manager.classify_bash_command("git commit -m 'test'"),
            ToolPermission::Write
        );
        assert_eq!(
            manager.classify_bash_command("npm install express"),
            ToolPermission::Write
        );
        assert_eq!(
            manager.classify_bash_command("mkdir test"),
            ToolPermission::Write
        );

        // Destructive commands
        assert_eq!(
            manager.classify_bash_command("rm -rf /tmp/test"),
            ToolPermission::Destructive
        );
        assert_eq!(
            manager.classify_bash_command("git push --force"),
            ToolPermission::Destructive
        );
        assert_eq!(
            manager.classify_bash_command("DROP TABLE users"),
            ToolPermission::Destructive
        );
    }

    #[test]
    fn test_auto_approve_mode() {
        let manager = ToolApprovalManager::new(ToolApprovalMode::AutoApprove);

        // All tools should be auto-approved
        assert!(!manager.needs_approval("bash", &json!({"command": "rm -rf /"})));
        assert!(!manager.needs_approval("write_file", &json!({})));
        assert!(!manager.needs_approval("read_file", &json!({})));
    }

    #[test]
    fn test_always_ask_mode() {
        let manager = ToolApprovalManager::new(ToolApprovalMode::AlwaysAsk);

        // Read-only should not need approval
        assert!(!manager.needs_approval("read_file", &json!({})));
        assert!(!manager.needs_approval("grep", &json!({})));

        // Write and execute should need approval
        assert!(manager.needs_approval("write_file", &json!({})));
        assert!(manager.needs_approval("bash", &json!({"command": "ls"})));
    }

    #[test]
    fn test_ask_once_mode() {
        let mut manager = ToolApprovalManager::new(ToolApprovalMode::AskOnce);

        // First call needs approval
        assert!(manager.needs_approval("write_file", &json!({})));

        // Record approval
        manager.record_approval("write_file");

        // Second call should not need approval
        assert!(!manager.needs_approval("write_file", &json!({})));

        // Read-only never needs approval
        assert!(!manager.needs_approval("read_file", &json!({})));
    }

    #[test]
    fn test_format_approval_prompt() {
        let manager = ToolApprovalManager::default();

        let prompt =
            manager.format_approval_prompt("bash", &json!({"command": "rm -rf /tmp/test"}));

        assert!(prompt.contains("bash"));
        assert!(prompt.contains("CRITICAL"));
        assert!(prompt.contains("rm -rf /tmp/test"));
        assert!(prompt.contains("WARNING"));
    }

    #[test]
    fn test_clear_approvals() {
        let mut manager = ToolApprovalManager::new(ToolApprovalMode::AskOnce);

        manager.record_approval("write_file");
        assert!(manager.is_approved("write_file"));

        manager.clear_approvals();
        assert!(!manager.is_approved("write_file"));
    }
}
