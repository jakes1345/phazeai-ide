use crate::error::PhazeError;
use crate::llm::traits::*;
use crate::tools::ToolDefinition;
use futures::channel::mpsc;
use serde::Deserialize;
use serde_json::Value;

pub struct ClaudeClient {
    client: reqwest::Client,
    api_key: String,
    model: String,
    base_url: String,
    /// `None` until set: [`default_max_tokens`] for the model.
    max_tokens: Option<u32>,
    /// `output_config.effort`, for the models that take it: how much the
    /// model thinks and writes. `None`: the model's own default.
    effort: Option<String>,
    /// The clearing horizon of the last agent request
    /// ([`mark_clearing_horizon`]), so the request after a clearing can
    /// read the conversation up to where it was.
    horizon: std::sync::Mutex<Option<Horizon>>,
}

/// The beta header that lets a request ask the API to drop a thinking block
/// whose conversation was edited, instead of refusing the request.
const THINKING_BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";

/// The levels `output_config.effort` takes.
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// A model id without a router's or a cloud's vendor prefix.
fn base_model(model: &str) -> &str {
    model
        .strip_prefix("anthropic/")
        .or_else(|| model.strip_prefix("anthropic."))
        .unwrap_or(model)
}

/// Models whose thinking blocks are tied to the model and the conversation
/// ("preserved thinking"): the API checks that nothing before a block
/// changed since it was written. Phaze edits history when it compacts or
/// trims a conversation, so on these it asks for such blocks to be dropped
/// rather than the request refused (Anthropic's migration guide: Claude
/// Fable 5.1, Opus 5.5, Sonnet 5.5 and Haiku 5.5).
pub fn binds_thinking(model: &str) -> bool {
    let m = base_model(model);
    [
        "claude-fable-5-1",
        "claude-fable-5-2",
        "claude-mythos-5-1",
        "claude-mythos-5-2",
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
    ]
    .iter()
    .any(|p| m.starts_with(p))
}

/// Models that can think before they answer (adaptive thinking): their
/// replies may hold thinking blocks, and thinking counts toward
/// `max_tokens`.
pub fn thinks(model: &str) -> bool {
    let m = base_model(model);
    binds_thinking(m)
        || [
            "claude-fable-5",
            "claude-mythos-5",
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
            "claude-opus-4-6",
            "claude-sonnet-4-6",
        ]
        .iter()
        .any(|p| m.starts_with(p))
}

/// Models that take `output_config.effort` (Claude Haiku 4.5 and the older
/// Sonnets don't).
pub fn takes_effort(model: &str) -> bool {
    thinks(model) || base_model(model).starts_with("claude-opus-4-5")
}

/// The reply length to ask for when none is set: room for thinking as well
/// as the answer on the models that think (thinking counts toward
/// `max_tokens`; asking for more costs nothing, only what is written is
/// billed), 8,192 otherwise.
pub fn default_max_tokens(model: &str) -> u32 {
    if thinks(model) {
        32_000
    } else {
        8_192
    }
}

/// When Claude Sonnet 4.5 retires: 2026-11-30 00:00 UTC, in seconds since
/// the Unix epoch.
const SONNET_4_5_RETIRES: u64 = 1_795_996_800;

/// The id to send for `model`: a Claude model Anthropic has retired (its
/// requests return 404) is sent as its replacement from Anthropic's
/// migration guide, so a saved setting keeps working. Claude Sonnet 4.5 is
/// sent as it is until it retires on November 30, 2026. Every other id is
/// sent as it is.
pub fn current_model_id(model: &str) -> &str {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    current_model_id_at(model, now)
}

fn current_model_id_at(model: &str, now: u64) -> &str {
    const RETIRED: &[(&str, &str)] = &[
        ("claude-opus-4-1-20250805", "claude-opus-5-5"),
        ("claude-opus-4-1", "claude-opus-5-5"),
        ("claude-opus-4-20250514", "claude-opus-5-5"),
        ("claude-opus-4-0", "claude-opus-5-5"),
        ("claude-sonnet-4-20250514", "claude-sonnet-5-5"),
        ("claude-sonnet-4-0", "claude-sonnet-5-5"),
        ("claude-3-haiku-20240307", "claude-haiku-5-5"),
        ("claude-3-7-sonnet-20250219", "claude-sonnet-5-5"),
        ("claude-3-7-sonnet-latest", "claude-sonnet-5-5"),
        ("claude-3-5-haiku-20241022", "claude-haiku-5-5"),
        ("claude-3-5-haiku-latest", "claude-haiku-5-5"),
        ("claude-3-opus-20240229", "claude-opus-5-5"),
        ("claude-3-opus-latest", "claude-opus-5-5"),
        ("claude-3-5-sonnet-20241022", "claude-sonnet-5-5"),
        ("claude-3-5-sonnet-20240620", "claude-sonnet-5-5"),
        ("claude-3-5-sonnet-latest", "claude-sonnet-5-5"),
        ("claude-3-sonnet-20240229", "claude-sonnet-5-5"),
        ("claude-2.1", "claude-opus-5-5"),
        ("claude-2.0", "claude-opus-5-5"),
    ];
    if let Some(&(_, new)) = RETIRED.iter().find(|(old, _)| model == *old) {
        return new;
    }
    let sonnet_4_5 = model == "claude-sonnet-4-5" || model == "claude-sonnet-4-5-20250929";
    if sonnet_4_5 && now >= SONNET_4_5_RETIRES {
        return "claude-sonnet-5-5";
    }
    model
}

impl ClaudeClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: crate::constants::models::DEFAULT_CLAUDE_MODEL.to_string(),
            base_url: crate::constants::endpoints::CLAUDE_BASE_URL.to_string(),
            max_tokens: None,
            effort: None,
            horizon: std::sync::Mutex::new(None),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Connect directly, ignoring `HTTP(S)_PROXY` (for a server on this machine).
    pub fn without_proxy(mut self) -> Self {
        if let Ok(client) = reqwest::Client::builder().no_proxy().build() {
            self.client = client;
        }
        self
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    pub fn with_max_tokens(mut self, max: u32) -> Self {
        self.max_tokens = Some(max);
        self
    }

    /// Ask for `effort` (one of [`EFFORT_LEVELS`]) on the models that take
    /// it; ignored for the others.
    pub fn with_effort(mut self, effort: impl Into<String>) -> Self {
        self.effort = Some(effort.into());
        self
    }

    /// The model id this client sends ([`current_model_id`]).
    fn model_id(&self) -> &str {
        current_model_id(&self.model)
    }

    /// A POST of `body` to the Messages API with this client's headers.
    fn post(&self, url: &str, body: &Value) -> reqwest::RequestBuilder {
        let request = self
            .client
            .post(url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json");
        let request = if binds_thinking(self.model_id()) {
            request.header("anthropic-beta", THINKING_BINDING_BETA)
        } else {
            request
        };
        request.json(body)
    }

    /// `<base_url>/v1/messages`. A trailing slash on the base URL is
    /// tolerated, as `openai_compat_url` tolerates it for the other clients.
    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }

    fn build_request_body(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        stream: bool,
    ) -> Value {
        let system: Vec<Value> = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .flat_map(|m| system_blocks(&m.content))
            .collect();
        let conversation = conversation_messages(messages);

        let model = self.model_id();
        let mut body = serde_json::json!({
            "model": model,
            "max_tokens": self.max_tokens.unwrap_or_else(|| default_max_tokens(model)),
            "messages": conversation,
        });

        if binds_thinking(model) {
            // Adaptive thinking is these models' default; it is spelled out
            // because `block_binding` is accepted only with it. A thinking
            // block from before a compaction or a trim is dropped by the API
            // instead of failing the request.
            body["thinking"] = serde_json::json!({
                "type": "adaptive",
                "block_binding": { "prefix_mismatch_behavior": "drop_block" },
            });
        }
        if let Some(effort) = self.effort.as_deref().filter(|_| takes_effort(model)) {
            body["output_config"] = serde_json::json!({ "effort": effort });
        }

        if !system.is_empty() {
            body["system"] = Value::Array(system);
        }

        if !tools.is_empty() {
            let tool_defs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters,
                    })
                })
                .collect();
            body["tools"] = Value::Array(tool_defs);
        }

        if stream {
            body["stream"] = Value::Bool(true);
        }

        // Only agent requests (with tools) move the horizon: a side request
        // through this client, such as a summary, is another conversation.
        let mut horizon = self.horizon.lock().unwrap_or_else(|e| e.into_inner());
        let now = place_cache_breakpoints(&mut body, *horizon);
        if !tools.is_empty() {
            *horizon = now;
        }
        body
    }
}

/// Anthropic caches a prompt prefix only up to a `cache_control` breakpoint,
/// and takes at most four per request.
const MAX_BREAKPOINTS: usize = 4;

fn ephemeral() -> Value {
    serde_json::json!({ "type": "ephemeral" })
}

/// A system message as content blocks: the part every session shares (the
/// base prompt and the project's instructions), then the part that is this
/// session's own (its git state and the summary of compacted history), each in
/// a block of its own so a breakpoint can sit between them. Empty blocks are
/// left out, as the API refuses them.
fn system_blocks(text: &str) -> Vec<Value> {
    let summary = format!("\n\n{}", crate::context::SUMMARY_HEADING);
    let split = if text.starts_with(crate::context::SUMMARY_HEADING) {
        0
    } else {
        [
            text.find(crate::context::system_prompt::GIT_HEADING),
            text.find(&summary),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(text.len())
    };
    let (shared, own) = text.split_at(split);
    [shared, own.trim_start_matches('\n')]
        .into_iter()
        .filter(|t| !t.trim().is_empty())
        .map(|t| serde_json::json!({ "type": "text", "text": t }))
        .collect()
}

/// A thinking block: what the API sends back unchanged and refuses a
/// `cache_control` on.
fn is_thinking(block: &Value) -> bool {
    matches!(
        block["type"].as_str(),
        Some("thinking" | "redacted_thinking")
    )
}

/// An assistant turn's blocks as the API sent them ([`Message::raw_content`]),
/// when they still say what the message says: the same text and the same
/// tool calls in the same order. A turn Phaze has rewritten since (a trimmed
/// or summarized history) is rebuilt from the message instead, without its
/// thinking, which the API would no longer accept as unchanged anyway.
fn replayed_blocks(m: &Message) -> Option<Vec<Value>> {
    if m.raw_content.is_empty() {
        return None;
    }
    let text: String = m
        .raw_content
        .iter()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    let raw_ids: Vec<&str> = m
        .raw_content
        .iter()
        .filter(|b| b["type"] == "tool_use")
        .filter_map(|b| b["id"].as_str())
        .collect();
    let ids: Vec<&str> = m
        .tool_calls
        .iter()
        .flatten()
        .map(|tc| tc.id.as_str())
        .collect();
    if text != m.content || raw_ids != ids {
        return None;
    }
    Some(
        m.raw_content
            .iter()
            .cloned()
            .map(|mut b| {
                if let Some(o) = b.as_object_mut() {
                    o.remove("cache_control");
                }
                b
            })
            .collect(),
    )
}

/// The conversation in Anthropic's shape. The results of one turn's tool calls
/// go back in a single user message, as the API documents for parallel calls
/// (sending each in a message of its own teaches the model to stop calling
/// tools in parallel).
fn conversation_messages(messages: &[Message]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages.iter().filter(|m| m.role != Role::System) {
        if let Some(ref tool_call_id) = m.tool_call_id {
            let result = serde_json::json!({
                "type": "tool_result",
                "tool_use_id": tool_call_id,
                "content": m.content,
            });
            let previous_is_results = out.last().is_some_and(|prev: &Value| {
                prev["role"] == "user"
                    && prev["content"]
                        .as_array()
                        .is_some_and(|c| c.iter().all(|b| b["type"] == "tool_result"))
            });
            if previous_is_results {
                if let Some(content) = out
                    .last_mut()
                    .and_then(|prev| prev["content"].as_array_mut())
                {
                    content.push(result);
                }
            } else {
                out.push(serde_json::json!({ "role": "user", "content": [result] }));
            }
        } else if let Some(blocks) = (m.role == Role::Assistant)
            .then(|| replayed_blocks(m))
            .flatten()
        {
            out.push(serde_json::json!({ "role": "assistant", "content": blocks }));
        } else if let Some(ref tool_calls) = m.tool_calls {
            let mut content: Vec<Value> = Vec::new();
            if !m.content.is_empty() {
                content.push(serde_json::json!({
                    "type": "text",
                    "text": m.content,
                }));
            }
            for tc in tool_calls {
                let args: Value = serde_json::from_str(&tc.function.arguments)
                    .unwrap_or(Value::Object(Default::default()));
                content.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tc.id,
                    "name": tc.function.name,
                    "input": args,
                }));
            }
            out.push(serde_json::json!({
                "role": "assistant",
                "content": content,
            }));
        } else if !m.images.is_empty() {
            // Pictures come before the words about them, as Anthropic
            // recommends; an empty text block is refused, so it's left out.
            let mut content: Vec<Value> = m
                .images
                .iter()
                .map(|img| {
                    serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": img.media_type,
                            "data": img.data,
                        },
                    })
                })
                .collect();
            if !m.content.is_empty() {
                content.push(serde_json::json!({ "type": "text", "text": m.content }));
            }
            out.push(serde_json::json!({ "role": m.role, "content": content }));
        } else {
            out.push(serde_json::json!({
                "role": m.role,
                "content": m.content,
            }));
        }
    }
    out
}

/// Mark the prefixes worth caching, in render order (tools, then system, then
/// messages): the last tool definition (the tool schemas, the same for every
/// session), the first and last system blocks (the shared base prompt, then
/// this session's own part) and the conversation tail, which moves forward
/// after each tool result so every request reads what the one before it
/// wrote. This is the placement opencode uses by default
/// (`packages/ai/src/cache-policy.ts`).
///
/// One more marks the clearing horizon (see [`mark_clearing_horizon`]) once
/// the conversation has a result that a clearing could replace. When that
/// would make five, the tool schemas give up theirs: the first system block
/// caches them too, and only a change to the shared prompt (another project,
/// or an edited instructions file) leaves the schemas to be read alone.
fn place_cache_breakpoints(body: &mut Value, last: Option<Horizon>) -> Option<Horizon> {
    let system_marks = body
        .get("system")
        .and_then(Value::as_array)
        .map_or(0, |s| s.len().min(2));
    let mut horizon = (false, None);
    if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
        horizon = mark_clearing_horizon(messages, last);
        mark_conversation_tail(messages);
    }
    let tools_fit = system_marks + 1 + usize::from(horizon.0) < MAX_BREAKPOINTS;
    if tools_fit {
        if let Some(last_tool) = body
            .get_mut("tools")
            .and_then(Value::as_array_mut)
            .and_then(|tools| tools.last_mut())
        {
            last_tool["cache_control"] = ephemeral();
        }
    }
    if let Some(system) = body.get_mut("system").and_then(Value::as_array_mut) {
        let last = system.len().saturating_sub(1);
        for i in [0, last] {
            if let Some(block) = system.get_mut(i) {
                block["cache_control"] = ephemeral();
            }
        }
    }
    horizon.1
}

/// Where a clearing horizon was: the block (`message`, `block` in the
/// request's `messages`) and a hash of the conversation up to and including
/// it, so a later request can tell whether that prefix is still the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Horizon {
    message: usize,
    block: usize,
    hash: u64,
}

impl Horizon {
    fn at(messages: &[Value], message: usize, block: usize) -> Option<Self> {
        let blocks = messages.get(message)?["content"].as_array()?;
        blocks.get(block)?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for m in &messages[..message] {
            std::hash::Hash::hash(&m.to_string(), &mut h);
        }
        std::hash::Hash::hash(&messages[message]["role"].to_string(), &mut h);
        for b in &blocks[..=block] {
            std::hash::Hash::hash(&b.to_string(), &mut h);
        }
        Some(Self {
            message,
            block,
            hash: std::hash::Hasher::finish(&h),
        })
    }

    /// Whether `messages` still hold the same conversation up to this block.
    fn holds(&self, messages: &[Value]) -> bool {
        Self::at(messages, self.message, self.block) == Some(*self)
    }
}

/// Put a breakpoint on the block just before the oldest tool result that a
/// clearing could still replace (`ConversationHistory::clear_old_tool_results`),
/// and say whether one was placed.
///
/// A clearing changes the conversation from that result on, and Anthropic
/// reads a cached prefix only where an earlier request put a breakpoint (or
/// within 20 blocks before one of this request's). The tail breakpoints of
/// earlier requests all end past that result, so without this one a
/// clearing read only the system prompt from cache and wrote the whole
/// conversation again at 1.25x. With it, every request since the result
/// arrived has cached the conversation up to it, and a clearing reads that
/// part. Until the next clearing the horizon stays put, so marking it costs
/// nothing: its prefix is already read from cache.
fn mark_clearing_horizon(messages: &mut [Value], last: Option<Horizon>) -> (bool, Option<Horizon>) {
    let mut names: std::collections::HashMap<String, String> = Default::default();
    let mut oldest = None;
    'find: for (i, message) in messages.iter().enumerate() {
        let Some(blocks) = message["content"].as_array() else {
            continue;
        };
        for (j, block) in blocks.iter().enumerate() {
            match block["type"].as_str() {
                Some("tool_use") => {
                    if let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str()) {
                        names.insert(id.to_string(), name.to_string());
                    }
                }
                Some("tool_result") => {
                    let tool = block["tool_use_id"]
                        .as_str()
                        .and_then(|id| names.get(id))
                        .map(String::as_str);
                    let content = block["content"].as_str().unwrap_or_default();
                    if crate::context::clearable(tool, content) {
                        oldest = Some((i, j));
                        break 'find;
                    }
                }
                _ => {}
            }
        }
    }
    // The block before it: an earlier result in the same message, or the
    // last block of the message before (the call that asked for it).
    let now = oldest.and_then(|(i, j)| match j {
        0 if i > 0 => {
            let before = messages[i - 1]["content"]
                .as_array()?
                .len()
                .checked_sub(1)?;
            Horizon::at(messages, i - 1, before)
        }
        0 => None,
        _ => Horizon::at(messages, i, j - 1),
    });
    // Right after a clearing the horizon has moved on, and the cache holds
    // the conversation only up to where it was: that is the breakpoint this
    // request reads from. The next request marks the new one.
    let mark = match (last, now) {
        (Some(old), Some(new)) if (old.message, old.block) < (new.message, new.block) => {
            Some(old).filter(|o| o.holds(messages)).or(Some(new))
        }
        (Some(old), None) => Some(old).filter(|o| o.holds(messages)),
        (_, new) => new,
    };
    let placed = mark.is_some_and(|h| {
        let block = &mut messages[h.message]["content"][h.block];
        let markable = !is_thinking(block)
            && (block["type"] != "text" || block["text"].as_str().is_some_and(|t| !t.is_empty()));
        if markable {
            block["cache_control"] = ephemeral();
        }
        markable
    });
    (placed, now)
}

/// Put a breakpoint on the last block of the conversation that can carry one.
/// A plain string content becomes a text block, since only blocks take
/// `cache_control`; an empty text (which the API refuses as a block) is passed
/// over for the message before it.
fn mark_conversation_tail(messages: &mut [Value]) {
    for message in messages.iter_mut().rev() {
        let content = &mut message["content"];
        if let Some(text) = content.as_str() {
            if text.is_empty() {
                continue;
            }
            *content = serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": ephemeral(),
            }]);
            return;
        }
        let block = content.as_array_mut().and_then(|blocks| {
            blocks.iter_mut().rev().find(|b| {
                !is_thinking(b)
                    && (b["type"] != "text" || b["text"].as_str().is_some_and(|t| !t.is_empty()))
            })
        });
        if let Some(block) = block {
            block["cache_control"] = ephemeral();
            return;
        }
    }
}

/// What the user is told when Claude's safety classifiers decline a request
/// (`stop_reason: "refusal"`, which newer models can return).
const REFUSAL_MESSAGE: &str =
    "Claude declined this request (its safety checks stopped the reply). \
     Rephrase it, or try another model.";

/// Append `text` to the string field `field` of the block at `index`.
fn append_field(
    blocks: &mut std::collections::BTreeMap<u64, Value>,
    index: u64,
    field: &str,
    text: &str,
) {
    if let Some(block) = blocks.get_mut(&index).and_then(Value::as_object_mut) {
        match block.get_mut(field) {
            Some(Value::String(value)) => value.push_str(text),
            _ => {
                block.insert(field.to_string(), Value::String(text.to_string()));
            }
        }
    }
}

/// Anthropic's token counts as a [`Usage`]. Its `input_tokens` is only the
/// part of the prompt after the last cache breakpoint that was neither read
/// from nor written to the cache, so the whole prompt is the sum of the three.
///
/// A stream reports the counts in two events: `message_start` carries the
/// input side and `message_delta` the output (and, from newer API versions,
/// the input side again, cumulatively). Fields in `delta` win over `start`.
fn claude_usage(start: Option<&Value>, delta: Option<&Value>) -> Option<Usage> {
    let field = |key: &str| -> u32 {
        delta
            .and_then(|d| d.get(key))
            .and_then(Value::as_u64)
            .or_else(|| start.and_then(|s| s.get(key)).and_then(Value::as_u64))
            .unwrap_or(0) as u32
    };
    let uncached = field("input_tokens");
    let read = field("cache_read_input_tokens");
    let written = field("cache_creation_input_tokens");
    let usage = Usage {
        input_tokens: uncached + read + written,
        output_tokens: field("output_tokens"),
        cache_read_tokens: read,
        cache_write_tokens: written,
    };
    (usage.input_tokens > 0 || usage.output_tokens > 0).then_some(usage)
}

#[derive(Debug, Deserialize)]
struct ClaudeApiResponse {
    content: Vec<Value>,
    usage: Option<Value>,
    #[serde(default)]
    stop_reason: Option<String>,
}

#[async_trait::async_trait]
impl LlmClient for ClaudeClient {
    async fn chat(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, PhazeError> {
        let url = self.messages_url();
        let request_body = self.build_request_body(messages, tools, false);

        let response =
            crate::llm::retry::send_with_retry("claude", || self.post(&url, &request_body).send())
                .await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            return Err(PhazeError::Llm(format!(
                "Claude API error ({}): {}",
                status, response_text
            )));
        }

        let api_response: ClaudeApiResponse = serde_json::from_str(&response_text)
            .map_err(|e| PhazeError::Llm(format!("Failed to parse response: {e}")))?;
        if api_response.stop_reason.as_deref() == Some("refusal") {
            return Err(PhazeError::Llm(REFUSAL_MESSAGE.to_string()));
        }

        // Every text block, in order: a model that thinks can put text
        // between its thinking and its tool calls.
        let content: String = api_response
            .content
            .iter()
            .filter(|c| c["type"] == "text")
            .filter_map(|c| c["text"].as_str())
            .collect();

        let tool_calls: Vec<ToolCall> = api_response
            .content
            .iter()
            .filter(|c| c["type"] == "tool_use")
            .map(|c| ToolCall {
                id: c["id"].as_str().unwrap_or_default().to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: c["name"].as_str().unwrap_or_default().to_string(),
                    arguments: serde_json::to_string(&c["input"]).unwrap_or_default(),
                },
            })
            .collect();

        let message = if tool_calls.is_empty() {
            Message::assistant(content)
        } else {
            Message::assistant_with_tools(content, tool_calls)
        };
        let message = if api_response.content.iter().any(is_thinking) {
            message.with_raw_content(api_response.content.clone())
        } else {
            message
        };

        Ok(LlmResponse {
            message,
            usage: claude_usage(None, api_response.usage.as_ref()),
        })
    }

    async fn chat_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<mpsc::UnboundedReceiver<StreamEvent>, PhazeError> {
        let url = self.messages_url();
        let request_body = self.build_request_body(messages, tools, true);

        // Retry only the request submission; once the SSE stream starts we
        // can't safely re-issue without duplicating tokens.
        let response =
            crate::llm::retry::send_with_retry("claude", || self.post(&url, &request_body).send())
                .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(PhazeError::Llm(format!(
                "Claude API error ({}): {}",
                status, text
            )));
        }

        let (tx, rx) = mpsc::unbounded();

        let mut stream = response.bytes_stream();
        tokio::spawn(async move {
            let mut buffer = String::new();
            // Maps content_block index → tool_use id, so delta events can find their tool call
            let mut tool_block_ids: std::collections::HashMap<u64, String> =
                std::collections::HashMap::new();
            // The input-side counts arrive in `message_start`, the output in
            // `message_delta`; one Usage event is sent from the two.
            let mut start_usage: Option<Value> = None;
            let mut usage_sent = false;
            // The turn's content blocks by index, as the API builds them, for
            // a turn that holds thinking blocks (`StreamEvent::RawContent`).
            let mut blocks: std::collections::BTreeMap<u64, Value> =
                std::collections::BTreeMap::new();
            let mut tool_inputs: std::collections::HashMap<u64, String> =
                std::collections::HashMap::new();

            while let Some(chunk) =
                super::stream_cancel::next_unless_dropped(&mut stream, &tx).await
            {
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.unbounded_send(StreamEvent::Error(e.to_string()));
                        break;
                    }
                };

                buffer.push_str(&String::from_utf8_lossy(&chunk));

                // Process SSE lines
                while let Some(line_end) = buffer.find('\n') {
                    let line = buffer[..line_end].trim().to_string();
                    buffer = buffer[line_end + 1..].to_string();

                    if line.is_empty() || !line.starts_with("data: ") {
                        continue;
                    }

                    let data = &line[6..];
                    if data == "[DONE]" {
                        let _ = tx.unbounded_send(StreamEvent::Done);
                        return;
                    }

                    if let Ok(event) = serde_json::from_str::<Value>(data) {
                        let event_type = event.get("type").and_then(|t| t.as_str());
                        let index = event.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                        match event_type {
                            Some("content_block_delta") => {
                                if let Some(delta) = event.get("delta") {
                                    let delta_type = delta.get("type").and_then(|t| t.as_str());
                                    match delta_type {
                                        Some("text_delta") => {
                                            if let Some(text) =
                                                delta.get("text").and_then(|t| t.as_str())
                                            {
                                                append_field(&mut blocks, index, "text", text);
                                                let _ = tx.unbounded_send(StreamEvent::TextDelta(
                                                    text.to_string(),
                                                ));
                                            }
                                        }
                                        Some("thinking_delta") => {
                                            if let Some(text) =
                                                delta.get("thinking").and_then(|t| t.as_str())
                                            {
                                                append_field(&mut blocks, index, "thinking", text);
                                            }
                                        }
                                        Some("signature_delta") => {
                                            if let Some(sig) =
                                                delta.get("signature").and_then(|t| t.as_str())
                                            {
                                                append_field(&mut blocks, index, "signature", sig);
                                            }
                                        }
                                        Some("input_json_delta") => {
                                            if let Some(partial) =
                                                delta.get("partial_json").and_then(|t| t.as_str())
                                            {
                                                tool_inputs
                                                    .entry(index)
                                                    .or_default()
                                                    .push_str(partial);
                                                if let Some(id) = tool_block_ids.get(&index) {
                                                    let _ = tx.unbounded_send(
                                                        StreamEvent::ToolCallDelta {
                                                            id: id.clone(),
                                                            arguments_delta: partial.to_string(),
                                                        },
                                                    );
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            Some("content_block_start") => {
                                if let Some(cb) = event.get("content_block") {
                                    blocks.insert(index, cb.clone());
                                    if cb.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                                        let id = cb
                                            .get("id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        let name = cb
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("")
                                            .to_string();
                                        tool_block_ids.insert(index, id.clone());
                                        let _ = tx.unbounded_send(StreamEvent::ToolCallStart {
                                            id,
                                            name,
                                        });
                                    }
                                }
                            }
                            Some("content_block_stop") => {
                                if let Some(block) = blocks.get_mut(&index) {
                                    if block["type"] == "tool_use" {
                                        let json = tool_inputs.remove(&index).unwrap_or_default();
                                        block["input"] = serde_json::from_str(&json)
                                            .unwrap_or_else(|_| serde_json::json!({}));
                                    }
                                }
                                if let Some(id) = tool_block_ids.remove(&index) {
                                    let _ = tx.unbounded_send(StreamEvent::ToolCallEnd { id });
                                }
                            }
                            Some("message_start") => {
                                start_usage = event.pointer("/message/usage").cloned();
                            }
                            Some("message_delta") => {
                                if event.pointer("/delta/stop_reason").and_then(Value::as_str)
                                    == Some("refusal")
                                {
                                    let _ = tx.unbounded_send(StreamEvent::Error(
                                        REFUSAL_MESSAGE.to_string(),
                                    ));
                                    return;
                                }
                                if let Some(usage) =
                                    claude_usage(start_usage.as_ref(), event.get("usage"))
                                {
                                    usage_sent = true;
                                    let _ = tx.unbounded_send(StreamEvent::Usage(usage));
                                }
                            }
                            Some("message_stop") => {
                                if blocks.values().any(is_thinking) {
                                    let _ = tx.unbounded_send(StreamEvent::RawContent(
                                        std::mem::take(&mut blocks).into_values().collect(),
                                    ));
                                }
                                if !usage_sent {
                                    if let Some(usage) = claude_usage(start_usage.as_ref(), None) {
                                        let _ = tx.unbounded_send(StreamEvent::Usage(usage));
                                    }
                                }
                                let _ = tx.unbounded_send(StreamEvent::Done);
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }

            let _ = tx.unbounded_send(StreamEvent::Done);
        });

        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::system_prompt::GIT_HEADING;
    use crate::context::SUMMARY_HEADING;
    use serde_json::json;

    fn tool(name: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.into(),
            description: format!("{name} things"),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            call_type: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
        }
    }

    fn breakpoints(body: &Value) -> Vec<String> {
        let mut found = Vec::new();
        for (i, t) in body["tools"].as_array().into_iter().flatten().enumerate() {
            if t.get("cache_control").is_some() {
                found.push(format!("tools[{i}]"));
            }
        }
        for (i, b) in body["system"].as_array().into_iter().flatten().enumerate() {
            if b.get("cache_control").is_some() {
                found.push(format!("system[{i}]"));
            }
        }
        for (i, m) in body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            for (j, b) in m["content"].as_array().into_iter().flatten().enumerate() {
                if b.get("cache_control").is_some() {
                    found.push(format!("messages[{i}][{j}]"));
                }
            }
        }
        found
    }

    #[test]
    fn the_shared_prompt_the_sessions_own_part_and_the_tail_are_cached() {
        let base = "You are Phaze.\n\n## Project-Specific Instructions\nUse tabs.";
        let system =
            format!("{base}{GIT_HEADING}- Git branch: main\n\n{SUMMARY_HEADING}\nFixed the port.");
        let messages = [
            Message::system(system),
            Message::user("Add a test"),
            Message::assistant_with_tools("", vec![call("c1", "read_file"), call("c2", "grep")]),
            Message::tool_result("c1", "fn main() {}"),
            Message::tool_result("c2", "src/main.rs:1"),
        ];
        let client = ClaudeClient::new("k");
        let body = client.build_request_body(&messages, &[tool("grep"), tool("read_file")], true);

        // The base prompt is a block of its own, so its breakpoint is shared
        // by every session; the git state and summary follow it.
        assert_eq!(body["system"][0]["text"], base);
        assert_eq!(
            body["system"][1]["text"],
            format!("## Git Status\n- Git branch: main\n\n{SUMMARY_HEADING}\nFixed the port.")
        );
        // Both tool results go back in one user message.
        assert_eq!(body["messages"].as_array().unwrap().len(), 3);
        assert_eq!(body["messages"][2]["role"], "user");
        assert_eq!(body["messages"][2]["content"][1]["tool_use_id"], "c2");
        assert_eq!(
            breakpoints(&body),
            ["tools[1]", "system[0]", "system[1]", "messages[2][1]"]
        );
    }

    /// A conversation with three tool rounds: `read_file` results big enough
    /// to clear, the first `cleared` of them already cleared.
    fn rounds(cleared: usize) -> Vec<Message> {
        let big = "x".repeat(crate::context::MIN_CLEARED_BYTES);
        let mut messages = vec![
            Message::system(format!("You are Phaze.{GIT_HEADING}- Git branch: main")),
            Message::user("Read the files"),
        ];
        for n in 1..=3 {
            let id = format!("c{n}");
            messages.push(Message::assistant_with_tools(
                "",
                vec![call(&id, "read_file")],
            ));
            let text = if n <= cleared {
                format!("{}read_file c{n}]", crate::context::CLEARED_PREFIX)
            } else {
                big.clone()
            };
            messages.push(Message::tool_result(&id, text));
        }
        messages
    }

    #[test]
    fn the_block_before_the_oldest_clearable_result_is_cached() {
        let client = ClaudeClient::new("k");
        let body = client.build_request_body(&rounds(0), &[tool("read_file")], true);
        // The call that asked for the first result, so a clearing that
        // starts there reads everything before it. Four breakpoints at most:
        // the tool schemas give theirs up, the first system block holds them.
        assert_eq!(
            breakpoints(&body),
            ["system[0]", "system[1]", "messages[1][0]", "messages[6][0]"]
        );
    }

    #[test]
    fn the_request_after_a_clearing_reads_up_to_the_old_horizon() {
        let client = ClaudeClient::new("k");
        let tools = [tool("read_file")];
        let before = client.build_request_body(&rounds(0), &tools, true);
        assert!(breakpoints(&before).contains(&"messages[1][0]".to_string()));
        // The first two results were cleared: the cache holds the
        // conversation up to the first one's call, and that is read.
        let after = client.build_request_body(&rounds(2), &tools, true);
        assert_eq!(
            breakpoints(&after),
            ["system[0]", "system[1]", "messages[1][0]", "messages[6][0]"]
        );
        // From the next request on, the horizon is the new oldest result's
        // call.
        let next = client.build_request_body(&rounds(2), &tools, true);
        assert_eq!(
            breakpoints(&next),
            ["system[0]", "system[1]", "messages[5][0]", "messages[6][0]"]
        );
    }

    #[test]
    fn an_old_horizon_in_a_changed_conversation_is_not_reused() {
        let client = ClaudeClient::new("k");
        let tools = [tool("read_file")];
        client.build_request_body(&rounds(0), &tools, true);
        // Another conversation through the same client: its first message
        // differs, so the old horizon's prefix is not cached for it.
        let mut other = rounds(2);
        other[1] = Message::user("Something else");
        let body = client.build_request_body(&other, &tools, true);
        assert_eq!(
            breakpoints(&body),
            ["system[0]", "system[1]", "messages[5][0]", "messages[6][0]"]
        );
    }

    #[test]
    fn small_and_checklist_results_set_no_horizon() {
        let messages = [
            Message::system("You are Phaze."),
            Message::user("Plan it"),
            Message::assistant_with_tools("", vec![call("t", "todo"), call("g", "grep")]),
            Message::tool_result("t", "y".repeat(2_000)),
            Message::tool_result("g", "src/main.rs:1"),
        ];
        let body = ClaudeClient::new("k").build_request_body(&messages, &[tool("todo")], true);
        assert_eq!(
            breakpoints(&body),
            ["tools[0]", "system[0]", "messages[2][1]"]
        );
    }

    #[test]
    fn a_prompt_with_nothing_session_specific_has_one_system_block() {
        let messages = [Message::system("You are Phaze."), Message::user("hi")];
        let body = ClaudeClient::new("k").build_request_body(&messages, &[], false);
        assert_eq!(body["system"].as_array().unwrap().len(), 1);
        // A string message becomes a text block so it can carry the marker.
        assert_eq!(
            body["messages"][0]["content"],
            json!([{ "type": "text", "text": "hi", "cache_control": { "type": "ephemeral" } }])
        );
        assert_eq!(breakpoints(&body), ["system[0]", "messages[0][0]"]);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn a_summary_without_a_system_prompt_is_the_only_block() {
        let summary = format!("{SUMMARY_HEADING}\nEarlier work.");
        let blocks = system_blocks(&summary);
        assert_eq!(blocks, vec![json!({ "type": "text", "text": summary })]);
    }

    #[test]
    fn the_tail_marker_skips_an_empty_reply() {
        let messages = [Message::user("hi"), Message::assistant("")];
        let body = ClaudeClient::new("k").build_request_body(&messages, &[], false);
        assert_eq!(breakpoints(&body), ["messages[0][0]"]);
        assert_eq!(body["messages"][1]["content"], "");
    }

    #[test]
    fn no_request_has_more_than_four_breakpoints() {
        let messages = [
            Message::system(format!("Base{GIT_HEADING}- Git branch: x")),
            Message::user("go"),
        ];
        let tools: Vec<ToolDefinition> = (0..30).map(|i| tool(&format!("t{i:02}"))).collect();
        let body = ClaudeClient::new("k").build_request_body(&messages, &tools, true);
        assert_eq!(breakpoints(&body).len(), MAX_BREAKPOINTS);
    }

    #[test]
    fn usage_counts_the_whole_prompt_with_its_cached_parts() {
        let start = json!({
            "input_tokens": 12,
            "cache_creation_input_tokens": 300,
            "cache_read_input_tokens": 4000,
            "output_tokens": 1,
        });
        let delta = json!({ "output_tokens": 57 });
        assert_eq!(
            claude_usage(Some(&start), Some(&delta)),
            Some(Usage {
                input_tokens: 4312,
                output_tokens: 57,
                cache_read_tokens: 4000,
                cache_write_tokens: 300,
            })
        );
        // A delta that repeats the input side (newer API versions) wins.
        let delta =
            json!({ "input_tokens": 12, "cache_read_input_tokens": 4100, "output_tokens": 9 });
        let usage = claude_usage(Some(&start), Some(&delta)).unwrap();
        assert_eq!((usage.input_tokens, usage.cache_read_tokens), (4412, 4100));
        assert_eq!(claude_usage(None, None), None);
    }

    /// Serve one SSE response on a local port and return its base URL.
    async fn serve_once(body: String) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = sock.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len: usize = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().to_string())
                        })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(response.as_bytes()).await.unwrap();
        });
        url
    }

    #[tokio::test]
    async fn a_stream_reports_one_usage_from_its_start_and_delta_events() {
        let events = [
            json!({ "type": "message_start", "message": { "usage": {
                "input_tokens": 20, "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": 3700, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Done." } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" }, "usage": { "output_tokens": 3 } }),
            json!({ "type": "message_stop" }),
        ];
        let body: String = events
            .iter()
            .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
            .collect();
        let url = serve_once(body).await;
        let client = ClaudeClient::new("k").with_base_url(url).without_proxy();
        let mut rx = client
            .chat_stream(&[Message::user("hi")], &[])
            .await
            .unwrap();
        use futures::StreamExt;
        let mut usages = Vec::new();
        let mut text = String::new();
        while let Some(event) = rx.next().await {
            match event {
                StreamEvent::Usage(u) => usages.push(u),
                StreamEvent::TextDelta(t) => text.push_str(&t),
                StreamEvent::Done => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(text, "Done.");
        assert_eq!(
            usages,
            [Usage {
                input_tokens: 3720,
                output_tokens: 3,
                cache_read_tokens: 3700,
                cache_write_tokens: 0,
            }]
        );
    }

    #[tokio::test]
    async fn dropping_the_stream_closes_the_provider_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (closed_tx, closed) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 64 * 1024];
            let _ = sock.read(&mut buf).await;
            let first = json!({ "type": "content_block_delta", "index": 0,
                "delta": { "type": "text_delta", "text": "Hel" } });
            let first = format!("event: content_block_delta\ndata: {first}\n\n");
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";
            sock.write_all(head.as_bytes()).await.unwrap();
            let part = format!("{:x}\r\n{first}\r\n", first.len());
            sock.write_all(part.as_bytes()).await.unwrap();
            // Still generating: send nothing more until the client hangs up.
            loop {
                match sock.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            let _ = closed_tx.send(());
        });
        let client = ClaudeClient::new("k").with_base_url(url).without_proxy();
        let mut rx = client
            .chat_stream(&[Message::user("hi")], &[])
            .await
            .unwrap();
        use futures::StreamExt;
        match rx.next().await {
            Some(StreamEvent::TextDelta(t)) => assert_eq!(t, "Hel"),
            other => panic!("expected the first text, got {other:?}"),
        }
        // What the agent does on Stop.
        drop(rx);
        tokio::time::timeout(std::time::Duration::from_secs(3), closed)
            .await
            .expect("the request was still open 3 s after Stop")
            .unwrap();
    }

    fn sse(events: &[Value]) -> String {
        events
            .iter()
            .map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap()))
            .collect()
    }

    /// [`serve_once`] that also hands back the request it was sent (head
    /// and body).
    async fn serve_once_capturing(
        body: String,
        content_type: &'static str,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0u8; 8192];
            loop {
                let n = sock.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&request).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let len: usize = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().to_string())
                        })
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&request).to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(response.as_bytes()).await.unwrap();
        });
        (url, rx)
    }

    fn request_json(request: &str) -> Value {
        let body = &request[request.find("\r\n\r\n").unwrap() + 4..];
        serde_json::from_str(body).unwrap()
    }

    /// A turn the way Claude Sonnet 5.5 streams one: a thinking block with
    /// its signature, a sentence, then a tool call.
    fn thinking_turn() -> Vec<Value> {
        vec![
            json!({ "type": "message_start", "message": { "usage": { "input_tokens": 9, "output_tokens": 1 } } }),
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "thinking", "thinking": "", "signature": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "Read the " } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "thinking_delta", "thinking": "file first." } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "signature_delta", "signature": "c2lnLTE=" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "text_delta", "text": "Looking." } }),
            json!({ "type": "content_block_stop", "index": 1 }),
            json!({ "type": "content_block_start", "index": 2, "content_block": { "type": "tool_use", "id": "toolu_1", "name": "read_file", "input": {} } }),
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "{\"path\":" } }),
            json!({ "type": "content_block_delta", "index": 2, "delta": { "type": "input_json_delta", "partial_json": "\"a.rs\"}" } }),
            json!({ "type": "content_block_stop", "index": 2 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "tool_use" }, "usage": { "output_tokens": 40 } }),
            json!({ "type": "message_stop" }),
        ]
    }

    fn thinking_blocks() -> Vec<Value> {
        vec![
            json!({ "type": "thinking", "thinking": "Read the file first.", "signature": "c2lnLTE=" }),
            json!({ "type": "text", "text": "Looking." }),
            json!({ "type": "tool_use", "id": "toolu_1", "name": "read_file", "input": { "path": "a.rs" } }),
        ]
    }

    #[tokio::test]
    async fn a_thinking_turn_comes_back_whole_and_the_request_asks_to_drop_stale_blocks() {
        let (url, request) = serve_once_capturing(sse(&thinking_turn()), "text/event-stream").await;
        let client = ClaudeClient::new("k")
            .with_model("claude-sonnet-5-5")
            .with_base_url(url)
            .without_proxy();
        let mut rx = client
            .chat_stream(&[Message::user("fix a.rs")], &[tool("read_file")])
            .await
            .unwrap();
        use futures::StreamExt;
        let mut raw = None;
        let mut text = String::new();
        let mut args = String::new();
        while let Some(event) = rx.next().await {
            match event {
                StreamEvent::RawContent(blocks) => raw = Some(blocks),
                StreamEvent::TextDelta(t) => text.push_str(&t),
                StreamEvent::ToolCallDelta {
                    arguments_delta, ..
                } => args.push_str(&arguments_delta),
                StreamEvent::Done => break,
                _ => {}
            }
        }
        assert_eq!(raw, Some(thinking_blocks()));
        // The thinking is not shown as the reply.
        assert_eq!(text, "Looking.");
        assert_eq!(args, r#"{"path":"a.rs"}"#);

        let request = request.await.unwrap();
        assert!(request
            .to_ascii_lowercase()
            .contains(&format!("anthropic-beta: {THINKING_BINDING_BETA}")));
        let body = request_json(&request);
        assert_eq!(body["model"], "claude-sonnet-5-5");
        assert_eq!(
            body["thinking"],
            json!({ "type": "adaptive", "block_binding": { "prefix_mismatch_behavior": "drop_block" } })
        );
        assert_eq!(body["max_tokens"], 32_000);
        assert!(body.get("output_config").is_none());
    }

    #[tokio::test]
    async fn a_turn_without_thinking_sends_no_raw_content() {
        let events = [
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hi." } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_stop" }),
        ];
        let (url, request) = serve_once_capturing(sse(&events), "text/event-stream").await;
        let client = ClaudeClient::new("k")
            .with_model("claude-haiku-4-5-20251001")
            .with_effort("low")
            .with_base_url(url)
            .without_proxy();
        let mut rx = client
            .chat_stream(&[Message::user("hi")], &[])
            .await
            .unwrap();
        use futures::StreamExt;
        while let Some(event) = rx.next().await {
            assert!(!matches!(event, StreamEvent::RawContent(_)), "{event:?}");
            if matches!(event, StreamEvent::Done) {
                break;
            }
        }
        // An older model gets neither the thinking settings nor an effort it
        // would refuse, nor the beta header.
        let request = request.await.unwrap();
        assert!(!request.to_ascii_lowercase().contains("anthropic-beta"));
        let body = request_json(&request);
        assert!(body.get("thinking").is_none());
        assert!(body.get("output_config").is_none());
        assert_eq!(body["max_tokens"], 8_192);
    }

    #[test]
    fn effort_goes_to_the_models_that_take_it() {
        let messages = [Message::user("hi")];
        let body = ClaudeClient::new("k")
            .with_model("claude-haiku-5-5")
            .with_effort("low")
            .build_request_body(&messages, &[], false);
        assert_eq!(body["output_config"], json!({ "effort": "low" }));
        let body = ClaudeClient::new("k")
            .with_model("claude-opus-5-5")
            .with_max_tokens(64_000)
            .build_request_body(&messages, &[], false);
        assert!(body.get("output_config").is_none());
        assert_eq!(body["max_tokens"], 64_000);
        // Claude Opus 5 thinks but does not bind its blocks.
        let body = ClaudeClient::new("k")
            .with_model("claude-opus-5")
            .build_request_body(&messages, &[], false);
        assert!(body.get("thinking").is_none());
        assert_eq!(body["max_tokens"], 32_000);
    }

    #[test]
    fn a_thinking_turn_is_sent_back_unchanged_and_never_carries_the_cache_marker() {
        let mut turn = Message::assistant_with_tools(
            "Looking.",
            vec![ToolCall {
                id: "toolu_1".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "read_file".into(),
                    arguments: r#"{"path":"a.rs"}"#.into(),
                },
            }],
        )
        .with_raw_content(thinking_blocks());
        let messages = [
            Message::user("fix a.rs"),
            turn.clone(),
            Message::tool_result("toolu_1", "1\tfn a() {}"),
        ];
        let body = ClaudeClient::new("k")
            .with_model("claude-sonnet-5-5")
            .build_request_body(&messages, &[], true);
        assert_eq!(body["messages"][1]["content"], json!(thinking_blocks()));
        assert_eq!(breakpoints(&body), ["messages[2][0]"]);

        // A final answer that ends the conversation: the marker goes on its
        // text, not on its thinking.
        let answer = Message::assistant("Done.").with_raw_content(vec![
            json!({ "type": "thinking", "thinking": "", "signature": "c2lnLTI=" }),
            json!({ "type": "text", "text": "Done." }),
        ]);
        let body =
            ClaudeClient::new("k").build_request_body(&[Message::user("x"), answer], &[], false);
        assert_eq!(body["messages"][1]["content"][0]["signature"], "c2lnLTI=");
        assert!(body["messages"][1]["content"][0]
            .get("cache_control")
            .is_none());
        assert_eq!(breakpoints(&body), ["messages[1][1]"]);

        // A turn rewritten since (another text) is rebuilt without thinking.
        turn.content = "[earlier steps removed]".into();
        let body = ClaudeClient::new("k").build_request_body(
            &[
                Message::user("fix a.rs"),
                turn,
                Message::tool_result("toolu_1", "ok"),
            ],
            &[],
            false,
        );
        let blocks = body["messages"][1]["content"].as_array().unwrap();
        assert!(blocks.iter().all(|b| !is_thinking(b)));
        assert_eq!(blocks[1]["id"], "toolu_1");
    }

    #[tokio::test]
    async fn a_refusal_is_an_error_not_an_empty_reply() {
        let events = [
            json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
            json!({ "type": "content_block_stop", "index": 0 }),
            json!({ "type": "message_delta", "delta": { "stop_reason": "refusal" }, "usage": { "output_tokens": 0 } }),
            json!({ "type": "message_stop" }),
        ];
        let (url, _request) = serve_once_capturing(sse(&events), "text/event-stream").await;
        let client = ClaudeClient::new("k")
            .with_model("claude-opus-5-5")
            .with_base_url(url)
            .without_proxy();
        let mut rx = client
            .chat_stream(&[Message::user("x")], &[])
            .await
            .unwrap();
        use futures::StreamExt;
        let mut error = None;
        while let Some(event) = rx.next().await {
            if let StreamEvent::Error(e) = event {
                error = Some(e);
                break;
            }
        }
        assert_eq!(error.as_deref(), Some(REFUSAL_MESSAGE));

        let reply = json!({ "content": [], "stop_reason": "refusal", "usage": { "input_tokens": 3, "output_tokens": 0 } });
        let (url, _request) = serve_once_capturing(reply.to_string(), "application/json").await;
        let client = ClaudeClient::new("k").with_base_url(url).without_proxy();
        let err = client.chat(&[Message::user("x")], &[]).await.unwrap_err();
        assert!(err.to_string().contains(REFUSAL_MESSAGE), "{err}");
    }

    #[tokio::test]
    async fn a_whole_reply_keeps_its_thinking_and_every_text_block() {
        let reply = json!({
            "content": [
                { "type": "thinking", "thinking": "", "signature": "c2ln" },
                { "type": "text", "text": "One. " },
                { "type": "text", "text": "Two." },
            ],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 3, "output_tokens": 4 },
        });
        let (url, _request) = serve_once_capturing(reply.to_string(), "application/json").await;
        let client = ClaudeClient::new("k")
            .with_model("claude-haiku-5-5")
            .with_base_url(url)
            .without_proxy();
        let response = client.chat(&[Message::user("x")], &[]).await.unwrap();
        assert_eq!(response.message.content, "One. Two.");
        assert_eq!(response.message.raw_content.len(), 3);
        assert_eq!(response.message.raw_content[0]["signature"], "c2ln");
    }

    #[test]
    fn retired_models_are_sent_as_their_replacements() {
        let before = SONNET_4_5_RETIRES - 1;
        assert_eq!(
            current_model_id_at("claude-sonnet-4-20250514", before),
            "claude-sonnet-5-5"
        );
        assert_eq!(
            current_model_id_at("claude-opus-4-1-20250805", before),
            "claude-opus-5-5"
        );
        assert_eq!(
            current_model_id_at("claude-3-5-haiku-20241022", before),
            "claude-haiku-5-5"
        );
        // Claude Sonnet 4.5 is the user's choice until the day it retires.
        assert_eq!(
            current_model_id_at("claude-sonnet-4-5-20250929", before),
            "claude-sonnet-4-5-20250929"
        );
        assert_eq!(
            current_model_id_at("claude-sonnet-4-5-20250929", SONNET_4_5_RETIRES),
            "claude-sonnet-5-5"
        );
        assert_eq!(
            current_model_id_at("claude-sonnet-4-5", SONNET_4_5_RETIRES),
            "claude-sonnet-5-5"
        );
        // Current models, and ids Phaze doesn't know, go as they are.
        for id in [
            "claude-haiku-4-5-20251001",
            "claude-opus-4-6",
            "claude-sonnet-5-5",
            "claude-next",
        ] {
            assert_eq!(current_model_id_at(id, SONNET_4_5_RETIRES), id);
        }
    }
}
