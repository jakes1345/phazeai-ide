use crate::error::PhazeError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Metadata about a saved conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationMetadata {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: usize,
    pub model: String,
    pub project_dir: Option<String>,
}

/// A complete saved conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedConversation {
    pub metadata: ConversationMetadata,
    pub messages: Vec<SavedMessage>,
    pub system_prompt: Option<String>,
}

/// A simplified message for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedMessage {
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub tool_name: Option<String>,
}

/// Index of all conversations
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ConversationIndex {
    conversations: Vec<ConversationMetadata>,
}

/// Manages persistence of conversations to disk
pub struct ConversationStore {
    base_dir: PathBuf,
}

impl ConversationStore {
    /// Create a new conversation store using the default directory (~/.phazeai/conversations/)
    pub fn new() -> Result<Self, PhazeError> {
        let base_dir = Self::get_conversations_dir()?;
        Self::with_dir(base_dir)
    }

    /// Create a conversation store with a custom directory (useful for testing)
    pub fn with_dir(base_dir: PathBuf) -> Result<Self, PhazeError> {
        // Ensure directory exists
        fs::create_dir_all(&base_dir).map_err(|e| {
            PhazeError::Config(format!("Failed to create conversations directory: {}", e))
        })?;

        Ok(Self { base_dir })
    }

    /// Get the conversations directory path
    fn get_conversations_dir() -> Result<PathBuf, PhazeError> {
        let home = dirs::home_dir()
            .ok_or_else(|| PhazeError::Config("Could not determine home directory".to_string()))?;

        Ok(home.join(".phazeai").join("conversations"))
    }

    /// Get path to the index file
    fn index_path(&self) -> PathBuf {
        self.base_dir.join("index.json")
    }

    /// Get path to a conversation file
    fn conversation_path(&self, id: &str) -> PathBuf {
        self.base_dir.join(format!("{}.json", id))
    }

    /// Load the conversation index
    fn load_index(&self) -> Result<ConversationIndex, PhazeError> {
        let path = self.index_path();

        if !path.exists() {
            return Ok(ConversationIndex::default());
        }

        let mut file = File::open(&path)
            .map_err(|e| PhazeError::Config(format!("Failed to open index file: {}", e)))?;

        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .map_err(|e| PhazeError::Config(format!("Failed to read index file: {}", e)))?;

        match serde_json::from_str(&contents) {
            Ok(index) => Ok(index),
            Err(e) => {
                // Recover from a corrupt index instead of failing startup.
                // Keep the broken file for debugging.
                Self::quarantine_corrupt_file(&path, "index");
                eprintln!("Warning: corrupt conversation index reset: {e}");
                Ok(ConversationIndex::default())
            }
        }
    }

    /// Save the conversation index
    fn save_index(&self, index: &ConversationIndex) -> Result<(), PhazeError> {
        let path = self.index_path();
        let contents = serde_json::to_string_pretty(index)
            .map_err(|e| PhazeError::Config(format!("Failed to serialize index: {}", e)))?;
        Self::write_atomic(&path, &contents, "index")
    }

    /// Remove index entries whose conversation files no longer exist.
    /// Also re-attaches orphaned conversation files that exist on disk but are
    /// missing from index metadata.
    /// Returns the reconciled index and persists it when changes are made.
    fn load_index_consistent(&self) -> Result<ConversationIndex, PhazeError> {
        let mut index = self.load_index()?;
        let mut changed = false;
        let known_ids: HashSet<String> = index.conversations.iter().map(|m| m.id.clone()).collect();

        // 1) Prune stale index entries (metadata points to missing file).
        index
            .conversations
            .retain(|m| self.conversation_path(&m.id).exists());
        if index.conversations.len() != known_ids.len() {
            changed = true;
        }

        // 2) Re-attach orphaned conversation files (file exists but not indexed).
        let mut current_ids: HashSet<String> =
            index.conversations.iter().map(|m| m.id.clone()).collect();
        if let Ok(entries) = fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name == "index.json" || !name.ends_with(".json") {
                    continue;
                }
                if name.contains(".tmp") || name.contains(".corrupt.") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                if current_ids.contains(stem) {
                    continue;
                }
                match Self::load_conversation_file(&path) {
                    Ok(conv) => {
                        let mut meta = conv.metadata;
                        if meta.id.is_empty() {
                            meta.id = stem.to_string();
                        }
                        if current_ids.insert(meta.id.clone()) {
                            index.conversations.push(meta);
                            changed = true;
                        }
                    }
                    Err(_) => {
                        Self::quarantine_corrupt_file(&path, "conversation");
                        changed = true;
                    }
                }
            }
        }

        if changed {
            // Keep latest conversation first, same as save() behavior.
            index
                .conversations
                .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            self.save_index(&index)?;
        }
        Ok(index)
    }

    /// Generate a unique conversation ID
    pub fn generate_id() -> String {
        // Use UUID v4 for truly unique IDs
        use uuid::Uuid;
        Uuid::new_v4().to_string()
    }

    /// Get current timestamp as ISO 8601 string
    fn timestamp() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Simple ISO 8601 format: YYYY-MM-DDTHH:MM:SSZ
        // Using chrono for proper formatting
        use chrono::{DateTime, TimeZone, Utc};
        let dt = DateTime::<Utc>::from_timestamp(now as i64, 0)
            .or_else(|| Utc.timestamp_opt(0, 0).single())
            .unwrap_or(DateTime::<Utc>::UNIX_EPOCH);
        dt.to_rfc3339()
    }

    /// Save a conversation to disk
    pub fn save(&self, conversation: &SavedConversation) -> Result<(), PhazeError> {
        // Save the conversation file
        let path = self.conversation_path(&conversation.metadata.id);
        let contents = serde_json::to_string_pretty(conversation)
            .map_err(|e| PhazeError::Config(format!("Failed to serialize conversation: {}", e)))?;
        Self::write_atomic(&path, &contents, "conversation")?;

        // Update the index
        let mut index = self.load_index()?;

        // Remove existing entry if present
        index
            .conversations
            .retain(|m| m.id != conversation.metadata.id);

        // Add updated metadata
        index.conversations.push(conversation.metadata.clone());

        // Sort by updated_at (most recent first)
        index
            .conversations
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        self.save_index(&index)?;

        Ok(())
    }

    /// Write file atomically via `*.tmp` then rename.
    fn write_atomic(path: &PathBuf, contents: &str, label: &str) -> Result<(), PhazeError> {
        let tmp_path = path.with_extension("json.tmp");
        // Best-effort cleanup for stale temp files from interrupted writes.
        let _ = fs::remove_file(&tmp_path);

        fs::write(&tmp_path, contents).map_err(|e| {
            PhazeError::Config(format!("Failed to write temporary {label} file: {e}"))
        })?;

        if let Err(rename_err) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(PhazeError::Config(format!(
                "Failed to rename {label} file: {rename_err}"
            )));
        }

        Ok(())
    }

    /// Move a corrupt JSON file aside with a timestamped extension.
    fn quarantine_corrupt_file(path: &PathBuf, label: &str) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let backup = path.with_extension(format!("json.corrupt.{ts}"));
        if let Err(e) = fs::rename(path, backup) {
            eprintln!("Warning: failed to quarantine corrupt {label} file: {e}");
        }
    }

    fn load_conversation_file(path: &PathBuf) -> Result<SavedConversation, PhazeError> {
        let mut file = File::open(path)
            .map_err(|e| PhazeError::Config(format!("Failed to open conversation file: {e}")))?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .map_err(|e| PhazeError::Config(format!("Failed to read conversation file: {e}")))?;
        serde_json::from_str(&contents)
            .map_err(|e| PhazeError::Config(format!("Failed to parse conversation file: {e}")))
    }

    /// Load a conversation from disk
    pub fn load(&self, id: &str) -> Result<SavedConversation, PhazeError> {
        let path = self.conversation_path(id);

        if !path.exists() {
            return Err(PhazeError::Config(format!(
                "Conversation not found: {}",
                id
            )));
        }

        match Self::load_conversation_file(&path) {
            Ok(conv) => Ok(conv),
            Err(e) => {
                // Recover from a corrupt conversation file by quarantining it and
                // pruning it from the index so list/search stay healthy.
                Self::quarantine_corrupt_file(&path, "conversation");
                if let Ok(mut index) = self.load_index() {
                    index.conversations.retain(|m| m.id != id);
                    let _ = self.save_index(&index);
                }
                Err(PhazeError::Config(format!(
                    "Conversation file was corrupt and has been quarantined: {e}"
                )))
            }
        }
    }

    /// List recent conversations
    pub fn list_recent(&self, limit: usize) -> Result<Vec<ConversationMetadata>, PhazeError> {
        let index = self.load_index_consistent()?;

        Ok(index.conversations.into_iter().take(limit).collect())
    }

    /// Delete a conversation
    pub fn delete(&self, id: &str) -> Result<(), PhazeError> {
        // Delete the conversation file
        let path = self.conversation_path(id);
        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                PhazeError::Config(format!("Failed to delete conversation file: {}", e))
            })?;
        }

        // Update the index
        let mut index = self.load_index()?;
        index.conversations.retain(|m| m.id != id);
        self.save_index(&index)?;

        Ok(())
    }

    /// Search conversations by title
    pub fn search(&self, query: &str) -> Result<Vec<ConversationMetadata>, PhazeError> {
        let index = self.load_index_consistent()?;
        let query_lower = query.to_lowercase();

        Ok(index
            .conversations
            .into_iter()
            .filter(|m| m.title.to_lowercase().contains(&query_lower))
            .collect())
    }
}

impl Default for ConversationStore {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| {
            // Fallback to a temp directory if home dir is unavailable
            let fallback = std::env::temp_dir().join("phazeai").join("conversations");
            let _ = fs::create_dir_all(&fallback);
            Self { base_dir: fallback }
        })
    }
}

impl SavedConversation {
    /// Create a new saved conversation
    pub fn new(
        id: String,
        title: String,
        model: String,
        project_dir: Option<String>,
        system_prompt: Option<String>,
    ) -> Self {
        let timestamp = ConversationStore::timestamp();

        Self {
            metadata: ConversationMetadata {
                id,
                title,
                created_at: timestamp.clone(),
                updated_at: timestamp,
                message_count: 0,
                model,
                project_dir,
            },
            messages: Vec::new(),
            system_prompt,
        }
    }

    /// Add a message to the conversation
    pub fn add_message(&mut self, message: SavedMessage) {
        self.messages.push(message);
        self.metadata.message_count = self.messages.len();
        self.metadata.updated_at = ConversationStore::timestamp();
    }

    /// Generate a title from the first user message
    pub fn generate_title_from_first_message(&mut self) {
        if let Some(first_user_msg) = self.messages.iter().find(|m| m.role == "user") {
            let title = first_user_msg.content.chars().take(80).collect::<String>();
            let title = if first_user_msg.content.len() > 80 {
                format!("{}...", title.trim())
            } else {
                title
            };
            self.metadata.title = title;
        }
    }
}

impl SavedMessage {
    /// Create a new saved message
    pub fn new(role: String, content: String, tool_name: Option<String>) -> Self {
        Self {
            role,
            content,
            timestamp: ConversationStore::timestamp(),
            tool_name,
        }
    }

    /// Create a user message
    pub fn user(content: String) -> Self {
        Self::new("user".to_string(), content, None)
    }

    /// Create an assistant message
    pub fn assistant(content: String) -> Self {
        Self::new("assistant".to_string(), content, None)
    }

    /// Create a system message
    pub fn system(content: String) -> Self {
        Self::new("system".to_string(), content, None)
    }

    /// Create a tool message
    pub fn tool(content: String, tool_name: String) -> Self {
        Self::new("tool".to_string(), content, Some(tool_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_generate_id() {
        let id1 = ConversationStore::generate_id();
        let id2 = ConversationStore::generate_id();

        assert_ne!(id1, id2);
        assert!(id1.contains('-'));
        assert!(id2.contains('-'));
    }

    #[test]
    fn test_timestamp() {
        let ts = ConversationStore::timestamp();
        assert!(ts.contains('T'));
        assert!(ts.len() > 10);
    }

    #[test]
    fn test_saved_message_constructors() {
        let user_msg = SavedMessage::user("Hello".to_string());
        assert_eq!(user_msg.role, "user");
        assert_eq!(user_msg.content, "Hello");
        assert!(user_msg.tool_name.is_none());

        let tool_msg = SavedMessage::tool("Result".to_string(), "grep".to_string());
        assert_eq!(tool_msg.role, "tool");
        assert_eq!(tool_msg.content, "Result");
        assert_eq!(tool_msg.tool_name, Some("grep".to_string()));
    }

    #[test]
    fn test_conversation_title_generation() {
        let mut conv = SavedConversation::new(
            "test-id".to_string(),
            "Untitled".to_string(),
            "gpt-4".to_string(),
            None,
            None,
        );

        conv.add_message(SavedMessage::user("This is a short message".to_string()));
        conv.generate_title_from_first_message();
        assert_eq!(conv.metadata.title, "This is a short message");

        let mut conv2 = SavedConversation::new(
            "test-id-2".to_string(),
            "Untitled".to_string(),
            "gpt-4".to_string(),
            None,
            None,
        );

        let long_message = "a".repeat(100);
        conv2.add_message(SavedMessage::user(long_message));
        conv2.generate_title_from_first_message();
        assert!(conv2.metadata.title.ends_with("..."));
        assert!(conv2.metadata.title.len() <= 83); // 80 + "..."
    }

    #[test]
    fn test_list_recent_prunes_stale_index_entries() {
        let tmp = TempDir::new().expect("temp dir");
        let store = ConversationStore::with_dir(tmp.path().to_path_buf()).expect("store");

        // Write an index that points to a non-existent conversation file.
        let stale = ConversationIndex {
            conversations: vec![ConversationMetadata {
                id: "missing-conv".to_string(),
                title: "stale".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
                message_count: 1,
                model: "test".to_string(),
                project_dir: None,
            }],
        };
        let mut f = File::create(tmp.path().join("index.json")).expect("create index");
        f.write_all(
            serde_json::to_string_pretty(&stale)
                .expect("serialize")
                .as_bytes(),
        )
        .expect("write");

        let recent = store.list_recent(10).expect("list");
        assert!(recent.is_empty(), "stale index entries should be pruned");
    }

    #[test]
    fn test_list_recent_recovers_orphaned_conversation_files() {
        let tmp = TempDir::new().expect("temp dir");
        let store = ConversationStore::with_dir(tmp.path().to_path_buf()).expect("store");

        // Write a valid conversation file without adding it to index.
        let conv = SavedConversation::new(
            "orphan-conv".to_string(),
            "Recovered Conversation".to_string(),
            "gpt-test".to_string(),
            None,
            None,
        );
        let conv_path = tmp.path().join("orphan-conv.json");
        std::fs::write(
            &conv_path,
            serde_json::to_string_pretty(&conv).expect("serialize conversation"),
        )
        .expect("write conversation");

        let recent = store.list_recent(10).expect("list");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, "orphan-conv");
        assert_eq!(recent[0].title, "Recovered Conversation");
    }
}
