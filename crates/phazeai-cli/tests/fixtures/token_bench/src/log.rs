//! A tiny leveled logger that writes to stderr.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    pub fn parse(raw: &str) -> Option<Level> {
        match raw.to_ascii_lowercase().as_str() {
            "error" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            _ => None,
        }
    }
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Info as u8);

pub fn set_level(level: Level) {
    LEVEL.store(level as u8, Ordering::Relaxed);
}

fn enabled(level: Level) -> bool {
    level as u8 <= LEVEL.load(Ordering::Relaxed)
}

fn write(level: Level, tag: &str, msg: &str) {
    if enabled(level) {
        eprintln!("[{tag}] {msg}");
    }
}

pub fn error(msg: &str) {
    write(Level::Error, "error", msg);
}

pub fn warn(msg: &str) {
    write(Level::Warn, "warn", msg);
}

pub fn info(msg: &str) {
    write(Level::Info, "info", msg);
}

pub fn debug(msg: &str) {
    write(Level::Debug, "debug", msg);
}
