# Token efficiency: measured baseline and the best-of plan

Goal: Phaze should spend about half the tokens other agents spend on the same
task, and prove it with measured numbers rather than estimates.

This spec turns the research in [`docs/research/`](research/) into a PR
series. Each change names its source and the number it is expected to move.
After each PR the token bench is rerun and its real numbers are added to the
results table below.

- [token-saving.md](research/token-saving.md): opencode's code, provider caching
  docs, Phaze today (Parts A to D).
- [agents-survey.md](research/agents-survey.md): Claude Code, Cursor, Aider, Cline,
  Codex CLI, and what Anthropic, OpenAI, DeepSeek, Aider and Cline have
  published (section 6).
- [models-and-local-runtime.md](research/models-and-local-runtime.md): models,
  cheap-model routing, vLLM/SGLang/llama.cpp/Ollama caching, LLMLingua, and the
  built-in runtime.

## 1. How it is measured

`cargo test -p phazeai-cli --lib token_bench -- --ignored --nocapture`
(`crates/phazeai-cli/src/app/tests/token_bench.rs`).

- **The task.** One fixed two-message session on a small Rust service (fixture
  in `crates/phazeai-cli/tests/fixtures/token_bench`, about 500 lines, set up
  as a git repository with a fixed commit, as a real project would be).
  - First message: find why the server crashes when `PORT` is set but empty, fix
    it, add tests.
  - Follow-up: log a warning when the default port is used.
  - 14 model requests, 14 tool calls (list, read, grep, edit, bash).
- **What is real.** The CLI's own agent setup (`single_prompt_agent`): system
  prompt, tool schemas, history handling and tool-result formatting. Real
  tools run on real files, so tool output is real.
- **What is scripted.** The model's side: which tools it calls, with which
  arguments, and its short replies. That makes every run make the same calls,
  so before and after compare the same work. Output tokens are therefore
  identical across PRs and not reported.
- **How tokens are counted.** Each captured request is laid out the way
  providers build a prompt (tool schemas, then messages) and counted with the
  `o200k` tokenizer. For each request the bench also counts the longest prefix
  it shares, token for token, with the previous request: what a prefix cache
  can reuse.
- **Effective cost.** Reusable tokens billed at 0.1x, the cache-read price at
  Anthropic, OpenAI and DeepSeek ([token-saving.md](research/token-saving.md)
  Part B). This is only what a provider *could* bill; whether it does depends
  on the provider (see the next section).

Absolute counts differ a little from any one provider's bill (each wraps
messages in its own template), and slightly from machine to machine (the
fixture's `.git` holds the local git version's sample hooks). The comparison that matters is before vs after
on the same harness.

## 2. Baseline: `main` at `fe95cec`

| Measure | Value |
|---|--:|
| Model requests | 14 |
| Input tokens sent, whole task | **122,828** |
| Tool schemas offered per request | 23 tools, 2,557 tokens |
| System prompt per request | 1,130 tokens |
| Share of all input that is tool schemas (2,557 x 14) | 29.1% |
| Share that is the system prompt (1,130 x 14) | 12.9% |
| `.git` internals in the one `list_files` result, resent in the 12 requests after it | about 1,740 tokens each time |
| Reusable by a prefix cache within the session | 111,360 (90.7%) |
| Effective input if that part were cached at 0.1x | 22,604 (18.4%) |
| First request of a new session sharing a prefix with the last one | **5 of 3,732 tokens** |

What the baseline means:

1. **On Claude, Phaze pays full price for all 122,828 tokens.** The Claude client
   sends no `cache_control` (`llm/claude.rs`), so Anthropic caches nothing
   ([token-saving.md](research/token-saving.md) C1). The 90% that could be
   reused is billed in full.
2. **Across sessions, nothing is reusable on any provider.** Tool definitions come
   from a `HashMap`, so every new agent sends them in a new random order and a
   new session shares 5 tokens with the last. Even with a fixed order, the
   system prompt lists the uncommitted files near its top, so a session that
   starts after edits differs from there on. The IDE chat builds a new agent
   for every message (`phazeai-ui/src/panels/chat.rs`), so in the IDE every
   message starts with a cold cache, even on OpenAI and DeepSeek, which cache
   automatically.
3. **Over 40% of every request is fixed overhead.** Of the 23 tools offered, the system
   prompt describes 8. The rest (`screenshot`, `browse`, `download`, `open`,
   `now`, `copy_path`, `move_path`, `delete_path`, `create_directory`,
   `web_search`, `fetch`, `diagnostics`, `find_path`, `build_search_index`,
   `code_search`) ride along on every request of a coding task.
4. **`list_files` lists `.git`.** It walks hidden files and `.gitignore` does not
   exclude `.git`, so a recursive listing returns git's object store and hooks
   (`tools/list.rs`). Measured by running the bench with and without a git
   repository: the listing's request grew from 845 to 2,581 new tokens, and
   that result is resent in every later request.

## 3. Targets

- **Raw input tokens:** 122,828 down to 61,000 or less on the same task. This cuts
  cost on every provider, including local models, where it is speed rather than
  money.
- **Effective cost on Claude:** at least 80% below today's full-price 122,828.
  This comes from caching once the prefix is stable.
- **New-session prefix reuse:** from 5 tokens to the whole static prefix (tools plus
  base system prompt).

Each PR reports what it actually hit. If a target is missed, the table says so.

## 4. The plan, in order

The order follows one rule from every source: caching only pays when the
prefix is byte-identical from one request to the next (Anthropic, OpenAI and
DeepSeek docs; vLLM and SGLang need token-identical prefixes from token 0;
[agents-survey.md](research/agents-survey.md) section 8 #1). So the prefix is fixed
first, then caching is switched on, then the prefix is made smaller.

### T1 (this PR): research, spec, token bench, baseline

### T2: a byte-stable prefix
- **Tool order.** Tool definitions are sorted by name, so the same tool set always
  serialises the same way (`tools/traits.rs`). Source: opencode `tool.ts:299`;
  OpenAI's caching guide counts tool order as part of the prefix.
- **Volatile facts last.** The git branch and uncommitted files move from the top
  of the system prompt to its end, after the static rules and instructions,
  so they no longer invalidate everything after them. Source: Claude Code and
  Codex keep changing state out of the cached prefix
  ([agents-survey.md](research/agents-survey.md) section 8 #1).
- **Moves:** new-session prefix reuse (5 tokens to the full static prefix), and the
  IDE chat's per-message cache.

### T3: Claude caching and cache measurement
- **Claude cache breakpoints.** `system` is sent as up to two blocks: the shared
  part (base prompt and project instructions, the same in every session of a
  project) and the session's own part (git status, then any compaction
  summary). `cache_control` breakpoints go on the last tool, the end of each
  system block and the conversation tail, 4 at most. Consecutive tool results
  are merged into one user message so the tail stays where the cache expects
  it. Source: Anthropic prompt-caching docs; opencode `cache-policy.ts:5-34`.
- **Cache usage fields.** `Usage` gains cache-read and cache-write counts, and
  `input_tokens` now means the whole prompt on every provider. They are parsed
  from Anthropic (`cache_read_input_tokens`, `cache_creation_input_tokens`,
  from both the stream's start and delta events), OpenAI and xAI
  (`prompt_tokens_details.cached_tokens`), DeepSeek (`prompt_cache_hit_tokens`)
  and llama-server (`timings.cache_n`). Phaze could not see a cache hit before
  ([models-and-local-runtime.md](research/models-and-local-runtime.md) section 0).
  The CLI status line shows how much came from cache, and the cost estimate
  prices Claude's reads at 0.1x and writes at 1.25x.
- **The bench gains a Claude-format run.** It sends the real Anthropic request
  bodies to a local server and bills them by the breakpoints actually sent,
  using Anthropic's prefix and 20-block lookback rules.
- **Moves:** effective cost on Claude (target: down 80% or more).

### T3b: OpenAI-compatible caching hints
- **One cache key per project.** OpenAI, Azure, Cerebras, Fireworks and
  OpenRouter get `prompt_cache_key`; xAI gets the same key as its
  `x-grok-conv-id` header. The key is a hash of the project root, so every
  session and every IDE message of a project (the IDE builds a new agent per
  message) reaches the server holding the shared prefix that T2 made stable,
  and the path never leaves the machine. Only providers known to accept the
  field get it, because strict servers answer an unknown body field with a
  400. Sources: opencode `packages/ai/src/providers/*.ts` (`promptCacheKey:
  true` on exactly those five) and `providers/xai.ts:65`; OpenAI's 60% to 87%
  hit-rate report ([agents-survey.md](research/agents-survey.md) #13); docs.x.ai
  "Maximizing cache hits". OpenAI caps the key at 64 characters and advises
  about 15 requests a minute per key, which one person's project stays under.
- **Streams report usage.** Streams send `stream_options.include_usage`, as
  opencode does for every OpenAI Chat route (`openai-chat.ts:900`). OpenAI
  sends no usage in a stream without it (OpenAI's May 2024 announcement), nor
  does Gemini's compatibility endpoint (Google AI forum); llama-server accepts
  it (llama.cpp issue 15443). A server that answers 400 or 422 naming
  `stream_options` is asked once more without it and remembered, so a strict
  custom server still works. Usage is reported once per stream, the last value
  seen, because servers differ on which chunk carries it.
- **Moves:** OpenAI-side cache hits across sessions, and real usage and cache
  counts from OpenAI streams, which Phaze never had.

### T4: a lean system prompt and a lean tool set
- **System prompt.** `TOOL_GUIDELINES` repeated the tool schemas, listed tools
  that were not registered and showed worked tool calls; the "Available Tools"
  list repeated the tool definitions sent with the request. Both are gone. The
  fixed part is now one block of rules, about 1 KB, which names only tools the
  agent has (a test checks this). opencode's base prompt is 756 bytes
  (`system.txt`). Source: [token-saving.md](research/token-saving.md) C6, D8.
- **Tool set.** A coding session offers its core tools: read, write, edit, bash,
  grep, glob, list and code search. The rest (web, file management, memory,
  diagnostics, screenshots, the index rebuild, and every MCP tool) are held
  back behind one `more_tools` tool, whose description gives each a line.
  The model calls it with a name to get that tool's parameters (they come
  back as a tool result, so the prefix only ever grows at the end), then with
  the name and `arguments` to run it. The agent treats that second call as a
  call of the tool itself: approval, diff review, events and the record all
  see the real tool name and arguments. Sources: Anthropic tool search (77K to
  8.7K upfront tokens, accuracy 49% to 74%); Cursor (46.9% fewer tokens on MCP
  runs); opencode Code Mode catalog budget (`codemode/catalog.ts:50-57`).
- **Moves:** raw input. This is the largest single cut on this task, because
  tools plus prompt were 51% of all input.

### T5: compact tool results and bounded output

Built:

- **Plain-text results** (`tools/render.rs`, called from `tool_outcome` in
  `agent/core.rs`). Tools still return JSON, which the UI, the CLI and the
  tests read; only what the model is told changes. Before, it got the JSON
  pretty-printed, so every file read was one escaped string and every grep hit
  repeated its keys and path. Now:
  - read_file: the numbered lines (`12\tcode`, no padding), and when the read
    stops early, `[lines 1-2000 of 2500; read on with offset 2001]`
  - bash: stdout, then `[stderr]` and stderr if any, then `[exit code N]` only
    when the command failed
  - grep: hits grouped under their file, as `rg --heading` shows them
  - glob, list_files: one path per line, directories ending in `/`
  - write_file, edit_file: one line (`Edited src/a.rs (1 replacement)`)
  - code_search: each snippet under `file:start-end`
  - anything else (MCP tools, web): compact JSON, or the string itself
  - paths under the current directory are shown relative to it; the tools
    resolve relative paths against it, so they round-trip
- **Default caps:**
  - read_file: 2,000 lines, 50 KiB and 2,000 characters a line when no
    `limit` is given (it had no cap); the result says where to read on
  - grep: 100 hits (was 500), each line cut at 300 characters
  - glob: 500 paths (was 1,000); list_files: 1,000 entries
  - every capped listing says it stopped and that narrowing the search shows
    the rest
  - grep, glob and list_files skip `.git`, `.hg`, `.svn` and `.jj` (they walk
    hidden files, so a recursive listing went through every git object)

Not done, on purpose: spilling cut output to a file. A cut read already says
how to read on, and a capped search can be narrowed. For bash, the tools can
only read inside the workspace, so a spill file would have to be written into
the project; bash keeps its 30,000-byte head-and-tail cut per stream.

Source: opencode `tool-output.ts:13-120`, `read.ts`, `grep.ts`,
`to-llm-message.ts:114-128`; Claude Code and Codex cap the same way
([agents-survey.md](research/agents-survey.md) #12).

### T6: context that stays cached
- **No in-place rewrites.** Overflow handling stops editing earlier messages
  (the `STEPS_REMOVED_NOTE` rewrite in `context/history.rs`).
- **Send unchanged until a ceiling, then compact once.** This replaces trimming
  before every request and the 100-message cap. The ceiling is the window minus
  max(15%, 16k). Source: opencode `compaction.ts:1163-1171`; OpenAI: "avoid
  truncation".
- **Compaction reuses the cache.** It appends the summary request to the
  unchanged conversation instead of re-sending the transcript as fresh text.
  Source: opencode `compaction.ts:495-530`; Claude Code and Cline do the same.
- **Old tool results are cleared in batches, never per step.** On Claude this
  uses `clear_tool_uses` with `clear_at_least`. Anthropic reports 84% fewer
  tokens and 29% better results. Source:
  [agents-survey.md](research/agents-survey.md) section 6.
- **After compaction, re-inject** the instruction files, the plan and recently
  edited files.
- **Moves:** long sessions. The bench task is short, so a long-session variant is
  added here.

### T7: side jobs on a cheap model, and local-model settings
- **Cheap models for side jobs only.** Compaction summaries, titles and commit
  messages go to a cheap model. The main loop never switches model per turn,
  because each switch throws away the provider's cache for that model. Phaze's
  keyword router does exactly that today
  ([models-and-local-runtime.md](research/models-and-local-runtime.md) section 1.4).
- **Thinking off for helper calls** (`enable_thinking: false`, or a low
  `reasoning_effort`).
- **Ollama:** send `num_ctx` and `keep_alive`. Ollama's default window is 4,096
  tokens on GPUs under 24 GiB, while Phaze assumes 8,192.
- **llama-server:** one pinned slot per session and `--cache-reuse`.

### Not adopted
- **LLMLingua-style prompt compression.** Its own follow-up work reports poor
  results on code, it needs a second model, and its rewrites break cache
  prefixes. Deterministic trimming (T5) does the job without those costs
  ([models-and-local-runtime.md](research/models-and-local-runtime.md) section 3.4).
- **Fan-out multi-agent by default.** Anthropic reports multi-agent runs cost
  about 15x the tokens, and coding parallelises poorly. Subagents are kept for
  verbose, read-only work that returns a short summary.

## 5. Results

Rerun the bench after each PR and add a row. Real numbers only.

| After | Input tokens sent | vs baseline | Reusable in session | New-session prefix shared | Notes |
|---|--:|--:|--:|--:|---|
| Baseline (`fe95cec`) | 122,828 | - | 90.7% | 5 of 3,732 | Claude: nothing cached |
| T2: stable prefix | 122,884 | +0.05% | 90.7% | 3,695 of 3,736 | Only the uncommitted-files line and the new message differ now. The new git heading adds 4 tokens per request. |
| T3: Claude caching | 123,156 (Claude run) | +0.27% | 90.7% | 3,709 of 3,764 read from cache | Claude bill, in input-token units: **25,529 (20.7% of sent)**, from 111,666 read, 11,490 written, 0 uncached. The same bench on the T2 commit: 123,206 sent, 0 read, billed 100%. That is a **79.3% cut**, just short of the 80% target. The OpenAI run is unchanged from T2 (122,884). |
| T4: lean prompt and tools | 91,678 | -25.4% | 89.9% | 1,466 of 1,507 | Per request, the system prompt fell from 1,134 to 257 tokens and the tools from 23 schemas (2,557 tokens) to 8 schemas plus `more_tools` (1,205). Claude bill: **19,809 (21.6% of sent)** from 82,507 read and 9,247 written, an **83.9% cut** from the T2 commit's 123,206, past the 80% target. The raw target (61,000 or less) is not met yet: T5 is next. |
| T5: compact tool results | 52,695 | -57.1% | 89.4% | 1,477 of 1,518 | Tool results as plain text instead of pretty JSON, plus default caps. Raw input fell 42.5% from T4's 91,678, past the 61,000 target. Claude bill: **11,693 (22.2% of sent)** from 47,192 read and 5,579 written, a **90.5% cut** from the T2 commit's 123,206. |

## 6. Proposed separately: a built-in local runtime and Hugging Face models

This series is not part of the token work. It needs a go-ahead before any of it
is built. Full findings:
[models-and-local-runtime.md](research/models-and-local-runtime.md) section 4.

- **Keep the managed `llama-server` child process; don't embed an engine.**
  - Embedding via `llama-cpp-2` adds a C++/CMake build and per-GPU builds, and
    a GPU driver crash would take the whole IDE down.
  - mistral.rs has no Vulkan, so AMD and Intel GPUs would fall back to CPU.
  - Candle would mean writing an inference engine.
  - Phaze already downloads and runs `llama-server` (`local_ai/`), so nothing
    external like Ollama is needed.
- **L1, GPU detection.** `local_ai/hardware.rs` detects RAM and disk but no GPU.
  Probe VRAM with `llama-server --list-devices`, using the same binary Phaze
  already installs.
- **L2, the right build.** `install.rs` downloads the CPU-only build. Pick the
  Vulkan, CUDA or Metal build to match the GPU. This is the largest local speed
  win.
  - To check first: the research found that llama.cpp's numbered builds now show
    as pre-releases. If GitHub's `releases/latest` returns a tag with no
    binaries, fresh installs fail. Not verified from here.
- **L3, Hugging Face browser.**
  - Search GGUF models through the Hub API (`/api/models?library=gguf`).
  - List quantisations with their sizes and SHA-256 from the repo tree.
  - Read the KV-cache parameters from the GGUF header with an HTTP range
    request, and show only the files that fit this machine's VRAM or RAM at the
    chosen context length.
  - Download with checksum verification, within the Hub's rate limits (500
    anonymous API calls per 5 minutes, more with a token).
- **L4, IDE panel.** Browse, filter by "fits this machine", download, run.
- **Effort:** about 2 to 3 weeks across the four PRs.
