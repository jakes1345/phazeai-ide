# PhazeAI status: what is real, what is broken, how to build and test

> Companion to [`MISSION.md`](MISSION.md). **This file goes stale; re-check a claim against the code before building on it, and correct the file when you find it wrong.**
>
> **Which code this describes.** `main` at `1facf80` (2026-10-31). Since the last version it includes:
> - PR #8: the feature tests run real tests, and MCP works with real servers.
> - PR #7: the Vim ex bar saves for real.
> - PR #9: the AI helpers can't run tools, and each approval card answers only its own call.
> - PR #10: Stop always stops, and trimming keeps the question.
> - PR #11: settings that work, and off switches for background sends.
>
> **Last verified:** 2026-10-31 for items marked fixed: the tests named in their PRs, plus CI on `main` after each merge. Other statuses date from 2026-10-30. They were written against `e8f00d2` from reading the code, then reviewed twice by readers who tried to disprove each claim; expect a few errors. Items 17 and up were found by probes and reviews during the 2026-10-31 fixes. Nothing was run end to end except where stated. Statistics from test runs are from one working session and cannot be reproduced from the repo.

## Where we are

**Implemented, and the engine has tests that call the production code:** agent loop and cancel; tool approval classifier (15 unit tests); privacy policy and audit log; in-process code search (BM25, 110 spec tests plus unit tests); context compaction; the built-in local AI's download, verify and install logic (unit-tested against mock servers; the real end-to-end test is `#[ignore]`; catalog sizes and hashes were re-checked against Hugging Face on 2026-10-30); the AI design engine; the CLI (TUI and `-p` mode). Also tested:
- the MCP client, against the official filesystem server in CI;
- the Vim ex bar's save and quit commands;
- the text-only AI helpers;
- settings save and load;
- Stop and history trimming in the agent loop.

**Implemented, but with little or no test of the real code** (the UI panels and `lsp_bridge.rs` have almost no unit tests, and the older test files mostly test copies; see Known problem 10): editor basics; LSP completions / hover / go-to-definition / diagnostics; DAP (one smoke test that silently skips without a debugger; only `lldb-dap`, `lldb-vscode` and `gdb` 14+); Git panel (parsing helpers are tested, the flows are not); streaming chat panel (retry-shaping and compaction-state helpers are tested; the streaming and cancel path is not); terminal; Composer; the MCP watchdog (restarts lazily on the next MCP tool call; only the restart cap is tested).

**Partly real:**
- *Vim mode:* movement and common edits, no counts. The ex bar's `:w`, `:wq`, `:x`, `:q`, `:qa` and `:wqa` save, and check for unsaved work, for real.
- *LSP rename:* treats UTF-16 columns as bytes, can panic on non-ASCII, rewrites files with `lines().join("\n")` (loses CRLF and trailing newlines) and bypasses open buffers. *Code actions and inlay hints:* limited.
- *Ctrl+K inline edit:* inserts the reply at the cursor; it does not replace the selection, despite the README. Like the commit-message button and the terminal bar, it makes a text-only model call (no tools).
- *Composer:* real agent loop with three approval modes. "Auto-approve all" attaches no approval. "Approve destructive" asks for every call the classifier does not class ReadOnly. "Approve all" asks for every call. Each approval card answers only the call it shows, and the approval function is tested. Diff cards come from `git diff HEAD` after the run, not a pre-write review, omit new untracked files and include the user's earlier uncommitted changes. The run itself has no tests.
- *MCP host:* stdio only, configured in `<project>/.phazeai/mcp.json` (`{"servers":[...]}`; the CLI reads it from the cwd, not the git root).
  - It speaks MCP's newline-delimited JSON-RPC, and CI tests it against the official `@modelcontextprotocol/server-filesystem`. Before PR #8 it never worked with a real server.
  - **Its tool calls do not go through the privacy policy** (Known problem 17).
  - The repo-root `.mcp.json` is **not read by PhazeAI**. Claude Code does read it, so a Claude Code run in this repo tries to load that Supabase server.
- *Terminal:* real PTY, but a line/block renderer, not a full cell grid.
- *Workspace search panel:* prefers `rg`. Without it, a substring scan ignores regex, whole-word and globs. An invalid regex shows no results and no error.
- *Themes and `.vsix`:* the editor's syntax colours are hard-coded (syntect `base16-ocean.dark` and a fixed table), so **no user theme changes editor text**. User TOML themes recolour the chrome; 15 palette fields are read nowhere (`diag_*`, `bracket_1-4`, `find_match_*`, `indent_guide`, `cursor_line_bg`, `matching_bracket_bg`, `syn_comment`, `syn_operator`) and the other `syn_*` fields only colour the outline list. A `.vsix` installs and is listed, but its themes, snippets and language configs are **never converted or applied**.
- *Provider routing:* real; model lists and prices are hard-coded and may be stale.
- *Code completion (FIM):* a strip under the editor shows the first line of a suggestion, and Tab inserts the whole suggestion (up to 3 lines).
  - It is on by default and fires after cursor moves and edits (300 ms and 600 ms debounces).
  - It sends up to about 1.9 KB of code to the active provider, paid cloud providers included.
  - `[editor] ai_completions` turns it off entirely: Settings › Editor, or the palette's "Toggle AI Completions".
  - The privacy deny rules, `local_only` and the audit log still apply.
- *First run:* a readiness card exists (provider ready, rust-analyzer found).
- *Memory:* a global `memory` tool stores `~/.phazeai/memory.json`; there is no per-project memory.
- *Smallest built-in model (1.5B):* its catalog notes say it writes tool calls as plain text, so agent actions are unreliable on it. That matters for the "works without a strong machine" goal.
- *Roots after "Open Folder":* the agent's **file sandbox root is fixed at startup** (from the git root of the cwd, or the cwd) and is not updated, so the file tools refuse paths in a newly opened folder outside it. The workspace, the **privacy policy root and the code-search root do follow** Open Folder, so the sandbox and the privacy policy can name different projects.

**Claimed as shipped but absent:** the multi-agent planner / coder / reviewer pipeline and the Account panel (README), and the removed `phazeai-cloud` crate (still named in README and CONTRIBUTING).
**Honestly marked as planned:** cloud tier (architecture undecided), real-time collaboration, ACP client, mobile companion, plugin marketplace.

## Known problems (the first work items)

**P0: safety and honesty**

1. **Fixed (PR #7):** the Vim ex bar's `:w`, `:wq` and `:x` save for real. `:q` and `:qa` refuse to quit over unsaved tabs (`:q!` discards). Quitting closes the window properly, so the close handler runs and stops the built-in model server and saves the session.
2. **Fixed (PR #9):** Ctrl+K, the commit-message button and the terminal bar make text-only model calls: no tools are offered and none can run. The terminal bar types one line only and refuses a reply with a control character in it. The helpers now report failures, and refuse a reply cut short by an error.
3. **Provider tiles that do nothing.** Settings shows 19 providers; only 9 exist as the active provider (`LlmProvider`). Picking Mistral, DeepSeek, xAI, Cohere, Perplexity, Azure, Cerebras, Fireworks, SambaNova or GitHub Models saves nothing, silently (`provider_name_to_llm_provider` returns `None`). The CLI `--provider` rejects them. They work only through `[model_routes]`. Hide them or widen the enum.
4. **Fixed (PR #9):** Composer's "Approve all" asks before every call. Each approval card answers only the call it shows. Before, clicking Allow on a card for a read-only call that had already run approved the next write or command, unseen.
5. **Fixed (PR #11):** `[editor] ai_completions` turns background code completion off entirely.
6. **Fixed (PR #11):** `organize_imports_on_save`, `inlay_hints` and `code_lens` come from `config.toml`. Toggles are saved, and a save writes only the keys that changed. Organize-imports-on-save is off unless the config turns it on.
7. **Fake inlay hints and a silent no-op.** When the language server errors or returns nothing, `lsp_bridge.rs` shows regex-guessed type hints that look real (inlay hints are on by default). With no language server attached, `organize_rust_imports`'s result is discarded (`lsp_bridge.rs` ~701). Separately, inlay hints never reach the editor at all (Known problem 21). Wiring them will expose the guessed hints, so fix both together.
8. **Network claims that are false or misleading.** The start-up update check is now a setting (`[updates] check_at_start`), skipped when the folder PhazeAI starts in is local-only. The GitHub Actions panel no longer fetches while hidden (PR #11). **Still open:** The README FAQ says "The IDE itself never needs the network" (about *needing*; the check fails silently) and answers "does it phone home?" with telemetry only. `index.html` says "No cloud. No subscriptions. No data leaving your box." (~1181-1182) and, separately, "no telemetry" (~1462), and still says the GUI is built with egui. Telemetry is real, opt-in and off by default. Editing the README and `index.html` needs the owner's approval.
9. **Docs claim things that do not exist:** the multi-agent pipeline, the Account panel and cloud tier, "Ctrl+K rewrites the selection", `.vsix` theme import.

*Found 2026-10-31:*

17. **MCP tool calls bypass the privacy policy.** `McpToolBridge::execute` sends the model's arguments to the server and returns its result unchecked. Against the official filesystem server, the model could:
    - read `.env` by a relative, absolute or `..` path, through `read_multiple_files`, or with an `edit_file` dry run;
    - move a denied directory out of the policy;
    - rewrite `.phazeai/privacy.toml` to `deny = []`;
    - reach denied files through Unicode-equivalent spellings of their paths.

    Listings also name denied files. A fix is in progress.
18. **Chat's diff approval can show a different change than the one written.** For `edit_file` the preview applies the edit once (`replacen(.., 1)`), but with `replace_all` the tool changes every occurrence. A fix is in progress.
19. **`[llm] base_url` is ignored for chat and completions.** Only the readiness probe reads it, so requests can go to a different host than the one configured. Observed: the probe on a custom port, `POST /api/chat` on `localhost:11434`. A fix is in progress.

**P1: our tests cannot yet be trusted as evidence**

10. About 284 of roughly 1,200 tests (counted by attribute) never call production code: `crates/phazeai-ui/tests/tier1_state.rs` (46), `lsp_tests.rs` (73), `terminal_tests.rs` (54) and about 111 of the 135 in `crates/phazeai-core/tests/editor_tests.rs` test hand-copied logic inside the test files, and stay green when the real code breaks. New tests must call the real functions (export them if needed).
11. **Fixed (PR #8):** every `feature-tests.yml` job builds and runs real tests, and the workflow runs on every PR. The MCP job runs against the official filesystem server and the LSP job against `rust-analyzer`; Eval Harness was removed because there is no eval feature. **Still open:** the DAP test passes without testing when no debugger or C compiler is installed, which is always the case in `ci.yml`.
12. The GUI suite (`tier2_integration.rs`) is the flakiest part: it is timing- and pixel-based under software rendering. In one working session, after the harness changes in PR #5 (now on `main`), the single picker scenario passed 27 of 28 runs (3 of 10 before, on `main`) and the full suite 6 of 7; about 1 run in 12 never reached a stable frame in 60 s, which predates the PR. *These numbers are author-measured and not reproducible from the repo.* The harness now says why it gave up and saves the last frame. Measure the same scenario on `main` before blaming a PR.

*Found 2026-10-31:*

20. **The system prompt is not sized to the context budget.** Project instructions go in whole: `CLAUDE.md` is about 20 KB here, roughly 7,000 tokens. A local model's default budget is about 4,100 tokens, so with Ollama or LM Studio every request starts over budget, and trimming cuts tool outputs to bare markers. A fix is in progress.
21. **Peek Definition, Ctrl+T workspace symbols and inlay hints never show language-server results.** `EditorState` gets fresh signals for `peek_def_*`, `workspace_symbols` and the split state. The main editor's inlay data is a signal nothing sets, instead of the LSP's.
22. **The editor does not take keyboard input until clicked**, after a file opens from the picker or an overlay closes with Escape. Nothing requests focus for the editor view.
23. `session.toml` (`SessionState`) has no test that calls production code.

**P2: hygiene and packaging**

13. The public repo contains personal or tool-generated content: `AGENTS.md` (another tool's memory, with absolute home and storage paths); home-directory paths with the author's username in `install.sh`, `script/launch_phaze.sh`, `assets/PhazeAI.desktop` and some tests; a media path in `python/training/strip_quant_config.py`; the author's Supabase project reference in `.mcp.json` **and** hard-coded in `telemetry.rs` (scrubbing `.mcp.json` alone does not remove it); a personal hardware write-up in `HARDWARE.md`. The anon key in `telemetry.rs` is public by design, but `supabase/setup.sql` grants the anon role INSERT **and SELECT** on the raw telemetry table, so anyone holding the key can read every row and flood the table. Git history also contains the author's machine hostname in commit emails; removing that needs a history rewrite, which the rules forbid. Rotating the key or restricting the policy is an owner action.
14. **No packaging path is verified.** `install.sh` builds, copies the `phazeai` binary, then aborts copying `target/release/phazeai-ide`, which no longer exists. `scripts/build-deb.sh` and `scripts/build-appimage.sh` run `cargo build -p phazeai-ide` (a deleted package) and die. `release.yml` builds an AppImage and `.deb` from `phazeai-ui` and was not tested here; the AppImage always embeds a 1x1 placeholder icon and the `.deb` ships no icon (`assets/icons/phazeai-256.png` does not exist; the real icons are in `assets/branding/`). `cargo run` and `cargo install --path crates/phazeai-ui` (`make install-ide`) are the paths that exist.
15. **Stale docs:** `BETA_AUDIT_TODO.md` (sidecar), `CONTRIBUTING.md` (cloud crate, "WASM" plugins), the README status table (shows DAP as planned and `ai_panel` as pending; both done), README lines on the multi-agent pipeline and Account panel, `CLAUDE.md` (its panel list still names `ai_panel`), and `ROADMAP.md` current-state notes for 1.2, 1.3, 1.4, 1.5, 1.6, 2.1, 2.2 (which also gives the wrong storage path) and 3.1.
16. **Fixed (PR #7):** the tab-bar panics came from signals created inside a re-running effect. A tab's unsaved flag now lives in the panel's scope (0 panics in 16 runs).

*Found 2026-10-31:*

24. **Killed language servers are never reaped.** `lsp/client.rs` kills without `wait()`, so each killed server stays a zombie process until PhazeAI exits. A fix is in progress.
25. **The model router matches keywords as substrings.** "Explanation" contains "plan", so any prompt saying "no explanation" goes to the reasoning route. A fix is in progress.
26. **MCP client gaps.** It does not answer requests the server sends (such as `ping`). Pending requests wait 30 s when a server dies. `McpToolDef.description` is required, though MCP makes it optional.
27. **Stop drops the response stream, not the request.** The OpenAI client's spawned SSE reader keeps the HTTP response open until the provider finishes. In the CLI's long-lived runtime the request carries on in the background.
28. **Settings saves can land out of order.** Each change spawns its own save thread.
29. **The commit-message request's audit entry lists no files**, though the request carries the staged diff.
30. `plugin_canary_e2e` hard-codes `<workspace>/target/debug`, so it fails when `CARGO_TARGET_DIR` is set.

## Build and test on Linux (Linux Mint / Ubuntu)

```bash
# For a human; an automated run must not use sudo and should report which package is missing.
# Rust: install with rustup (rust-toolchain.toml pins 1.93; apt's cargo ignores the pin).
sudo apt install build-essential pkg-config libssl-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libwayland-dev libxcb-xfixes0-dev libxcb-shape0-dev libxcb-render0-dev \
  libudev-dev libinput-dev libdbus-1-dev
# Only for the GUI test suite (separate, because it is not needed to build):
sudo apt install xvfb xdotool imagemagick fluxbox mesa-vulkan-drivers libgl1-mesa-dri

cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings    # the CI form; local docs omit --all-targets
cargo test --workspace
xvfb-run -a cargo test -p phazeai-ui --test tier2_integration -- --ignored --test-threads=1   # GUI suite
```

The first build needs the network (Floem is a git dependency) and about 14 GB of disk. The `-dev` packages mirror CI; not all are needed to compile, but the runtime libraries are. CI authority is `.github/workflows/ci.yml` (some README and file-header instructions are stale). `cargo test --workspace` reads the real `~/.config/phazeai/config.toml` in some tests.

**Run the IDE or CLI** (a human may just use `cargo run -p phazeai-ui` / `-p phazeai-cli`). An automated run must build first, then start the built binary with a throwaway home and no provider keys, because a changed `HOME` breaks `cargo`/`rustup`:

```bash
T=$(mktemp -d)
env -u ANTHROPIC_API_KEY -u OPENAI_API_KEY HOME=$T XDG_CONFIG_HOME=$T/.config XDG_DATA_HOME=$T/.local/share target/debug/phazeai-ui
```

**Where things live.** Config `~/.config/phazeai/config.toml`. GUI log `~/.config/phazeai/logs/` (daily files); TUI log `~/.config/phazeai/cli.log`; `-p` mode logs to stderr. `crash-recovery/` holds unsaved buffer contents. `XDG_CONFIG_HOME` is honoured for these **but not for `session.toml`, which is always `$HOME/.config/phazeai/session.toml`**. Downloaded models and `llama.cpp` live under `${XDG_DATA_HOME:-~/.local/share}/phazeai/`. A second root, `~/.phazeai/`, holds `conversations/`, `memory.json`, `plugins/` and `extensions/`. Per project: `<project>/.phazeai/` (`privacy.toml`, `mcp.json`, `designs/`).

**Things that bite.**
- The IDE's workspace, file sandbox and privacy policy all start from the git root of the working directory (or the directory itself if it is not a git repo). Launching from `$HOME` makes everything below `$HOME` writable to the agent.
- Running the GUI or CLI otherwise uses your real `~/.config/phazeai`, the keyring and any provider keys in the environment. The GUI test harness avoids that by pointing `HOME` and `XDG_CONFIG_HOME` at a temp directory.
- The GUI suite hard-codes `target/debug/phazeai-ui`, falls back to `DISPLAY=:0` (so **never run it without `xvfb-run`**; it would drive the real desktop), starts `fluxbox` with the real `HOME` (creating `~/.fluxbox`), and can leave `Xvfb`, `fluxbox` and `phazeai-ui` processes behind if it dies. It rewrites the 17 tracked PNGs in `crates/phazeai-ui/tests/snapshots/`, creates two untracked ones every run (`s5_baseline.png`, `s5_palette_opened.png`), and adds `settle_timeout.png` on a rendering timeout. Before committing, `git checkout -- crates/phazeai-ui/tests/snapshots`, delete the untracked PNGs, stage files by name, and read `git status`.
- The GUI suite must run from inside the git checkout: the picker scenario expects `AGENTS.md` and `Cargo.toml` in the workspace listing.
- **Worktrees that share one `CARGO_TARGET_DIR` can run each other's builds without warning.** Cargo names workspace crates by their path relative to the workspace root and judges freshness by file times, so a test binary built from another worktree can run in place of yours. Give each worktree its own target dir. If you must share one, touch the sources before every build (`find crates -name '*.rs' -exec touch {} +`) and check the `Compiling ... (<this worktree>)` line.
