# Models, inference-side caching, and the built-in local runtime: research for Phaze IDE

Researched 2026-11-10. Every claim has a URL. **[UNVERIFIED]** marks a claim I could not confirm from a primary source. "Phaze:" lines describe what `main` at commit `fe95cec` does.

---

## 0. What Phaze does today (read from the code)

- **`ModelInfo` has no cached-input price.** `crates/phazeai-core/src/llm/provider.rs` stores only `input_cost_per_m` and `output_cost_per_m`. The `Usage` struct in `llm/traits.rs` has only `input_tokens` and `output_tokens`. `OpenAIUsage` in `llm/openai.rs` reads `prompt_tokens` and `completion_tokens`, so it drops `prompt_tokens_details.cached_tokens`. **Phaze cannot measure a cache hit on any provider right now.**
- **The model router is per-turn and heuristic.** `ModelRouter` (`llm/model_router.rs`) is built in `config/mod.rs::build_llm_client` whenever `[model_routes]` is set. On each `chat`/`chat_stream` call it classifies the last user message by keyword. If routes exist, consecutive turns of one conversation can go to different models. Provider caches are per model (for example, Alibaba: "Caches are isolated per account and per model", https://www.alibabacloud.com/help/en/model-studio/context-cache), so every switch throws the cached prefix away. The router has no job types for compaction, titles, commit messages or file picking.
- **Compaction uses the same model, with a fresh prompt.** `context/compaction.rs` sends a new `SYSTEM_PROMPT` plus the transcript re-serialised as text. That prefix matches nothing already cached, so the main model is billed full price on the whole transcript.
- **Ollama.** `llm/ollama.rs` sends no `num_ctx` and no `keep_alive`. `context_budget.rs` assumes an 8,192-token window for Ollama, but Ollama's documented default is **4,096** (https://raw.githubusercontent.com/ollama/ollama/main/docs/faq.mdx: "By default, Ollama uses a context window size of 4096 tokens"). Phaze therefore budgets about twice what the server keeps.
- **Built-in llama-server.** `local_ai/server.rs::build_args` passes only `-m --host --port -c --jinja --alias`. It sets no slot pinning and no `--cache-reuse`, and never reads `timings.cache_n`.
- **No thinking control.** Nothing sends `chat_template_kwargs` / `enable_thinking`, `reasoning_effort`, `x-grok-conv-id` or `prompt_cache_key` (grep found none). Two catalog models think by default (Qwen3.5, per its `good_at` text), so short helper calls spend output tokens on reasoning.

---

## 1. Models for the picker and for cheap-model routing

Prices are USD per 1M tokens: input / cached input / output.

### 1.1 Provider facts

| Family | Current IDs | Base URL (OpenAI-compatible?) | Price in / cached / out | Context | Caching | Tool calls | Open weights |
|---|---|---|---|---|---|---|---|
| **Qwen (Alibaba Model Studio)** | `qwen3.8-max` $2/–/$6; `qwen3.7-plus` $0.4/–/$1.6 (≤256K); `qwen3.5-plus` $0.4/–/$2.4; `qwen3.8-flash` $0.15/–/$0.47; **`qwen3.7-flash` $0.03/–/$0.13 (≤32K input)**, $0.10/$0.40 (32K–256K); `qwen3.5-flash` $0.10/–/$0.40 (https://www.alibabacloud.com/help/en/model-studio/model-pricing) | Yes: `https://{WorkspaceId}.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` (https://www.alibabacloud.com/help/en/model-studio/context-cache) | Cached price is set by percentage, see Caching | up to 1M (tiers) | **Implicit**: automatic, cannot be disabled, ≥1,024 tokens, prefix match on `messages`, hit ≈20% of input price. **Explicit**: `cache_control: {"type":"ephemeral"}` (max 4 markers, ignored on `tools`), creation 125%, hit 10%, 5-min TTL refreshed on hit. Reported in `usage.prompt_tokens_details.cached_tokens` (same source) | Yes | Yes (Apache-2.0 on HF) |
| **Qwen3-Coder (API)** | `qwen/qwen3-coder-flash` $0.195 / $0.039 / $0.975, 1M ctx (OpenRouter listing, https://openrouter.ai/qwen/qwen3-coder-flash). qwen3-coder-plus price was not on the official page I could read **[UNVERIFIED]** | via Alibaba | as left | 1M | qwen3-coder-flash supports implicit and explicit cache; qwen3-coder-plus is **not listed** for either (context-cache page above) | Yes | see 1.3 |
| **DeepSeek** | `deepseek-flash` (now served by DeepSeek-V4.1-Flash-1017), `deepseek-v4-pro`. Legacy `deepseek-v4-flash` is still accepted and billed as Flash (https://api-docs.deepseek.com/quick_start/pricing) | Yes: `https://api.deepseek.com`; Anthropic-compatible at `/anthropic` (same page) | flash $0.30 / $0.006 / $1.20; v4-pro $1.32 / $0.044 / $3.96; 50% off-peak (https://developer.puter.com/tutorials/deepseek-api-pricing/, third party that says it copies the official page) **[UNVERIFIED against the official table: it was not readable from here]** | 1M (same third party) | Automatic disk cache (see [token-saving.md](token-saving.md)) | Yes (same) | Yes |
| **Mistral** | Codestral `codestral-25-08` $0.30/$0.03/$0.90; Mistral Small 4 $0.15/$0.015/$0.60; Mistral Large 4 $0.70/$0.07/$2.10; Ministral 3 3B/8B/14B $0.10/$0.15/$0.20 in and out, cached at 10% (https://docs.mistral.ai/inference/pricing; IDs are taken from doc URL slugs) | Yes: `https://api.mistral.ai/v1` (Phaze constant) | as left | Codestral 256K | Cached-input column exists; mechanics on a separate page **[UNVERIFIED mechanics]** | Yes | Devstral Small 2 (`labs-devstral-small-2512`, 256K ctx, Apache-2.0) is **deprecated on the API as of 2026-02-27** (https://docs.mistral.ai/models/devstral-small-2-25-12); weights remain open |
| **Moonshot Kimi** | `kimi-k3.1-pro` $2.00/$0.20/$10, 1M; `kimi-k3` $3/$0.30/$15; `kimi-k2.6` $0.95/$0.16/$4, 256K; `kimi-k2.7-code` $0.95/$0.19/$4, 256K. K2.x closed to new accounts on 2026-10-31 (https://developer.puter.com/tutorials/kimi-api-pricing/, citing platform.kimi.ai) **[third-party prices]** | Yes: `https://api.moonshot.ai/v1` (https://platform.kimi.ai/docs/pricing/chat-k2.md) | as left | as left | Automatic prefix caching; K3 also bills cache writes (5-min TTL = input price, 1-hour TTL = 2×) (Puter page) | Yes | K2 weights were open; K3 status **[UNVERIFIED]** |
| **Zhipu / Z.ai GLM** | GLM-5.3 $1.4/$0.26/$4.4; **GLM-5.3-Flash $0.15/$0.03/$0.50**; GLM-4.7 / 4.6 / 4.5 $0.6/$0.11/$2.2; GLM-4.5-Air $0.2/$0.03/$1.1; **GLM-4.7-Flash and GLM-4.5-Flash free** (https://docs.z.ai/guides/overview/pricing) | Yes: `https://api.z.ai/api/paas/v4/chat/completions` (https://docs.z.ai/guides/capabilities/cache) | as left | – | **Implicit, automatic**; reported in `usage.prompt_tokens_details.cached_tokens`; hits "usually 50%" of the price (cache page) | Yes | Yes (GLM-4.x on HF) |
| **Google Gemini** | `gemini-3.5-flash` $1.50/$0.15/$9; `gemini-3.1-pro-preview` $2/$0.20/$12 (≤200K); `gemini-3-flash-preview` $0.50/$0.05/$3; `gemini-3.1-flash-lite` $0.25/$0.025/$1.50; `gemini-2.5-pro` $1.25/$0.125/$10; **`gemini-2.5-flash` $0.30/$0.03/$2.50**; **`gemini-2.5-flash-lite` $0.10/$0.01/$0.40** (https://ai.google.dev/gemini-api/docs/pricing) | Yes: `https://generativelanguage.googleapis.com/v1beta/openai/` (Phaze constant) | as left | 1M | **Implicit caching on by default for 2.5 and newer.** Minimum 2,048 tokens (2.5) or 4,096 (3.1 Pro and newer Flash). Put shared content first and send similar requests close together. Gemini 4: a turn must add ≥2,050 tokens or the last 2,050 are re-billed (https://ai.google.dev/gemini-api/docs/caching) | Yes | No (Gemma is separate) |
| **Meta Llama** | Llama 4 Scout (109B total / 17B active, 10M ctx) and Maverick (400B / 17B, 1M), April 2025, Llama 4 Community License. In April 2026 Meta put Muse Spark in its chatbots in place of Llama (https://en.wikipedia.org/wiki/Llama_(language_model)) | Via hosts (Groq, Together, Cerebras…) | per host | – | per host | Llama 3.1/3.3 have a native llama.cpp tool handler (https://raw.githubusercontent.com/ggml-org/llama.cpp/master/docs/function-calling.md) | Yes |
| **xAI Grok** | `grok-4.7` $2/$0.50/$6, 500K; `grok-build-0.1` $1/$0.20/$2, 256K; `grok-4.3` $1.25/$0.20/$2.50, 1M; `grok-4.20-0309-(non-)reasoning` $1.25/$0.20/$2.50, 1M. Prompts ≥200K tokens bill at long-context rates (https://docs.x.ai/developers/pricing). **grok-code-fast is not on the current pricing or models pages** (https://docs.x.ai/docs/models) **[status UNVERIFIED]** | Yes: `https://api.x.ai/v1` | as left | as left | Cache entries live per server. Send `x-grok-conv-id` (Chat Completions) or `prompt_cache_key` (Responses) to route a conversation to the same server. Reported in `usage.prompt_tokens_details.cached_tokens` (https://docs.x.ai/developers/advanced-api-usage/prompt-caching/maximizing-cache-hits) | Yes; `reasoning_effort` is configurable on grok-4.7 (models page) | No |

### 1.2 Stale entries in Phaze

All in `crates/phazeai-core/src/llm/provider.rs::known_models` and `constants.rs` unless noted.

- **DeepSeek:** `deepseek-chat` / `deepseek-reasoner` at 64K context and $0.27/$1.10. Current IDs are `deepseek-flash` / `deepseek-v4-pro` with 1M context. Because `context_budget.rs` reads `context_window` from this list, Phaze compacts DeepSeek sessions at about 51K tokens when the model holds 1M. That means summarization calls the model never needed. Whether `deepseek-chat` is still accepted is **[UNVERIFIED]**.
- **Gemini:** `gemini-2.5-flash` is listed at $0.075/$0.30; the official price is $0.30/$2.50. The 1.5 and 2.0 entries are old. No Gemini 3.x entries.
- **Mistral:** `mistral-large-latest` at $2/$6 (Large 4 is $0.70/$2.10). `mistral-small-latest` has a 32K window. No cached prices.
- **xAI:** `DEFAULT_XAI_MODEL = "grok-3"`. The list has `grok-3`, `grok-3-mini` and `grok-2-1212`; none are on the current pricing page.
- **Together / Fireworks:** offer `Qwen2.5-Coder-32B` (32K) rather than a Qwen3-Coder model.
- **Ollama defaults in `constants.rs`:** `BASE_CODER = qwen2.5-coder:14b`, `BASE_PLANNER = llama3.2:3b`, `BASE_REVIEWER = deepseek-coder-v2:16b`. All are 2024-era.
- **Missing providers:** Moonshot, Z.ai and Alibaba Model Studio. All three are OpenAI-compatible and report `prompt_tokens_details.cached_tokens`, so each can be one `ProviderId` variant plus a base URL.
- **Recommendation:** add `cached_input_cost_per_m: Option<f64>` to `ModelInfo`. Better still, move the price table into a data file that can be refreshed without a release, since every row above changed within a year.

### 1.3 Open weights for local use

- **Qwen3-Coder-30B-A3B-Instruct:** 30.5B total, 3.3B active, MoE (128 experts, 8 active), 4 KV heads, 262K native context, Apache-2.0, non-thinking only (https://huggingface.co/Qwen/Qwen3-Coder-30B-A3B-Instruct). GGUF size **[UNVERIFIED]**; check the tree API before adding it to the catalog.
- **Qwen3-Coder-Next:** 80B total, 3B active, `qwen3next` hybrid DeltaNet/attention, 262K context, Apache-2.0, non-thinking. GGUF Q4_K_M is 48.4 GB, Q5_K_M 56.7 GB, Q8_0 84.8 GB. Recommended flags: `--jinja -ngl 99 -fa on … -c 40960 --no-context-shift` (https://huggingface.co/Qwen/Qwen3-Coder-Next-GGUF). Fits 64 GB machines only.
- **Already in Phaze's catalog** (`local_ai/catalog.rs`): Qwen2.5-Coder 1.5B and 7B, Qwen3.5 9B, gpt-oss-20b (MXFP4), Qwen3.5 35B-A3B. All are Apache-2.0 and hash-pinned. The catalog is current for 2026 general models but has **no non-thinking coder MoE**.

### 1.4 Recommendations

**(a) Cheap models for sub-tasks** (compaction, titles, file picking, classification, commit messages). Rank by the price you actually pay, which depends on cache behaviour.

1. **Titles, commit messages, classification** (short, under 32K, no shared prefix, so caching is irrelevant): `qwen3.7-flash` at $0.03 in / $0.13 out for ≤32K, GLM-4.7-Flash (free), and `gemini-2.5-flash-lite` at $0.10/$0.40. Turn thinking **off** for these calls.
2. **Compaction:** usually do **not** send it to a different model. Phaze can append the summary instruction to the main conversation's existing prefix, so the transcript is read at the cache-hit rate. Hit rates: DeepSeek flash $0.006, Gemini 2.5 Flash $0.03, Kimi K2.6 $0.16 (vs $0.95), Qwen ≈20% of input. Those are often cheaper than a "cheap" model reading the transcript cold, which is what Phaze's current compaction request does even on the same model. Rule for the router: `cost(job) = min(main.cached_rate × prefix + main.out_rate × out, cheap.in_rate × prefix + cheap.out_rate × out)`.
3. **File picking / retrieval:** a fast cheap model in a subagent that returns file paths with line ranges, not prose (the Windsurf SWE-grep pattern, section 2). Candidates: `deepseek-flash`, `gemini-2.5-flash`, `qwen3-coder-flash`.
4. **Route per job, never per turn.** Keep the main agent loop on one model. Use the router only for the separate side requests above. The current keyword router breaks the provider cache whenever it switches models mid-conversation.

**(b) Local models that reliably do tool calls** (llama.cpp `--jinja`):

- **Native handlers:** llama.cpp lists native tool-call formats for Llama 3.1/3.3/3.2, Hermes 2/3, Qwen 2.5 and Qwen 2.5 Coder, Mistral Nemo, Command R7B and GPT-OSS (Harmony). Other templates fall back to "Generic", which "may consume more tokens and be less efficient". `parallel_tool_calls` is off by default (https://raw.githubusercontent.com/ggml-org/llama.cpp/master/docs/function-calling.md).
- **Phaze's own test** (catalog comment): Qwen2.5-Coder-1.5B "wrote tool calls as plain text". Keep it out of agent mode and use it only for one-shot text jobs (`llm/text_only.rs`).
- **Ranked picks:** gpt-oss-20b (native Harmony handler), Qwen3.5 9B / 35B-A3B (already in the catalog), Qwen3-Coder-30B-A3B (the model card advertises a dedicated tool-call format; llama.cpp handler coverage **[UNVERIFIED]**), and Qwen3-Coder-Next on 64 GB machines.

---

## 2. Other agents: standout patterns only

- **Qwen-Code CLI:** "deferred" tools. Tools outside `tools.eager` contribute only a short catalog entry and are reached through a `tool_search` → `tool_call` bridge. "The declared tool list stays byte-stable, so the prompt-cache prefix survives discovery." Skills list only name and description until invoked, about 55 tokens each across 84 skills. Its advice: measure *idle cost* (input tokens for one trivial question) with `/context detail` (https://qwenlm.github.io/qwen-code-docs/en/users/features/context-cost/). `/stats` shows "N (x%) of input tokens were served from the cache" (https://qwenlm.github.io/qwen-code-docs/en/users/features/token-caching/). Phaze can copy the byte-stable tool list and the cache-percentage readout.
- **Windsurf SWE-grep (Cognition):** the retrieval subagent runs at most 4 serial turns with up to 8 parallel tool calls per turn, using only grep, glob and read. It returns **files plus line ranges**, scored by weighted F1 with β=0.5, which favours precision "because polluting the main agent's context hurt more than omitting some context" (https://cognition.com/blog/swe-grep).
- **Roo Code:** condensing uses the active model on purpose, because a different model degrades summaries of tool-heavy history. It reserves 30% of the window (20% output, 10% buffer). On a context-overflow error it trims 25% and retries (https://roocodeinc.github.io/Roo-Code/features/intelligent-context-condensing/). This supports the "compact on the same model, on the cached prefix" recommendation above.
- **Continue:** separate model roles (chat, autocomplete, edit, apply, embed, rerank) (https://docs.continue.dev/customize/model-roles/intro). Autocomplete uses `maxPromptTokens: 1024` with a prefix/suffix split, and recommends small models (Qwen2.5-Coder 1.5B/7B, Codestral): "Most of the state-of-the-art autocomplete models are no more than 10b parameters" (https://docs.continue.dev/customize/deep-dives/autocomplete).

---

## 3. Inference-side efficiency a client can exploit

### 3.1 vLLM automatic prefix caching and SGLang RadixAttention

- **vLLM:** each KV block's hash = hash(parent block hash, block token IDs, extra keys such as LoRA ID, multimodal hashes and `cache_salt`). "We only cache full blocks." Eviction is LRU, and a finished request's last block goes first (https://docs.vllm.ai/en/stable/design/prefix_caching/). A hit needs a **token-identical prefix from token 0**, counted in whole blocks, with the same salt. Cached-token reporting needs the server flag `--enable-prompt-tokens-details` (named in vLLM CLI docs, e.g. https://docs.vllm.ai/en/v0.11.0/cli/serve.html) **[behaviour UNVERIFIED]**.
- **SGLang:** a radix tree over token sequences keeps the KV of finished requests, matches the longest cached prefix, evicts LRU leaves, and uses cache-aware scheduling. It reports up to 5× throughput and no noticeable overhead when nothing hits (https://lmsys.org/blog/2024-01-17-sglang).
- **What this means for Phaze's request layout** (applies to every server and provider above):
  1. Order the request as: system prompt → tool schemas (sorted, serialised deterministically) → stable project context → history → new turn.
  2. Keep nothing volatile (date, cwd listing, token counters, git status) in the system prompt; put it in the latest user turn.
  3. Never rewrite earlier history in place. Truncating an old tool result changes every block after it. Compaction is the only allowed rewrite, and it should be rare.
  4. `serde_json` output must be byte-stable, which means a fixed key order for tool schemas and arguments.

### 3.2 llama.cpp server: Phaze's built-in runtime

From https://raw.githubusercontent.com/ggml-org/llama.cpp/master/tools/server/README.md:

- **`cache_prompt`** (default `true`): reuses the KV cache from the previous request so only the differing suffix is processed. Caveat: logits are "not guaranteed to be bit-for-bit identical", so results may be nondeterministic.
- **`id_slot`**: pins the request to a slot. The default `-1` picks any idle slot. `--parallel/-np` defaults to `-1` (auto). **With more than one slot, two requests from the same conversation can land in different slots and miss.** Phaze should pin each conversation (or the agent) to one slot, or pass `-np 1`.
- **`--cache-ram N`** (default 8192 MiB) and **`--cache-idle-slots`** (default on) keep idle slots' prompts in RAM. This helps when side jobs (titles, commit messages) share the server with the main agent.
- **`--cache-reuse N` / `n_cache_reuse`** (default 0, off): reuses chunks of at least N tokens via KV shifting even when the prefix differs in the middle. Useful after compaction or edits.
- **`n_keep` / `--keep`**: tokens kept when the context overflows. **`--context-shift`** defaults to *disabled*.
- **`--slot-save-path` with `POST /slots/{id}?action=save|restore|erase`**: persists a slot's KV to disk. This could restore a long session's prefix after an IDE restart without re-processing.
- **Measuring hits:** `/v1/chat/completions` returns `timings.cache_n` ("number of prompt tokens reused from cache"), `timings.prompt_n` (tokens actually processed), and a standard `usage.prompt_tokens_details.cached_tokens`. `/completion` returns `tokens_cached` and `tokens_evaluated`. **`timings.cache_n` is the authoritative number.** The README example shows `cached_tokens: 0` beside a `cache_n` of 236 (separate examples), so whether the two always agree is **[UNVERIFIED]**.
- **Thinking off per request:** `chat_template_kwargs: {"enable_thinking": false}` (same README).
- **Constrained output:** `response_format` supports `json_schema`, which uses grammar-based sampling.
- **Fit to memory:** `--fit` (default on) adjusts unset arguments to fit device memory, with `--fit-ctx` minimum 4096. `-ngl` defaults to `auto`. `-ctk/-ctv` set the KV cache type (default f16; `q8_0` allowed).

### 3.3 Ollama

- **`keep_alive`** defaults to 5 minutes. After that the model unloads and the next request reloads it and re-processes the whole prompt. Set `keep_alive` per request or via `OLLAMA_KEEP_ALIVE` (https://raw.githubusercontent.com/ollama/ollama/main/docs/faq.mdx).
- **Default context is 4096.** Override with `num_ctx` in `options` or `OLLAMA_CONTEXT_LENGTH`. RAM scales with `OLLAMA_NUM_PARALLEL × context length` (same FAQ).
- **Responses** carry `prompt_eval_count` ("number of tokens in the prompt") and `prompt_eval_duration` ("time spent … evaluating uncached prompt tokens") (https://raw.githubusercontent.com/ollama/ollama/main/docs/api.md). The docs give **no explicit cached-token count**. A hit can only be inferred from a short `prompt_eval_duration`. Whether `prompt_eval_count` excludes cached tokens is **[UNVERIFIED]**. Phaze currently records `prompt_eval_count` as `input_tokens`.

### 3.4 Prompt compression (LLMLingua family): honest verdict

- **LLMLingua:** a small LM (README examples: GPT2-small, LLaMA-7B, phi-2) scores tokens; a budget controller and iterative token-level compression then drop the low-value ones. "Up to 20x" compression "with little performance loss" (https://arxiv.org/abs/2310.05736; https://raw.githubusercontent.com/microsoft/LLMLingua/main/README.md).
- **LongLLMLingua:** up to +21.4% accuracy on NaturalQuestions with about 4× fewer tokens, 94% cost reduction on LooGLE, and 1.4–2.6× latency gains at 2–6× compression (https://arxiv.org/abs/2310.06839).
- **LLMLingua-2:** keep/drop token classification with an XLM-RoBERTa-large or mBERT encoder trained on LLM-distilled data. 2–5× compression; 3–6× faster than earlier compressors; 1.6–2.9× end-to-end latency gain (https://arxiv.org/abs/2403.12968).
- **For code, LLMLingua is a poor fit.** It "overlooks code-specific structures", giving "suboptimal performance in programming tasks". The code-specific LongCodeZip ranks function-level chunks by conditional perplexity, then prunes blocks: up to 5.6× "without degrading task performance" (https://arxiv.org/abs/2510.00446).
- **Verdict for a Rust client:**
  - Do not ship token-dropping compression in the agent loop. It needs a second model resident (an encoder of roughly 0.5B parameters **[size UNVERIFIED]** via candle/ONNX).
  - It rewrites content non-deterministically, which **destroys byte-identical prefixes and therefore provider caching**. Caching already gives 80–99% off on repeated input.
  - Dropped tokens in code and paths break edits.
  - The useful idea is **structural, deterministic** reduction applied once, *before* text enters history: tree-sitter function-level selection (LongCodeZip's coarse stage, without the LM), outline-only file views, and truncation of tool output with head/tail plus a "N lines omitted" marker. That needs no model and keeps the cache intact.

---

## 4. Built-in local runtime feasibility

### 4.1 What exists in `crates/phazeai-core/src/local_ai/`

- **`mod.rs`:** the `[local_ai]` settings are `model`, `server_path`, `ctx_size` and `auto_start`. The module doc says it needs "no Ollama, LM Studio or API key".
- **`hardware.rs`:** probes total and available RAM (via `sysinfo`), physical and logical cores, free disk on the models volume, OS and arch. **It does not detect a GPU or VRAM at all.**
- **`catalog.rs`:** 5 fixed Apache-2.0 GGUF entries, each pinned to an HF commit with exact size and SHA-256 "taken from the Hugging Face API (`/api/models/<repo>/tree/<rev>`)". Each entry carries `min_ram`, `ctx_size` (16,384) and a hand-computed `kv_bytes_per_token`. `runtime_bytes(ctx) = size + kv_bytes_per_token × ctx + COMPUTE_OVERHEAD`. `fit()` returns Fits / Tight / TooBig against **RAM only**. `recommend()` picks the largest entry whose RAM tier fits.
- **`download.rs`:** resumable `.part` download via HTTP `Range`. It re-hashes the resumed prefix, checks size and SHA-256 before an atomic rename, and checks disk first.
- **`install.rs`:** finds `llama-server` in `server_path`, then `PATH`, then a Phaze-managed copy. Otherwise it downloads it from `api.github.com/repos/ggml-org/llama.cpp/releases/latest`, choosing **CPU-only assets** (`-bin-win-cpu-x64.zip`, `-bin-ubuntu-x64.tar.gz`, `-bin-macos-arm64.tar.gz`, …) and verifying the GitHub asset `digest`.
- **`server.rs`:** spawns `llama-server` as a child on 127.0.0.1 at a free port and polls `/health`. A pid file cleans up after a crash. The `BuiltIn` provider talks to it through the existing OpenAI client.

**Possible bug to check first [needs verification].** The releases page shows the `bNNNN` builds (e.g. b12207 on 8 Nov) marked **Pre-release** (https://github.com/ggml-org/llama.cpp/releases). The "latest release" page shows a tag `v0.7.0` with **no binary assets**, only a link to nightly b11669 (https://github.com/ggml-org/llama.cpp/releases/latest). GitHub's `/releases/latest` API skips pre-releases. If that holds, `install_latest` finds no matching asset and every fresh install fails with the manual-install hint. I could not call the GitHub API from here to confirm. Test with `curl https://api.github.com/repos/ggml-org/llama.cpp/releases/latest`; if confirmed, list `/releases?per_page=5` and take the newest `b*` tag that has assets.

**Current release assets** (b12207, same releases page): Windows `win-cpu-x64`, `win-cuda-12.4-x64`, `win-cuda-13.5-x64`, `win-vulkan-x64`, `win-rocm-10.0-x64`; Ubuntu `ubuntu-x64` and `ubuntu-vulkan-x64`; macOS `macos-arm64` and `macos-x64`. Phaze installs the CPU build even on machines with a GPU. That is the biggest local speed loss, and it is a one-function change in `asset_suffixes`/`select_asset`. Whether the macOS arm64 build enables Metal is **[UNVERIFIED]**, though upstream builds have historically done so.

### 4.2 Options compared

| | (a) llama.cpp in-process (`llama-cpp-2`) | (b) mistral.rs (`mistralrs`) | (c) Candle | (status quo+) managed `llama-server` child |
|---|---|---|---|---|
| Maintenance | v0.1.161 on 2026-11-03, 1.9M downloads (https://crates.io/api/v1/crates/llama-cpp-2). "Does not follow semver meaningfully", "very close to raw bindings" (https://github.com/utilityai/llama-cpp-rs) | GitHub v0.9.5, 7.8k stars (https://github.com/EricLBuehler/mistral.rs); crates.io `mistralrs` lags at 0.8.1 from 2026-04-02 (https://crates.io/api/v1/crates/mistralrs) | `candle-core` 0.11.1 on 2026-10-31, 11.5M downloads (https://crates.io/api/v1/crates/candle-core) | Upstream ships builds several times a day (releases page) |
| GGUF quants | All of llama.cpp's | GGUF 2–8 bit plus GPTQ, AWQ, HQQ, FP8, ISQ (mistral.rs README) | "llama.cpp quantized types", but only for the architectures Candle implements (Llama, Mistral, Mixtral, Qwen3-MoE…) (https://github.com/huggingface/candle) | All |
| GPU backends | features `cuda`, `metal`, `vulkan`, `rocm`, `opencl`, `webgpu`, plus `dynamic-backends` (crates.io) | CUDA and Metal only; **no Vulkan or ROCm** (README; crates.io features) | CUDA and Metal; no Vulkan flag (crates.io) | CPU, CUDA, Vulkan and ROCm on Windows/Linux; Metal on macOS (release assets) |
| Chat template + tool-call parsing | docs.rs shows only `chat_template` / `apply_chat_template` on `LlamaModel` and **no tool-call parser** (https://docs.rs/llama-cpp-2/latest/llama_cpp_2/model/struct.LlamaModel.html). The Jinja and tool parsing in `llama-server --jinja` lives in llama.cpp's C++ `common/chat` layer, which Phaze would have to reach or reimplement. There is a `common` feature flag; whether it exposes chat parsing is **[UNVERIFIED]** | "Integrated tool calling with grammar enforcement and a strict schema mode", plus an OpenAI-compatible server (README) | None. Only one example mentions function calling (README) | Present today (`--jinja`, native handlers, `json_schema` grammar) |
| Constrained output | `llguidance` and `sampler` features exist (crates.io) | Grammar and strict schema (README) | Write it yourself | `json_schema` / `response_format` grammar |
| Perf vs llama.cpp | Same engine | Developer-posted "up to 2.8× faster" on GB10/H100/B200 CUDA, which is **unverified, not GGUF-specific** (https://aiweekly.co/node/2280). No Vulkan, so AMD/Intel GPUs fall back to CPU | No independent GGUF benchmark found **[UNVERIFIED]** | Reference |
| Build | CMake plus a C++ toolchain at Phaze build time; one Phaze binary per GPU backend unless `dynamic-backends` is used | Rust plus CUDA toolkit or Metal at build time | Pure Rust (kernels for CUDA/Metal) | Nothing; binary downloaded and verified at runtime |
| Self-contained? | Yes, in-process. But a GPU driver crash or OOM kills the IDE | Yes, in-process, same caveat | Yes, but Phaze would be writing an engine: model code, templates, tool parsing | No user install; one child process that Phaze downloads, verifies, starts and kills. Crash-isolated |

### 4.3 Hugging Face hookup

- **Search:** GGUF repos are tagged `library=gguf` (https://huggingface.co/docs/hub/en/gguf), so `GET https://huggingface.co/api/models?library=gguf&search=<q>&sort=downloads&limit=N`. The parameter names follow the documented web filter. Whether `expand[]=gguf` returns architecture and context metadata is **[UNVERIFIED]**: the OpenAPI spec section I could read did not list it (https://huggingface.co/.well-known/openapi.md).
- **Files and sizes:** `GET /api/models/{ns}/{repo}/tree/{rev}?recursive=…`, paginated (`limit` 1,000, `cursor`), plus `treesize` (OpenAPI spec above). Phaze's catalog comments already rely on this endpoint returning size and SHA-256 per file.
- **Auth and limits:** public repos need no token. Limits are per 5-minute window. Hub API: 500 per IP (anonymous) / 1,000 (free user) / 2,500 (PRO). **Resolvers (`/resolve/` downloads): 3,000 / 5,000 / 12,000.** A 429 carries `RateLimit: …;r=…;t=…`; sleep `t` seconds and retry. HF's advice is to send `HF_TOKEN` and prefer resolver calls (https://huggingface.co/docs/hub/en/rate-limits). Cache search results and debounce typing, because 500 calls per 5 minutes is easy to hit from a search box.
- **`hf-hub` crate:** v1.0.0 (2026-07-10), 24M downloads, features `blocking`, `rustls-tls`, `socks` (https://crates.io/api/v1/crates/hf-hub). Phaze already has its own reqwest download plus verify path, so `hf-hub` adds little for downloads. A thin hand-written client for `/api/models` and `/tree` (about 150 lines) is simpler and keeps the size and SHA-256 checks.
- **llama-server can also download:** `-hf <user>/<model>[:quant]` (default Q4_K_M) and `-hff` (README). But that bypasses Phaze's SHA-256 pinning, so keep Phaze's downloader.
- **Estimating fit for an arbitrary repo:**
  - Weights = file size. Shards: sum the `-0000N-of-0000M` files.
  - KV bytes per token = 2 × n_layers(full-attention) × n_kv_heads × head_dim × bytes(cache type). This is the formula `catalog.rs` hand-codes.
  - Read those values from the GGUF header with one HTTP `Range` request for the first few MB of the file. Use a resolver URL, which falls in the high-limit bucket. Keys: `<arch>.block_count`, `<arch>.attention.head_count_kv`, `<arch>.attention.key_length`, `<arch>.context_length` (GGUF metadata keys **[from the GGUF spec, not fetched here]**). HF's own JS parser does exactly this on remote files (gguf docs page).
  - Hybrid and SWA models (Qwen3.5, Qwen3-Next) need a per-architecture full-attention-layer count, as `catalog.rs` already notes. Mark those "estimate" and let `--fit` (default on) be the runtime safety net.
  - `-ctk q8_0` roughly halves the KV term.
- **Detecting VRAM:**
  - **Cheapest, no new dependencies:** `llama-server --list-devices` "print list of available devices and exit" (README). It uses the same backend that will run the model. The exact output format to parse is **[UNVERIFIED]**.
  - **NVIDIA:** `nvml-wrapper` 0.13.0 (2026-08-31) (https://crates.io/api/v1/crates/nvml-wrapper); whether it loads NVML dynamically is **[UNVERIFIED]**. Alternatively, `nvidia-smi --query-gpu=memory.total,memory.free --format=csv`.
  - **Apple:** `MTLDevice.recommendedMaxWorkingSetSize`, "an approximation of how much memory … this GPU device can allocate without affecting its runtime performance" (https://developer.apple.com/documentation/metal/mtldevice/recommendedmaxworkingsetsize). Use it instead of total RAM on unified memory.
  - **Vulkan (AMD, Intel, any):** `VK_EXT_memory_budget` gives per-heap `heapBudget` and `heapUsage` via `VkPhysicalDeviceMemoryBudgetPropertiesEXT`; use the device-local heaps (https://docs.vulkan.org/refpages/latest/refpages/source/VK_EXT_memory_budget.html).

### 4.4 Recommendation

**Keep the managed `llama-server` child process; do not embed.**

- It is already self-contained from the user's view: no Ollama and no manual install, with a verified download, crash isolation, and llama.cpp's full Jinja, tool-call and grammar stack (the hardest part to reproduce).
- Embedding through `llama-cpp-2` drags CMake/C++ and per-backend builds into Phaze's build. It loses the server's tool parsing unless Phaze reimplements it, and lets a GPU fault take down the editor.
- mistral.rs is the only credible in-process engine, but it has no Vulkan, so AMD and Intel GPUs on Windows/Linux would run on CPU, and its crates.io releases lag.
- Candle would mean writing an engine.
- For a truly offline first run, bundle the `llama-server` binary in the installer as a sidecar.

**PR series, separate from token-efficiency work.** It touches `local_ai/*` and the model-picker UI, not the agent loop.

1. **(S, 1–2 days) Fix the install source.** Confirm the `/releases/latest` problem and select the newest `b*` release with assets. Choose a GPU asset: Vulkan on Windows/Linux by default, CUDA when NVML or `nvidia-smi` finds an NVIDIA card, Metal on macOS arm64, CPU as fallback. Add `--list-devices` probing to `hardware.rs` as `gpus: Vec<{name, vram_total, vram_free}>`.
2. **(S) Make `catalog::fit` VRAM-aware.** Full offload when weights + KV fit VRAM, partial when they don't, CPU otherwise. Keep `--fit` on as a backstop.
3. **(M, about 1 week) HF browser.** Search through `/api/models?library=gguf`, list quants via `/tree`, read the GGUF header by HTTP Range for KV parameters, show a fit badge per quant, and download through the existing `download_verified` using the tree's SHA-256. Handle 429 with `RateLimit` back-off and accept an optional `HF_TOKEN`. Show the license and gate status, and warn on non-Apache/MIT licenses.
4. **(S) Runtime flags for cache reuse.** `-np 1` (or a fixed `id_slot`), `--cache-reuse 256`, optional `--slot-save-path` for session restore, and read `timings.cache_n` into `Usage`. This item is shared with the token-efficiency series.

**Rough total:** 2–3 weeks for one developer. In-process embedding would be a multi-week project that adds risk without a user-visible gain.

---

## What Phaze does with it

| Technique / model | Concrete change | Phaze file(s) | Source |
|---|---|---|---|
| Cached-token accounting | Add `cached_input_tokens` to `Usage`; parse `usage.prompt_tokens_details.cached_tokens` (xAI, Alibaba, Z.ai, llama.cpp) and `timings.cache_n` (llama-server); show a "served from cache" % like Qwen-Code `/stats` | `llm/traits.rs`, `llm/openai.rs`, UI status bar | docs.x.ai maximizing-cache-hits; alibabacloud context-cache; docs.z.ai cache; llama.cpp server README; qwen-code token-caching |
| Cached price in model table | `ModelInfo.cached_input_cost_per_m`; cost meter uses it | `llm/provider.rs` (`known_models`, `estimated_cost`) | provider pricing pages in 1.1 |
| Refresh stale models | DeepSeek → `deepseek-flash` / `deepseek-v4-pro` (1M); Gemini 2.5 Flash price fix and 3.x added; Mistral Large 4 / Small 4 / Codestral cached; xAI → `grok-4.3` / `grok-4.7` / `grok-build-0.1`; add Moonshot, Z.ai, Alibaba providers; move table to a data file | `llm/provider.rs`, `constants.rs` | 1.1 sources |
| Correct context windows | Stale 64K DeepSeek window causes needless compaction; fixed by the table refresh | `llm/context_budget.rs` (reads provider list) | DeepSeek third-party page; code |
| Router by job, not per turn | Add `TaskType::{Compaction, Title, CommitMessage, FilePick, Classify}`; main agent loop never switches model; side jobs use the cheapest model by `min(main cached, cheap uncached)` | `llm/model_router.rs`, `config/mod.rs` | Alibaba "isolated per … model"; Roo condensing doc |
| Compaction on the cached prefix | Append the summary instruction as a final user turn to the existing conversation instead of a fresh system prompt plus re-serialised transcript | `context/compaction.rs` | Roo Code doc; cache-hit prices in 1.1 |
| Thinking off for helpers | Send `chat_template_kwargs: {"enable_thinking": false}` (llama.cpp, Qwen) or a low `reasoning_effort` (xAI) for titles, commit messages and inline edits | `llm/text_only.rs`, `llm/openai.rs` | llama.cpp README; docs.x.ai models |
| Cheap side-job models | Defaults: `qwen3.7-flash` (≤32K, $0.03/$0.13), GLM-4.7-Flash (free), `gemini-2.5-flash-lite` | `config/mod.rs` defaults, `constants.rs` | Alibaba pricing; docs.z.ai pricing; Gemini pricing |
| Sticky cache routing | Send `x-grok-conv-id` = conversation id for xAI | `llm/openai.rs` | docs.x.ai maximizing-cache-hits |
| Byte-stable prefix | Deterministic tool-schema serialisation and order; nothing volatile in the system prompt; never rewrite old history except at compaction | agent prompt builder, `tools/` | vLLM prefix-caching design; SGLang blog |
| Deferred tools | Short catalog plus `tool_search` for rarely used tools, keeping the declared tool list byte-stable | `tools/` registry, agent | qwen-code context-cost |
| Retrieval subagent returns line ranges | File-pick subagent: ≤4 turns, ≤8 parallel grep/glob/read calls, returns paths plus ranges, not prose | agent / `search/` | cognition.com/blog/swe-grep |
| Ollama context and residency | Send `options.num_ctx` = Phaze's budget window and `keep_alive` (e.g. "30m"); set the Ollama default in `context_budget` to 4096 when unknown | `llm/ollama.rs`, `llm/context_budget.rs` | Ollama FAQ and API docs |
| llama-server cache flags | `-np 1` (or a fixed `id_slot`), `--cache-reuse 256`, optional `--slot-save-path` plus slot restore on reopen | `local_ai/server.rs::build_args` | llama.cpp server README |
| No LLMLingua in the loop | Use deterministic structural trimming instead: tree-sitter outlines and function selection, head/tail tool-output truncation | tool output handling, `search/` | LLMLingua papers; LongCodeZip |
| Local tool-call models | Keep Qwen2.5-Coder-1.5B text-only; consider adding Qwen3-Coder-30B-A3B (non-thinking MoE) after checking GGUF size and tool format | `local_ai/catalog.rs` | Qwen HF cards; llama.cpp function-calling doc |
| **Runtime: install source** | Verify `/releases/latest` (v0.7.0 with no assets?); pick the newest `b*` release with assets | `local_ai/install.rs` | github.com/ggml-org/llama.cpp/releases |
| **Runtime: GPU builds** | Choose Vulkan / CUDA / Metal asset by detected GPU, CPU as fallback | `local_ai/install.rs::asset_suffixes`, `select_asset` | llama.cpp release assets |
| **Runtime: VRAM detection** | `llama-server --list-devices` first; NVML or `nvidia-smi` on NVIDIA; Metal `recommendedMaxWorkingSetSize`; Vulkan `VK_EXT_memory_budget` | `local_ai/hardware.rs` | llama.cpp README; nvml-wrapper; Apple docs; Vulkan refpage |
| **Runtime: VRAM-aware fit** | `fit()` uses VRAM plus RAM; KV from GGUF header; `-ctk q8_0` option; keep `--fit` on | `local_ai/catalog.rs` | llama.cpp README (`--fit`, `-ctk`) |
| **Runtime: HF browser** | `/api/models?library=gguf` search, `/tree` sizes and SHA-256, GGUF header via HTTP Range, 429 back-off, optional `HF_TOKEN`; download through the existing verified path | new `local_ai/hub.rs`, `local_ai/download.rs`, picker UI | HF gguf docs; HF rate-limits; HF OpenAPI |
| **Runtime: embedding** | Do **not** embed llama.cpp, mistral.rs or Candle; optionally ship `llama-server` as an installer sidecar | packaging | crates.io / README pages in 4.2 |
