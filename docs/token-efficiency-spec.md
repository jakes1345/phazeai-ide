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

### T3: provider caching and cache measurement
- **Claude cache breakpoints.** `system` is sent as blocks (the base prompt, then
  the compaction summary in a block of its own), with `cache_control`
  breakpoints on the last tool, the end of the base system prompt and the
  conversation tail (4 at most). Source: Anthropic prompt-caching docs; opencode
  `cache-policy.ts:5-34`.
- **Cache usage fields.** `Usage` gains cache-read and cache-write counts,
  parsed from Anthropic (`cache_read_input_tokens`), OpenAI
  (`prompt_tokens_details.cached_tokens`), DeepSeek
  (`prompt_cache_hit_tokens`) and llama-server (`timings.cache_n`). Phaze
  cannot see a cache hit today
  ([models-and-local-runtime.md](research/models-and-local-runtime.md) section 0).
- **OpenAI-compatible providers.** OpenAI gets `prompt_cache_key` set to the session
  id, which raised hit rates from 60% to 87% in OpenAI's own report. xAI gets
  `x-grok-conv-id`. Streams send `stream_options.include_usage` so usage is
  reported at all.
- **The bench gains a Claude-format run** that reports billed-equivalent tokens
  from the breakpoints actually sent.
- **Moves:** effective cost on Claude (target: down 80% or more).

### T4: a lean system prompt and a lean tool set
- **System prompt.** `TOOL_GUIDELINES` repeats the tool schemas and lists tools
  that are not registered; it is cut to about 1 KB of rules. opencode's base
  prompt is 756 bytes (`system.txt`). Source:
  [token-saving.md](research/token-saving.md) C6, D8.
- **Tool set.** A coding session offers its core tools: read, write, edit, bash,
  grep, glob, list and code search. Rare tools sit behind one `tool_search` call
  that appends a tool's schema when it is first needed, so the prefix only ever
  grows at the end. Sources: Anthropic tool search (77K to 8.7K upfront tokens,
  accuracy 49% to 74%); Cursor (46.9% fewer tokens on MCP runs); opencode Code
  Mode catalog budget (`codemode/catalog.ts:50-57`).
- **Moves:** raw input. This is the largest single cut on this task, because
  tools plus prompt are 51% of all input.

### T5: compact tool results and bounded output
- **Plain-text results.** Tool results go back as text, not pretty-printed JSON
  (`agent/core.rs`): read as numbered lines, grep grouped by file, bash as
  stdout, stderr and exit code. opencode's formats measured 2.4x fewer bytes on
  grep and 1.11x on read. Source: opencode `to-llm-message.ts:114-128`,
  `read.ts`, `grep.ts`.
- **Default caps, with the full output spilled to a file whose path is in the
  marker:**
  - read_file: 2,000 lines / 50 KiB (it has no default cap today)
  - bash: tail-biased 50 KiB
  - grep: 100 matches
  - list_files: skip `.git` and other VCS directories

  Source: opencode `tool-output.ts:13-120`; Claude Code and Codex do the same
  ([agents-survey.md](research/agents-survey.md) #12).
- **Moves:** raw input (the message half of every request).

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
