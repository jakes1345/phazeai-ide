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

    /// True for a tool that only reads (files, the code index, the web) and
    /// keeps no state between calls, so its calls, and calls of other such
    /// tools, can run at the same time without changing what any of them
    /// returns. When the model asks for a run of such calls in one round, the
    /// agent runs them together instead of one after another. `false`, the
    /// default, for anything that writes, runs commands or keeps state
    /// (`bash` keeps its working directory between calls).
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.parameters_schema(),
        }
    }
}

/// The name of the one tool through which the model reaches the tools a
/// registry holds back ([`ToolRegistry::register_deferred`]).
pub const MORE_TOOLS: &str = "more_tools";

/// The tools an agent offers, kept in name order: [`ToolRegistry::definitions`]
/// lists the same set the same way every time, so the tools part of a request
/// is byte-identical across requests, agents and sessions, which is what a
/// provider's prompt cache needs to reuse it.
///
/// A tool can also be held back ([`ToolRegistry::register_deferred`]): its
/// schema is not sent with every request. The request carries one small
/// [`MORE_TOOLS`] tool instead, whose description names each held-back tool
/// in a line; the model calls it with a tool's name to get that tool's
/// parameters, then with the name and `arguments` to run it. A tool the model
/// rarely needs then costs a line, not its whole schema, on every request.
pub struct ToolRegistry {
    tools: BTreeMap<String, Box<dyn Tool>>,
    deferred: BTreeMap<String, Box<dyn Tool>>,
}

/// What a call of [`MORE_TOOLS`] asks for.
#[derive(Debug, Clone, PartialEq)]
pub enum MoreToolsCall {
    /// The parameters of the named tool.
    Describe(String),
    /// Run the named tool with these arguments.
    Run { name: String, arguments: Value },
}

impl MoreToolsCall {
    /// Read the arguments of a [`MORE_TOOLS`] call. `arguments` may come as an
    /// object or as a JSON string holding one (models do both); absent or
    /// null means the call asks for the tool's parameters.
    pub fn parse(params: &Value) -> Result<Self, String> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .ok_or_else(|| format!("{MORE_TOOLS} needs `name`, the tool to use"))?
            .to_string();
        match params.get("arguments") {
            None | Some(Value::Null) => Ok(Self::Describe(name)),
            Some(Value::String(text)) if text.trim().is_empty() => Ok(Self::Describe(name)),
            Some(Value::String(text)) => serde_json::from_str::<Value>(text)
                .map(|arguments| Self::Run { name, arguments })
                .map_err(|e| format!("`arguments` is not valid JSON: {e}")),
            Some(arguments) => Ok(Self::Run {
                name,
                arguments: arguments.clone(),
            }),
        }
    }
}

/// The first sentence of a tool description, for the one-line listing in
/// [`MORE_TOOLS`]: up to the first ". " or line break, at most 100 chars.
fn first_sentence(description: &str) -> String {
    let text = description.trim();
    let mut end = text.len();
    if let Some(i) = text.find(". ") {
        end = end.min(i + 1);
    }
    if let Some(i) = text.find('\n') {
        end = end.min(i);
    }
    let line = text[..end].trim();
    if line.chars().count() <= 100 {
        return line.to_string();
    }
    let cut: String = line.chars().take(99).collect();
    format!("{}…", cut.trim_end())
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: BTreeMap::new(),
            deferred: BTreeMap::new(),
        }
    }

    /// Offer `tool` with every request. Replaces a tool of the same name,
    /// offered or held back.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.deferred.remove(&name);
        self.tools.insert(name, tool);
    }

    /// Hold `tool` back: listed by name in [`MORE_TOOLS`] and run through
    /// it, its schema sent only when the model asks for it. Replaces a tool
    /// of the same name, offered or held back.
    pub fn register_deferred(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.remove(&name);
        self.deferred.insert(name, tool);
    }

    /// The named tool, offered or held back.
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .get(name)
            .or_else(|| self.deferred.get(name))
            .map(|t| t.as_ref())
    }

    /// True when `name` is a held-back tool.
    pub fn is_deferred(&self, name: &str) -> bool {
        self.deferred.contains_key(name)
    }

    /// Every tool the agent can run, offered and held back, in name order.
    pub fn list(&self) -> Vec<&dyn Tool> {
        let mut all: Vec<&dyn Tool> = self
            .tools
            .values()
            .chain(self.deferred.values())
            .map(|t| t.as_ref())
            .collect();
        all.sort_by(|a, b| a.name().cmp(b.name()));
        all
    }

    /// What is sent with each request: the offered tools in name order, then
    /// [`MORE_TOOLS`] when any tool is held back. The same bytes every time
    /// for the same set of tools.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> =
            self.tools.values().map(|t| t.to_definition()).collect();
        if let Some(more) = self.more_tools_definition() {
            defs.push(more);
        }
        defs
    }

    fn more_tools_definition(&self) -> Option<ToolDefinition> {
        if self.deferred.is_empty() {
            return None;
        }
        let mut description = String::from(
            "Use a tool not listed above. Call with `name` alone to get its parameters, \
             then with `name` and `arguments` to run it. Tools:",
        );
        for tool in self.deferred.values() {
            description.push_str(&format!(
                "\n- {}: {}",
                tool.name(),
                first_sentence(tool.description())
            ));
        }
        Some(ToolDefinition {
            name: MORE_TOOLS.to_string(),
            description,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Tool name" },
                    "arguments": { "type": "object", "description": "The tool's arguments; leave out to get its parameters" }
                },
                "required": ["name"]
            }),
        })
    }

    /// The answer to a [`MORE_TOOLS`] call that asks for `name`'s
    /// parameters: its description and parameter schema.
    pub fn describe(&self, name: &str) -> Result<Value, String> {
        match self.get(name) {
            Some(tool) => Ok(serde_json::json!({
                "name": tool.name(),
                "description": tool.description(),
                "parameters": tool.parameters_schema(),
            })),
            None => Err(format!(
                "No tool named `{name}`. Tools available through {MORE_TOOLS}: {}",
                self.deferred.keys().cloned().collect::<Vec<_>>().join(", ")
            )),
        }
    }
}

impl Default for ToolRegistry {
    /// The full tool set. The tools a coding task uses on most turns (read,
    /// write, edit, search, list, run) are offered with every request; the
    /// rest are held back behind [`MORE_TOOLS`] (see [`ToolRegistry`]).
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(super::ReadFileTool));
        registry.register(Box::new(super::WriteFileTool));
        registry.register(Box::new(super::BashTool::default()));
        registry.register(Box::new(super::GrepTool));
        registry.register(Box::new(super::ListFilesTool));
        registry.register(Box::new(super::GlobTool));
        registry.register(Box::new(super::EditTool));
        registry.register_deferred(Box::new(super::FindPathTool));
        registry.register_deferred(Box::new(super::FetchTool));
        registry.register_deferred(Box::new(super::WebSearchTool));
        registry.register_deferred(Box::new(super::CopyPathTool));
        registry.register_deferred(Box::new(super::MovePathTool));
        registry.register_deferred(Box::new(super::DeletePathTool));
        registry.register_deferred(Box::new(super::CreateDirectoryTool));
        registry.register_deferred(Box::new(super::NowTool));
        registry.register_deferred(Box::new(super::OpenTool));
        registry.register_deferred(Box::new(super::DiagnosticsTool));
        registry.register_deferred(Box::new(super::MemoryTool));
        registry.register_deferred(Box::new(super::BrowseTool));
        registry.register_deferred(Box::new(super::DownloadTool));
        registry.register_deferred(Box::new(super::ScreenshotTool));
        registry.register_deferred(Box::new(super::TodoTool));
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
        registry.register(Box::new(super::BashTool::default()));
        registry.register_deferred(Box::new(super::FindPathTool));
        registry.register_deferred(Box::new(super::NowTool));
        registry.register_deferred(Box::new(super::FetchTool));
        registry.register_deferred(Box::new(super::WebSearchTool));
        registry.register_deferred(Box::new(super::DiagnosticsTool));
        registry.register_deferred(Box::new(super::MemoryTool));
        registry.register_deferred(Box::new(super::BrowseTool));
        registry.register_deferred(Box::new(super::DownloadTool));
        registry.register_deferred(Box::new(super::ScreenshotTool));
        registry.register_deferred(Box::new(super::TodoTool));
        registry
    }

    /// Total number of tools the agent can run, offered and held back.
    pub fn tool_count(&self) -> usize {
        self.tools.len() + self.deferred.len()
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
        let (more, offered) = defs.split_last().unwrap();
        assert_eq!(more.name, MORE_TOOLS);
        let mut sorted = names(offered);
        sorted.sort_unstable();
        assert_eq!(names(offered), sorted);
    }

    #[test]
    fn a_held_back_tool_is_a_line_in_more_tools_not_a_schema() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(super::super::ReadFileTool));
        registry.register_deferred(Box::new(super::super::NowTool));
        registry.register_deferred(Box::new(super::super::FetchTool));

        let defs = registry.definitions();
        assert_eq!(names(&defs), ["read_file", MORE_TOOLS]);
        let listing = &defs[1].description;
        assert!(listing.contains("\n- fetch: "), "{listing}");
        assert!(listing.contains("\n- now: "), "{listing}");
        assert!(listing.find("- fetch").unwrap() < listing.find("- now").unwrap());

        // Still reachable and runnable by name, and counted.
        assert!(registry.get("now").is_some());
        assert!(registry.is_deferred("now"));
        assert!(!registry.is_deferred("read_file"));
        assert_eq!(registry.tool_count(), 3);
        let all: Vec<&str> = registry.list().iter().map(|t| t.name()).collect();
        assert_eq!(all, ["fetch", "now", "read_file"]);
    }

    #[test]
    fn without_held_back_tools_there_is_no_more_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(super::super::ReadFileTool));
        assert_eq!(names(&registry.definitions()), ["read_file"]);
    }

    #[test]
    fn registering_again_moves_a_tool_between_offered_and_held_back() {
        let mut registry = ToolRegistry::new();
        registry.register_deferred(Box::new(super::super::NowTool));
        registry.register(Box::new(super::super::NowTool));
        assert_eq!(names(&registry.definitions()), ["now"]);
        registry.register_deferred(Box::new(super::super::NowTool));
        assert_eq!(names(&registry.definitions()), [MORE_TOOLS]);
        assert_eq!(registry.tool_count(), 1);
    }

    #[test]
    fn describe_gives_the_schema_or_names_what_exists() {
        let mut registry = ToolRegistry::new();
        registry.register_deferred(Box::new(super::super::NowTool));
        let now = registry.describe("now").unwrap();
        assert_eq!(now["name"], "now");
        assert_eq!(now["parameters"], super::super::NowTool.parameters_schema());
        let err = registry.describe("nope").unwrap_err();
        assert!(err.contains("nope") && err.contains("now"), "{err}");
    }

    #[test]
    fn more_tools_calls_parse_in_every_form_models_send() {
        use serde_json::json;
        assert_eq!(
            MoreToolsCall::parse(&json!({"name": "now"})),
            Ok(MoreToolsCall::Describe("now".into()))
        );
        assert_eq!(
            MoreToolsCall::parse(&json!({"name": "now", "arguments": null})),
            Ok(MoreToolsCall::Describe("now".into()))
        );
        assert_eq!(
            MoreToolsCall::parse(&json!({"name": "fetch", "arguments": {"url": "u"}})),
            Ok(MoreToolsCall::Run {
                name: "fetch".into(),
                arguments: json!({"url": "u"})
            })
        );
        assert_eq!(
            MoreToolsCall::parse(&json!({"name": "fetch", "arguments": "{\"url\":\"u\"}"})),
            Ok(MoreToolsCall::Run {
                name: "fetch".into(),
                arguments: json!({"url": "u"})
            })
        );
        assert!(MoreToolsCall::parse(&json!({})).is_err());
        assert!(MoreToolsCall::parse(&json!({"name": "fetch", "arguments": "{oops"})).is_err());
    }

    #[test]
    fn a_long_description_is_cut_to_one_short_line() {
        assert_eq!(first_sentence("Reads a file. Then more."), "Reads a file.");
        assert_eq!(first_sentence("Line one\nline two"), "Line one");
        let long = "x".repeat(300);
        let line = first_sentence(&long);
        assert_eq!(line.chars().count(), 100);
        assert!(line.ends_with('…'));
    }
}
