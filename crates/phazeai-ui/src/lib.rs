use std::path::PathBuf;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

pub mod ai_helpers;
pub mod app;
pub mod code_search;
pub mod commands;
pub mod components;
pub mod crash_recovery;
pub mod custom_theme;
pub mod debug_session;
pub mod domain_state;
pub mod editor_command;
pub mod lsp_bridge;
pub mod notify;
pub mod panels;
pub mod theme;
pub mod tokens;
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

    let file_appender = RollingFileAppender::new(Rotation::DAILY, log_dir, "phazeai-ui.log");

    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Keep the guard alive for the lifetime of the app
    std::mem::forget(_guard);

    // RUST_LOG, when set and usable, applies to both outputs unchanged.
    // Otherwise the log file gets our own info-level events, while the
    // console only shows warnings and errors. The GPU stack (wgpu/naga) and
    // the windowing layer report driver details at info level many times per
    // second on some machines, so they are held to warnings everywhere.
    let user_filter = std::env::var("RUST_LOG").ok();
    let (file_directives, ignored) = log_directives(user_filter.as_deref(), "info");
    let (console_directives, _) = log_directives(user_filter.as_deref(), "warn");

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_target(true)
                .with_thread_ids(true)
                .with_filter(EnvFilter::new(file_directives)),
        )
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(!cfg!(windows))
                .with_target(true)
                .with_filter(EnvFilter::new(console_directives)),
        )
        .init();

    if let Some(problem) = ignored {
        tracing::warn!(target: "phazeai_ui", "{problem}");
    }
}

/// Directives for one log output: `RUST_LOG` when it is set and usable,
/// otherwise [`default_filter`] for `default_level`. The second value, when
/// set, says why `RUST_LOG` was ignored, for a warning once logging is up.
///
/// `EnvFilter::new` silently drops directives it can't parse, and a
/// `RUST_LOG` made only of bad directives then enables nothing — not even
/// the panic hook's output. `phazeai_ui:debug` is not a parse error either:
/// it names a target called `phazeai_ui:debug`, which nothing logs to, with
/// the same effect, so a lone `:` is treated as the typo for `=` it is.
fn log_directives(rust_log: Option<&str>, default_level: &str) -> (String, Option<String>) {
    let Some(user) = rust_log.map(str::trim).filter(|s| !s.is_empty()) else {
        return (default_filter(default_level), None);
    };
    match rust_log_problem(user) {
        None => (user.to_string(), None),
        Some(problem) => (
            default_filter(default_level),
            Some(format!(
                "ignoring RUST_LOG={user:?} ({problem}); using the default log filter"
            )),
        ),
    }
}

/// Why `directives` can't serve as a log filter, if it can't.
fn rust_log_problem(directives: &str) -> Option<String> {
    if let Err(e) = EnvFilter::try_new(directives) {
        return Some(format!("invalid directive: {e}"));
    }
    directives
        .split(',')
        .map(str::trim)
        .find(|d| has_lone_colon(d.split(['=', '[']).next().unwrap_or("")))
        .map(|d| {
            format!(
                "`{d}` names a target, not a level; write `{}`",
                d.replacen(':', "=", 1)
            )
        })
}

/// A `:` that isn't part of a `::` path separator.
fn has_lone_colon(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len()).any(|i| {
        b[i] == b':' && !(i > 0 && b[i - 1] == b':') && !(i + 1 < b.len() && b[i + 1] == b':')
    })
}

/// Log directives used when `RUST_LOG` is not set: `level` for everything,
/// with the chatty graphics and windowing crates capped at `warn`.
fn default_filter(level: &str) -> String {
    const NOISY: &[&str] = &[
        "wgpu",
        "wgpu_core",
        "wgpu_hal",
        "naga",
        "winit",
        "cosmic_text",
    ];
    let mut directives = level.to_string();
    for target in NOISY {
        directives.push_str(&format!(",{target}=warn"));
    }
    directives
}

/// Install a process-wide panic hook that logs the panic (with location and
/// backtrace) via `tracing::error!` to the rolling log file and stderr, then
/// chains to the default hook so the process still aborts with a useful
/// stderr trace. Without this, a panic on the Floem UI thread causes the
/// window to vanish silently with no log entry — the primary cause of the
/// "closes immediately" symptom.
///
/// Idempotent: only installs once per process.
pub fn install_panic_hook() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info
                .payload()
                .downcast_ref::<&'static str>()
                .copied()
                .map(str::to_string)
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown location>".to_string());
            let backtrace = std::backtrace::Backtrace::force_capture();
            tracing::error!(
                target: "phazeai_ui::panic",
                location = %location,
                payload = %payload,
                backtrace = %backtrace,
                "panic on thread '{}'",
                std::thread::current().name().unwrap_or("<unnamed>")
            );
            default_hook(info);
        }));
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_log_is_used_when_valid() {
        let (filter, warning) = log_directives(Some("phazeai_ui=debug,wgpu=warn"), "info");
        assert_eq!(filter, "phazeai_ui=debug,wgpu=warn");
        assert_eq!(warning, None);
        assert_eq!(
            log_directives(Some("debug"), "info"),
            ("debug".to_string(), None)
        );
        // `::` is a path separator, not a typo.
        assert_eq!(
            log_directives(Some("phazeai_ui::panels=debug"), "info").1,
            None
        );
    }

    #[test]
    fn missing_or_empty_rust_log_uses_defaults() {
        assert_eq!(log_directives(None, "warn"), (default_filter("warn"), None));
        assert_eq!(
            log_directives(Some("  "), "info"),
            (default_filter("info"), None)
        );
        assert!(default_filter("info").starts_with("info,wgpu=warn"));
    }

    #[test]
    fn bad_rust_log_falls_back_to_defaults_with_a_warning() {
        for bad in [
            "phazeai_ui:debug",
            "phazeai_ui=verbose",
            "[[",
            "info,phazeai_ui::panels:trace",
        ] {
            let (filter, warning) = log_directives(Some(bad), "info");
            assert_eq!(filter, default_filter("info"), "{bad}");
            let warning = warning.unwrap_or_else(|| panic!("{bad} should warn"));
            assert!(warning.contains(bad), "{warning}");
        }
        let (_, warning) = log_directives(Some("phazeai_ui:debug"), "info");
        assert!(warning.unwrap().contains("phazeai_ui=debug"));
    }
}
