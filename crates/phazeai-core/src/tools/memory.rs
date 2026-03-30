use crate::error::PhazeError;
use crate::tools::traits::{Tool, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Default)]
struct MemoryStore {
    items: HashMap<String, String>,
}

pub struct MemoryTool;

#[async_trait::async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &str {
        "memory"
    }

    fn description(&self) -> &str {
        "Auto-memory storage. Use this to persist facts, project knowledge, patterns, and contextual memory permanently across sessions. Action can be 'store', 'search', or 'list'."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The action to perform: 'store', 'search', or 'list'",
                    "enum": ["store", "search", "list"]
                },
                "key": {
                    "type": "string",
                    "description": "The memory key to store under or search for (e.g., 'auth_architecture', 'db_schema_notes'). Required for 'store' and 'search'."
                },
                "content": {
                    "type": "string",
                    "description": "The content to permanently save. Required for 'store'."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let action = params
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| PhazeError::tool("memory", "Missing required parameter: action"))?;

        let memory_dir = if let Some(home) = dirs::home_dir() {
            home.join(".phazeai")
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(".phazeai")
        };
        let memory_file = memory_dir.join("memory.json");

        if !memory_dir.exists() {
            let _ = tokio::fs::create_dir_all(&memory_dir).await;
        }

        let mut store: MemoryStore = if memory_file.exists() {
            let data = tokio::fs::read_to_string(&memory_file).await.map_err(|e| {
                PhazeError::tool("memory", format!("Failed to read memory file: {}", e))
            })?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            MemoryStore::default()
        };

        match action {
            "store" => {
                let key = params
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| PhazeError::tool("memory", "Missing 'key' for store action"))?;
                let content = params
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        PhazeError::tool("memory", "Missing 'content' for store action")
                    })?;

                store.items.insert(key.to_string(), content.to_string());

                let serialized = serde_json::to_string_pretty(&store).unwrap();
                tokio::fs::write(&memory_file, serialized)
                    .await
                    .map_err(|e| {
                        PhazeError::tool("memory", format!("Failed to save memory file: {}", e))
                    })?;

                Ok(serde_json::json!({
                    "success": true,
                    "message": format!("Stored memory under key: {}", key),
                }))
            }
            "search" => {
                let key_query = params.get("key").and_then(|v| v.as_str()).unwrap_or("");

                let mut results = HashMap::new();
                for (k, v) in &store.items {
                    if k.contains(key_query) || key_query.is_empty() {
                        results.insert(k.clone(), v.clone());
                    }
                }

                Ok(serde_json::json!({
                    "results": results
                }))
            }
            "list" => {
                let keys: Vec<String> = store.items.keys().cloned().collect();
                Ok(serde_json::json!({
                    "memory_keys": keys
                }))
            }
            _ => Err(PhazeError::tool(
                "memory",
                format!("Unknown action: {}", action),
            )),
        }
    }
}
