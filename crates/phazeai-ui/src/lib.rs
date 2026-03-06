pub mod app;
pub mod components;
pub mod lsp_bridge;
pub mod panels;
pub mod theme;

pub use app::launch_phaze_ide;
pub use theme::{PhazePalette, PhazeTheme, ThemeVariant};
