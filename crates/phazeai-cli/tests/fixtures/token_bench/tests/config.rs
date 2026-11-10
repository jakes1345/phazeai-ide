//! Config parsing, through an in-memory environment.

use std::collections::HashMap;

use tally::config::{Config, ConfigError, DEFAULT_MAX_NAMES, DEFAULT_PORT};

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |key| map.get(key).cloned()
}

#[test]
fn defaults_apply_when_nothing_is_set() {
    let config = Config::from_lookup(env(&[])).unwrap();
    assert_eq!(config.port, DEFAULT_PORT);
    assert_eq!(config.max_names, DEFAULT_MAX_NAMES);
    assert_eq!(config.bind_address(), "127.0.0.1:8080");
}

#[test]
fn a_set_port_is_used() {
    let config = Config::from_lookup(env(&[("PORT", "9090")])).unwrap();
    assert_eq!(config.port, 9090);
}

#[test]
fn an_ipv6_host_is_bracketed() {
    let config = Config::from_lookup(env(&[("HOST", "::1"), ("PORT", "7000")])).unwrap();
    assert_eq!(config.bind_address(), "[::1]:7000");
}

#[test]
fn a_bad_host_is_an_error() {
    let err = Config::from_lookup(env(&[("HOST", "localhost:80")])).unwrap_err();
    assert_eq!(err, ConfigError::BadHost("localhost:80".into()));
}

#[test]
fn zero_max_names_is_an_error() {
    let err = Config::from_lookup(env(&[("TALLY_MAX_NAMES", "0")])).unwrap_err();
    assert_eq!(err, ConfigError::BadMaxNames("0".into()));
}
