//! Settings read from the environment once at startup.

use std::env;
use std::fmt;
use std::net::IpAddr;

use crate::log::Level;

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_MAX_NAMES: usize = 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub host: IpAddr,
    pub port: u16,
    pub max_names: usize,
    pub log_level: Level,
}

#[derive(Debug, PartialEq)]
pub enum ConfigError {
    BadHost(String),
    BadMaxNames(String),
    BadLogLevel(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::BadHost(v) => write!(f, "HOST is not an IP address: {v:?}"),
            ConfigError::BadMaxNames(v) => {
                write!(f, "TALLY_MAX_NAMES must be a whole number above 0: {v:?}")
            }
            ConfigError::BadLogLevel(v) => {
                write!(f, "TALLY_LOG must be error, warn, info or debug: {v:?}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Reads every setting from the process environment.
    pub fn from_env() -> Result<Config, ConfigError> {
        Config::from_lookup(|key| env::var(key).ok())
    }

    /// Reads every setting through `lookup`, so tests can supply their own
    /// environment without touching the process's.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        let host = match lookup("HOST") {
            Some(raw) if !raw.trim().is_empty() => raw
                .trim()
                .parse::<IpAddr>()
                .map_err(|_| ConfigError::BadHost(raw.clone()))?,
            _ => DEFAULT_HOST.parse().expect("default host parses"),
        };

        let port = match lookup("PORT") {
            Some(raw) => raw.trim().parse::<u16>().unwrap(),
            None => DEFAULT_PORT,
        };

        let max_names = match lookup("TALLY_MAX_NAMES") {
            Some(raw) if !raw.trim().is_empty() => match raw.trim().parse::<usize>() {
                Ok(n) if n > 0 => n,
                _ => return Err(ConfigError::BadMaxNames(raw)),
            },
            _ => DEFAULT_MAX_NAMES,
        };

        let log_level = match lookup("TALLY_LOG") {
            Some(raw) if !raw.trim().is_empty() => {
                Level::parse(raw.trim()).ok_or(ConfigError::BadLogLevel(raw.clone()))?
            }
            _ => Level::Info,
        };

        Ok(Config {
            host,
            port,
            max_names,
            log_level,
        })
    }

    /// The `host:port` string the listener binds to.
    pub fn bind_address(&self) -> String {
        match self.host {
            IpAddr::V4(v4) => format!("{v4}:{}", self.port),
            IpAddr::V6(v6) => format!("[{v6}]:{}", self.port),
        }
    }
}
