use super::file::FileWrite;
use crate::error::PhazeError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type ToolResult = Result<Value, PhazeError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;

    async fn execute(&self, params: Value) -> ToolResult;

    /// For a tool whose whole effect is writing one file (`write_file`,
    /// `edit_file`): the write this call would make, worked out without
    /// making it, or the error the call would fail with. `None`, the
    /// default, for every other tool.
    ///
    /// An agent with a diff hook shows this write for review and then
    /// commits exactly what was approved ([`FileWrite::commit`]) instead of
    /// calling [`Tool::execute`]; so a tool that returns `Some` here must make
    /// `execute` do the same thing: plan, then commit.
    async fn plan_write(&self, _params: &Value) -> Option<Result<FileWrite, PhazeError>> {
        None
    }

    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}

/// The tools an agent offers, kept in name order: [`ToolRegistry::definitions`]
/// lists the same set the same way every time, so the tools part of a request
/// is byte-identical across requests, agents and sessions, which is what a
/// provider's prompt cache needs to reuse it.
pub struct ToolRegistry {
    tools: BTreeMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools.get(name).map(|t| t.as_ref())
    }

    pub fn list(&self) -> Vec<&dyn Tool> {
        self.tools.values().map(|t| t.as_ref()).collect()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|t| t.to_definition()).collect()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        // Original tools
        registry.register(Box::new(super::ReadFileTool));
        registry.register(Box::new(super::WriteFileTool));
        registry.register(Box::new(super::BashTool::default()));
        registry.register(Box::new(super::GrepTool));
        registry.register(Box::new(super::ListFilesTool));
        registry.register(Box::new(super::GlobTool));
        registry.register(Box::new(super::EditTool));
        // New tools
        registry.register(Box::new(super::FindPathTool));
        registry.register(Box::new(super::FetchTool));
        registry.register(Box::new(super::WebSearchTool));
        registry.register(Box::new(super::CopyPathTool));
        registry.register(Box::new(super::MovePathTool));
        registry.register(Box::new(super::DeletePathTool));
        registry.register(Box::new(super::CreateDirectoryTool));
        registry.register(Box::new(super::NowTool));
        registry.register(Box::new(super::OpenTool));
        registry.register(Box::new(super::DiagnosticsTool));
        registry.register(Box::new(super::MemoryTool));
        registry.register(Box::new(super::BrowseTool));
        registry.register(Box::new(super::DownloadTool));
        registry.register(Box::new(super::ScreenshotTool));
        registry
    }
}

impl ToolRegistry {
    /// Read-only tool set: suitable for Ask / read-only AI modes.
    /// Includes only tools that cannot modify the filesystem or run commands.
    pub fn read_only() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(super::ReadFileTool));
        registry.register(Box::new(super::GrepTool));
        registry.register(Box::new(super::GlobTool));
        registry.register(Box::new(super::ListFilesTool));
        registry.register(Box::new(super::FindPathTool));
        registry.register(Box::new(super::NowTool));
        registry.register(Box::new(super::MemoryTool));
        registry
    }

    /// Standard tool set: read + write + diagnostics, but no destructive ops.
    /// Suitable for Debug / Plan modes.
    pub fn standard() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(super::ReadFileTool));
        registry.register(Box::new(super::GrepTool));
        registry.register(Box::new(super::GlobTool));
        registry.register(Box::new(super::ListFilesTool));
        registry.register(Box::new(super::FindPathTool));
        registry.register(Box::new(super::NowTool));
        registry.register(Box::new(super::FetchTool));
        registry.register(Box::new(super::WebSearchTool));
        registry.register(Box::new(super::DiagnosticsTool));
        registry.register(Box::new(super::MemoryTool));
        registry.register(Box::new(super::BashTool::default()));
        registry.register(Box::new(super::BrowseTool));
        registry.register(Box::new(super::DownloadTool));
        registry.register(Box::new(super::ScreenshotTool));
        registry
    }

    /// Total number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(defs: &[ToolDefinition]) -> Vec<&str> {
        defs.iter().map(|d| d.name.as_str()).collect()
    }

    #[test]
    fn definitions_come_in_name_order_whatever_the_registration_order() {
        let mut forward = ToolRegistry::new();
        forward.register(Box::new(super::super::ReadFileTool));
        forward.register(Box::new(super::super::GrepTool));
        forward.register(Box::new(super::super::BashTool::default()));
        let mut backward = ToolRegistry::new();
        backward.register(Box::new(super::super::BashTool::default()));
        backward.register(Box::new(super::super::GrepTool));
        backward.register(Box::new(super::super::ReadFileTool));

        assert_eq!(names(&forward.definitions()), ["bash", "grep", "read_file"]);
        let json = |r: &ToolRegistry| serde_json::to_string(&r.definitions()).unwrap();
        assert_eq!(json(&forward), json(&backward));
    }

    #[test]
    fn every_new_agent_offers_the_default_tools_identically() {
        let first = serde_json::to_string(&ToolRegistry::default().definitions()).unwrap();
        for _ in 0..8 {
            let again = serde_json::to_string(&ToolRegistry::default().definitions()).unwrap();
            assert_eq!(first, again);
        }
        let defs = ToolRegistry::default().definitions();
        let mut sorted = names(&defs);
        sorted.sort_unstable();
        assert_eq!(names(&defs), sorted);
    }
}
