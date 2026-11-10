//! The in-memory counter table.

use std::collections::BTreeMap;
use std::sync::Mutex;

#[derive(Debug, PartialEq)]
pub enum StoreError {
    BadName,
    Full,
    NotFound,
}

pub struct Store {
    counters: Mutex<BTreeMap<String, u64>>,
    max_names: usize,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Store {
    pub fn new(max_names: usize) -> Store {
        Store {
            counters: Mutex::new(BTreeMap::new()),
            max_names,
        }
    }

    pub fn get(&self, name: &str) -> Result<u64, StoreError> {
        if !valid_name(name) {
            return Err(StoreError::BadName);
        }
        let counters = self.counters.lock().expect("store lock");
        counters.get(name).copied().ok_or(StoreError::NotFound)
    }

    pub fn bump(&self, name: &str) -> Result<u64, StoreError> {
        if !valid_name(name) {
            return Err(StoreError::BadName);
        }
        let mut counters = self.counters.lock().expect("store lock");
        if !counters.contains_key(name) && counters.len() >= self.max_names {
            return Err(StoreError::Full);
        }
        let value = counters.entry(name.to_string()).or_insert(0);
        *value = value.saturating_add(1);
        Ok(*value)
    }

    pub fn remove(&self, name: &str) -> Result<(), StoreError> {
        if !valid_name(name) {
            return Err(StoreError::BadName);
        }
        let mut counters = self.counters.lock().expect("store lock");
        counters.remove(name).map(|_| ()).ok_or(StoreError::NotFound)
    }

    pub fn list(&self) -> Vec<(String, u64)> {
        let counters = self.counters.lock().expect("store lock");
        counters.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }
}
