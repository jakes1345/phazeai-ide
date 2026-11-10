# Token-saving research for Phaze IDE

Date: 2026-11-10. opencode: [sst/opencode](https://github.com/sst/opencode) at commit `37729b1`. Phaze: `main` at commit `fe95cec`.
Every opencode path below is relative to the opencode repository root; every Phaze path is relative to this repository's root.
"Verified" means I read the file or page myself. Anything else is marked **unverified**.

Important framing: two different things get called "saving tokens".
- **Billed tokens / cost** go down with prompt caching (cached reads are billed at 0.1x or less), even when the raw token count stays the same.
- **Raw tokens sent** go down with smaller prompts, tighter tool output, pruning and compaction.
opencode does both. Phaze currently does neither well. The largest single lever is caching, and caching only works if the prefix stays byte-identical from one request to the next.

---

## Part A: how opencode saves tokens (from the code)

Note: this HEAD is the new `packages/core` + `packages/ai` architecture (Effect-based). The older `packages/opencode/src/session/*` code that many blog posts describe is gone from this clone.

### A1. Prompt caching

**One cache policy for every provider, applied before the request is lowered to each wire format.**
- `packages/ai/src/cache-policy.ts:5-12`: the default `"auto"` policy places breakpoints on **the last tool definition, the first and last distinct system parts, and the conversation tail**. "Advancing the tail after each tool result keeps recent conversation prefixes reusable during long agent runs."
- `cache-policy.ts:27-34`: `AUTO = { tools: true, system: true, messages: { tail: 1 } }`, `BREAKPOINT_CAP = 4`.
- `cache-policy.ts:42-46`: caching is on by default (`undefined` resolves to `"auto"`). `"none"` turns off automatic placement.
- `cache-policy.ts:99-109`: `markLastTool` puts the hint on the last tool, recursing into namespaces.
- `cache-policy.ts:117-127`: `markSystemBoundaries` marks system part index 0 and the last index.
- `cache-policy.ts:137-161`: the tail breakpoint goes on the last text part of the last message, or on the last cacheable part of a tool-result-only message.
- `cache-policy.ts:215-227`: manual hints count against the cap of 4, and the automatic policy fills only the slots that are left.
- `cache-policy.ts:62-78`: the policy only runs for protocols that honour inline markers (Anthropic Messages and its compatible routes, Bedrock Converse, OpenRouter, Alibaba and others). OpenAI and Gemini are skipped because their caching is implicit (comment at `:59-61`).
- `cache-policy.ts:80-90`: gateway special cases. Anthropic models behind OpenRouter or Vercel get AUTO. Qwen gets `{system, tail}` because it ignores tool breakpoints. Others get NONE, because "Gemini uses only the last breakpoint, so a conversation-tail breakpoint writes a new cache every step and costs more than none."
- `packages/ai/src/protocols/anthropic-messages.ts:530-540`: the Anthropic lowering emits `{type:"ephemeral", ttl?}` and silently drops markers past 4, because the API returns 400 above that. Tools get `cache_control` at `:560`.
- `packages/ai/src/protocols/utils/cache.ts:29-35`: the shared `cacheControl()` counter, capped at 4.
- TTL: `cache-policy.ts:56-57` passes the request-level TTL. `utils/cache.ts:37-44` maps OpenAI lifetimes: `"24h"` goes to `prompt_cache_retention`, anything else to `prompt_cache_options.ttl`. `model-request.ts:328` sets `cache: { ttl }` only when the model config sets one, so the default is the provider default of 5 minutes on Anthropic.

**OpenAI-style cache routing key**
- `packages/core/src/session/model-request.ts:327`: `promptCacheKey` is derived from the session affinity ID, so every request in a session shares one key. The same block sets session-affinity headers (`x-session-affinity`, `X-Session-Id`, `x-opencode-session`, `:312-325`).
- `packages/ai/src/protocols/openai-chat.ts:850-853`: `prompt_cache_key` is sent only when the model's capability says it is supported.
- `packages/core/src/plugin/provider/prompt-cache-key.ts:9-35`: an allowlist of OpenAI-compatible providers that accept `prompt_cache_key`.

**Keeping the prefix byte-stable**
- **Tool order is deterministic.** `packages/core/src/tool.ts:297-301`: direct tool definitions are sorted by name, and the single Code Mode `execute` tool is appended last.
- **The system prompt is two parts, built from stable inputs only.** `packages/core/src/session/model-request.ts:105-128` (`baseTranscript`):
  - part 1 is the agent's prompt, or `system.txt` plus tool guidance (`packages/core/src/session/system-prompt.ts:5-28`);
  - part 2 is the *initial* instruction baseline.
  These two parts are exactly the "first and last system part" breakpoints.
- **Volatile facts never rewrite the system prompt.** Date, environment, MCP and skill catalogs and AGENTS.md are "instruction sources" with an `initial` renderer and a `changed` renderer (`packages/core/src/instructions/index.ts:29-55`). The baseline is frozen at session start. When a value changes, a delta message is appended chronologically (`packages/core/src/session/instruction-state.ts:37-60`) and the system prompt stays the same.
  - Example: the date. `packages/core/src/instructions/builtins.ts:24-32` gives `initial: "Today's date: …"` and `changed: "Today's date is now: …"`. The date is a day string (`toDateString()`), not a timestamp.
  - Environment block (cwd, git-repo yes/no, platform, temp dir): `builtins.ts:33-52`. **There is no git status, branch or dirty-file list.**
- **Source ordering is chosen for cache sharing.** `packages/core/src/session/context.ts:162-173`: "Ordered from most to least shared across sessions so the baseline stays a reusable prompt-cache prefix: user-level catalog and guidance, then project instructions, then the date and environment."
- **Tools stay on the last step.** `packages/core/src/session/runner/llm.ts:274-275`: at the max-steps limit opencode sends `toolChoice: "none"` but keeps the tool definitions, "to preserve the provider's cached prefix."
- **Reasoning-effort changes go in messages.** `packages/ai/src/effort-updates.ts:1-2`: changing top-level effort invalidates the cache, so routes that support it send per-message effort markers instead.
- **Plan-mode reminders are injected near the tail** "so the cached prefix stays warm" (`packages/core/src/plugin/plan.ts`, the `context` hook, comment "appending near the tail").

### A2. Context management

**Compaction** (`packages/core/src/session/compaction.ts`)
- Defaults: `auto: true, keep: 15_000, precompute: false` (`:269`). `keep` is "Tokens of recent conversation kept verbatim beside the summary" (`:46`).
- Trigger ceiling: `calculateCeiling` (`:1163-1171`) is `window − max(floor(window × 0.15), 16_000)`. The 16k floor applies only when the window is at least 32k. Constants: `RESERVE_SHARE = 0.15` (`:135`), `RESERVE_MIN = 16_000` (`:129`). A user `buffer` (tokens or "N%") overrides this (`:44-45`, `:1167-1169`). Unknown windows never auto-trigger (`:1164-1166`, `:137`).
- `due()` (`:440-449`) compares the provider-measured prompt size from the last response, plus an estimate of what came after it, against the ceiling.
- **The compaction request reuses the prompt cache.** The summary prompt is *appended* as a user message to the unchanged conversation (`:495-503`). The comment at `:524-530` says: "Automatic compaction starts past the ceiling, so the whole conversation is sent unchanged while the window can still fit the summary … which still reuses the cache for that shorter prefix." If the reply skips the template, one `NUDGE` retry follows (`:247-248`, `:505-511`).
- The summary prompt is a structured template (`:173-204`): Objective, Requirements, Decisions, Work State (Completed/Active/Blocked), Next Move, Relevant Files (at most 15), Important Context. Rules at `:206-212`: terse single-line bullets; preserve exact paths, symbols and errors; "Do not mention the summary process." `buildPrompt` (`:214-245`) tells the model to leave out AGENTS.md, environment and setup ("The next agent receives current versions of all of these separately"). An **update** variant merges into the previous checkpoint.
- **The kept tail is not stored twice.** A summary references kept messages by `metadata.keepID`, and the recent text is rebuilt in memory (`packages/core/src/session/history.ts:236-275`).
- **Background precompute (off by default):** `PRECOMPUTE_LEAD = 60_000` tokens before the ceiling (`:158`). A background summary starts only while at least `CACHE_MARGIN = 30_000` ms of the 5-minute cache lifetime is left (`:160-162`), so the summary request reads from the cache. The swapped-in tail is capped at 40% of the budget (`:169`).
- Overflow retry: after a "too long" rejection the request shrinks to `[0.85, 0.7, 0.5, 0.35]` of its size (`:127`). The transcript is degraded in levels: outputs clipped to 2,000 chars from start and end, then outputs removed, then user text only, then omitted (`:144-150`).
- **Native compaction:** Anthropic `compact_20260112` is supported in the wire schema (`packages/ai/src/protocols/anthropic-messages.ts:60-74` with trigger `value >= 50000`, and `:1004-1016`). OpenAI Responses `context_management` is supported too (`openai-responses.ts:135`, `:261`). `compaction.ts:314` takes the native path when `model.compaction.type === "native"`.

**Pruning of old tool outputs**
- **Not present at this HEAD.** `packages/core/src/config/normalize.ts:353-354` treats `compaction.prune` and `tail_turns` as "omitted unsupported legacy setting" (`:808-811`). `rg -i prune packages/core/src` finds only git prune.
- The older opencode behaviour, from a **secondary** source I read (https://lin-guanguo.github.io/llm-memory-research/opencode.research/, citing `packages/opencode/src/session/compaction.ts`): `PRUNE_PROTECT = 40_000` (the newest 40k tokens of tool output are kept), `PRUNE_MINIMUM = 20_000` (prune only if more than 20k can be reclaimed), the last 2 user turns are skipped, the `skill` tool is protected, and pruned outputs are marked `compacted` and left out of what is sent. **Not verified against code.** The current opencode dropped this in favour of one-shot compaction that keeps the cache.

### A3. What is sent per turn

- **Base system prompt is tiny.** `packages/core/src/session/runner/prompt/system.txt` is **756 bytes**. Tool guidance lines are added only for tools that are present (`system-prompt.ts:9-27`). There is one prompt for all providers at this HEAD. Per-provider prompts (anthropic.txt, gemini.txt) existed in older versions but are **not in this clone**.
- **Tool gating.**
  - Permission rules filter the snapshot: tools whose rules wholly deny them are removed (`packages/core/src/tool.ts:260-262`).
  - Per-model edit tool: GPT models (not `oss`, not `gpt-4`) get `patch`, and lose `edit` and `write`; all other models lose `patch` (`packages/core/src/tool/plugin/patch.ts:288-301`).
  - **Code Mode (deferred tools).** Tools default to `codemode: true` (`packages/core/src/tool/AGENTS.md:34`). Core tools opt out with `codemode: false` (read, grep, glob, edit, write, patch, shell, subagent, todo, skill, question, webfetch, websearch). MCP and namespaced tools go behind one `execute` tool. Their catalog is rendered as an instruction within an **inline budget of 2,000 tokens** at 4 chars/token, with descriptions cut to 120 chars (`packages/core/src/codemode/catalog.ts:50-57`). When the catalog is partial, the model must call `search(...)` inside `execute` (`packages/core/src/codemode/instructions.ts:8-20`). Catalog changes are sent as diffs with a 1,000-token budget (`:23`, `:43-133`). The catalog is cached by identity across steps (`tool.ts:270-273`).
- **Output bounding and spill-to-file.**
  - Generic: `packages/core/src/tool-output.ts:13-14` sets `MAX_LINES = 2_000` and `MAX_BYTES = 50 KiB`. When either is exceeded, the head is kept, the full text is written to a file, and a marker `[showing lines 1-N of M; full output saved to <file>]` is appended (`:69-120`). It is applied to every tool result in the runner (`packages/core/src/session/runner/step.ts:133`). Retention is 7 days (`:15`).
  - Shell: keeps the **tail**, the last 50 KiB or 2,000 lines, with exact line numbers and the full-output path (`packages/core/src/shell.ts:259-290`). Limits are configurable via `tool_output.max_lines/max_bytes`.
  - Read: `MAX_READ_LINES = 2_000`, `MAX_READ_BYTES = 50 KiB`, `MAX_LINE_LENGTH = 2_000` with a truncation suffix (`packages/core/src/tool/read-filesystem.ts:14-21`). Paging kicks in when the file is over 50 KiB or an offset or limit is given (`:155`, `:171`). Output format is `N: line` with a header `Read file X, lines a-b` and `[Output truncated. Continue reading with offset: N]` (`packages/core/src/tool/plugin/read.ts:323-362`).
  - Grep and glob: `DEFAULT_SEARCH_LIMIT = 100` (`packages/core/src/filesystem.ts:66`). They fetch limit+1 to detect truncation (`packages/core/src/tool/search.ts:15-27`). Grep output is grouped by file as `path:` then `  Line N: text` (`packages/core/src/tool/plugin/grep.ts:44-61`).
  - Webfetch: 5 MiB markdown cap (`html-markdown.ts:50`), and the generic 50 KiB bound applies after that.
- **Tool results are sent as plain text, not JSON.** `packages/core/src/session/runner/to-llm-message.ts:114-128`: a single text content part is sent as `{type:"text", value}`.
- **Short tool descriptions.** For example, shell is 8 short sentences (`packages/core/src/tool/plugin/shell.ts:34-44`), and edit's is one paragraph (`edit.ts:275`).

### A4. Small or cheap model use

- The title agent uses the provider's small model: `packages/core/src/session/context.ts:99-134` calls `model.small()`. `packages/core/src/model.ts:306-318` picks the first available of families `gpt-luna`, `gemini-flash-lite`, `gemini-flash`, `claude-haiku`, with the **minimal reasoning variant** (`context.ts:200-213`: none, minimal, low). The legacy `small_model` config maps to the title agent (`packages/core/src/config/normalize.ts:137-150`).
- Compaction uses the session model unless a `compaction` agent with its own model is configured (`compaction.ts:278-299`).
- Hidden `title` and `summary` agents deny all tools (`packages/core/src/plugin/agent.ts:102-116`).

### A5. Other token cuts

- **Edit returns one line, not the file or a diff:** `Edited <file> (N replacements)` (`packages/core/src/tool/plugin/edit.ts:358`). The diff goes to UI metadata only.
- **Todo returns a summary, not the list:** `Todos updated: 2/5 completed, 1 in progress. In progress: "..."` (`packages/core/src/tool/plugin/todo.ts:31-42`).
- No LSP diagnostics after edits yet: the TODOs at `edit.ts:258` and `write.ts:38` say so.
- Output-token cap headroom: `maxTokens` is computed as window minus input with `WINDOW_HEADROOM = 0.01` (`model-request.ts:60-64`, `:291-296`).
- Image bounding and unsupported-media replacement (`model-request.ts:140-155`, `boundImages` at `:330`).

---

## Part B: current best practice (primary sources I read)

| # | Technique | What the source says (verified) | Source | Applies to Phaze? |
|---|---|---|---|---|
| B1 | Anthropic prompt caching | Up to **4** breakpoints. Minimum cacheable length: **512** tokens (Opus 5.5, Sonnet 5.5, Haiku 5.5, Fable/Mythos 5.x), **1,024** (Sonnet 4.x and 5, Opus 4/4.1/4.8), **2,048** (Opus 4.7), **4,096** (Opus 4.5/4.6, Haiku 4.5). Shorter prompts are silently not cached. TTL 5m by default, refreshed free on use; `"ttl":"1h"` also available; longer TTLs must come before shorter ones. Prices: write 5m = **1.25x**, write 1h = **2x**, read = **0.1x** (0.05x Opus/Sonnet 5.5, 0.025x Fable/Mythos 5.1/5.2). Prefix order is **tools → system → messages**; a tool-definition change invalidates everything; `tool_choice` or image changes invalidate messages only. Lookback checks **20 positions** per breakpoint. Writes happen only at breakpoints. **Top-level `cache_control`** gives automatic caching that moves the breakpoint to the last cacheable block as the conversation grows; it uses 1 of the 4 slots, and breaks if the last block changes every request. Usage: `input_tokens` counts only tokens after the last breakpoint; total = `cache_read_input_tokens + cache_creation_input_tokens + input_tokens`. Cache hits are not deducted from rate limits. | https://platform.claude.com/docs/en/build-with-claude/prompt-caching | Yes. Biggest win for the Claude path. |
| B2 | Anthropic context editing | Beta header `context-management-2025-06-27`. `clear_tool_uses_20250919`: trigger default **100,000 input tokens**, keep default **3 tool uses**, `clear_at_least` (skip unless N tokens can be freed), `exclude_tools`, `clear_tool_inputs` (default false). Cleared results become placeholders. **Clearing invalidates the cached prefix**, so clear in large batches with `clear_at_least`. `clear_thinking_20251015` also exists. The response reports `context_management.applied_edits[].cleared_input_tokens`. No measured savings are published. | https://platform.claude.com/docs/en/build-with-claude/context-editing | Claude: native. Others: reimplement client-side. |
| B3 | Anthropic server-side compaction | `compact_20260112` in `context_management.edits`, beta `compact-2026-01-12`. Trigger default **150,000**, minimum **50,000**. `pause_after_compaction`, `instructions` (replaces the default prompt). The response holds a `compaction` block; pass the full content back and the API ignores everything before it. Put `cache_control` on the compaction block and **a breakpoint at the end of the system prompt, so the system cache survives compaction**. Billing: sum `usage.iterations`. Tools may be called during summarization unless instructed not to. | https://platform.claude.com/docs/en/build-with-claude/compaction-threshold (overview page https://platform.claude.com/docs/en/build-with-claude/compaction names a different beta header, `compact-2026-09-04`, for on-demand compaction) | Claude 4.6+ only. Optional. |
| B4 | Anthropic tool search / deferred tools | 58 tools on 5 MCP servers came to about 55K tokens. Upfront cost fell from about **77K to 8.7K tokens (85%)**. MCP eval accuracy rose (Opus 4: 49% to 74%). It **doesn't break prompt caching** because deferred tools are left out of the prompt. Use it when definitions exceed 10K tokens or there are more than 10 tools. Programmatic tool calling: **37%** fewer tokens on complex tasks. | https://anthropic.com/engineering/advanced-tool-use | MCP tools: yes, via a client-side catalog plus search, as opencode does, which works with any provider. |
| B5 | Code execution with MCP | **150,000 → 2,000 tokens (98.7%)** by progressive disclosure (tools as files, `search_tools`) and filtering data in code before it returns to the model. | https://www.anthropic.com/engineering/code-execution-with-mcp | Conceptually. opencode's Code Mode is this pattern. Large effort. |
| B6 | OpenAI prompt caching | **Automatic** for supported models. Minimum **1,024** tokens on GPT-5.6+; earlier models' cached tokens are rounded down to multiples of 128. Reads up to 95% off (0.1x on GPT-5.6+). `prompt_cache_key` routes related requests to one cache: keep about **15 req/min per key**. Retention `in_memory` (about 5-10 min idle, up to 1h) or `24h`; newer models use `prompt_cache_options.ttl` (`30m`/`12h`). Tool definitions **including their ordering**, model, schemas, reasoning effort and `parallel_tool_calls` must stay stable. "Append new messages rather than rewriting history. Avoid … summarization or truncation." Usage field `usage.input_tokens_details.cached_tokens` (Responses API). | https://developers.openai.com/api/docs/guides/prompt-caching | Yes. Free once the prefix is stable plus a key is sent. |
| B7 | xAI / Grok | `x-grok-conv-id` header (Chat Completions) or `prompt_cache_key` (Responses) routes a conversation to the same server, where its cache lives. Cached-token field `usage.prompt_tokens_details.cached_tokens`. Minimum length and discount are not on that page. | https://docs.x.ai/developers/advanced-api-usage/prompt-caching/maximizing-cache-hits | Yes, via the OpenAI-compatible client. |
| B8 | DeepSeek | Automatic and on by default. Hits require a **full prefix-unit match**. Best effort. Cleared within hours to days. Usage fields `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`. Price is not on that page. | https://api-docs.deepseek.com/guides/kv_cache | Yes. Needs only a stable prefix. |
| B9 | Gemini | Implicit caching is on by default for 2.5+. Minimum **2,048** (2.5 Flash/Pro) or **4,096** (3.x, 4 Argon). Put large common content first and send similar prefixes close together in time. Gemini 4 caches the full previous request only if the new one adds at least 2,050 tokens. Usage field `usage.total_cached_tokens`. Explicit caching is not in the Interactions API or Gemini 4. | https://ai.google.dev/gemini-api/docs/caching?lang=rest | Yes. Stable prefix only, no markers. opencode notes the tail-breakpoint anti-pattern for Gemini through gateways (`cache-policy.ts:80-82`). |
| B10 | Ollama | Default context depends on VRAM: **4k (<24 GiB)**, 32k (24-48 GiB), 256k (48 GiB+). Agents should set **at least 64,000** (`OLLAMA_CONTEXT_LENGTH`). The page does not document per-request `num_ctx`. | https://docs.ollama.com/context-length | Yes. A small window silently loses context. |
| B11 | llama.cpp server | `cache_prompt: true` is the default. The KV cache is reused for the shared prefix, and only the remainder is reprocessed. Slot selection by similarity (`-sps 0.5`) or pinned with `id_slot`. No benchmark numbers. | https://github.com/ggml-org/llama.cpp/discussions/13606 | Yes. Stable prefix plus a pinned slot per session. |
| B12 | Aider repo map | A ranked map of files and key symbols, sized to a `--map-tokens` budget (**default 1k**) and adjusted dynamically. Ranking is a graph of file dependencies. | https://aider.chat/docs/repomap.html | Phaze already has `context/repo_map.rs` and `analysis/outline.rs`, but nothing in the agent path uses it (see C9). |
| B13 | Aider edit formats | `whole` "can be slow and costly"; `diff` (search/replace) and `udiff` return only the changed parts. | https://aider.chat/docs/more/edit-formats.html | Phaze already uses search/replace (`edit_file`). Keep it. |
| B14 | Claude Code practice | MCP tool definitions are **deferred by default**, so only names and server instructions enter context. Delegate verbose work (tests, logs, docs) to **subagents** so only a summary returns. Hooks can pre-filter tool output. Move rarely used instructions to on-demand skills. `/compact` is itself a large request. The first message after the cache lifetime misses and reprocesses the full context. | https://code.claude.com/docs/en/costs | Subagents and deferred MCP: yes. |
| B15 | Token-efficient tool use | The docs page now **redirects to the model migration guide**. A search snippet of the zh-CN page says it started with Sonnet 3.7 and saved 14% output tokens on average (up to 70%). | https://docs.claude.com/zh-CN/docs/agents-and-tools/tool-use/token-efficient-tool-use (snippet only) | **Unverified and legacy.** Do nothing. |

Derived arithmetic (mine, from the multipliers in B1, not a published measurement): in an agent loop, step k re-sends the whole prefix P(k-1) plus a new delta d(k). Without caching, input cost is the sum of P(k). With a moving tail breakpoint, cost is about 0.1 × P(k-1) + 1.25 × d(k). Once P is much larger than d, which is normal after a few tool rounds, input cost falls by roughly 85-88% on Sonnet 4.x pricing and more on 5.5 (read at 0.05x). Claude Code's own `/usage` example shows "91% of input tokens from cache" (B14 page).

---

## Part C: Phaze IDE today (what I read)

### C1. No prompt caching on any provider
- `crates/phazeai-core/src/llm/claude.rs:56-141` (`build_request_body`): `system` is a single joined **string** (`:62-67`, `:121-123`). No `cache_control` on tools, system or messages. No top-level `cache_control`.
- `claude.rs:163-167`: usage parses only `input_tokens` and `output_tokens`, so `cache_creation_input_tokens` and `cache_read_input_tokens` are never read. Streaming reads usage only from `message_delta` (`claude.rs:378-398`).
- `crates/phazeai-core/src/llm/traits.rs:97-100`: `Usage` has only `input_tokens` and `output_tokens`, with no cache fields.
- `crates/phazeai-core/src/llm/openai.rs:126-133`: `OpenAIRequest` has no `prompt_cache_key`, no `user`, and **no `stream_options`**. The stream parser at `:372-389` expects `stream_options: {include_usage:true}` usage chunks, which OpenAI only sends when that option is set, so streaming usage is likely always missing on real OpenAI. No `cached_tokens` parsing.
- `crates/phazeai-core/src/llm/ollama.rs:123-128`: no options are set (`num_ctx`, `keep_alive`). With tools, it uses non-streaming chat (`:179-183`).

### C2. The prefix is not byte-stable, so caching would miss even if enabled
- **Tool order is random per registry instance.** `crates/phazeai-core/src/tools/traits.rs:5,58`: `ToolRegistry { tools: HashMap<String, Box<dyn Tool>> }`. `definitions()` iterates `self.tools.values()` (`:80-82`). Rust's `HashMap` uses a per-instance random seed, so every new registry gets a different order.
- **The UI chat builds a new Agent, and therefore a new HashMap, for every user message.** `crates/phazeai-ui/src/panels/chat.rs:511-516` calls `Agent::new(client)`, and `Agent::new` creates `ToolRegistry::default()` (`agent/core.rs:84-87`). The same function reconnects MCP and registers tools again (`chat.rs:525-529`). Result: **the tools block, which is first in the Anthropic prefix order, changes on every user turn**, which invalidates tools, system and messages. Within one turn, across tool rounds, the order is stable, so a fix there would still pay off.
- **The compaction summary is spliced into the system message** (`crates/phazeai-core/src/context/history.rs:93-110`). Each compaction rewrites the system block. This is unavoidable for the summary itself, but it also invalidates the base prompt's cache. B3 recommends keeping a separate breakpoint at the end of the base system prompt.
- **Sliding-window trimming shifts the prefix.**
  - `history.rs:20` sets `max_messages: 100`. `trim_if_needed` (`:218-228`) drops the oldest turn whenever the count goes past 100. After that, almost every new message changes the start of `messages`.
  - `trim_to_token_budget` runs before **every** LLM call (`agent/core.rs:333-338`; `history.rs:249-256`). It drops whole earlier turns, then `shrink_last_turn` **edits the user question in place** (appending `STEPS_REMOVED_NOTE`, `history.rs:270-276`) and cuts tool outputs. OpenAI's guide (B6) calls this out: "Avoid changes that reset the prefix, such as summarization or truncation."
- The system prompt itself is built **once per session** in the CLI (`crates/phazeai-cli/src/app/mod.rs:857-879`, `worker.rs:127-132`), so git branch and dirty files there do not change per turn. Good, though they go stale. In the UI chat the system prompt is `chat_system_prompt` (skill, mode hint and skills menu, `chat.rs:371-387`), rebuilt per message. It is stable unless the mode or skill changes.

### C3. Compaction
- `crates/phazeai-core/src/context/compaction.rs:11-15`: trigger at **0.8** of the budget, keep **0.4** of the budget verbatim, summary up to **0.15** of the budget. It uses the **same model** (`self.llm`, `agent/core.rs:308-313`).
- The summary request is a **fresh request** with its own system prompt and a re-rendered plain-text transcript (`compaction.rs:24-52`; `history.rs:168-197`). **None of the conversation's cached prefix is reused**, so every compaction pays full price for the whole history. opencode appends the summary instruction to the unchanged conversation instead (A2).
- Default budget when not configured: `max_context_tokens: 32768` (`agent/core.rs:90`). With a configured provider, `context_budget()` is window − output − 2,000 (`crates/phazeai-core/src/llm/context_budget.rs:15,43-54`). Claude falls back to a 200k window, OpenAI to 128k, Ollama to 8,192 (`:22-37`).
- Token estimate is chars/3 (`history.rs:366-383`).

### C4. Tool results are pretty-printed JSON
- `crates/phazeai-core/src/agent/core.rs:742-751`: `serde_json::to_string_pretty(&value)` for every successful tool result.
- Every tool returns a JSON object. For example, `read_file` returns `{content, path, total_lines, lines_shown}` (`crates/phazeai-core/src/tools/file.rs:81-86`), and grep returns `{matches:[{file,line,content}…], count, pattern}` (`crates/phazeai-core/src/tools/grep.rs:70-74`, `:136-140`).
- Measured on real Phaze files (bytes, not tokens; I had no tokenizer installed):
  - `read_file` of 300 lines of `agent/core.rs`: Phaze's pretty JSON is **13,903 B** against opencode-style text at **12,538 B** (1.11x), with **603 escape sequences** (`\n`, `\t`, `\"`), which tokenize worse than raw text.
  - grep, 150 matches of `fn ` in `context/`: Phaze's pretty JSON is **25,777 B** against grouped text at **10,758 B** (**2.40x**), because of the repeated `"file"`, `"line"` and `"content"` keys plus indentation.
- The read line prefix is `{:>6}\t` (`file.rs:77`): 7 characters per line, against 3-5 for opencode's `N: `.

### C5. Tool output bounds
| Tool | Phaze bound | opencode bound |
|---|---|---|
| read_file | **none by default** (`file.rs:65-77` takes `usize::MAX`) | 2,000 lines / 50 KiB / 2,000 chars per line |
| bash | stdout **30,000 B** + stderr **30,000 B**, middle-cut (`bash.rs:12`, `:148-150`) | 50 KiB / 2,000 lines total, **tail** kept, full output saved to a file |
| grep | **500** matches (`grep.rs:130-132`) | 100, grouped by file |
| fetch | 50,000 B (`fetch.rs:6`) | 5 MiB markdown, then the 50 KiB generic bound |
| browse | 30,000 chars (`browse.rs:10`) | n/a |
| diagnostics | 100 items + 10,000 B raw (`diagnostics.rs:8,11`) | n/a |
| any result in history | 12,000 chars, middle-cut (`history.rs:75-78`) | generic 50 KiB / 2,000 lines, rest spilled to a file |

Phaze has no spill-to-file. The cut-away part is gone, so the model re-runs commands to see it.

### C6. Duplicated and oversized system prompt
- `crates/phazeai-core/src/context/system_prompt.rs:441-490`: CORE_IDENTITY (935 chars), PLANNING_GUIDELINES (361), **TOOL_GUIDELINES (2,635)**, SAFETY_RULES (392), plus an "Available Tools" name list (`:465-472`). TOOL_GUIDELINES re-describes 17 tools and gives 3 worked examples. The tool schemas already carry that information, and some of the tools it lists (`open`, `fetch`, `web_search`) are not passed in the CLI's `with_tools` list (`phazeai-cli/src/app/mod.rs:860-869`).
- Tool schemas: 21 tool definitions in `tools/*.rs` total about **10,146 chars** of description plus schema, roughly 2.5k tokens at 4 chars/token. That is my estimate; I did not tokenize. All are sent every call (`agent/core.rs:341`). `SCHEMA_RESERVE` in the budget is only 2,000 tokens (`context_budget.rs:15`).
- MCP tools are all registered as normal tools (`register_mcp_tools`). Nothing defers them.

### C7. Small model
`crates/phazeai-core/src/llm/model_router.rs` routes by task type (`:48`, `:185`), but compaction always uses the agent's own model. I did not see a title-generation path in core.

### C8. UI chat already drops tool turns between user turns
`crates/phazeai-ui/src/panels/chat.rs:306-312` rebuilds history from UI bubbles, keeping only user and assistant text and skipping `ChatRole::Tool`. This works like aggressive pruning: cheap on raw tokens, and it keeps a stable prefix up to the previous user message. The CLI path (`with_shared_conversation`) keeps tool results.

### C9. Repo map is not wired into the agent path
Phaze already has `context/repo_map.rs`, `context/builder.rs` (`ContextBuilder`, with a `repo_map` field at `builder.rs:6`) and `analysis/outline.rs` (`generate_repo_map` at `:320`). A grep of `crates/` for `repo_map`, `RepoMap` and `ContextBuilder` outside their own files found only re-exports in `lib.rs:36-37`. Neither the CLI nor the UI agent calls them, so they have no effect on tokens today.

---

## Part D: ranked changes for Phaze (highest saving first)

1. **Make the prefix byte-stable (prerequisite for 2-4).**
   - Change `ToolRegistry` to `BTreeMap`, or keep insertion order and sort `definitions()` by name (`tools/traits.rs:58-82`). Basis: opencode `tool.ts:299`; OpenAI guide (B6) lists tool ordering as part of the prefix.
   - Build the agent and registry once per chat session in the UI instead of per message (`phazeai-ui/src/panels/chat.rs:511-529`), or at least sort.
   - Put the compaction summary in its own system block after the base prompt (`history.rs:93-110`).
   - Do not mutate earlier messages: no in-place `STEPS_REMOVED_NOTE` (`history.rs:270-276`), and no per-call front-trimming (`core.rs:336`, `history.rs:218-228`).
   - Effect: enables 85-90% input-cost reduction from caching. Without this, caching gives about 0.

2. **Anthropic caching** in `llm/claude.rs`. Send `system` as an array of text blocks: block 1 is the stable base prompt, block 2 the summary or instructions. Put `cache_control: {type:"ephemeral"}` on the last tool, the end of block 1, and the conversation tail (last block of the last message). Alternatively use the top-level automatic `cache_control` plus explicit breakpoints on tools and system, at most 4 in total.
   - Optional `ttl:"1h"` for the tools and system breakpoints when sessions idle past 5 minutes. Writes cost 2x, so use it only for long-idle sessions.
   - Add `cache_read`/`cache_creation` to `Usage` (`llm/traits.rs:97-100`) and parse them in both chat and stream (`claude.rs:163-167`, `:378-398`).
   - Basis: B1; opencode `cache-policy.ts:5-34`, `anthropic-messages.ts:530-560`.

3. **Replace sliding-window trimming with "send unchanged until the ceiling, then compact once".**
   - Ceiling = window − max(15% of window, 16k) (opencode `compaction.ts:1163-1171`). Keep about 15k tokens of verbatim tail (`:269`). Remove the 100-message cap (`history.rs:20`) and the per-call `trim_to_token_budget` (`core.rs:333-338`), keeping trimming only as an overflow fallback.
   - Basis: B6 ("append … avoid truncation"); opencode A2.
   - Effect: every step between compactions becomes a cache hit, and there are fewer compactions.

4. **Make compaction cache-friendly.** Send the existing conversation unchanged, plus a final user message holding the summary prompt ("do not call tools"), instead of the transcript re-render (`compaction.rs:38-52`, `history.rs:168-197`). Most summary input is then cache reads. Optionally use Anthropic `compact_20260112` (minimum trigger 50k) on supported models, or a cheaper model through `model_router`.
   - Basis: opencode `compaction.ts:495-503`, `:524-530`; B3.

5. **Send tool results as plain text.** Add `fn to_model_text(&Value) -> String` per tool, or change the return type: read becomes a header plus `N: line`, grep becomes grouped `path:` / `  Line N: text`, bash becomes plain stdout/stderr plus the exit code. Replace `to_string_pretty` with compact `to_string` as the fallback (`agent/core.rs:745-748`).
   - Basis: opencode `to-llm-message.ts:114-128`, `read.ts:323-362`, `grep.ts:44-61`.
   - Effect: measured 2.4x fewer bytes on grep and 1.11x on read, plus fewer escape tokens.

6. **Bound outputs with spill-to-file.** read_file: default 2,000 lines / 50 KiB / 2,000 chars per line, with a "continue at offset N" marker (`tools/file.rs:58-86`). bash: one tail-biased 50 KiB / 2,000-line window with the full log saved to a temp file whose path goes in the marker (`tools/bash.rs:12`, `:148-150`). grep: 100 matches (`grep.rs:130`). Lower `MAX_TOOL_RESULT_LEN` to fit, or replace it with the same generic bound (`history.rs:75`).
   - Basis: opencode `tool-output.ts:13-120`, `shell.ts:259-290`, `read-filesystem.ts:14-21`, `filesystem.ts:66`.

7. **Prune or clear old tool results in batches.** On Claude use `clear_tool_uses_20250919` with `clear_at_least` (for example 20k) and `keep` 3-5, with the beta header. For other providers, do the same client-side in one batch when the threshold is crossed, then let the new prefix re-cache. Never do it per step.
   - Basis: B2; old opencode PRUNE_PROTECT 40k / PRUNE_MINIMUM 20k (secondary source, unverified).

8. **Shrink the system prompt.** Drop TOOL_GUIDELINES' tool catalogue and the 3 examples, and drop the "Available Tools" list (`system_prompt.rs:465-472`, `:528-588`). Keep about 1 KB of rules, as opencode does with 756 B (`system.txt`).
   - Effect: about 3 KB (roughly 750 tokens) off every request. Note the minimum cacheable length (B1), but the prefix grows past it after the first tool round anyway.

9. **Defer MCP and rare tools.** Keep core tools direct (read, edit, write, bash, grep, glob, list). Put MCP tools, and rarely used built-ins (browse, screenshot, download, open, now, copy/move/delete), behind a catalog (name plus a description of at most 120 chars, within a token budget) and a `tool_search`/`load_tool` call that adds schemas only when needed. Append newly loaded schemas at the end of the tool list, or as messages, so the prefix survives. Or use Anthropic's tool search on Claude.
   - Basis: B4 (85%); opencode `codemode/catalog.ts:50-57`, `instructions.ts:8-20`.
   - Effect: large with MCP, modest without it (built-in schemas are about 2.5k tokens).

10. **OpenAI-compatible caching hygiene.** Send `prompt_cache_key` set to the session ID (opencode `model-request.ts:327`), and `x-grok-conv-id` for xAI (B7). Add `stream_options:{include_usage:true}` (`openai.rs:126-133`). Parse `cached_tokens`, and `prompt_cache_hit_tokens` for DeepSeek (B8).

11. **Local models.** Ollama: set `num_ctx` (or document `OLLAMA_CONTEXT_LENGTH` of at least 64k) and `keep_alive` (`ollama.rs:123-128`), because the default 4k window on GPUs under 24 GiB silently drops context (B10). llama.cpp: rely on `cache_prompt`, plus a stable prefix and one `id_slot` per session (B11).

12. **Cheap model for side tasks.** Use the small model for compaction, titles and summaries (opencode `model.ts:306-318`, `context.ts:99-134`).

13. **Subagents for verbose work.** Run test, log and doc fetches in an isolated child agent with a read-only tool set (`ToolRegistry::read_only()` already exists at `tools/traits.rs:115-127`) and return only a summary (B14; opencode `subagent.ts`, the `explore` agent in `plugin/agent.ts:74-92`).

---

## opencode: non-token patterns

### Planning and to-do tracking
- One `todowrite` tool and **no `todoread`** (`packages/core/src/tool/plugin/todo.ts:12-24`). Input is the full list `todos: [{content, status: pending|in_progress|completed|cancelled}]`. The result is a one-line summary (`:31-42`); the list itself goes to UI metadata (`:73-77`). The tool is behind a feature flag (`:51-56`; I did not check the flag's default). Description text: `packages/core/src/tool/todowrite.txt`.
- **Plan mode** is a primary agent, `plan` (`packages/core/src/plugin/plan.ts`): read-only, with `edit` denied except under `~/.opencode/plan/*`. Entering and leaving inject `<system-reminder>` messages near the tail. Edit failures in plan mode are rewritten to a clear error ("Cannot use edit to modify files outside the Plan directory").

### Subagents (task tool)
- The tool is `subagent` (`packages/core/src/tool/plugin/subagent.ts`). Inputs (`:20-39`): `agent`, `description` (3-5 words), `prompt`, optional `model` ("NEVER set this unless the user explicitly asks"), optional `sessionID` to send a follow-up to an existing child, and `background`.
- A child session starts with fresh context, and the description tells the model to "include all relevant context" (`:48-55`). Background mode returns at once with "DO NOT sleep, poll … or duplicate this subagent's work" (`:7-15`).
- Built-in agents (`packages/core/src/plugin/agent.ts:54-116`):
  - `build`: the default.
  - `general`: may not use `question` or `subagent`.
  - `explore`: deny-all, then allow shell, grep, glob, webfetch, websearch and read; `.env` reads ask; subagent denied.
  - hidden `compaction`, `title` and `summary` agents.
  - The explore prompt is at `:4-15`.

### Permission model
- Rules are `{action, resource, effect: allow|deny|ask}` with wildcards. **Last match wins** (`findLast`). With no match, the effect is **ask** (`packages/core/src/permission.ts:94-104`).
- Configured denies are checked first. Saved approvals (per project) are added as allow rules after that (`:159-189`).
- Agent permissions are merged with session permissions (`:168-173`). A missing agent means deny-all (`:20`).
- "Unattended" rules settle asks when nobody is present (`:59-65`).
- Shell: the command is parsed, and each sub-command becomes a resource. The saved approval pattern is the command prefix found by an arity table plus `*`, for example `git status *` (`packages/core/src/shell/parse.ts:17`, `:180`, `:603-609`; `packages/core/src/tool/plugin/shell.ts:127-131`).
- `edit`, `write` and `patch` share the `edit` action (`packages/core/src/tool/AGENTS.md:52`). Paths outside the workspace need an extra `external_directory` approval (`edit.ts:1-6`).
- Tool filtering is catalog visibility only, not authorization (`AGENTS.md:54`).

### Edit tool format
- **Exact string replace**, `edit{path, oldString, newString, replaceAll?}` (`edit.ts:23-34`).
- Matching cascade: exact match, then typographic quote/dash/space normalization (`:48-53`), then per-line trailing-whitespace-insensitive matching (`:66-90`). CRLF/LF is preserved.
- Failures:
  - identical strings or an empty `oldString` are rejected;
  - no match gives "Could not find oldString … It must match exactly" plus a **hint** (`:332`). `missHint` (`:191-199`) reports "newString is already in the file, so this edit may already be applied", or shows the closest matching lines numbered like Read output;
  - several matches give "Found N matches … add more surrounding context or set replaceAll" (`:337`).
- Success returns one line (`:358`). The file is formatted after the edit (`formatted` at `:350`).
- GPT models get `patch` instead, using the Codex-style envelope `*** Begin Patch` / `*** Add|Update|Delete File:` / `@@` / `*** Move to:` (`packages/core/src/tool/patch.txt`; gating at `patch.ts:288-301`).

### Tool design
- Direct tools: read, grep, glob, edit, write, patch, shell, subagent, todowrite, skill, question, webfetch, websearch, image-generate. Namespaced Code Mode tools: `opencode.*` (artifact, schedule, mcp-resource and others) and all MCP tools (registration flags listed in `packages/core/src/tool/plugin/*.ts`).
- Descriptions are short and imperative. They state the output caps ("When output is large, the full result is saved to a file and a truncated preview is returned", `shell.ts:40-41`) and the anti-patterns ("DO NOT poll for completion", `shell.ts:57`).
- Output caps are covered in A3.
- Invalid tool arguments return the first 5 schema issues plus the arguments as given, and ask the model to retry (`packages/core/src/tool/runtime.ts:90-100`).

---

## Not verified / caveats

- opencode's older prune constants (40k/20k, last 2 turns, `skill` protected) come from a third-party write-up, not code. This HEAD has no pruning.
- Per-provider system prompts in older opencode: not in this clone.
- Token figures for Phaze (system prompt about 1.1k tokens, schemas about 2.5k) are character-based estimates (chars/4). The JSON-vs-text comparison is in bytes. No tokenizer was available.
- The Anthropic compaction overview page names beta `compact-2026-09-04` (on-demand), while the threshold page names `compact-2026-01-12` for `compact_20260112`. Check the current header before shipping.
- I did not read Anthropic's streaming docs. That Claude's input usage arrives in `message_start`, which Phaze ignores (`claude.rs:378-398`), is from memory and **unverified**.
- That OpenAI streaming omits usage without `stream_options.include_usage` is likewise not on a page I read this session (**unverified**). Phaze's own comment at `openai.rs:372-373` assumes the option.
- Token-efficient tool use: the docs page redirects. The 14%/70% figures come only from a search snippet.
- I did not check the default of opencode's `todo` feature flag.
- Gemini explicit caching storage price and TTL: not on the page I read.
