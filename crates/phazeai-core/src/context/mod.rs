mod builder;
pub mod compaction;
mod history;
pub mod persistence;
pub mod repo_map;
pub mod rules;
pub mod system_prompt;

pub use builder::ContextBuilder;
pub use history::ConversationHistory;
pub use history::SUMMARY_HEADING;
pub use persistence::{ConversationMetadata, ConversationStore, SavedConversation, SavedMessage};
pub use repo_map::RepoMapGenerator;
pub use system_prompt::{collect_git_info, ProjectType, SystemPromptBuilder};
