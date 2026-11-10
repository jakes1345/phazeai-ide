//! `subagent`: hand a self-contained question about the code to a helper.
//!
//! The helper is a fresh agent on the provider's cheaper model
//! ([`crate::Settings::small_model`]) with only read-only file tools and a
//! short prompt of its own. It reads and searches as much as the question
//! needs, and only its answer comes back to the main conversation. A search
//! that would take a dozen reads then costs the main model one short tool
//! result instead of every file it looked at, on every later request, and the
//! reads themselves are billed at the cheaper model's price. The model can ask
//! for several in one turn; they run at the same time.
//!
//! The helper's calls are reported as [`AgentEvent::SubagentUsage`] so a cost
//! meter can count them at the helper model's price.

use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use serde_json::Value;

use super::core::Agent;
use crate::agent_event::AgentEvent;
use crate::error::PhazeError;
use crate::llm::LlmClient;
use crate::tools::{Tool, ToolRegistry, ToolResult};

/// Makes the helper's model client, once per question.
pub type MakeLlm = Arc<dyn Fn() -> Result<Box<dyn LlmClient>, PhazeError> + Send + Sync>;

/// Where events of the agent that owns the tool go while it runs.
pub(crate) type EventSink = Arc<StdMutex<Option<tokio::sync::mpsc::UnboundedSender<AgentEvent>>>>;

/// The most model turns a helper may take for one question.
pub const SUBAGENT_MAX_ITERATIONS: usize = 30;
/// The longest answer passed back, in bytes; a longer one is cut with a note.
const MAX_ANSWER_BYTES: usize = 8 * 1024;

/// How to run helpers: their model and its context budget.
#[derive(Clone)]
pub struct SubagentConfig {
    make_llm: MakeLlm,
    /// The helper's model, for the cost meter and the result line.
    pub model: String,
    /// The helper model's context budget, in tokens.
    pub context_budget: usize,
    /// The project the helper works in, named in its prompt.
    pub root: Option<PathBuf>,
    pub max_iterations: usize,
}

impl std::fmt::Debug for SubagentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentConfig")
            .field("model", &self.model)
            .field("context_budget", &self.context_budget)
            .field("root", &self.root)
            .field("max_iterations", &self.max_iterations)
            .finish()
    }
}

impl SubagentConfig {
    pub fn new(make_llm: MakeLlm, model: impl Into<String>, context_budget: usize) -> Self {
        Self {
            make_llm,
            model: model.into(),
            context_budget,
            root: None,
            max_iterations: SUBAGENT_MAX_ITERATIONS,
        }
    }

    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max.max(1);
        self
    }

    /// The helper's system prompt.
    fn system_prompt(&self) -> String {
        let place = self
            .root
            .as_ref()
            .map(|r| format!(" working in {}", r.display()))
            .unwrap_or_default();
        format!(
            "You are a research helper for a coding agent{place}. Answer the question you are \
             given with your read-only tools; read only what it needs. Reply with the answer \
             alone, no preamble: the facts asked for, with file paths and line numbers. Keep it \
             under 300 words unless the question needs more. If something can't be found, say \
             where you looked."
        )
    }
}

/// The tools a helper gets: reading and searching files, nothing else.
fn helper_tools() -> ToolRegistry {
    let mut tools = ToolRegistry::new();
    tools.register(Box::new(crate::tools::ReadFileTool));
    tools.register(Box::new(crate::tools::GrepTool));
    tools.register(Box::new(crate::tools::GlobTool));
    tools.register(Box::new(crate::tools::ListFilesTool));
    tools.register(Box::new(crate::tools::FindPathTool));
    tools
}

pub struct SubagentTool {
    config: SubagentConfig,
    events: EventSink,
}

impl SubagentTool {
    pub(crate) fn new(config: SubagentConfig, events: EventSink) -> Self {
        Self { config, events }
    }

    async fn answer(&self, question: &str) -> Result<String, PhazeError> {
        let llm = (self.config.make_llm)()?;
        let helper = Agent::new(llm)
            .with_tools(helper_tools())
            .with_system_prompt(self.config.system_prompt())
            .with_context_budget(self.config.context_budget)
            .with_max_iterations(self.config.max_iterations);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = self.events.lock().ok().and_then(|g| g.clone());
        let model = self.config.model.clone();
        // Pass each of the helper's model calls on to the owner's meter as
        // it happens, so a stopped run still counts what it paid for.
        let forward = async move {
            while let Some(event) = rx.recv().await {
                if let (AgentEvent::CallUsage(usage), Some(sink)) = (event, &sink) {
                    let _ = sink.send(AgentEvent::SubagentUsage {
                        model: model.clone(),
                        usage,
                    });
                }
            }
        };
        let (result, ()) = tokio::join!(helper.run_with_events(question, tx), forward);
        let response = result?;
        let answer = response.content.trim();
        if answer.is_empty() {
            return Err(PhazeError::tool(
                "subagent",
                format!(
                    "The helper gave no answer after {} model turns.",
                    response.iterations
                ),
            ));
        }
        let shown = crate::text::truncate_bytes(answer, MAX_ANSWER_BYTES);
        let cut = if shown.len() < answer.len() {
            "\n[... the rest of the helper's answer is cut]"
        } else {
            ""
        };
        Ok(format!(
            "{shown}{cut}\n\n(helper: {} model turns on {})",
            response.iterations, self.config.model
        ))
    }
}

#[async_trait::async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Hand a self-contained question about the code to a helper on a cheaper model; only its \
         answer comes back. Use it for searches that take many reads (where is X handled, how \
         does Y work), so they don't fill this conversation. The helper can only read files, \
         sees nothing of this conversation, and several asked in one turn run together."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question, with everything the helper needs to know"
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, params: Value) -> ToolResult {
        let question = params
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| PhazeError::tool("subagent", "subagent needs a `question`"))?;
        self.answer(question).await.map(Value::String)
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }
}
