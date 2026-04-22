use crate::tools::traits::{Tool, ToolResult};
use serde_json::Value;

pub struct NowTool;

#[async_trait::async_trait]
impl Tool for NowTool {
    fn name(&self) -> &str {
        "now"
    }

    fn description(&self) -> &str {
        "Get the current date, time, timezone, and unix timestamp. Useful for time-aware operations, logging, and timestamp generation."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _params: Value) -> ToolResult {
        let now = chrono::Local::now();

        Ok(serde_json::json!({
            "datetime": now.format("%Y-%m-%d %H:%M:%S").to_string(),
            "date": now.format("%Y-%m-%d").to_string(),
            "time": now.format("%H:%M:%S").to_string(),
            "timezone": now.format("%Z").to_string(),
            "utc_offset": now.format("%:z").to_string(),
            "unix_timestamp": now.timestamp(),
            "iso8601": now.to_rfc3339(),
            "day_of_week": now.format("%A").to_string(),
        }))
    }
}
