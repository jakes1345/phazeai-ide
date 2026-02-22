mod builder;
mod history;
pub mod persistence;
pub mod system_prompt;

pub use builder::ContextBuilder;
pub use history::ConversationHistory;
pub use persistence::{ConversationMetadata, ConversationStore, SavedConversation, SavedMessage};
pub use system_prompt::{collect_git_info, ProjectType, SystemPromptBuilder};
