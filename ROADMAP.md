# PhazeAI IDE — Roadmap to 1.0 and Beyond

This is the honest version. Every item is grounded in the current codebase. Nothing is aspirational padding.

**Current state:** pre-beta. CI is green (fmt/clippy/test/tier2 GUI), core paths work end-to-end, but there are focus-correctness bugs, UX gaps, and architecture debt that block daily-driver use.

---

## Phase 1 — Stable Beta
*Goal: someone other than the author can open it, use it for a day, and not hit a sharp edge.*

### 1.1 Unified shortcut dispatch
**Why it's broken:** Ctrl+B/Ctrl+J/Ctrl+\ are handled in three different places — root view, editor view, terminal view. When focus shifts between them, shortcuts either fire twice or not at all. This will keep regressing as new panels are added.

**Fix:** `CommandRegistry` (already in `commands.rs`) owns all bindings. Every focused surface dispatches to it instead of reimplementing. Root view handles the ones that are truly global.

**Files:** `app.rs`, `commands.rs`, `panels/editor.rs`, `panels/terminal.rs`

### 1.2 Chat cancel / retry / failure UX
**Why it's broken:** streaming works but there's no cancel button, no retry path, and failures mutate the last message text rather than showing a distinct error state.

**Fix:** `chat.rs` gets a cancel token threaded through the agent channel; a Stop button shows during stream; errors render as a dismissable error card; retry re-sends last user message.

**Files:** `panels/chat.rs`

### 1.3 Single canonical AI surface
**Why it's broken:** `ai_panel.rs` and `chat_panel.rs` are two overlapping chat implementations. They will drift.

**Fix:** pick one as canonical (chat_panel, it has streaming), fold any unique ai_panel behaviors in, delete ai_panel. One surface, clearly labeled.

**Files:** `panels/ai_panel.rs`, `panels/chat.rs`, `app.rs`

### 1.4 Composer safety hardening
**Why it's broken:** Composer runs agent+bash in the workspace with minimal guardrails. Approval mode is set but not visibly surfaced. Non-git workspaces don't degrade gracefully.

**Fix:** show workspace path + approval mode as persistent header in composer panel; confirm before running in a non-git dir; tool diffs are always expanded by default on first run.

**Files:** `panels/composer.rs`

### 1.5 Session persistence — one model
**Why it's broken:** session save/load is scattered across reactive signal side effects throughout `app.rs`. There's no single session struct, no migration, and panel state sometimes doesn't restore.

**Fix:** explicit `SessionState` struct with `load()` / `save()` / `migrate(version)`. Called once on launch and once on `WindowClosed`. All panel state goes through it.

**Files:** `app.rs`, new `session.rs`

### 1.6 First-run readiness checks
**Why it matters:** right now the IDE launches silently even when no provider is configured, rust-analyzer isn't installed, or the sidecar Python env is broken.

**Fix:** on first launch (no `settings.toml`), show a setup checklist: provider configured?, LSP binary found for workspace language?, sidecar enabled + Python found?. Non-blocking — user can skip — but actionable.

**Files:** `app.rs`, `panels/settings.rs`

---

## Phase 2 — 1.0 Release
*Goal: shippable to strangers. Package it, document it, make it fast.*

### 2.1 DAP debugger
The biggest missing feature vs VS Code. `portable-pty` already gives us a terminal; we need a DAP (Debug Adapter Protocol) client layer.

**Scope:**
- DAP client in `phazeai-core/src/dap/` (JSON-RPC over stdin/stdout, same pattern as LSP client)
- Launch adapters: `codelldb` (Rust/C++), `debugpy` (Python), `js-debug` (Node)
- UI: breakpoint gutter in editor, variables/call-stack panel, step controls

**Files:** new `phazeai-core/src/dap/`, `panels/editor.rs` (gutter), new `panels/debug.rs`

**Difficulty:** high — DAP state machine is complex, gutter interaction needs editor internals

### 2.2 Conversation persistence
**Why it matters:** losing every chat on restart makes the AI feel like a toy.

**Fix:** conversations stored as append-only JSONL in `~/.config/phazeai/conversations/<uuid>.jsonl`. Chat panel shows recent conversations in a sidebar. Composer conversations stay ephemeral by design (clearly labeled).

**Files:** `panels/chat.rs`, new `phazeai-core/src/context/persistence.rs`

### 2.3 Real semantic search
**Why it's weak now:** sidecar uses TF-IDF keyword matching but is named "semantic search / embeddings" throughout the Rust code. That's a lie and it affects result quality.

**Two options:**
- **Option A (fast):** fix the naming — call it keyword search, remove embedding references, update `sidecar/README.md`. Done in a day.
- **Option B (real):** replace TF-IDF with a local embedding model (`fastembed-rs` or ONNX Runtime + `all-MiniLM-L6-v2`). Persist embeddings to disk. True semantic similarity. 2-3 weeks.

**Recommended:** ship Option A for 1.0, plan Option B for 1.1.

**Files:** `phazeai-core/src/context/repo_map.rs`, `phazeai-sidecar/src/tool.rs`, `sidecar/server.py`

### 2.4 Multi-line find / find in selection
Currently find/replace is single-line regex only. Multi-line patterns and "find in selection" are table stakes.

**Files:** `panels/editor.rs` (find bar), `phazeai-core/src/editor/find.rs`

### 2.5 Terminal split
One terminal tab, no split. Common workflow: test output left, REPL right.

**Fix:** `TerminalState` holds `Vec<PtyInstance>` instead of one; panel renders them in configurable split. Ctrl+Shift+5 to split horizontally.

**Files:** `panels/terminal.rs`

### 2.6 Multi-repo workspaces
Opening two unrelated repos in one window. Needed for monorepo + service repo simultaneously.

**Fix:** `ProjectState.workspace_roots: Vec<PathBuf>` instead of one root. LSP manager spawns per-root. Explorer shows multiple root nodes.

**Files:** `domain_state/mod.rs`, `panels/explorer.rs`, `lsp_bridge.rs`

### 2.7 Distribution packages
Right now: `cargo run`. For 1.0: people need to install it without Rust.

**What to build:**
- Linux: `.deb` (dpkg), `.AppImage` (appimage-builder), `PKGBUILD` for AUR
- macOS: `.app` bundle + `.dmg` (via `cargo-bundle`)
- Windows: MSVC build + NSIS installer
- GitHub Actions release workflow: tag → CI builds → GitHub Release assets

**Files:** `.github/workflows/release.yml`, `Cargo.toml` (cargo-bundle metadata)

### 2.8 Documentation site
`README.md` is good but not a substitute for a real docs site.

**Scope:** mdBook or Starlight (Astro). Sections: Getting Started, Keybindings reference, Provider setup, MCP integration guide, Plugin API. Deploy to GitHub Pages on push to `main`.

---

## Phase 3 — Growth
*Goal: people choose PhazeAI over Cursor/Zed for specific reasons they can name.*

### 3.1 Cloud tier — decide and build
This is the most important architectural decision. Three options:

| Option | Pros | Cons |
|---|---|---|
| **Cloudflare Workers + D1** | Edge latency, cheap egress, no servers to manage | D1 is SQLite-class, limited for complex queries; Workers has cold start |
| **Supabase** | Postgres, RLS, auth, realtime all included | Vendor lock-in, self-host complexity |
| **Self-hosted Postgres + Stripe** | Full control, battle-tested | Ops burden, infra cost |

**Recommendation:** Supabase for getting started (auth + RLS + Postgres + storage in one), with a self-host escape hatch. The `phazeai-cloud` crate skeleton already has the auth token/keyring wiring — build the backend behind it.

**Phase 3.1 scope:**
- Auth: magic link or GitHub OAuth via Supabase Auth
- Billing: Stripe Checkout + webhook → entitlement record in Postgres
- Hosted inference: route requests to Claude/GPT via server-side key (user never sees the key)
- Audit log: every agent tool call logged per user for billing + safety

### 3.2 ACP (Agent Client Protocol) client
ACP is an emerging standard for agent-to-agent communication. Building an ACP client means any ACP server can drive the chat panel — opens a plugin/integration ecosystem.

**Files:** new `phazeai-core/src/acp/`

### 3.3 Plugin marketplace
Right now plugins are native cdylib loaded from a path. For a real ecosystem:
- WASM-based plugin sandbox (safer than cdylib, portable)
- Registry server (can be a static JSON file on GitHub Pages to start)
- Install/update flow in the Extensions panel
- Plugin manifest format: capabilities, permissions, version constraints

**Files:** `phazeai-plugin-api/`, `panels/extensions.rs`

### 3.4 Performance profiling + crash reporting
For a GPU-rendered IDE to feel premium, it needs to actually perform:
- Frame time budget: <16ms per frame for 60fps (Floem/Vello renders on GPU, should be fine, but measure it)
- Startup time target: <2s cold launch
- Crash reporter: on panic, write a minidump + last 100 log lines to `~/.config/phazeai/crashes/`; prompt user to submit

**Files:** `main.rs`, new `crash_reporter.rs`

---

## Phase 4 — Masterpiece
*Goal: people talk about it unprompted.*

### 4.1 Real-time collaboration (CRDT)
The hardest item on this list. True multiplayer editing requires:
- CRDT document model (Automerge-rs or Diamond Types) replacing the current `Vec<String>` rope
- Presence protocol (who's in the file, where's their cursor)
- Network transport (WebSocket + relay server, or peer-to-peer via WebRTC)

This is a 3-6 month full-time project. Don't start it until Phases 1-3 are solid.

### 4.2 AI that knows your whole codebase
Beyond per-file context and sidecar keyword search:
- Incremental AST-level index (tree-sitter for 40+ languages)
- True vector embeddings persisted to disk (HNSW index, `usearch` crate)
- Agent can query "find all callers of this function across the workspace" instantly
- Context window management: relevant chunks ranked by embedding similarity before injection

### 4.3 Inline AI review / pair programmer mode
Always-on mode where the AI watches edits and surfaces suggestions inline (like GitHub Copilot but agentic). Requires:
- Low-latency provider path (Groq/local Ollama, not Claude Opus)
- Debounced trigger on edit pause
- Ghost-text rendering in the editor

### 4.4 Mobile companion app
Read-only view of open files, review AI suggestions, approve composer tool calls from phone. Pairs with cloud tier auth.

---

## Decision Log

These are open architectural decisions that should be resolved before building the feature:

| Decision | Options | Blocking |
|---|---|---|
| Cloud backend | Cloudflare/D1 vs Supabase vs self-hosted Postgres | Phase 3.1 |
| Semantic search | Fix naming (ship fast) vs real embeddings (ship right) | Phase 2.3 |
| CRDT library | Automerge-rs vs Diamond Types vs Yrs | Phase 4.1 |
| Plugin sandbox | WASM (safe, portable) vs cdylib (current, fast) | Phase 3.3 |
| Collab transport | WebSocket relay vs WebRTC P2P | Phase 4.1 |

---

## What "masterpiece" actually means

Not features. The bar is:

1. **It starts in under 2 seconds.** GPU-rendered means no excuse for slow launch.
2. **The AI knows where it is.** Workspace-aware, codebase-indexed, not just file-aware.
3. **You can trust it with your codebase.** Composer shows what it changed, approval modes work, nothing happens silently.
4. **It works offline.** Editor, terminal, git, LSP — all zero-network. AI degrades gracefully to local model.
5. **It installs like software, not a dev tool.** `.deb`, `.dmg`, `.exe` — no Cargo required.
6. **Other people use it for real work and tell you about it unprompted.** That's the actual bar.

---

## Execution order (next 90 days)

```
Week 1-2   Phase 1.1  Unified shortcut dispatch
Week 2-3   Phase 1.2  Chat cancel/retry/failure UX  
Week 3     Phase 1.3  Single AI surface (delete ai_panel)
Week 4     Phase 1.4  Composer safety
Week 4-5   Phase 1.5  Session persistence — one model
Week 5     Phase 1.6  First-run readiness checks
Week 6     Phase 2.3A Fix semantic search naming (1 day)
Week 6-8   Phase 2.4  Multi-line find
Week 8-9   Phase 2.5  Terminal split
Week 9-10  Phase 2.7  Distribution packages + release CI
Week 10-11 Phase 2.8  Documentation site
Week 11-12 Phase 3.1  Cloud tier architecture decision + auth skeleton
```

After week 12, the product is 1.0-shippable. Everything after is growth.
