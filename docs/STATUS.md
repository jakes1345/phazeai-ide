# PhazeAI status: what is real, what is broken, how to build and test

> Companion to [`MISSION.md`](MISSION.md). **This file goes stale; re-check a claim against the code before building on it, and correct the file when you find it wrong.**
>
> **Which code this describes.** `main` at `3af10ed`, which is PR #5 (the Python sidecar removal, the in-process code search and the GUI-test fixes) merged on 2026-10-31.
>
> **Last verified:** 2026-10-30. Statuses were written against `e8f00d2`, the head of PR #5 that became `main`, from reading the code, then two rounds of review by readers who tried to disprove each claim. Each round still found errors, so expect a few more. Nothing was run end to end except where stated. Statistics from test runs are from one working session and cannot be reproduced from the repo.

## Where we are

**Implemented, and the engine has tests that call the production code:** agent loop and cancel; tool approval classifier (15 unit tests); privacy policy and audit log; in-process code search (BM25, 110 spec tests plus unit tests); context compaction; the built-in local AI's download, verify and install logic (unit-tested against mock servers; the real end-to-end test is `#[ignore]`; catalog sizes and hashes were re-checked against Hugging Face on 2026-10-30); the AI design engine; the CLI (TUI and `-p` mode).

**Implemented, but with little or no test of the real code** (the UI panels and `lsp_bridge.rs` have almost no unit tests, and the older test files mostly test copies; see Known problem 9): editor basics; LSP completions / hover / go-to-definition / diagnostics; DAP (one smoke test that silently skips without a debugger; only `lldb-dap`, `lldb-vscode` and `gdb` 14+); Git panel (parsing helpers are tested, the flows are not); streaming chat panel (retry-shaping and compaction-state helpers are tested; the streaming and cancel path is not); terminal; Composer; the MCP watchdog (restarts lazily on the next MCP tool call; only the restart cap is tested).

**Partly real:**
- *Vim mode:* movement and common edits, no counts.
- *LSP rename:* treats UTF-16 columns as bytes, can panic on non-ASCII, rewrites files with `lines().join("\n")` (loses CRLF and trailing newlines) and bypasses open buffers. *Code actions and inlay hints:* limited.
- *Ctrl+K inline edit:* inserts the reply at the cursor; it does not replace the selection, despite the README.
- *Composer:* real agent loop. Its three approval modes are really **two behaviours**: "Auto-approve all" attaches no approval; "Approve destructive" and "Approve all" are identical (both prompt for everything the classifier does not class ReadOnly), so the "Approve all" label overstates (Known problem 4). Diff cards come from `git diff HEAD` after the run, not a pre-write review, omit new untracked files and include the user's earlier uncommitted changes. No tests.
- *MCP host:* stdio only, configured in `<project>/.phazeai/mcp.json` (`{"servers":[...]}`; the CLI reads it from the cwd, not the git root). The repo-root `.mcp.json` is **not read by PhazeAI**; Claude Code does read it, so a Claude Code run in this repo tries to load that Supabase server.
- *Terminal:* real PTY, but a line/block renderer, not a full cell grid.
- *Workspace search panel:* prefers `rg`. Without it, a substring scan ignores regex, whole-word and globs. An invalid regex shows no results and no error.
- *Themes and `.vsix`:* the editor's syntax colours are hard-coded (syntect `base16-ocean.dark` and a fixed table), so **no user theme changes editor text**. User TOML themes recolour the chrome; 15 palette fields are read nowhere (`diag_*`, `bracket_1-4`, `find_match_*`, `indent_guide`, `cursor_line_bg`, `matching_bracket_bg`, `syn_comment`, `syn_operator`) and the other `syn_*` fields only colour the outline list. A `.vsix` installs and is listed, but its themes, snippets and language configs are **never converted or applied**.
- *Provider routing:* real; model lists and prices are hard-coded and may be stale.
- *Code completion (FIM):* a strip under the editor shows the first line of a suggestion, and Tab inserts the whole suggestion (up to 3 lines). It is **always on with no setting**, fires after cursor moves (300 ms and 600 ms debounces) and sends up to about 1.9 KB of code to the active provider, paid cloud providers included. The privacy deny rules, `local_only` and the audit log still apply (Known problem 5).
- *First run:* a readiness card exists (provider ready, rust-analyzer found).
- *Memory:* a global `memory` tool stores `~/.phazeai/memory.json`; there is no per-project memory.
- *Smallest built-in model (1.5B):* its catalog notes say it writes tool calls as plain text, so agent actions are unreliable on it. That matters for the "works without a strong machine" goal.
- *Roots after "Open Folder":* the agent's **file sandbox root is fixed at startup** (from the git root of the cwd, or the cwd) and is not updated, so the file tools refuse paths in a newly opened folder outside it. The workspace, the **privacy policy root and the code-search root do follow** Open Folder, so the sandbox and the privacy policy can name different projects.

**Claimed as shipped but absent:** the multi-agent planner / coder / reviewer pipeline and the Account panel (README), and the removed `phazeai-cloud` crate (still named in README and CONTRIBUTING).
**Honestly marked as planned:** cloud tier (architecture undecided), real-time collaboration, ACP client, mobile companion, plugin marketplace.

## Known problems (the first work items)

**P0: safety and honesty**

1. **Fake save and data loss.** Vim `:w` shows "Saved" and writes nothing. `:wq` and `:x` show "Saved" then call `std::process::exit(0)` without saving. `:q`, `:qa` and `:wqa` exit with no dirty-buffer check (`:wqa` shows no toast at all). The exit also skips the window-close handler, which stops the built-in model server and saves session state. (`phazeai-ui/src/app/overlays.rs` ~1689-1701; the real save is in `panels/editor.rs`.) Auto-save is on by default with a 1.5 s debounce and crash recovery exists, which shrinks the loss window but does not remove it.
2. **AI helpers with unrestricted tools.** Ctrl+K inline edit (`overlays.rs` ~881), the AI commit-message button (`panels/git.rs` ~2423) and the AI terminal-command bar (`panels/terminal.rs` ~2125) build `Agent::new(client)` with the default tool set (21 tools, including bash, write_file, delete_path, fetch, download) and no approval function. For Ctrl+K and the commit message, project content (selected code, a staged diff) is in the prompt, so injected text could drive tools unasked. The terminal bar sends only what the user typed, but it writes the model's reply into the terminal, so a reply with an embedded newline would run at once. **What limits the damage:** the file tools (`write_file`, `edit_file`, `delete_path`, `move_path`, `copy_path`, `create_directory`, and `download`'s output path) are confined to the workspace root and refuse protected system paths and the privacy policy file. `bash` refuses only when its *starting directory* is outside the workspace and is not itself a protected path (so a starting directory of `/`, `/tmp` or `$HOME` is not refused), plus a word-level privacy check that cannot see variables, substitutions or scripts. `fetch` (any method and body), `web_search`, `browse`, `open` (runs `xdg-open` on any path or URL), `screenshot` (writes anywhere) and the URL side of `download` have no gating, so data can leave. **Fix by making these plain tool-free chat calls** (`client.chat(&msgs, &[])`, as `fim.rs` and `panels/editor.rs` do), not by adding prompts.
3. **Provider tiles that do nothing.** Settings shows 19 providers; only 9 exist as the active provider (`LlmProvider`). Picking Mistral, DeepSeek, xAI, Cohere, Perplexity, Azure, Cerebras, Fireworks, SambaNova or GitHub Models saves nothing, silently (`provider_name_to_llm_provider` returns `None`). The CLI `--provider` rejects them. They work only through `[model_routes]`. Hide them or widen the enum.
4. **Labels that lie.** Composer's "Approve all" behaves exactly like "Approve destructive" (above) and auto-approves read tools despite its comment "require approval before every tool call".
5. **No off switch for background code completion.** FIM (above) sends code to the active provider on cursor moves with no setting and before any user action. Deny rules and `local_only` apply, but a user who does not want any background sends cannot turn it off.
6. **Settings that are ignored.** `[editor] organize_imports_on_save`, `inlay_hints` and `code_lens` in `config.toml` are never read into the IDE: `IdeState` creates fresh signals with hard-coded values (`app.rs` ~921-924; the config-backed signals at ~665-667 are never stored). Organize-imports-on-save is therefore **on at every start**, so with a language server every save also rewrites imports.
7. **Fake inlay hints and a silent no-op.** When the language server errors or returns nothing, `lsp_bridge.rs` shows regex-guessed type hints that look real (inlay hints are on by default). With no language server attached, `organize_rust_imports`'s result is discarded (`lsp_bridge.rs` ~701).
8. **Network claims that are false or misleading.** The IDE calls the GitHub releases API on every start (`updater.rs`; `app.rs` ~710-729) with no setting to turn it off and no `local_only` check. The README FAQ says "The IDE itself never needs the network" (about *needing*; the check fails silently) and answers "does it phone home?" with telemetry only. `index.html` says "No cloud. No subscriptions. No data leaving your box." (~1181-1182) and, separately, "no telemetry" (~1462), and still says the GUI is built with egui. Telemetry is real, opt-in and off by default. Gating the update check behind a setting and `local_only` is a tightening a run may do; editing the README and `index.html` needs the owner's approval.
9. **Docs claim things that do not exist:** the multi-agent pipeline, the Account panel and cloud tier, "Ctrl+K rewrites the selection", `.vsix` theme import.

**P1: our tests cannot yet be trusted as evidence**

10. About 284 of roughly 1,200 tests (counted by attribute) never call production code: `crates/phazeai-ui/tests/tier1_state.rs` (46), `lsp_tests.rs` (73), `terminal_tests.rs` (54) and about 111 of the 135 in `crates/phazeai-core/tests/editor_tests.rs` test hand-copied logic inside the test files, and stay green when the real code breaks. New tests must call the real functions (export them if needed).
11. CI steps that cannot fail or fail for the wrong reason, all in `feature-tests.yml`, **which GitHub has disabled for inactivity (it runs nothing today; re-enabling it is a repo setting)**. When it last ran, 7 of its 8 jobs failed at build because they do not install `libdbus-1-dev` and `pkg-config`. Behind that: `phazeai eval --help || echo …` (no `eval` command; prints a false message); `--test stress_test` (no such target; fails, scheduled runs only); name filters that match no test (`lsp_bridge`, `edit_file`, `bash_tool`, `grep_tool`, `glob_tool`, `list_dir`; cargo exits 0); `rust-analyzer` and `npm` installs that end in `|| true`. The DAP test passes without testing when no debugger or C compiler is installed, which is always the case in `ci.yml`. Fixing these edits workflows, which needs the owner's approval.
12. The GUI suite (`tier2_integration.rs`) is the flakiest part: it is timing- and pixel-based under software rendering. In one working session, after the harness changes in PR #5 (now on `main`), the single picker scenario passed 27 of 28 runs (3 of 10 before, on `main`) and the full suite 6 of 7; about 1 run in 12 never reached a stable frame in 60 s, which predates the PR. *These numbers are author-measured and not reproducible from the repo.* The harness now says why it gave up and saves the last frame. Measure the same scenario on `main` before blaming a PR.

**P2: hygiene and packaging**

13. The public repo contains personal or tool-generated content: `AGENTS.md` (another tool's memory, with absolute home and storage paths); home-directory paths with the author's username in `install.sh`, `script/launch_phaze.sh`, `assets/PhazeAI.desktop` and some tests; a media path in `python/training/strip_quant_config.py`; the author's Supabase project reference in `.mcp.json` **and** hard-coded in `telemetry.rs` (scrubbing `.mcp.json` alone does not remove it); a personal hardware write-up in `HARDWARE.md`. The anon key in `telemetry.rs` is public by design, but `supabase/setup.sql` grants the anon role INSERT **and SELECT** on the raw telemetry table, so anyone holding the key can read every row and flood the table. Git history also contains the author's machine hostname in commit emails; removing that needs a history rewrite, which the rules forbid. Rotating the key or restricting the policy is an owner action.
14. **No packaging path is verified.** `install.sh` builds, copies the `phazeai` binary, then aborts copying `target/release/phazeai-ide`, which no longer exists. `scripts/build-deb.sh` and `scripts/build-appimage.sh` run `cargo build -p phazeai-ide` (a deleted package) and die. `release.yml` builds an AppImage and `.deb` from `phazeai-ui` and was not tested here; the AppImage always embeds a 1x1 placeholder icon and the `.deb` ships no icon (`assets/icons/phazeai-256.png` does not exist; the real icons are in `assets/branding/`). `cargo run` and `cargo install --path crates/phazeai-ui` (`make install-ide`) are the paths that exist.
15. **Stale docs:** `BETA_AUDIT_TODO.md` (sidecar), `CONTRIBUTING.md` (cloud crate, "WASM" plugins), the README status table (shows DAP as planned and `ai_panel` as pending; both done), README lines on the multi-agent pipeline and Account panel, `CLAUDE.md` (its panel list still names `ai_panel`), and `ROADMAP.md` current-state notes for 1.2, 1.3, 1.4, 1.5, 1.6, 2.1, 2.2 (which also gives the wrong storage path) and 3.1.
16. **Caught panics when a file opens from the picker.** Opening a file logs about 6 panics from the editor tab bar (`util::safe_get` catches a read of a disposed signal; observed count, from a runtime log). The cause is probably signals created inside a re-running effect (`panels/editor.rs` ~1150 and ~1350), **not** the position-keyed tab list, so re-keying the list may not fix it. Other uses of `tab.dirty` outside `safe_get` may be affected (unverified).

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
