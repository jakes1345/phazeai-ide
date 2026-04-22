pub mod ai;
pub mod editor;
pub mod project;
pub mod workbench;

use floem::reactive::RwSignal;
use crate::theme::PhazeTheme;

pub use ai::AiState;
pub use editor::EditorState;
pub use project::ProjectState;
pub use workbench::{WorkbenchState, SearchResult};

/// The root IDE state, decomposed into logical sub-states.
#[derive(Clone)]
pub struct IdeState {
    pub workbench: WorkbenchState,
    pub editor: EditorState,
    pub ai: AiState,
    pub project: ProjectState}

impl IdeState {
    pub fn theme(&self) -> RwSignal<PhazeTheme> {
        self.workbench.theme
    }
}
