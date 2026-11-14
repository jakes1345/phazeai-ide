# Plan: local AI on the GPU, a security reviewer, and (maybe) our own model

Written 2026-11-14. Builds on [`docs/research/models-and-local-runtime.md`](../research/models-and-local-runtime.md), which designed most of Phase 1 but none of it is built. Serves two lines of [`MISSION.md`](../MISSION.md): *it works for people without a strong machine or a paid account*, and *honest software*.

## The short version

1. **Fix the built-in model installer.** It broke for everyone on 2026-11-13 (below).
2. **Run local models on the GPU.** Vulkan works on AMD, Intel and NVIDIA with stock drivers. No ROCm, no CUDA toolkit, no `sudo`.
3. **Measure local models with `phazeai eval`** so every later decision rests on numbers.
4. **Build the security reviewer from a scanner plus an existing model.** No training.
5. **Fine-tune only if step 3 or step 4 shows a gap** that training can close, and only on data we are allowed to use.

Each phase ends in something a user can use, and each one gates the next.

## What we measured (2026-11-14, owner's machine)

| | |
|---|---|
| GPU | AMD Radeon RX 9060 XT (Navi 44, RDNA4, `gfx1200`), **16 GiB VRAM** |
| Vulkan | Works: `RADV GFX1200`, Vulkan 1.4.318 (Mesa) |
| ROCm | Not installed |
| RAM / free disk | 31 GiB / 92 GiB |
| Ollama 0.40.11, `qwen2.5:7b` | **100% CPU**: 8.3 tokens/s generating, 27.8 tokens/s reading the prompt, 80 s to load |
| PhazeAI Local | No `llama-server` or model installed |

**The same model on the GPU** (llama.cpp `b12383` Vulkan build, `llama-bench -ngl 99 -p 512 -n 128`, run by the owner):

| `qwen2.5:7b` Q4_K_M (4.36 GiB) | CPU (Ollama) | GPU (Vulkan) | Gain |
|---|---|---|---|
| Generating | 8.3 tokens/s | **66.7 ± 0.6 tokens/s** | 8× |
| Reading the prompt | 27.8 tokens/s | **2,044 ± 22 tokens/s** | 73× |

The Vulkan backend found the card's matrix cores (`KHR_coopmat`), bf16 and resizable BAR on stock Mesa, with no ROCm installed. This is the gain Phase 1 brings every user with a GPU. A 4,000-token file goes from about 2.5 minutes to read to about 2 seconds.

## Phase 0: the installer is broken (P0, about a day) — lookup fixed

**Done:** the release lookup (first bullet below). **Not done:** pinning a tag, which waits on the owner's answer.

`local_ai/install.rs` asks GitHub for `releases/latest`. On 2026-11-13 llama.cpp started marking its numbered builds (`b12377` … `b12383`) as **pre-releases**. "Latest" is now a `v1.0.0` that holds only `nightly-tag.txt`, so `select_asset` finds nothing and every new install of PhazeAI Local fails. The research doc flagged this risk; it has now happened.

- List `releases?per_page=30` and take the newest release whose tag matches `b<digits>` and that carries the asset we need (pre-releases included). Do not use `latest`.
- Keep the digest rule: no SHA-256 digest, no install.
- Test against the real release shape: a `v1.0.0` with one `.txt` asset next to `b*` pre-releases with binaries.
- Better still: **pin a known-good `b*` tag in the code** and bump it on purpose, with the digest checked at install. A moving target broke us once already, and a pin makes installs reproducible. Recommended.

## Phase 1: the GPU (about 1–2 weeks)

**Done (items 1–4 below), in the PR after #89.** Checked on the owner's RX 9060 XT: PhazeAI installs `b12383` Vulkan, `--list-devices` sees the card (16,304 MiB), a 16 GiB card is now recommended gpt-oss 20B (13.5 GiB with headroom), and a real server reports "all 29 layers on the GPU". **Not done:** a click-through of Settings › Local AI in the running IDE, and a measured tokens/s in the panel (item 4's last part).

From the research doc's PR series, items 1–2, in order:

1. **Pick a GPU build.** `asset_suffixes` only ever returns CPU builds (`install.rs:190`, and a test asserts "never the Vulkan build"). Choose Vulkan on Linux and Windows, Metal on Apple Silicon, CUDA only when an NVIDIA card is found, and CPU as the fallback. If the GPU build fails its first health check, fall back to the CPU build automatically and say so in Settings › Local AI.
2. **Detect the GPU.** `hardware.rs` knows RAM, cores and disk but nothing about GPUs. Run `llama-server --list-devices` after install and store `gpus: [{name, vram_total, vram_free}]`.
3. **Size models by VRAM, not RAM.** `hub::best_pick` and `catalog::fit` size by RAM only. With weights plus KV cache in VRAM, offload fully; when they don't fit, offload partly; with no GPU, use the CPU. Keep llama-server's `--fit` on as a backstop.
4. **Tell the truth in the UI.** Settings › Local AI shows "Running on: AMD Radeon RX 9060 XT (Vulkan), 28/28 layers on GPU" or "CPU", plus measured tokens/s from the server's own `timings`. Never "GPU" when the server fell back to CPU.

**What this unlocks on a 16 GiB card:** `gpt-oss 20B` (MXFP4, about 12 GiB) fits whole, and so does `Qwen3.5 9B`. `Qwen3.5 35B-A3B` runs partly offloaded, which a mixture-of-experts model tolerates well. All are already in `catalog.rs` under Apache-2.0.

**Done when:** a fresh install on this machine picks the Vulkan build, the panel shows the GPU, and `phazeai -p` against the built-in model beats Ollama's CPU figure by a measured margin recorded in `STATUS.md`.

## Phase 2: measure before deciding anything (about 1 week)

`phazeai eval` already runs coding tasks and reports solved / cost / time. Point it at local models:

- Run `phazeai eval --provider builtin` on the 6 tasks, 3 runs each, for every catalog model that fits. Record solve rate and tokens/s next to Claude Sonnet 4.5, which scored 4/6 on 2026-11-13 with one task since fixed.
- Grow the task set past 6. Add tasks that use tools (multi-file edits, running tests) and tasks drawn from the SWE-Gym distribution. Six tasks can't separate models.
- Add a **security track**: about 50 functions from PrimeVul's *test* split (half vulnerable, half fixed). The check is "flags the vulnerable one with the right CWE and leaves the fixed one alone". Score precision and recall, not just accuracy.

**Gate:** if a catalog model solves the coding tasks within a usable margin of the cloud model, the "free local AI" promise is met without training. Spend the time on Phase 3, not Phase 4.

## Phase 3: security reviewer without training (about 2 weeks)

The fastest route to a trustworthy security feature is the one that can't hallucinate a vulnerability on its own:

1. **Scanners find candidates.** Run **Semgrep** (open-source engine, community rules) on the changed files. Check the rules' licence before bundling them; the registry rules carry their own licence. Add the language-specific tools already common in each ecosystem (`cargo audit` for dependency advisories, Bandit for Python).
2. **The model triages and explains.** For each finding the model sees the code around it and the rule, then answers: real or false positive, why, and the suggested fix. It works with any provider, local included, and runs through the same privacy policy and audit log as everything else.
3. **The user decides.** Findings show in a Problems-style list with the scanner's rule ID and the model's verdict side by side. A fix goes through the existing diff review. Never auto-applied.
4. **Where it runs:** a "Review for security" command on the staged diff (next to the AI commit message), and an optional pre-commit check.

**Done when:** on the Phase 2 security track it beats "scanner alone" on precision without losing recall, measured on the same functions.

## Phase 4: fine-tuning, only if the numbers ask for it

**Start only if** Phase 2 or Phase 3 shows a specific, repeatable gap: a model that knows the answer but fumbles PhazeAI's tool-call format, or a triage step that is too noisy. "Our own model" is not a goal by itself; a measured gap is.

**Method:** LoRA fine-tuning of an existing open model (Qwen2.5-Coder 1.5B–7B or a Qwen3.5 size), not training from scratch. Training from scratch on one consumer GPU would take months to produce something worse than the free models we already ship.

**Hardware:** on AMD this needs ROCm plus PyTorch. RDNA4 (`gfx1200`) support in ROCm is recent and patchy, and installing it needs `sudo`, so the owner does that part. QLoRA's 4-bit path (bitsandbytes) is the least reliable piece on AMD; plan on plain LoRA in bf16 for ≤3B models on 16 GiB, or rent a cloud GPU by the hour for 7B. The result is exported to GGUF and served by the same llama-server as everything else.

**Data:** only sources we are allowed to train on. Licence labels below are what each Hugging Face card says (checked 2026-11-14). The code inside a dataset keeps its own upstream licences, so read each card's terms before use.

| Source | For | Licence on the card |
|---|---|---|
| PrimeVul (`colin/PrimeVul` mirror) | Security: vulnerable/fixed C/C++ functions, cleanest labels | MIT |
| CVEfixes (GitHub `secureIT-project/CVEfixes`) | Security: real fix commits, many languages | check repo |
| NIST Juliet / SARD | Security: synthetic examples per CWE, in small doses | public domain |
| GitHub Advisory Database, OSV.dev, NVD | Linking CVEs to fix commits | per source |
| CommitPackFT (`bigcode/commitpackft`) | Commit messages, code edits | MIT |
| SWE-Gym (`SWE-Gym/SWE-Gym`) | Agent work: issue → patch | MIT |
| SWE-bench Verified | **Evaluation only, never training** | none on card |
| The Stack v2 (`bigcode/the-stack-v2`) | General code; gated, opt-out respected | other (accept terms) |
| PhazeAI's own history and `evals/` | Tool-call format, house style | ours |

**Hard rules:**
- **No outputs from Claude, GPT or Gemini as training data.** Their terms forbid using outputs to build a competing model. That also rules out most "code instruct" sets on Hugging Face that were generated with GPT-4: check how each dataset was made, not just its licence.
- **Keep test data out of training.** Anything in the Phase 2 eval, including PrimeVul's test split, never enters training; contaminated scores are lies, which breaks the "honest software" principle.
- **Ship a model only if it beats its own base model on the eval.** Otherwise ship the base model.

## Order of work and rough size

| # | Work | Size | Who |
|---|---|---|---|
| 0 | Fix installer release lookup, pin a `b*` tag | S | agent |
| – | Run the GPU benchmark command above | 1 min | owner |
| 1 | GPU build + detection + VRAM sizing + honest panel | M | agent |
| 2 | Local-model eval runs, bigger task set, security track | M | agent |
| 3 | Scanner + model security review | M | agent |
| 4 | (Conditional) ROCm install, LoRA experiment | L | owner installs ROCm; agent writes and runs the pipeline |

## Open questions for the owner

- Is pinning a llama.cpp version (and bumping it on purpose) fine, versus always taking the newest?
- Which languages matter most for the security reviewer? That decides which scanners to bundle first.
- Is a paid cloud GPU for a fine-tuning run acceptable later, or must everything run on this machine?
