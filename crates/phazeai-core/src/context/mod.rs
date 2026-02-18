mod history;
mod builder;
pub mod system_prompt;
pub mod persistence;

pub use history::ConversationHistory;
pub use builder::ContextBuilder;
pub use system_prompt::{SystemPromptBuilder, ProjectType, collect_git_info};
pub use persistence::{ConversationStore, ConversationMetadata, SavedConversation, SavedMessage};
