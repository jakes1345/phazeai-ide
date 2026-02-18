use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::Read;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use crate::error::PhazeError;

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
        let home = dirs::home_dir().ok_or_else(|| {
            PhazeError::Config("Could not determine home directory".to_string())
        })?;

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

        let mut file = File::open(&path).map_err(|e| {
            PhazeError::Config(format!("Failed to open index file: {}", e))
        })?;

        let mut contents = String::new();
        file.read_to_string(&mut contents).map_err(|e| {
            PhazeError::Config(format!("Failed to read index file: {}", e))
        })?;

        serde_json::from_str(&contents).map_err(|e| {
            PhazeError::Config(format!("Failed to parse index file: {}", e))
        })
    }

    /// Save the conversation index
    fn save_index(&self, index: &ConversationIndex) -> Result<(), PhazeError> {
        let path = self.index_path();
        let contents = serde_json::to_string_pretty(index).map_err(|e| {
            PhazeError::Config(format!("Failed to serialize index: {}", e))
        })?;

        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, contents).map_err(|e| {
            PhazeError::Config(format!("Failed to write temporary index file: {}", e))
        })?;

        fs::rename(&tmp_path, &path).map_err(|e| {
            PhazeError::Config(format!("Failed to rename index file: {}", e))
        })?;

        Ok(())
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
        use chrono::{DateTime, Utc};
        let dt = DateTime::<Utc>::from_timestamp(now as i64, 0)
            .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).unwrap());
        dt.to_rfc3339()
    }

    /// Save a conversation to disk
    pub fn save(&self, conversation: &SavedConversation) -> Result<(), PhazeError> {
        // Save the conversation file
        let path = self.conversation_path(&conversation.metadata.id);
        let contents = serde_json::to_string_pretty(conversation).map_err(|e| {
            PhazeError::Config(format!("Failed to serialize conversation: {}", e))
        })?;

        let tmp_path = path.with_extension("json.tmp");
        fs::write(&tmp_path, contents).map_err(|e| {
            PhazeError::Config(format!("Failed to write temporary conversation file: {}", e))
        })?;

        fs::rename(&tmp_path, &path).map_err(|e| {
            PhazeError::Config(format!("Failed to rename conversation file: {}", e))
        })?;

        // Update the index
        let mut index = self.load_index()?;

        // Remove existing entry if present
        index.conversations.retain(|m| m.id != conversation.metadata.id);

        // Add updated metadata
        index.conversations.push(conversation.metadata.clone());

        // Sort by updated_at (most recent first)
        index.conversations.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        self.save_index(&index)?;

        Ok(())
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

        let mut file = File::open(&path).map_err(|e| {
            PhazeError::Config(format!("Failed to open conversation file: {}", e))
        })?;

        let mut contents = String::new();
        file.read_to_string(&mut contents).map_err(|e| {
            PhazeError::Config(format!("Failed to read conversation file: {}", e))
        })?;

        serde_json::from_str(&contents).map_err(|e| {
            PhazeError::Config(format!("Failed to parse conversation file: {}", e))
        })
    }

    /// List recent conversations
    pub fn list_recent(&self, limit: usize) -> Result<Vec<ConversationMetadata>, PhazeError> {
        let index = self.load_index()?;

        Ok(index
            .conversations
            .into_iter()
            .take(limit)
            .collect())
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
        let index = self.load_index()?;
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
        Self::new().expect("Failed to create conversation store")
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
}
