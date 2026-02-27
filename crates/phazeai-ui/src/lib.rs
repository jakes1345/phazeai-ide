pub mod theme;
pub mod components;
pub mod panels;
pub mod app;
pub mod lsp_bridge;

pub use theme::{PhazeTheme, PhazePalette, ThemeVariant};
pub use app::launch_phaze_ide;
