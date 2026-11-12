# PhazeAI status: what is real, what is broken, how to build and test

> Companion to [`MISSION.md`](MISSION.md). **This file goes stale; re-check a claim against the code before building on it, and correct the file when you find it wrong.**
>
> **Which code this describes.** `main` at `7255f5b` (2026-11-12). Since the last version (`1facf80`, PRs #7–#11) it includes:
> - Safety: MCP tool calls obey the privacy policy (#13); the approved diff is exactly what gets written (#14); `[llm] base_url` is honoured and a key never goes there unencrypted (#16); local-model requests skip proxies (#21); permission rules and tool hooks (#49).
> - Agent: the system prompt fits the context budget (#17); long tool output never crashes (#18); word-start router matching (#19); real tool examples (#20); prompt caching and cache-hit reporting (#35–#37, #40); lean prompt and on-demand tools (#38, #39); cheap model for side jobs (#41); keep or undo each hunk of an edit (#44) and undo a whole run (#45); live token and cost meter (#46); other tools' rules files (#47); subagents and a todo list (#48); `-p` JSON output and MCP over HTTP (#51); `phazeai eval` (#52).
> - Providers: every provider in Settings can be the active one (#42); fill-in-the-middle per provider (#43); live model lists (#50); `[[providers]]` entries take effect, LM Studio `/v1` and Ollama-at-a-path addresses work (#53).
> - Editor: auto-save writes the edited tab (#25); the editor takes focus after opening a file or closing a popup, shortcuts work while typing, undo no longer crashes (#55); Ctrl+T, Peek and inlay hints show language-server results, guessed hints removed (#56); F12, Vim keys, Tab-to-accept and the find box work while typing (#57).
> - Other: language servers, debug adapters and MCP servers are reaped (#23); gdb breakpoints set before launch (#54); install scripts build the real IDE (#22); README and landing page match the code (#29, #31).
>
> **Last verified:** 2026-11-12. Fixed items: the tests named in their PRs, plus CI on `main` after each merge. Each open item was re-checked against `7255f5b` by reading the code (not by running it). #55 and #57 were also driven in the real IDE under Xvfb. Older statuses date from 2026-10-30. They were written against `e8f00d2` from reading the code, then reviewed twice by readers who tried to disprove each claim; expect a few errors. Items 17 and up were found by probes and reviews during the 2026-10-31 fixes. Nothing was run end to end except where stated. Statistics from test runs are from one working session and cannot be reproduced from the repo.

## Where we are

**Implemented, and the engine has tests that call the production code:** agent loop and cancel; tool approval classifier (16 unit tests); privacy policy and audit log; in-process code search (BM25, 110 spec tests plus unit tests; optional matching by meaning through an embedding endpoint the user runs, tested against a real local HTTP server that speaks the Ollama and OpenAI formats, not against a real model); context compaction; the built-in local AI's download, verify and install logic (unit-tested against mock servers; the real end-to-end test is `#[ignore]`; catalog sizes and hashes were re-checked against Hugging Face on 2026-10-30); the AI design engine; the CLI (TUI and `-p` mode). Also tested:
- the MCP client, against the official filesystem server in CI;
- the Vim ex bar's save and quit commands;
- the text-only AI helpers;
- settings save and load;
- Stop and history trimming in the agent loop;
- permission rules and tool hooks (#49), the per-hunk keep/undo of an agent's edit (#44) and run undo (#45);
- editor key routing and focus (#55, #57): unit tests plus real-IDE runs under Xvfb.

**Implemented, but with little or no test of the real code** (the UI panels and `lsp_bridge.rs` have almost no unit tests, and the older test files mostly test copies; see Known problem 10): editor basics; LSP completions / hover / go-to-definition / diagnostics; DAP (one smoke test that silently skips without a debugger; only `lldb-dap`, `lldb-vscode` and `gdb` 14+); Git panel (parsing helpers are tested, the flows are not); streaming chat panel (retry-shaping and compaction-state helpers are tested; the streaming and cancel path is not); terminal; Composer; the MCP watchdog (restarts lazily on the next MCP tool call; only the restart cap is tested).

**Partly real:**
- *Vim mode:* movement and common edits, no counts. The ex bar's `:w`, `:wq`, `:x`, `:q`, `:qa` and `:wqa` save, and check for unsaved work, for real.
- *LSP rename:* treats UTF-16 columns as bytes, can panic on non-ASCII, rewrites files with `lines().join("\n")` (loses CRLF and trailing newlines; `apply_text_edits`, `lsp_bridge.rs` ~1277) and bypasses open buffers. *Code actions:* limited. *Inlay hints, code lens and folding ranges:* main pane only; split panes get empty signals (`app.rs` ~3486, ~3810).
- *Vim normal mode* (#57) works while typing, but the cursor can sit past a line's last character, so `x` there deletes the newline.
- *Ctrl+K inline edit:* inserts the reply at the cursor; it does not replace the selection, despite the README. Like the commit-message button and the terminal bar, it makes a text-only model call (no tools).
- *Composer:* real agent loop with three approval modes. "Auto-approve all" attaches no approval. "Approve destructive" asks for every call the classifier does not class ReadOnly. "Approve all" asks for every call. Each approval card answers only the call it shows, and the approval function is tested. Diff cards come from `git diff HEAD` after the run, not a pre-write review, omit new untracked files and include the user's earlier uncommitted changes. The run itself has no tests. (The chat panel, unlike Composer, reviews each hunk before the write and can undo a run: #44, #45.)
- *MCP host:* stdio servers, and remote servers by `url` (#51), configured in `<project>/.phazeai/mcp.json` (`{"servers":[...]}`; the CLI reads it from the cwd, not the git root).
  - It speaks MCP's newline-delimited JSON-RPC, and CI tests it against the official `@modelcontextprotocol/server-filesystem`.
  - Tool calls go through the privacy policy, and results are filtered (#13).
  - The repo-root `.mcp.json` is **not read by PhazeAI**. Claude Code does read it, so a Claude Code run in this repo tries to load that Supabase server.
- *Terminal:* real PTY, but a line/block renderer, not a full cell grid.
- *Workspace search panel:* prefers `rg`. Without it, a substring scan ignores regex, whole-word and globs. An invalid regex shows no results and no error.
- *Themes and `.vsix`:* the editor's syntax colours are hard-coded (syntect `base16-ocean.dark` and a fixed table), so **no user theme changes editor text**. User TOML themes recolour the chrome; 15 palette fields are read nowhere (`diag_*`, `bracket_1-4`, `find_match_*`, `indent_guide`, `cursor_line_bg`, `matching_bracket_bg`, `syn_comment`, `syn_operator`) and the other `syn_*` fields only colour the outline list. A `.vsix` installs and is listed, but its themes, snippets and language configs are **never converted or applied**.
- *Provider routing:* real. Model lists come from each provider and are cached for a day (#50). Prices are a hard-coded table (`cost.rs`); unknown models get no price.
- *Code completion (FIM):* a strip under the editor shows the first line of a suggestion and how many more it has, and Tab inserts the whole suggestion (up to 6 lines from a fill-in-the-middle API, 3 from the chat prompt).
  - It is on by default and fires 300 ms after you stop typing: one request per pause. Moving the cursor without typing sends nothing.
  - PhazeAI Local (llama.cpp `/infill`), Ollama (`/api/generate` with a suffix), DeepSeek (`/beta/completions`, on Flash) and Mistral (`/v1/fim/completions`, Codestral) get the code as it is, with no prompt around it. `[editor] completion_model` picks the model for Ollama, DeepSeek and Mistral. Other providers, and a model that turns such a request down, get a chat prompt.
  - It sends the 40 lines before the cursor and the 20 after to the active provider, paid cloud providers included.
  - `[editor] ai_completions` turns it off entirely: Settings › Editor, or the palette's "Toggle AI Completions".
  - The privacy deny rules, `local_only` and the audit log still apply.
- *First run:* a readiness card exists (provider ready, rust-analyzer found).
- *Memory:* a global `memory` tool stores `~/.phazeai/memory.json`; there is no per-project memory.
- *Smallest built-in model (1.5B):* its catalog notes say it writes tool calls as plain text, so agent actions are unreliable on it. That matters for the "works without a strong machine" goal.
- *Roots after "Open Folder":* **fixed (this PR).** The agent's file sandbox now follows the workspace like the privacy policy and code search do. Relative tool paths (`src/main.rs`, a `list_files`/`grep`/`glob` with no path) are read from the open folder, not the folder the IDE started in, and the shell starts over at the new root. Left open: a run still in progress when another folder is opened carries on in the new folder.

**Claimed as shipped but absent:** the README is accurate since #29. `CONTRIBUTING.md` (~35) still names the removed `phazeai-cloud` crate and "WASM" plugins, and `ROADMAP.md` (~134) its skeleton.
**Honestly marked as planned:** cloud tier (architecture undecided), real-time collaboration, ACP client, mobile companion, plugin marketplace.

## Known problems (the first work items)

**P0: safety and honesty**

1. **Fixed (PR #7):** the Vim ex bar's `:w`, `:wq` and `:x` save for real. `:q` and `:qa` refuse to quit over unsaved tabs (`:q!` discards). Quitting closes the window properly, so the close handler runs and stops the built-in model server and saves the session.
2. **Fixed (PR #9):** Ctrl+K, the commit-message button and the terminal bar make text-only model calls: no tools are offered and none can run. The terminal bar types one line only and refuses a reply with a control character in it. The helpers now report failures, and refuse a reply cut short by an error.
3. **Fixed (PR #42):** every provider in Settings and the CLI `--provider` can be the active provider. Perplexity and GitHub Models are no longer offered (retired).
4. **Fixed (PR #9):** Composer's "Approve all" asks before every call. Each approval card answers only the call it shows. Before, clicking Allow on a card for a read-only call that had already run approved the next write or command, unseen.
5. **Fixed (PR #11):** `[editor] ai_completions` turns background code completion off entirely.
6. **Fixed (PR #11):** `organize_imports_on_save`, `inlay_hints` and `code_lens` come from `config.toml`. Toggles are saved, and a save writes only the keys that changed. Organize-imports-on-save is off unless the config turns it on.
7. **Partly fixed (PR #56):** the regex-guessed inlay hints are gone; only the server's hints are shown. **Still open:** with no language server attached, organize-imports-on-save does nothing (`organize_rust_imports`'s result is discarded, `lsp_bridge.rs` ~701).
8. **Fixed (PRs #29, #31):** the README has a "Network use" section (update check, telemetry, downloads, Actions, model lists, remote MCP), and `index.html` no longer claims "no telemetry", "No data leaving your box" or egui.
9. **Fixed in README and `index.html` (PRs #29, #31).** Leftovers in `CONTRIBUTING.md` and `ROADMAP.md`: see 15.

*Found 2026-10-31:*

17. **Fixed (PR #13):** MCP tool calls are checked against the privacy policy before they run (`mcp_bridge.rs` ~112), and listings and text results are filtered.
18. **Fixed (PR #14):** the agent writes exactly the approved change (`Tool::plan_write`), `replace_all` included.
19. **Fixed (PRs #16, #53):** `[llm] base_url` and `[[providers]]` addresses are used for every request, and a key is never sent to a plain-HTTP address on another machine.

**P1: our tests cannot yet be trusted as evidence**

10. About 284 of roughly 1,200 tests (counted by attribute) never call production code: `crates/phazeai-ui/tests/tier1_state.rs` (46), `lsp_tests.rs` (73), `terminal_tests.rs` (54) and about 111 of the 135 in `crates/phazeai-core/tests/editor_tests.rs` test hand-copied logic inside the test files, and stay green when the real code breaks. New tests must call the real functions (export them if needed).
11. **Fixed (PR #8):** every `feature-tests.yml` job builds and runs real tests, and the workflow runs on every PR. The MCP job runs against the official filesystem server and the LSP job against `rust-analyzer`. (`phazeai eval`, #52, is a separate graded-task runner, not a CI job.) **Still open:** the DAP test passes without testing when no debugger or C compiler is installed, which is always the case in `ci.yml`.
12. The GUI suite (`tier2_integration.rs`) is the flakiest part: it is timing- and pixel-based under software rendering. In one working session, after the harness changes in PR #5 (now on `main`), the single picker scenario passed 27 of 28 runs (3 of 10 before, on `main`) and the full suite 6 of 7; about 1 run in 12 never reached a stable frame in 60 s, which predates the PR. *These numbers are author-measured and not reproducible from the repo.* The harness now says why it gave up and saves the last frame. Measure the same scenario on `main` before blaming a PR.

*Found 2026-10-31:*

20. **Fixed (PR #17):** project instructions take at most a quarter of the context budget (`fit_instructions`, `system_prompt.rs`).
21. **Fixed (PR #56):** Peek Definition, Ctrl+T and inlay hints show the language server's results (main pane only; see "Partly real").
22. **Fixed (PRs #55, #57):** the editor takes focus when its tab becomes active and when an overlay closes; the app's shortcuts, F-keys, Vim keys, Tab-to-accept and the find box work while typing. Not driven in the real IDE: Tab accepting a real AI suggestion (needs a model), and each editing shortcut that moved into the editor (Ctrl+Shift+K, folding, Alt+Up/Down ...).
23. `session.toml` (`SessionState`) has no test that calls production code.

**P2: hygiene and packaging**

13. The public repo contains personal or tool-generated content (`install.sh` cleaned in #22): `AGENTS.md` (another tool's memory, with absolute home and storage paths); home-directory paths with the author's username in `script/launch_phaze.sh`, `assets/PhazeAI.desktop` and some tests; a media path in `python/training/strip_quant_config.py`; the author's Supabase project reference in `.mcp.json` **and** hard-coded in `telemetry.rs` (scrubbing `.mcp.json` alone does not remove it); a personal hardware write-up in `HARDWARE.md`. The anon key in `telemetry.rs` is public by design, but `supabase/setup.sql` grants the anon role INSERT **and SELECT** on the raw telemetry table, so anyone holding the key can read every row and flood the table. Git history also contains the author's machine hostname in commit emails; removing that needs a history rewrite, which the rules forbid. Rotating the key or restricting the policy is an owner action.
14. **Partly fixed (PR #22):** `install.sh`, `scripts/build-deb.sh` and `scripts/build-appimage.sh` build `phazeai-ui` (not run in CI). **Still open:** `release.yml` looks for the missing `assets/icons/phazeai-256.png`, so its AppImage embeds a 1x1 placeholder icon and its `.deb` ships no icon (the real icons are in `assets/branding/`). Changing the workflow needs the owner's approval.
15. **Partly fixed (PR #29, README):** still stale: `BETA_AUDIT_TODO.md` (sidecar), `CONTRIBUTING.md` (cloud crate, "WASM" plugins), `ROADMAP.md` (~134, the cloud crate skeleton, plus the current-state notes listed before) and `CLAUDE.md` (its panel list still names `ai_panel`, which does not exist).
16. **Fixed (PR #7):** the tab-bar panics came from signals created inside a re-running effect. A tab's unsaved flag now lives in the panel's scope (0 panics in 16 runs).

*Found 2026-10-31:*

24. **Fixed (PR #23):** language servers, debug adapters and MCP servers are reaped (`process::ChildSlot`).
25. **Fixed (PR #19):** keywords match at word starts, so "no explanation" no longer routes to Reasoning ("planet" still does, on purpose).
26. **Fixed (this PR):** the MCP client answers a server's `ping` (over stdio and in an HTTP event stream) and turns down other server requests with "Method not found" instead of leaving them unanswered; a server's request id is no longer taken for the answer to one of the client's. A server that exits fails its waiting requests at once rather than after 30 s, and a request to it is not sent. A tool without a `description` is listed.
27. **Fixed (PR #65):** Stop now ends the request. The OpenAI-compatible, Claude and Ollama stream readers read through `llm/stream_cancel.rs`, which checks every 200 ms whether the receiver is gone and then drops the response, closing the connection, so a stopped run stops generating and billing. Tested against real local sockets that keep the reply open.
28. **Settings saves can land out of order.** Each change spawns its own save thread; `SETTINGS_WRITE_LOCK` stops overlapping writes but not reordering.
29. **The commit-message request's audit entry lists no files**, though the request carries the staged diff.
30. `plugin_canary_e2e` hard-codes `<workspace>/target/debug`, so it fails when `CARGO_TARGET_DIR` is set.

*Found 2026-11-12:*

31. **Fixed (this PR):** opening another folder moves the agent's file sandbox, its relative paths and its shell with it (`app/tool_root.rs`). Detail under "Roots after Open Folder".
32. **The terminal is a line/block renderer**, so full-screen programs (`vim`, `htop`, `less`) inside it draw wrongly.
33. **No end-to-end run with a real model** (cloud key or built-in) has been recorded for the chat, agent or Composer paths; their tests use mocks.
34. **Fixed (this PR):** a `config.toml` with a mistake in it was replaced by the defaults. The IDE loaded the defaults with only a line on stderr, and its provider/model effect saved them at every start, so one typo erased the whole file. Now `Settings::save` refuses while the file can't be read, the IDE shows "line L, column C: reason" in a toast at start and at the top of Settings, and the CLI prints it.

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
