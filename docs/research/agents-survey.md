# Coding-agent design survey: Claude Code, Cursor, Aider, Cline, Codex CLI, mapped to Phaze IDE

Researched 2026-11-10. opencode and the generic prompt-caching material are covered in [token-saving.md](token-saving.md).

**Evidence and citation rules**
- Open-source claims cite the source read for this survey: [Aider](https://github.com/Aider-AI/aider) at `5dc9490`, [Cline](https://github.com/cline/cline) at `e1f499a`, [Codex](https://github.com/openai/codex) at `aa39ded`. Paths are given relative to each repo root, with line numbers.
- Closed-source claims (Claude Code, Cursor) cite the vendor's own docs or blog only. Anything inferred is marked **[inferred]**.
- Numbers come from the source as stated. Where the source's own arithmetic disagrees, that is noted.
- **[unverified]** marks claims I could not confirm from a primary source.
- Phaze paths are relative to `crates/`, at `main` commit `fe95cec`. Parallel read-only tool calls were added after that commit (see the agent changes in PR #33) and are not reflected here.

---

## 1. Claude Code (Anthropic, closed source)

### 1.1 Context and history
- **Auto-compaction (when it runs).** It runs automatically "before the context window fills". The threshold depends on the model, and the user can set it with `/autocompact <tokens>`. Source: https://code.claude.com/docs/en/context-window
- **Auto-compaction (how it summarizes).** Claude Code "passes the message history to the model to compress it." The summary keeps "architectural decisions, unresolved bugs, and implementation details" and drops "redundant tool outputs". Source: https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents
- **What survives compaction.** Source: https://code.claude.com/docs/en/context-window
  - The system prompt is kept.
  - The project-root CLAUDE.md, auto memory and the plan file are re-injected from disk.
  - Claude Code re-reads up to 5 recently modified files. A file over 5,000 tokens comes back as a path reference only.
  - Invoked skill bodies are re-injected, capped at 5k tokens per skill and 25k in total.
  - `SessionStart` hooks with the `compact` matcher re-run.
- **Compaction reuses the prompt cache.** The summarization request is sent "with the same system prompt, tools, and history as your conversation, plus a summarization instruction appended as a final user message". While the cache is warm, `/compact` therefore "costs a fraction of what the context size suggests". Sources: https://code.claude.com/docs/en/prompt-caching and https://claude.com/blog/lessons-from-building-claude-code-prompt-caching-is-everything
  - The blog explains why: a summarizer with a different system prompt and no tools "shares no prefix" and pays the full uncached rate.
- **Partial compaction.** `/rewind` offers "Summarize from here" and "Summarize up to here". The user can also run `/compact <focus>`, or put a `# Compact instructions` section in CLAUDE.md. Sources: https://code.claude.com/docs/en/checkpointing and https://code.claude.com/docs/en/costs
- **Tool-result clearing.** The API offers this as "context editing": it "automatically clears stale tool calls and results". Source: https://claude.com/resources/articles/context-management
  - It gave 29% better results on an internal agentic-search eval, and 39% when combined with the memory tool.
  - On a 100-turn web-search eval it cut token use by 84%.
  - Claude Code's `/usage` counts "clearing old tool results" as an "expected rebuild" cache miss, which shows Claude Code does this itself. Source: https://code.claude.com/docs/en/costs
- **Large outputs.**
  - Bash output inline: up to about 30,000 characters. Past that, Claude Code saves the output to a file and returns the path plus a 2,000-character preview. Claude then reads or greps the file.
  - Read tool: at most 25,000 tokens per call, and it refuses a whole file over 256 KB. Larger files are read with `offset`/`limit`.
  - Source: https://code.claude.com/docs/en/tools-reference
  - "We restrict tool responses to 25,000 tokens by default." Source: https://www.anthropic.com/engineering/writing-tools-for-agents
- **Just-in-time context.** CLAUDE.md loads up front, and everything else is fetched with glob, grep, and head/tail as needed. Source: context-engineering post above.
  - CLAUDE.md files in subdirectories, and rules with a `paths:` field, load only when Claude reads a matching file. Source: https://code.claude.com/docs/en/memory
- **Subagents.** A subagent runs in its own context window. Only "the summary and a small metadata trailer come back". Source: https://code.claude.com/docs/en/context-window
  - Anthropic says a subagent explores with "tens of thousands of tokens" and returns "often 1,000-2,000 tokens". Source: context-engineering post.
  - The cost guide says to use `model: haiku` for simple subagents, and warns that a fresh subagent "can end up slower" because it must re-gather context. Sources: https://code.claude.com/docs/en/costs and https://code.claude.com/docs/en/best-practices

### 1.2 Prompt caching and token economy
- **Request layer order.** System prompt and tools come first, then project context (CLAUDE.md, memory), then the conversation. A change to the system prompt "invalidates everything". Source: https://code.claude.com/docs/en/prompt-caching
- **Dynamic information goes into messages, not the system prompt.** File-change notices, plan mode, skills and output-style switches are all appended as messages or `<system-reminder>` blocks.
  - Editing CLAUDE.md mid-session deliberately does not apply until `/clear` or `/compact`, so the cached prefix is kept.
  - When a file changes on disk, Claude Code appends a reminder and does not rewrite the earlier read.
  - Source: prompt-caching doc and blog above.
- **Plan mode is a tool, not a tool-set swap.** "keep *all* tools in the request at all times". EnterPlanMode and ExitPlanMode are tools. Source: blog above.
- **Deferred tools.** MCP and other tools are sent as name-only stubs with `defer_loading: true`, and the full schema loads on demand. The tool list from the first request is kept for the whole conversation, so an MCP server connecting mid-session does not bust the cache. Sources: prompt-caching doc; https://www.anthropic.com/engineering/advanced-tool-use
  - Results: 77K to 8.7K upfront tokens (the post says "85%"; its own numbers give about 89%).
  - Tool-selection accuracy on Opus 4 rose from 49% to 74%.
- **No mid-session model switch.** Each model has its own cache. In the blog's example, switching to Haiku at about 100k tokens into an Opus conversation "would actually be more expensive" than letting Opus answer. For a real model change, use a subagent handoff. Source: blog above.
- **Cache hygiene is measured.**
  - `/usage` shows a "Prompt cache (main)" line with hit %, misses, and a "likely cause" such as "tool definitions changed". Source: https://code.claude.com/docs/en/costs
  - Anthropic declares SEVs when the cache hit rate drops. Source: blog above.
- **TTL.** The default is 5 minutes. A 1-hour TTL is used on subscriptions and can be set with `promptCacheTtl`. Before the 1-hour cache expires on an idle session, Claude Code compacts it ("idle compaction"). Source: prompt-caching doc.
- **Cheaper-model routing.** Haiku is used for simple subagents. The `opusplan` setting uses Opus during plan mode and Sonnet during execution; each plan-mode toggle is a model switch, which costs a cache rebuild. Source: prompt-caching doc.
- **Output preprocessing hooks.** A `PreToolUse` hook rewrites `npm test` and similar commands so only failures are piped back. The doc says this takes context "from tens of thousands of tokens to hundreds". Source: https://code.claude.com/docs/en/costs

### 1.3 Edit format
- Edit is exact string replacement: `old_string` must match exactly once, or the call sets `replace_all`. There is no regex or fuzzy matching. Source: https://code.claude.com/docs/en/tools-reference
- Edits to a file that changed on disk are allowed if `old_string` still matches uniquely. Otherwise Claude must re-read the file. Same source.
- This design traces to the SWE-bench scaffold: a `str_replace` tool with a unique-match rule and absolute paths scored 49% on SWE-bench Verified (the previous state of the art was 45%). Source: https://www.anthropic.com/engineering/swe-bench-sonnet

### 1.4 Planning and to-dos
- Plan mode is a permission mode and is also exposed as tools. The plan is written to disk and re-injected after compaction. Sources: prompt-caching and context-window docs.
- TaskCreate, TaskGet, TaskUpdate and TaskList (formerly TodoWrite) are on by default only for older models: Opus 4.7, Sonnet 4.6, Haiku 4.5 and earlier. Newer models do without them by default. Source: https://code.claude.com/docs/en/tools-reference
  - **[inferred]** Explicit to-do tools help weaker or older models most.

### 1.5 Tools
- **Read-only file tools.** Read, Grep and Glob run without a prompt inside the working directories. Source: https://code.claude.com/docs/en/permissions
- **Bash.** A built-in set of read-only commands runs without prompting.
- **Tool-name strings are shared.** The same names are used in permission rules, hook matchers and subagent tool lists. Source: tools-reference.
- **Tool-design guidance.** Source: https://www.anthropic.com/engineering/writing-tools-for-agents
  - Consolidate tools rather than mapping one tool per endpoint.
  - Use namespacing.
  - Add a `response_format` concise/detailed enum. The example response was 206 tokens detailed and 72 tokens concise.
  - Write actionable errors.
  - Rewriting tool descriptions cut task time by 40%. Source: https://www.anthropic.com/engineering/multi-agent-research-system

### 1.6 Permissions, sandbox, hooks, checkpoints
- **Rule evaluation order.** Rules are "evaluated in order: deny, then ask, then allow. The first match in that order determines the outcome". An allow rule cannot carve an exception out of a deny rule. Source: https://code.claude.com/docs/en/permissions
- **Rule syntax.**
  - `Bash(npm run *)`. `Bash(ls:*)` is the same as `Bash(ls *)`.
  - `Read(./.env)`, `Edit(/src/**/*.ts)`, `WebFetch(domain:example.com)`, `mcp__server__tool`.
  - Path anchors: `//` absolute, `~/` home, `/` relative to the settings file, `./` relative to cwd. Bare filenames follow gitignore semantics.
- **Compound commands are split.** The separators are `&&`, `||`, `;`, `|`, `|&`, `&` and newline. Allow rules must match every subcommand. Deny and ask rules match any subcommand, including ones inside `$()`, subshells and loops. "Yes, don't ask again" on a compound command saves one rule per subcommand.
- **Settings scopes.** Managed, user, project (`.claude/settings.json`), and local (`.claude/settings.local.json`). The repo's allow rules wait for a folder-trust dialog. Source: https://code.claude.com/docs/en/best-practices
- **Sandbox.**
  - Uses bubblewrap on Linux and Seatbelt on macOS.
  - Filesystem: writes are limited to the cwd.
  - Network: only through a unix socket to an outside proxy, which allowlists domains.
  - "Sandboxing safely reduces permission prompts by 84%" (internal figure). Source: https://www.anthropic.com/engineering/claude-code-sandboxing
- **Hooks.** Source: https://code.claude.com/docs/en/hooks
  - Main events: `PreToolUse`, `PostToolUse`, `PostToolBatch`, `PermissionRequest`, `UserPromptSubmit`, `Stop`, `SubagentStop`, `PreCompact`/`PostCompact`, `SessionStart`/`SessionEnd`, and more.
  - Exit code 2 blocks the action, and stderr is fed back as the reason.
  - JSON output can set `permissionDecision` (allow/deny/ask/defer), `updatedInput`, and `additionalContext`, which is capped at 10k characters; longer text is saved to a file and Claude gets a 2k-character preview.
  - A `Stop` hook can force Claude to keep working, for example until typecheck passes. It is capped at 8 consecutive blocks.
- **Deterministic vs non-deterministic rules.** Best practice puts rules that must never be broken in hooks or deny rules, not CLAUDE.md, because "CLAUDE.md is for guidelines". Source: best-practices.
- **Checkpoints.** Source: https://code.claude.com/docs/en/checkpointing
  - Each user prompt creates a checkpoint, and the last 100 are kept.
  - Restore options: code and conversation, conversation only, code only, or summarize from/up to here.
  - Limitation: only edits made with the edit tools are tracked. Bash-only changes and background-subagent edits are not.
  - Rewind lands on an already-cached prefix, so it is cache-friendly. Source: prompt-caching doc.

### 1.7 Project memory
- **CLAUDE.md layers.** Managed, then user `~/.claude/CLAUDE.md`, then project `./CLAUDE.md`, then `CLAUDE.local.md`. Files are concatenated from the filesystem root down to the cwd. Subdirectory files load lazily. Source: https://code.claude.com/docs/en/memory
  - `@path` imports go at most 4 hops deep.
  - `.claude/rules/*.md` files with a `paths:` field load when Claude reads a matching file.
  - Size target: under 200 lines.
- **Auto memory.** `MEMORY.md` is an index; its first 200 lines or 25 KB load each session. Topic files load on demand.
- **Skills.** Only the name and description sit in context; the body loads when the skill is used. Source: best-practices.
- **AGENTS.md.** Not read natively. Docs recommend `@AGENTS.md` inside CLAUDE.md, or a symlink. Source: memory doc.

---

## 2. Cursor (closed source)

### 2.1 Context
- **Dynamic context discovery.** Source: https://cursor.com/blog/dynamic-context-discovery
  - Long shell and MCP outputs are written to files instead of being truncated. The agent runs `tail` and reads more if needed.
  - When the context fills, the chat history is saved as a file and the summary references it, so the agent can grep the history to recover lost detail.
  - Terminal sessions are synced to files.
  - Skills show only their name and description.
  - MCP tools are lazily loaded, "reduced total agent tokens by 46.9%" (statistically significant, with high variance).
- **Semantic search alongside grep.** Source: https://cursor.com/en-US/blog/semsearch
  - Accuracy on Cursor Context Bench was 12.5% higher on average (6.5% to 23.5% depending on the model).
  - Code retention rose 0.3%, and 2.6% on repos with 1,000 or more files.
  - Without semantic search, dissatisfied follow-up requests rose 2.2%.
  - "the combination of these two leads to the best outcomes."
  - The embedding model is trained on agent sessions.

### 2.2 Token economy
Source: "Improved token efficiency", Sep 23 2026, https://cursor.com/en-US/blog/improved-token-efficiency. Together these changes cut user token cost by 7% with "no reduction in agent quality".
- The system prompt was trimmed by about 66%, removing step-by-step instructions that newer models no longer need. Changes were validated with A/B tests on real traffic, not only evals.
- Rarely used built-in tools moved to on-demand loading. Static-context tool tokens fell 60%. Read, search, edit, shell, `ask_question` and `create_plan` stay static.
- Explicit cache breakpoints are placed after the stable prefix. Volatile content (skills, subagents, environment info) moved into a "phantom user message" after the breakpoint. Cold cache misses fell 20%.
- The Read tool numbers only every 10th line instead of every line, at roughly 3 to 5 tokens per number saved. Cache-read tokens fell 1.6% with no quality loss.
- Subagents are used less aggressively, and subagent model switching is restricted.

### 2.3 Edit format: the apply model
Source: https://cursor.com/blog/instant-apply. Speed details: https://fireworks.ai/blog/cursor
- The main model sketches the edit, then a fine-tuned Llama-3-70B "fast apply" model rewrites the full file.
- Speculative edits, which use the original file as the draft, reach about 1,000 tokens/s: about 13x vanilla inference and 9x the previous GPT-4 speculative setup.
- Why full rewrites:
  - "Diffs force the model to think in fewer tokens."
  - Models have seen more full files than diffs.
  - "Models are notoriously bad at counting line numbers."
- Evaluated on about 450 edits to files under 400 lines. It almost matches claude-3-opus-diff and beats gpt-4-turbo and gpt-4o. The authors note grading may be biased toward Claude.
- **[inferred]** This needs a dedicated hosted apply model, so it doesn't transfer directly to a model-agnostic client.

### 2.4 Planning, rules, memory
- **Plan mode.** `create_plan` is kept as a flow-critical static tool (from the token-efficiency post).
- **Rules.** Source: https://cursor.com/docs/rules
  - Files live in `.cursor/rules/*.mdc` with frontmatter fields `description`, `globs` and `alwaysApply`.
  - Four modes: Always, Apply Intelligently (the agent reads the description), Specific Files (globs), and Manual (@-mention).
  - Nested AGENTS.md files are supported; "more specific instructions taking precedence".
  - Guidance: keep rules under 500 lines.
- Checkpoints exist in Cursor, but I did not fetch Cursor's checkpoint docs. **[unverified detail]**

---

## 3. Aider (open source, Python)

### 3.1 Repo map: tree-sitter plus PageRank
- **Defaults.** `aider/repomap.py:47-57` sets `map_tokens=1024`, `map_mul_no_files=8` and `refresh="auto"`. When no files are in the chat, the budget grows to `min(map_tokens*8, context_window - padding)` (`repomap.py:120-133`).
- **Graph.** `repomap.py:365-530` builds it from tree-sitter tags. Each file is a node, and there is an edge from each file that references an identifier to each file that defines it.
  - Edge weight is ×10 if the user mentioned the identifier, ×10 for long snake/camel/kebab names (8 or more characters), ×0.1 for `_private` names, and ×0.1 if more than 5 files define the identifier.
  - Edges from files already in the chat get ×50.
  - The reference count is square-rooted so frequent mentions don't dominate.
- **Ranking.** Personalized PageRank (`nx.pagerank(..., personalization=..., dangling=...)`, line ~525) gives chat files and mentioned files `100/len(fnames)` personalization.
- **Fitting the budget.** A binary search over how many ranked tags fit the token budget, accepting within 15% error (`repomap.py:629-708`).
- **Caching.**
  - Tags are cached in SQLite and keyed by file mtime (`get_tags`, `repomap.py:227-265`).
  - The rendered map is cached by (chat files, other files, budget, mentions) (`repomap.py:586-627`).
  - When `--cache-prompts` is on, map refresh switches to `files` mode so the map stays byte-stable between turns and keeps the cache prefix (`aider/main.py:954-955`).
- **Evidence.** Repo map instead of RAG is credited in "How aider scored SOTA 26.3% on SWE Bench Lite" (`aider/website/_posts/2024-05-22-swe-bench-lite.md:11,13`). There, first-attempt GPT-4o alone scored 20.3% (`:142` table).

### 3.2 History and summarization
- **Summarizer.** `aider/history.py:7-80` `ChatSummary` keeps the most recent messages that fit in half the budget verbatim. It summarizes the older "head" recursively (up to depth 3), splitting only at an assistant message.
- **Budget.** `max_chat_history_tokens = min(max(max_input_tokens/16, 1024), 8192)` (`aider/models.py:356-358`). The summary is deliberately small.
- **Runs in the background.** The summarizer runs in a background thread (`base_coder.py:1002-1045`, `summarize_start`/`summarize_end`) and uses **the weak model first**: `ChatSummary([main_model.weak_model, main_model], ...)` (`aider/main.py:949-952`).

### 3.3 Prompt caching
- **Chunk order.** `aider/coders/chat_chunks.py:16-26` orders chunks as system, examples, read-only files, repo map, done (history), chat files, current, reminder.
- **Breakpoints.** `add_cache_control_headers` (`:28-41`) places three: after system/examples, after repo map/read-only files, and after chat files.
- **Cache warming.** `base_coder.py:1340-1392` sends a `max_tokens=1` request every 4m55s, up to N times (`--cache-keepalive-pings`), to keep Anthropic's 5-minute cache alive while the user thinks. Docs: `aider/website/docs/usage/caching.md`.

### 3.4 Edit formats and accuracy
- **Formats.** whole, diff (SEARCH/REPLACE), diff-fenced, udiff, editor-diff, editor-whole and patch. Docs: `aider/website/docs/more/edit-formats.md`.
- **Unified diff fixed laziness.** For GPT-4 Turbo, SEARCH/REPLACE scored 20% and udiff 61% on the laziness benchmark, which is 3x less "lazy commenting" (`_posts/2023-12-21-unified-diffs.md:36-37`). Prompting for "high level diffs" matters: without it, editing errors rise 30-50% (`:238`).
- **Weak models fail at diffs.** Polyglot leaderboard (`website/_data/polyglot_leaderboard.yml`): Qwen2.5-Coder-32B scored 8.0% pass with `diff` and 71.6% well-formed, against 16.4% pass with `whole` and 99.6% well-formed. Frontier models (gpt-5, o3, gemini-2.5-pro) top the board with `diff`/`diff-fenced`. For example, gpt-5 high scores 88.0% and is 91.6% well-formed.
- **JSON-wrapped code hurts.** "LLMs are bad at returning code in structured JSON responses" (`_posts/2024-08-14-code-in-json.md:237`). Sonnet scored lower with JSON even without syntax errors.
- **Architect/editor split.** A strong reasoner describes the change and a cheaper editor model writes the edit. o1-preview plus DeepSeek or o1-mini reached the then state of the art of 85% (`_posts/2024-09-26-architect.md:15-25`). "o3 (high) + gpt-4.1 architect" scored 78.2% at $17.55, against o3 (high) solo at 81.3% for $21.23 (leaderboard data).
- **Edit failure handling.** `editblock_coder.py:41-125`:
  - It tries exact, then whitespace-insensitive, then dropping a spurious leading blank line, then `...` elision.
  - Fuzzy edit-distance matching exists but is **dead code** (`return` at line 183 sits before it).
  - On failure it reports each failed block, a "Did you mean" with the closest real lines (`find_similar_lines`, threshold 0.6), whether the REPLACE text is already present, and "The other N blocks were applied successfully. Don't re-send them."
- **Lint and test loop.** After each edit Aider runs a tree-sitter lint, then `--test-cmd`, and feeds errors back (`_posts/2024-05-22-linting.md`, `swe-bench-lite.md:49-90`).

### 3.5 Other
- **Weak model.** Used for commit messages and summaries (`models.py:603-620`, `main.py:950`).
- **Laziness and overeager prompts.** `coders/base_prompts.py:12-17`.
- **Conventions.** Loaded as read-only files with `--read CONVENTIONS.md`, which are cached as part of the read-only chunk.
- **Not built in.** Aider has no permission system or sandbox. Git commits are its undo (`/undo` reverts aider's last commit).

---

## 4. Cline (open source, TypeScript; now an SDK monorepo)

### 4.1 Context
- **Constants.** `sdk/packages/core/src/extensions/context/compaction-shared.ts:15-35`:
  - `CONTEXT_WINDOW_INPUT_RATIO=0.9` and `COMPACTION_TRIGGER_RATIO=0.9`, so compaction triggers at about 81% of the raw window.
  - `DEFAULT_TARGET_RATIO=0.7`, `DEFAULT_PRESERVE_RECENT_TOKENS=20_000`, `DEFAULT_SUMMARY_MAX_OUTPUT_TOKENS=8_192`.
  - `TOOL_RESULT_CHAR_LIMIT=2_000` and `FILE_CONTENT_CHAR_LIMIT=2_000`, used inside summaries.
  - `MAX_INPUT_UNDERESTIMATE_FACTOR=4` corrects char-based estimates against the provider's real counts.
- **Two strategies.** `compaction.ts:162-190`:
  - `basic` is deterministic and makes no LLM call (`basic-compaction.ts`). It drops old assistant tool work and leaves a "tool activity summary" of the files read and edited in its place. The last 3 assistant texts are kept verbatim (`PRESERVED_ASSISTANT_TEXT_COUNT = 3`, line 61). Messages kept by an earlier compaction are "frozen" and never re-folded (`:398-411`), so repeated compactions process only new messages.
  - `agentic` uses an LLM summary and keeps the most recent 20k tokens.
- **Cost of agentic compaction.** "Summarization leverages your existing prompt cache ... so it costs about the same as any other tool call". Docs: `docs/features/auto-compact.mdx`.
- **Task scoping.** Docs recommend a new task per feature to keep context clean (`docs/core-workflows/task-management.mdx:22-42`).

### 4.2 Edit format, chosen per model
- "A session offers `editor` or `apply_patch`, never both. By default, Act mode offers `apply_patch` to GPT and Codex models ... and `editor` to other models; Plan mode offers neither." The `toolRoutingRules` option overrides this. Source: `docs/tools-reference/all-cline-tools.mdx:19-25`.
  - This is the clearest open-source precedent for **model-specific edit tools** in a model-agnostic agent.

### 4.3 Plan/Act
- Plan mode can read and search but "cannot modify any files or execute commands". The conversation carries into Act mode. Plan and Act can use **separate models**: a strong reasoner for planning and a faster model for implementation. Source: `docs/core-workflows/plan-and-act.mdx`.
- In the SDK, Plan mode simply isn't offered the edit tools (above).

### 4.4 Checkpoints (best design found)
- `sdk/packages/core/src/hooks/checkpoint-hooks.ts`:
  - Each checkpoint is a `git stash create`-style commit. Untracked files are captured through a **separate persistent `GIT_INDEX_FILE`** in a per-session scratch dir, so git's stat cache skips re-hashing unchanged files (`:20-40`, `:245-340`).
  - Snapshots are stored as **private refs** under `refs/cline/checkpoints/{sessionId}/` (`:432`). The working tree, the user's index and `git stash list` are never touched.
  - Stale scratch dirs are reaped after 14 days (`:10`).
- Docs (`docs/core-workflows/checkpoints.mdx`):
  - A checkpoint is taken after every tool use (edits and commands).
  - Restore options: Files, Task only, or Files and Task.
  - Compare opens a diff.
  - Large repos can disable checkpoints.

### 4.5 Permissions, rules, memory
- **Auto-approve.** Per category: read project / read all / edit project / edit all / safe commands / all commands. The model flags commands as safe or requiring approval. Source: `docs/features/auto-approve.mdx`. There are also hooks (`docs/customization/hooks.mdx`, not read in detail).
- **Rules.** `.clinerules/` or `.cline/rules/`, plus `.cursorrules`, `.windsurfrules`, `AGENTS.md` and `~/.agents/AGENTS.md`. Each can be toggled in a panel. **Conditional rules** use `paths:` YAML frontmatter matched against open, mentioned and edited files. Source: `docs/customization/cline-rules.mdx:20-180`.
- **Memory Bank.** A convention of markdown files (projectbrief, activeContext, progress...) that the agent reads at the start of a task. Source: `docs/best-practices/memory-bank.mdx` (read briefly).

---

## 5. OpenAI Codex CLI (open source, Rust)

### 5.1 Context
- **Auto-compact limit.** `min(config_limit, 90% of context window)` (`codex-rs/protocol/src/openai_models.rs:579-590`).
- **Compaction prompt.** "CONTEXT CHECKPOINT COMPACTION. Create a handoff summary for another LLM that will resume the task", covering progress, decisions, constraints, next steps and critical data (`codex-rs/prompts/templates/compact/prompt.md`). The summary is prefixed with "Another language model started to solve this problem ..." (`summary_prefix.md`).
- **Compacted history** = initial context + **the user's own messages, most recent first, up to 20,000 tokens** (`COMPACT_USER_MESSAGE_MAX_TOKENS`, `codex-rs/core/src/compact.rs:64`, `:716-800`) + the summary.
  - The user's intent is kept verbatim and the agent's work is summarized.
- **Retained tool outputs.** Explicitly marked client-authored tool outputs survive whole or are dropped (`compact_retained_tool_outputs.rs:1-20`).
- **The model can see its own budget.** The `get_context_remaining` tool returns tokens left, and `new_context_window` lets the model start a fresh window itself (`core/src/tools/handlers/get_context_remaining_spec.rs:15`, `new_context_window_spec.rs:13`).
- **Tool-output truncation.**
  - The default per-model policy is `bytes(10_000)` (`protocol/src/openai_models.rs:1228`).
  - Truncation is **middle** truncation with a header: "Warning: truncated output (original token count: N) / Total output lines: M" (`codex-rs/utils/output-truncation/src/lib.rs:20-50`). A 20% allowance covers serialization.

### 5.2 Caching
- **`prompt_cache_key` is session-derived.** Subagents and child threads use `source:parent_thread_id` so they share the parent's cache (`core/src/client.rs:712-730`).
- **Cache-break reasons are logged.** The code compares each request with the previous one and names what changed: `model_changed`, `tools_changed`, `tool_choice_changed`, `parallel_tool_calls_changed`, `reasoning_changed`, `prompt_cache_key_changed`, and more (`core/src/client.rs:425-500`). The destructuring is exhaustive "so new request fields require an explicit reuse decision".
- **OpenAI's own guidance.** Source: https://developers.openai.com/cookbook/examples/prompt_caching_201
  - Keep tools, schemas and ordering stable.
  - Use `allowed_tools` instead of pruning the tool list.
  - Per-conversation `prompt_cache_key`: one coding customer went "from 60% to 87%" hit rate.
  - Responses API with `previous_response_id`: "40-80% better cache utilization" for reasoning models.

### 5.3 Edit format: apply_patch (V4A)
- **Grammar.** A Lark grammar in `codex-rs/apply-patch/src/parser.rs:1-25`: `*** Begin Patch`, then Add / Delete / Update File hunks, optional `*** Move to:`, `@@ <context line>` anchors (no line numbers), `+`/`-`/space lines, and `*** End of File`.
- **Lenient parsing on purpose.** It is always lenient because gpt-4.1 needs it (`parser.rs:47-53`).
- **Context matching.** `apply-patch/src/seek_sequence.rs:1-100` steps down in strictness: exact, then trailing whitespace ignored, then trimmed, then **Unicode punctuation normalized** (dashes, smart quotes, NBSP to ASCII). An EOF-anchored search runs first when the hunk ends with `*** End of File`.
- **OpenAI's tool doc.** Return `status: failed` plus a clear `output` for each `call_id`, and decide in advance whether a batch rolls back atomically. Source: https://developers.openai.com/ja-JP/api/docs/guides/tools-apply-patch (JA mirror of the EN page).
- **Why it matters for Phaze.** Cline routes GPT and Codex models to `apply_patch` by default (§4.2). **[inferred]** OpenAI models are post-trained on this format.

### 5.4 Planning and tools
- **`update_plan` tool.** Items with status pending / in_progress / completed. "At most one step can be in_progress at a time" (`core/src/tools/handlers/plan_spec.rs:42-47`).
- **Tool search.** Deferred tools (`exposure.is_deferred()`) are found with **BM25** over their descriptions (`core/src/tools/handlers/tool_search.rs:14-17,74`).
- **Parallel calls.** Each tool declares `supports_parallel_tool_calls()` (`core/src/tools/registry.rs:720-722`).

### 5.5 Sandbox and policy
- **Sandbox modes.** read-only / workspace-write (the default for local work) / danger-full-access.
- **Approval policies.** on-request / never (`untrusted` is no longer selectable). There is also an optional `auto_review` reviewer agent. Source: https://learn.chatgpt.com/docs/sandboxing
- **Platforms.**
  - macOS: Seatbelt.
  - Linux: **bubblewrap plus seccomp**, with legacy Landlock code kept. Codex prefers `bwrap` on PATH and falls back to a bundled one (`codex-rs/linux-sandbox/README.md:10-23`).
  - Windows: a native sandbox.
- **execpolicy.** Starlark `prefix_rule(pattern=[...], decision=allow|prompt|forbidden, justification=..., match=[...], not_match=[...])` (`codex-rs/execpolicy/README.md`).
  - The `match`/`not_match` examples are **unit tests checked at load time**.
  - A forbidden rule's justification can name an alternative such as "Use `jj` instead of `git`".
  - `host_executable()` restricts which absolute paths may match a basename rule.

### 5.6 Project memory (AGENTS.md)
- **Global scope.** `AGENTS.override.md`, otherwise `AGENTS.md`.
- **Project scope.** Codex walks from the git root down to the cwd, taking at most one file per directory (override, then AGENTS.md, then fallback names). Files are concatenated root-first, so deeper files win.
- **Size cap.** Loading stops at `project_doc_max_bytes`, 32 KiB by default (`codex-rs/config/src/config_toml.rs:77`; https://learn.chatgpt.com/docs/agent-configuration/agents-md).

---

## 6. Builders' guidance (what the authors published), with "What Phaze does with it"

Each entry gives the technique and its mechanism, the evidence, and **Phaze action**.

### Anthropic
1. **Simplest thing that works. Add complexity only when it measurably helps.** ("Building effective agents", https://www.anthropic.com/engineering/building-effective-agents)
   - **Phaze:** gate each change below behind a token/cost eval: a fixed task set plus a cache-hit/token report. Add `crates/phazeai-core/src/telemetry.rs` counters and a `phazeai-cli` `--eval` harness.
2. **Tool formats the model writes easily.** Avoid line-number diffs and code inside JSON. Use absolute paths (poka-yoke): after Anthropic made its SWE-bench tool require absolute paths, "the model used this method flawlessly". (Same post, Appendix 2. Aider confirms the JSON finding in §3.4.)
   - **Phaze:** `tools/edit.rs` and `tools/file.rs` accept and echo absolute paths, and the `sandbox::resolve_within_workspace` check stays. Never ask for line-numbered hunks.
3. **Context engineering.** Source: https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents
   - Compaction tuned for recall first, then precision.
   - **Tool-result clearing**, "the safest lightest touch form of compaction".
   - Structured notes (NOTES.md or a to-do file).
   - Subagents that return 1-2k-token summaries.
   - Just-in-time file references.
   - **Phaze:** `context/history.rs` gains a `clear_old_tool_results(keep_last_n)` pass that runs before LLM compaction, and `context/compaction.rs` becomes the second stage. A `task` subagent tool goes in `tools/` and runs `agent/core.rs` with a fresh `ConversationHistory` and a read-only tool set.
4. **Context editing plus memory tool.** 29% better results, 39% with the memory tool, and 84% fewer tokens on 100-turn runs (https://claude.com/resources/articles/context-management).
   - **Phaze:** same as #3. Replace the key-value `tools/memory.rs` with a file-backed `.phazeai/memory/` directory: an index file loaded at start (capped) plus topic files read on demand.
5. **Writing tools for agents.** Source: https://www.anthropic.com/engineering/writing-tools-for-agents
   - Consolidate overlapping tools.
   - A `response_format` concise/detailed option (206 vs 72 tokens).
   - Default truncation of about 25k tokens, with steering text.
   - Actionable errors.
   - Run evals and read raw transcripts.
   - **Phaze:** add `response_format` to `grep`, `glob` and `list` (`tools/grep.rs`, `glob.rs`, `list.rs`). Truncation text names the next action ("narrow with path=… or use offset"). Errors in `edit.rs` include closest matches (§3.4).
6. **Advanced tool use.** Source: https://www.anthropic.com/engineering/advanced-tool-use
   - Tool search with `defer_loading`: about 85-89% fewer upfront tool tokens, and it doesn't break caching.
   - Programmatic tool calling: 37% fewer tokens on complex research.
   - `input_examples`: parameter accuracy from 72% to 90%.
   - **Phaze:** MCP tools from `tools/mcp_bridge.rs` and rarely used built-ins (download, screenshot, browse, copy/move/delete) become deferred stubs. A `tool_search` tool (BM25, as in Codex) goes in `tools/mod.rs`. The full tool list is frozen at session start.
7. **Multi-agent research system.** Source: https://www.anthropic.com/engineering/multi-agent-research-system
   - 90.2% better than single-agent on internal research, but about 15x the tokens of chat.
   - Token usage explains 80% of the variance.
   - "Most coding tasks involve fewer truly parallelizable tasks than research."
   - Parallel tool calls cut research time by up to 90%.
   - **Phaze:** keep **parallel read-only tool calls**, which the working branch already has. Use subagents only for isolation of verbose reads, and run them on a cheaper model. No multi-agent fan-out by default.
8. **Long-running harness.** Source: https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents
   - An initializer writes a feature list as JSON with `passes` flags; JSON is chosen because models damage it less than Markdown.
   - A progress file, git commits as checkpoints, one feature per session, and end-to-end tests.
   - **Phaze:** `/plan` writes `.phazeai/plan.json` (steps with status). The plan is re-injected after compaction (`context/compaction.rs`) and edited only through an `update_plan` tool.
9. **Claude Code: prompt caching is everything.** Source: https://claude.com/blog/lessons-from-building-claude-code-prompt-caching-is-everything
   - Static content first; never edit the system prompt mid-session; dynamic info goes in `<system-reminder>` messages.
   - Plan mode as a tool; deferred tool stubs.
   - Compaction that forks the cached prefix.
   - No mid-session model switch.
   - Alert on hit rate.
   - **Phaze:** see Best-of #1-#3.
10. **Claude Code best practices.** Source: https://code.claude.com/docs/en/best-practices
    - CLAUDE.md under 200 lines, holding only what can't be inferred from the code.
    - Rules that must never be broken go in hooks or deny rules.
    - Give the agent a check to run (tests, typecheck, a Stop hook).
    - Review in a fresh subagent.
    - **Phaze:**
      - `context/system_prompt.rs` warns when instructions exceed about 200 lines or 25 KB.
      - Add a hooks runner (`crates/phazeai-core/src/hooks.rs`, new) with PreToolUse, PostToolUse and Stop.
      - Add a `verify_cmd` setting run by a Stop-equivalent in `agent/core.rs`.
11. **SWE-bench scaffold.** A minimal prompt, bash plus a str_replace edit tool with a unique-match rule, absolute paths and clipped output scored 49% on SWE-bench Verified (https://www.anthropic.com/engineering/swe-bench-sonnet).
    - **Phaze:** `tools/edit.rs` already matches this shape. Keep it as the default edit tool for Anthropic and other non-OpenAI models.

### OpenAI
12. **Prompt caching guide.** Source: https://developers.openai.com/api/docs/guides/prompt-caching
    - Automatic prefix caching from 1,024 tokens.
    - Explicit breakpoints on GPT-5.6 and later.
    - Cached reads are "discounted up to 95%" (0.1x the normal input price on most new models).
    - `prompt_cache_retention: "24h"`.
    - Keep tools and ordering identical.
    - `usage.input_tokens_details.cached_tokens`.
    - **Phaze:** `llm/openai.rs` sends `prompt_cache_key = session_id` and parses `cached_tokens` into an extended `llm/traits.rs::Usage { cache_read, cache_write }`. Tool definitions are sorted deterministically in `tools/mod.rs`.
13. **Prompt Caching 201.** Per-conversation `prompt_cache_key` raised one customer's hit rate from 60% to 87%. Use `allowed_tools` instead of removing tools. Compaction breaks the cache, so choose its frequency with evals. Responses API reasoning items give 40-80% better cache use. Source: https://developers.openai.com/cookbook/examples/prompt_caching_201
    - **Phaze:** same as #12. Plan mode restricts tools by *policy*: the approval layer refuses writes, and `tools.definitions()` stays unchanged. For OpenAI reasoning models, add a Responses-API path to `llm/openai.rs` later.
14. **apply_patch / V4A.** A model-native patch format with create, update and delete operations, plus per-call `failed` status and a clear output (§5.3).
    - **Phaze:** add `tools/apply_patch.rs`, porting Codex's grammar and `seek_sequence` ladder. `agent/core.rs` registers `apply_patch` instead of `edit_file` for OpenAI/Codex model ids, routed by `llm/model_router.rs`, as Cline does (§4.2).
15. **Codex sandbox and execpolicy.** bwrap+seccomp, Seatbelt, workspace-write by default, and prefix rules with load-time example tests (§5.5).
    - **Phaze:** add a `tools/bash.rs` sandbox wrapper (bwrap on Linux, `sandbox-exec` on macOS, network off unless allowed). Rules engine in `tools/approval.rs` (Best-of #6).

### DeepSeek
16. **Context caching on disk.** Source: https://api-docs.deepseek.com/news/news0802
    - Automatic and prefix-only, in 64-token units.
    - Hits cost 0.1x a miss (originally $0.014 vs $0.14 per M).
    - First-token latency for a 128K prompt fell from 13 s to 0.5 s.
    - Cache entries live "a few hours to days".
    - Usage fields `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`.
    - **Phaze:** nothing to configure. The prefix discipline in Best-of #1 earns this for free. Parse the two usage fields in `llm/openai.rs`, which serves OpenAI-compatible endpoints. **[inferred]** Phaze routes DeepSeek through that client.
17. **V3.2-Exp DeepSeek Sparse Attention.** Benchmarks "on par with V3.1-Terminus". "API prices drop 50%+". Source: https://api-docs.deepseek.com/news/news250929.md
    - There is no client-side lever. The implication is that long, cached contexts on DSA models are cheap. Compaction thresholds should be **per-model cost-aware**, not one global 80%.
    - **Phaze:** `llm/context_budget.rs` gets a per-provider compaction trigger. It runs later on cheap, cache-heavy providers and earlier on expensive uncached ones.

### Aider and Cline benchmark notes
18. **Aider polyglot.** Weak or local models are malformed in `diff` format 28% of the time and do 2x better with `whole` (Qwen2.5-Coder-32B 8.0% vs 16.4%). Frontier models do best with diff formats (§3.4).
    - **Phaze:** `llm/model_router.rs` picks the edit tool by model. Local Ollama models get `write_file` (whole file) as the primary edit path. Frontier models get `edit_file` or `apply_patch`.
19. **Aider architect/editor.** Architect plus editor matched or beat solo at lower cost (85% state of the art in 2024; o3+gpt-4.1 scored 78.2% at $17.55 vs o3 solo 81.3% at $21.23).
    - **Phaze:** optional `/architect` mode in `agent/core.rs`. The plan comes from the main model and the edits from a cheap "editor" model set in `llm/model_router.rs`.
20. **Cline.** Deterministic "basic" compaction needs no LLM call and freezes earlier output. Agentic compaction reuses the prompt cache. Compaction triggers at about 81% of the window and keeps the most recent 20k tokens verbatim (§4.1).
    - **Phaze:** two-stage compaction in `context/history.rs` + `context/compaction.rs` (Best-of #2).

---

## 7. Phaze IDE today (main @ fe95cec), relevant findings

| Area | Current state (file:line) | Gap vs best-of |
|---|---|---|
| Prompt caching | No `cache_control` anywhere. `llm/claude.rs:62-121` sends `system` as a plain string. `llm/traits.rs:96-100` `Usage{input_tokens, output_tokens}` has no cache fields. | **On Anthropic, Phaze pays full input price every turn of a 200-turn loop.** Hit rate can't be measured. The cache_control mechanics are covered by the other researcher. |
| Summary placement | `context/history.rs:93-110` appends the compaction summary **into the system message**. | Every compaction rewrites the system prompt. It should go in a message after the stable prefix (Claude Code layer model). |
| System prompt stability | Built once per agent (`phazeai-cli/src/app/mod.rs:885-895`). It includes git branch and dirty files (`context/system_prompt.rs:441-462`). | Fine within a session. Dirty files should move to a first user-turn reminder so sessions in the same directory share a prefix (as Claude Code does). |
| Compaction | Trigger 0.8 (`context/compaction.rs:11`). Keeps 40% recent (`:13`), summary 15% (`:15`). Called each iteration in `agent/core.rs:304-330`, then hard-trimmed (`:337`). | No cheap tool-result clearing stage before it. The summarizer request doesn't reuse the cached prefix. **[inferred]** `compaction::request` builds its own short message list (`:38-55`). User messages aren't kept verbatim (Codex keeps 20k tokens of them). |
| Tool output cap | 12,000 chars with middle cut (`context/history.rs:70-83`). | Close to Codex (10 KB). Missing: a "truncated, N lines / tokens" header with next-step steering, and spilling the full output to a file the agent can grep (Claude Code, Cursor). |
| read_file | No default limit. Numbers every line (`tools/file.rs:55-74`). | Add a default line cap plus `offset`/`limit` steering, a token cap (about 25k), and optionally sparse line numbers (Cursor: -1.6%). |
| Repo map | `context/repo_map.rs` exists (tree-sitter symbols, `max_tokens 4096`) but **is not used anywhere**: only re-exported at `lib.rs:37` and `context/mod.rs:11`. It has no ranking. | Port Aider's PageRank ranking and binary-search fit. Inject it as a cached block after the system prompt, refreshed only when files change. |
| Edit tool | Single exact `old_text` → `new_text`, with `context` or `replace_all` (`tools/edit.rs:17-19,109-114`). The error is just "old_text not found". | Add Aider-style "did you mean" lines and the Codex whitespace/Unicode ladder. Route `apply_patch` for OpenAI models and whole-file for weak local models. |
| Approval | Modes (read / edit / all) plus per-tool classification (`tools/approval.rs:9-30,75-90,155-175`). No rule syntax. | Add Claude Code-style deny > ask > allow rules with `Tool(pattern)`, compound-command splitting, and settings scopes. |
| Bash sandbox | Path resolution only (`tools/sandbox.rs:94-150`). No OS sandbox. | bwrap/seatbelt wrapper (Codex, Claude Code: 84% fewer prompts). |
| Undo | `/undo` = discard **all** uncommitted changes (`phazeai-cli/src/commands.rs:316-319,600-611`). | Per-turn checkpoints on private refs (Cline design), with restore code / conversation / both. |
| Plan | `/plan` sets a prompt mode only (`commands.rs:581`). | Policy-enforced read-only mode with the tool list unchanged, a plan file and an `update_plan` tool. |
| Memory | Key-value JSON store (`tools/memory.rs:8-30`). CLAUDE.md, AGENTS.md and `.phazeai/instructions.md` are loaded with a parent walk (`context/system_prompt.rs:355-420`). | Add a size cap (32 KiB, like Codex), conditional `paths:` rules, lazy loading of subdirectory instructions, an index plus topic memory files, and `@imports`. |
| Model routing | `llm/model_router.rs` classifies by keywords into task types. | Per-turn model switching would bust caches. Route at the *session or subagent* level only, plus a weak model for summaries and commit messages. |

---

## 8. Best-of for Phaze, by dimension

Ranked by impact. The headline metric is token cost; task success comes second. The impact estimates are mine and marked as estimates; each cites the evidence it rests on.

### #1 Cache-stable prefix and real caching on every provider (token cost, largest)
- **Pattern.**
  - Order each request as: tools (sorted, frozen per session), system prompt (static), project instructions plus repo map (session-static), conversation.
  - Never mutate anything above the conversation mid-session. Dynamic facts (git status, file-changed notices, plan text, mode changes) go in `<system-reminder>` blocks appended to the next user or tool message.
  - Anthropic: explicit `cache_control` breakpoints, the last one on the latest message.
  - OpenAI: `prompt_cache_key = session_id`.
  - DeepSeek and Ollama: prefix discipline alone.
  - Measure `cache_read`/`cache_write` and log the *reason* for each miss.
- **Sources.** Claude Code (prompt-caching doc and blog), Codex (`client.rs:425-500` miss reasons, `:712` cache key), Aider (`chat_chunks.py` breakpoints), Cursor (phantom user message: -20% cold misses).
- **Evidence.**
  - Cached input costs about 0.1x on Anthropic, OpenAI (new models) and DeepSeek.
  - OpenAI: hit rate from 60% to 87% with a cache key.
  - DeepSeek: TTFT from 13 s to 0.5 s on 128K.
  - Claude Code alerts on hit rate.
  - **Estimate:** on a long agent loop where more than 90% of input is re-sent prefix, input cost falls by roughly 70-85%. This is the "half the tokens" goal on its own.
- **Phaze files.** `llm/claude.rs`, `llm/openai.rs`, `llm/traits.rs` (Usage cache fields plus StreamEvent), `context/history.rs:93-110` (move the summary out of the system message), `context/system_prompt.rs:441-462` (dirty files become a first-turn reminder), `tools/mod.rs` (deterministic definition order), `telemetry.rs` (hit rate and miss reason).

### #2 Two-stage compaction: clear tool results first, then cache-sharing summary (token cost and long-task success)
- **Pattern.**
  - **Stage A** (free, deterministic): replace tool results older than the last N turns with a one-line stub ("[read src/x.rs 340 lines, cleared]"). Rebuild on a batched schedule so the cache isn't busted every turn: clear in large chunks when about 20-30% of the window is stale.
  - **Stage B**: at about 85-90% of the window, ask for an LLM summary using the *same* system, tools and history plus an appended instruction, so the summary request reads from cache. Use Codex's "handoff summary" prompt.
  - Keep the user's messages verbatim (up to about 20k tokens) and the last 20k tokens of history.
  - Re-inject the instruction files, the plan file and up to 5 recently edited files (only under 5k tokens each; paths otherwise).
  - Optionally keep the full transcript on disk and reference it in the summary (Cursor).
- **Sources.** Anthropic context editing (84% fewer tokens, +29%), Claude Code compaction, Codex (`compact.rs:64`, prompt template), Cline (basic/agentic split, trigger at 0.9×0.9, 20k recent), Cursor (history file).
- **Phaze files.** `context/history.rs` (new `clear_stale_tool_results`), `context/compaction.rs` (prompt, kept user messages, cache-sharing request, re-injection), `agent/core.rs:304-337`, `llm/context_budget.rs` (per-provider trigger).

### #3 Deferred tools plus tool search, with a frozen tool list (token cost)
- **Pattern.**
  - Core tools stay static: read, grep, glob, edit, bash, the plan tool, and ask.
  - MCP tools and rarely used built-ins (download, browse, screenshot, copy/move/delete) are sent as name-only stubs. A `tool_search` tool (BM25, as in Codex) loads their schemas.
  - Never add or remove definitions mid-session. Plan mode and permission changes are enforced in the approval layer, not by editing `tools`.
- **Evidence.** Anthropic: 77K to 8.7K upfront tokens, accuracy 49% to 74%. Cursor: -46.9% tokens on MCP runs, -60% static tool tokens. Claude Code: plan mode as a tool.
- **Phaze files.** `tools/mod.rs`, `tools/traits.rs` (a `deferred` flag), `tools/mcp_bridge.rs`, new `tools/tool_search.rs`, `tools/approval.rs` (plan-mode refusal).

### #4 Ranked repo map (Aider PageRank) and a bounded read_file (token cost and success)
- **Pattern.**
  - Wire in the unused `context/repo_map.rs`. Add Aider's reference graph with weights (×10 mentioned, ×10 long names, ×0.1 private names and names defined in more than 5 files, ×50 from chat files, sqrt of reference count) and personalized PageRank.
  - Binary-search the number of tags against the budget: 1k tokens by default, up to 8x when no files are in context.
  - Cache tags by mtime. Inject the map in the session-static layer, and refresh only on file-set change (Aider's `files` mode when caching).
  - `read_file` gets a default cap of about 2,000 lines or 25k tokens, with explicit continuation hints.
- **Evidence.** Aider SWE-bench Lite: 26.3% (20.3% first attempt) credited to the repo map. Cursor: -1.6% from sparse line numbers. Claude Code: 25k-token Read cap.
- **Phaze files.** `context/repo_map.rs`, `analysis/` (tags), `context/builder.rs`, `tools/file.rs:42-80`.

### #5 Edit tool chosen per model, with self-correcting failures (task success; fewer retry tokens)
- **Pattern.** Keep `edit_file` (str_replace with a unique match) as the default.
  - Add `apply_patch` (V4A; port the Codex parser and `seek_sequence`) for OpenAI/Codex models.
  - Make whole-file `write_file` primary for small local models.
  - On failure, return the closest matching lines ("did you mean", Aider threshold 0.6), say whether the new text is already present, and report how many edits in the batch succeeded.
  - Match on a ladder: exact, rstrip, trim, Unicode-normalized.
  - After edits, run a lint or `verify_cmd` and feed errors back.
- **Evidence.** Claude Code and SWE-bench (49% with str_replace). Codex (apply_patch). Cline routes by model. Aider (weak models 2x better with whole; diff errors rise 30-50% without high-level diffs; laziness 20% to 61% with udiff; JSON-wrapped code hurts).
- **Phaze files.** `tools/edit.rs`, new `tools/apply_patch.rs`, `tools/file.rs`, `llm/model_router.rs`, `agent/core.rs` (tool set chosen at session start, so caching holds).

### #6 Rule-based permissions, OS sandbox, hooks (fewer prompts, safety)
- **Pattern.**
  - Claude Code rule grammar in `settings.json` (user, project, local), evaluated deny > ask > allow: `Bash(cargo test *)`, `Read(./.env)`, `Edit(/src/**)`, `WebFetch(domain:x)`, `mcp__srv__tool`.
  - Split compound commands on `&& || ; | & newline`. Allow rules must match every part; deny matches any part, including `$()`.
  - Optionally borrow Codex's `match`/`not_match` load-time tests for rules.
  - bwrap (Linux) or Seatbelt (macOS) sandbox for `bash`: writes limited to the workspace, network off or proxy-allowlisted. Sandboxed commands can be auto-allowed.
  - Hooks: PreToolUse (exit 2 blocks; JSON can return `updatedInput`, e.g. to filter test output), PostToolUse (formatter), Stop (verify gate).
- **Evidence.** Claude Code sandboxing: 84% fewer prompts. Hook-based output filtering: "tens of thousands of tokens to hundreds".
- **Phaze files.** `tools/approval.rs`, `tools/bash.rs`, `tools/sandbox.rs`, new `hooks.rs`, `config/`.

### #7 Per-turn checkpoints on private git refs, plus rewind (success and recovery)
- **Pattern.**
  - Before each user turn, and optionally after each write tool, snapshot the working tree (tracked plus untracked) with a separate `GIT_INDEX_FILE` into `refs/phazeai/checkpoints/<session>/<n>`, never touching the user's index, stash or HEAD.
  - `/rewind` offers restore code / conversation / both, and "summarize from here".
  - Rewinding the conversation truncates to an already-cached prefix.
  - Replaces today's `/undo`, which discards all uncommitted changes, including the user's own.
- **Evidence.** Cline (`checkpoint-hooks.ts`). Claude Code checkpointing; its limitation is that Bash edits are missed. Cline's snapshot-after-every-tool approach covers Bash.
- **Phaze files.** `git/`, `phazeai-cli/src/commands.rs:316,600`, `context/persistence.rs`, `agent/core.rs`.

### #8 Plan mode as policy plus a persisted plan (success)
- **Pattern.**
  - `/plan` keeps the same tools and the approval layer refuses writes (Cline semantics, Claude Code cache design).
  - The plan is written to `.phazeai/plan.json` (JSON per the Anthropic harness post) and changed through `update_plan`, with at most one step in progress (Codex).
  - The plan is re-injected after compaction.
  - Optionally use a different model for planning and acting (Cline, Aider architect), switched at the turn boundary.
  - **[inferred]** Prefer a subagent handoff over switching models in one conversation (Claude Code blog).
- **Phaze files.** `phazeai-cli/src/commands.rs:581`, `tools/approval.rs`, new `tools/plan.rs`, `context/compaction.rs`.

### #9 Subagents for verbose work on a cheaper model (token cost in the main context)
- **Pattern.**
  - A `task` tool runs a child agent with a fresh history, read-only tools and a cheap model.
  - It returns 1-2k tokens. The child shares the parent's `prompt_cache_key` lineage (Codex `source:parent_thread_id`).
  - Don't fan out by default.
- **Evidence.** Anthropic: subagent summaries of 1-2k tokens, but multi-agent costs about 15x and coding parallelizes poorly. Cursor reduced subagent pushing. Claude Code docs: subagents cost tokens and can be slower.
- **Phaze files.** new `tools/task.rs`, `agent/core.rs`, `llm/model_router.rs`.

### #10 Weak model for auxiliary calls (token cost)
- **Pattern.** Summaries (when the main model's cache is cold), commit messages, titles and classification go to a configured cheap model. Aider's `[weak_model, main_model]` fallback order.
- **Caveat.** While the main model's cache is warm, Claude Code's guidance is that a summary on the *same* model reading the cache is cheaper. Choose dynamically: weak model only when the cache is cold.
- **Phaze files.** `llm/model_router.rs`, `context/compaction.rs`, `git/`.

### #11 Instruction files: cap, scope, lazy-load (token cost)
- **Pattern.**
  - Keep reading CLAUDE.md, AGENTS.md and `.phazeai/instructions.md`.
  - Add a combined 32 KiB cap (Codex) and a warning above about 200 lines.
  - Root-first concatenation so deeper files win.
  - Subdirectory instruction files load when a file under them is first read, appended as a message, not added to the system prompt.
  - `.phazeai/rules/*.md` with `paths:` frontmatter (Claude Code, Cline, Cursor globs).
  - `@imports` up to 4 hops.
  - Memory becomes an index file (first 200 lines loaded) plus topic files.
- **Phaze files.** `context/system_prompt.rs:355-420`, `tools/memory.rs`, `context/builder.rs`.

### #12 Output spill-to-file plus steering headers (token cost)
- **Pattern.**
  - Bash, MCP and grep output over about 10-30k characters is saved to `.phazeai/tmp/<call>.txt`. The tool result is a header (original tokens, lines, path) plus head and tail, and the agent greps the file.
  - Add `response_format: concise|detailed` to search tools.
- **Evidence.** Claude Code: 30k inline, then a file and 2k preview. Cursor: files instead of truncation, fewer re-summarizations. Codex: truncation header. Anthropic tools post: 206 vs 72 tokens.
- **Phaze files.** `context/history.rs:70-83`, `tools/bash.rs`, `tools/grep.rs`, `tools/mcp_bridge.rs`, `text.rs`.

### #13 Cache keepalive (cost, situational)
- **Pattern.** Aider-style `max_tokens=1` pings every 4m55s for N pings while the IDE is idle mid-task, or the Anthropic 1-hour TTL for the main loop.
- **When it pays.** Only when user think-time often exceeds 5 minutes. Claude Code instead compacts an idle session before the cache expires.
- **Phaze files.** `llm/claude.rs`, `agent/core.rs`.

### Rationale behind the ranking
#1, #2 and #3 are multiplicative on *every* request of a 200-turn loop and need no model cooperation. #4 and #12 shrink what enters context. #5 cuts failed-edit retries, which is the main source of wasted output tokens. #6, #7 and #8 mainly raise success and safety. #9, #10, #11 and #13 are smaller or situational.

---

## 9. Could not verify, and caveats
- **Claude Code internals** (exact compaction prompt, tool-clearing thresholds, default auto-compact percentage per model) are not public. Behaviour above is from the docs. The default auto-compact thresholds page (`/docs/en/model-config#default-auto-compact-thresholds`) was not fetched.
- **Cursor:** apply-model accuracy numbers are vendor self-graded, with acknowledged Claude bias. I did not fetch checkpoint or plan-mode docs. The "Composer" model and RL details were not researched.
- **Codex docs moved** to learn.chatgpt.com. The sandboxing page doesn't state the network default or protected paths (`.git` read-only), so those are unverified. The Linux sandbox is bwrap+seccomp per the repo README, not Landlock-first as older docs said.
- **No head-to-head benchmark** compares apply_patch vs str_replace vs SEARCH/REPLACE on the same model. **[inferred]** The claim that OpenAI models are trained on V4A comes from Codex/Cline routing behaviour; OpenAI's apply_patch page makes no accuracy claim.
- **DeepSeek:** current cache-hit prices weren't readable (price image). The V3.2 DSA material gives no client-side lever.
- **Aider:** the fuzzy edit matcher is dead code in the current source (`editblock_coder.py:183`), so Aider's real ladder is exact, whitespace, blank-line, then `...`.
- **Cline:** moved to `sdk/packages/*`. The older `src/core/context/context-management` (file-read dedup, first-pair preservation) no longer exists in this clone, so it isn't cited.
- **Ollama prefix (KV) cache reuse** across requests is not verified here. **[inferred]** Prefix stability helps local models' time-to-first-token too.
- **Phaze:** analysis is of `main@fe95cec`. The caller's branch (200-turn default and parallel read-only calls) wasn't available. `DEFAULT_MAX_ITERATIONS=200` is already on main (`agent/core.rs:21`), but parallel calls are not.
- **Gemini CLI and Zed** were not researched; there was no time, and they are lower priority.
