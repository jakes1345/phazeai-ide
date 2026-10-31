# PhazeAI: what we are and what we are building

> **For people and for automated runs.** Read this first, then [`STATUS.md`](STATUS.md) (what is real today, known problems, how to build and test), `CLAUDE.md` (architecture and commands) and `ROADMAP.md`.
> This file changes rarely. Dated facts live in `STATUS.md`. If either file contradicts the code, the code is right: fix the file.

## What we are

PhazeAI is an AI-native code editor written in Rust. The desktop IDE (`phazeai-ui`, binary `phazeai-ui`) renders on the GPU through Floem. A terminal UI and one-shot CLI (`phazeai-cli`, binary `phazeai`) share the same engine (`phazeai-core`): agent loop, LLM clients, tools, LSP, DAP, MCP, privacy policy, code search and a built-in local model. No Electron. You bring your own AI provider (Claude, OpenAI, Gemini, Groq, Together, OpenRouter, Ollama or LM Studio) or use the built-in local model. MIT licensed. The README calls it **pre-beta**, and that is accurate.

## What we are trying to do

The owner's goal, from their own messages (not written in the repo): *the perfect IDE for a human and its best friend, an AI*; a tool that competes with the best AI editors on things people can name, without the cloud lock-in.

1. **A person and an AI in the same editor.** Chat, inline edit, a multi-file agent (Composer), later Tab completion and hunk-by-hunk review of agent edits.
2. **The user decides what an AI can see.** Per-project deny rules, a local-only switch, an audit log of what was shared.
3. **It works for people without a strong machine or a paid account.** A built-in local model with a verified download, free-tier providers, nothing extra to install for PhazeAI itself.
4. **Honest software.** What the UI says happened, happened.

## The bet: why someone would switch from Cursor

The owner's ambition is to build the editor people switch to from Cursor. Cursor is a VS Code fork with a strong Tab-completion model, a codebase index, polished agent review and the whole VS Code extension ecosystem. We will not win by copying that. These are **hypotheses to test, not facts**:

1. **Trust.** Per-project deny rules, a local-only mode and an audit log of what was shared. Real today, and rare.
2. **No lock-in.** Any provider on your own keys, a local model, no subscription required, MIT licensed.
3. **Native.** GPU-rendered Rust, small and fast. *Unmeasured: measure before claiming it.*
4. **An agent you can supervise.** Approval modes today; hunk-level review of agent edits to build.

What Cursor has that we do not: next-edit Tab completion, an embeddings index, polished diff review, VS Code extensions and themes (we install `.vsix` files but never apply them), a one-click installer, and years of polish. So the order of work is: earn trust first (the P0 list in `STATUS.md`), then the core loop (Tab completion, agent edit review, better search), then installability, then ecosystem. A feature that is half-built or that lies in the UI hurts the "trust" bet more than a missing feature does.

Who it is for is **not written down in the repo**. The roadmap's Phase 1 goal is that someone other than the author can use it for a day without hitting a sharp edge. Treat "developers who want AI help on their own terms" as the working assumption, and ask the owner before narrowing it.

## Principles

1. **Real implementations only.** No placeholders, mock or sample data, or success messages for things that did not happen. If something is not built, the UI and docs say so. (Owner rule. `STATUS.md` lists where we break it today.)
2. **The user controls what reaches an AI.** Code that sends project content to a model checks the privacy policy (`privacy::check_and_record`, `PrivacyPolicy::check`, `privacy::tool_check`) and notes attached files in the audit scope. All production LLM clients are built through `build_client_for`, which wraps them in `GuardedClient`. Know its limit: `GuardedClient` enforces `local_only` and the audit log; **deny globs are enforced at each call site**, and nothing at compile time stops a new unwrapped client or an unchecked file read.
3. **Tool calls go through an approval function that actually gates.** `tools/approval.rs` is the shared policy: it classes tools ReadOnly / Write / Execute / Destructive. Its read-only bash allowlist is token-based (extend it with a test, never with substring matching); its escalation list is substring-based and over-classifies (`git log --format=...` counts as Destructive); every non-bash tool outside the Read and Write lists, including `delete_path`, `fetch` and `download`, defaults to Execute, and Destructive is only ever returned for bash. `Agent::new` has **no approval function by default and the full tool set**. A new AI helper must either make a plain `client.chat(&msgs, &[])` call with no tools (safest), or pass `.with_approval(..)` **whose closure itself gates every non-read-only tool** (the IDE chat's closure waves `write_file` and `edit_file` through and relies on a separate diff hook, so copying it without the hook leaves writes ungated). `ToolRegistry::read_only()` is unused today and is not strictly read-only: it includes the `memory` tool, which writes `~/.phazeai/memory.json`.
4. **Nothing extra to install for PhazeAI itself.** No Python or Node runtime for core features. Be exact about the edges: `git` (the CLI), `rg`, language servers, MCP server runtimes and a model runtime are separate installs or downloads; the `diagnostics` tool runs `cargo check` / `npx tsc` / `python3 -m py_compile`; the Run File menu types `python3`, `node`, `npx ts-node`, `ruby`, `go` or `bash` into the terminal; the screenshot tool needs `scrot`, `grim` or `import`. (`python/training/` and `training/` are model-training scripts, not the product; `next-app/` is a Next.js scaffold.)
5. **Provider neutral.** Keys come from the OS keyring or an environment variable, never from `config.toml`, logs, chat or commits. Gemini stays selectable but is not suggested by default (owner preference).
6. **Honest names.** Code search is keyword search ("matches words, not meaning"), never "semantic". Status labels match the code.
7. **Native and fast.** No Electron. The roadmap's targets (start under 2 s, frames under 16 ms) are **not measured**.
8. **Permissive licences.** MIT core. The built-in model catalog holds Apache-2.0 models, with exact sizes and SHA-256 hashes pinned in code and checked on download.
9. **Know what is outside the guard.** Native plugins (`cdylib`) run in-process with full privileges, outside the privacy policy and approval rules.

## Where we are heading

**This section supersedes `ROADMAP.md`'s "Execution order". Where they disagree, this file wins; fix `ROADMAP.md` in a docs PR.** Do not start items the code already has (see `STATUS.md`).

1. The P0 and P1 problems in `STATUS.md`. Fixing the CI half of P1 means editing workflows, which needs the owner's approval, so take the test-quality half first.
2. A packaging path that is verified to work on Linux.
3. Open feature work, each as its own branch and PR:
   - embeddings next to BM25 (ROADMAP 2.3). It would add a native runtime and a model download, against Principle 4, and the roadmap lists it as an open decision, so **it needs owner approval**;
   - hunk-level accept/reject for agent edits;
   - per-project memory;
   - a fuller first-run setup;
   - inline ghost-text completion (part of ROADMAP 4.3, the pair-programmer mode).
4. Owner's direction, from their messages and not yet in `ROADMAP.md`: a curated list of MCP servers for game and creative tools (Godot, Unity, Blender, Steam); the design panel grown into our own version of that workflow, not a copy.
5. Later: cloud tier (undecided), plugin marketplace, collaboration.

## Definition of done

- Tests call the production code. For a bug fix, **prove the test**: remove the fix and confirm the test fails. Do it with a temporary commit or `git checkout <base> -- <file>`, never with bare `git stash` (the stash stack is shared across worktrees).
- Measured, not assumed: say how many runs, on which commit, and what failed.
- CI means `ci.yml` and `windows-build.yml`. (`feature-tests.yml` is disabled by GitHub and does not count.) It must be green on the PR head, including the GUI suite, and **it only runs for PRs that target `main`**; do not claim green CI for a PR it did not run on. A failure is root-caused. Never skip, disable or quarantine a test to get green.
- Escape hatch: if the only red is the known GUI "never finished rendering" timeout, show the harness log and the run count in the PR, leave it draft, and do not retry in a loop. Unrelated `cargo audit` or Windows failures are reported, not worked around.
- Docs and UI text match the code. If behaviour changes, update the claim.
- The PR says what was **not** done or **not** verified.

## Rules for automated runs

**Branches and PRs**
- Never push to `main` directly, never force-push, never touch someone else's branch, never merge someone else's PR. Merge **your own** PR only under "Merging your own PR" below. Name each branch for what it does (for example `fix/vim-ex-save`), with no `claude/` prefix, one theme per branch. (`main` is not branch-protected; the owner should turn that on.)
- Base: branch from `main`, and open the draft PR against `main`. CI only runs for PRs that target `main`.
- Open each PR as a draft, and take only items that do not depend on another run's unmerged PR. One theme per PR.
- `gh pr list`, `gh pr create` and `gh pr checks` use GraphQL, which is blocked in Claude Code sessions. Use REST: `gh api 'repos/jakes1345/phazeai-ide/pulls?state=open'`; create with `gh api repos/jakes1345/phazeai-ide/pulls -f title=… -f head=<branch> -f base=main -F draft=true -f body=…`; read CI with `gh api repos/jakes1345/phazeai-ide/commits/<sha>/check-runs`.
- Mark a draft ready with `POST /repos/jakes1345/phazeai-ide/pulls/<n>/ccr/ready_for_review` (the GraphQL route is blocked), and merge with `PUT /repos/jakes1345/phazeai-ide/pulls/<n>/merge`.
- Stage files by name, never `git add -A`, and read `git status` before every commit (the GUI suite leaves stray files in a tracked directory).

**Safety on the developer's machine**
- **No `sudo`.** If a package is missing, report which one. Never run `install.sh`, `script/*.sh`, `scripts/build-*.sh`, `scripts/update.sh` (it merges `main` into your branch and pulls multi-GB models through Ollama), `modelfiles/install.sh`, `make install` or `make install-ide`.
- Use `--ignored` only for the GUI-suite command in `STATUS.md`. A broader `--ignored` runs `ollama-rs` tests that pull and delete models on a local Ollama.
- Run the built GUI or CLI binary only with a throwaway `HOME`, `XDG_CONFIG_HOME` and `XDG_DATA_HOME` and provider keys unset, and the GUI suite only under `xvfb-run` (details in `STATUS.md`). Do not set `CARGO_TARGET_DIR`.
- Never put keys, tokens or personal paths in code, logs, commits or PR text. If a task needs a key, stop.
- Disk and time: a debug build is about 14 GB of `target/` per checkout, the first build needs the network, and the GUI suite takes minutes. Clean up leftover `Xvfb`, `fluxbox` and `phazeai-ui` processes if a run dies.

**Needs the owner's approval first**
- Loosening what the privacy policy, approval rules or sandbox allow. (Tightening does not need it: for example, gating the update check behind a setting and `local_only` is a tightening.)
- Deleting files or features; changing a public claim in the README or `index.html`; editing workflows; adding or updating a dependency (including `cargo update`, `Cargo.lock` and the Floem git revision); embeddings or any new runtime; anything you cannot verify.
- **When approval is needed and nobody is there:** write the question in the draft PR, mark the item blocked, and take the next independent item. Do not guess and do not stall.

**Behaviour**
- Do not retry a flaky test until it passes. Find the cause, or report it.
- Budgets (iterations, time, tokens) are set by the owner when the run starts. With none given, do one item per run. One run at a time per machine: runs share `~/.config/phazeai`, the keyring and the X display.

## Owner decisions

Recorded in the owner's words. The owner can change any of these by saying so.

- **2026-10-31, delegation.** The owner said: *"I am going to leave the rest of this project up to you remember this aims to be the cursor AI killer."* Read as: day-to-day engineering is delegated. Choose work from `STATUS.md`, open PRs, verify, and report. The ambition is in "The bet" above.
- **2026-10-31, branches.** Asked whether runs may push extra branches so several PRs can be open at once, the owner answered: *"don't name them claude branch name em real ones i'm tired of claude name in everything"*. Read as: yes to extra branches, named for what they do, never with a `claude/` prefix.
- **2026-10-31, merging.** Asked whether runs may merge their own PRs under the policy below, the owner chose: **"Yes, under the strict policy"**.
- **Still open, owner-only (not answered yet):** whether embeddings (a model download and native runtime) are allowed; whether the update check gets a setting or the claims change; branch protection on `main`; the Supabase telemetry policy and key; whether to schedule unattended runs. Until answered, do none of them.

### Merging your own PR

Merge only when **all** of these hold. If any fails, leave the PR for the owner and say which one.

1. CI is green on the exact head of the PR.
2. The PR body lists what was **not** verified.
3. Every new test was proven to fail without the fix.
4. The change does not loosen privacy, approval or sandbox rules, add a dependency or runtime, delete a feature, or edit a workflow.
5. The merge is guarded by the head SHA (`-f sha=<head>`). Afterwards, read CI on `main`; if it is red, revert at once and say so.

## Start of every run

1. Read this file, `STATUS.md`, `CLAUDE.md`, `ROADMAP.md`, `git log -20` and the open PRs (REST command above).
2. Write ten lines on what you think this project is for and what you will do now, and list anything that contradicts these files. Put them in the draft PR body.
3. Pick work from "Known problems" in `STATUS.md`, top down, **skipping any item that needs the owner's approval** (workflow edits, public-claim edits, deletions). Record why you skipped each.
4. Before relying on a claim in these files, check it against the code.
