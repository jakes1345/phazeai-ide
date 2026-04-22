use std::path::PathBuf;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub mod app;
pub mod commands;
pub mod components;
pub mod lsp_bridge;
pub mod panels;
pub mod domain_state;
pub mod theme;
pub mod util;

pub use app::launch_phaze_ide;
pub use theme::{PhazePalette, PhazeTheme, ThemeVariant};

/// Initialize logging system - creates logs in ~/.config/phazeai/logs/
pub fn init_logging() {
    let log_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("phazeai")
        .join("logs");
    
    std::fs::create_dir_all(&log_dir).ok();
    
    let file_appender = RollingFileAppender::new(
        Rotation::DAILY,
        log_dir,
        "phazeai-ui.log",
    );
    
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    
    // Keep the guard alive for the lifetime of the app
    std::mem::forget(_guard);
    
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    
    tracing_subscriber::registry()
        .with(env_filter)
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_target(true)
                .with_thread_ids(true)
        )
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(true)
                .with_target(true)
        )
        .init();
}

/// Log an error with context - use this instead of .ok() or .unwrap()
#[macro_export]
macro_rules! log_err {
    ($result:expr, $context:literal) => {
        match $result {
            Ok(val) => val,
            Err(e) => {
                tracing::error!(target: "phazeai_ui", error = %e, context = $context);
                return None;
            }
        }
    };
    ($result:expr, $context:literal, $default:expr) => {
        match $result {
            Ok(val) => val,
            Err(e) => {
                tracing::warn!(target: "phazeai_ui", error = %e, context = $context);
                $default
            }
        }
    };
}

/// Log and ignore - for operations where failure is acceptable but should be logged
#[macro_export]
macro_rules! log_ignore {
    ($result:expr, $context:literal) => {
        if let Err(e) = $result {
            tracing::debug!(target: "phazeai_ui", error = %e, context = $context);
        }
    };
}
