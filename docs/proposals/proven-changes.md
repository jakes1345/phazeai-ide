# Proposal: Proven changes and Find the holes

*Proposal; nothing is built. Written 2026-11-01 against `main` at `4afe54f` (PR #14 merged since, at `6c502c5`) from three designs (MVP-first, architecture-first, user-first). "Checked" means run that day; the rest is read from the code.*

## Summary

An AI finds and fixes real bugs when every claim must pass a check that runs code. When it only has to sound sure, it makes confident mistakes. So PhazeAI runs the check, not the model. For a bug fix, PhazeAI runs the agent's new test three times:

- on the old code, where it must fail;
- on the fixed code, where it must pass;
- with the fix taken out again, where it must fail.

Then it puts the fix back byte for byte and reports what ran and what was not checked. **Find the holes** reports only bugs it reproduced with a failing probe test, and each one has a **Fix it with proof** button. Tests and probes run on the user's machine, under today's privacy and approval rules, with costs shown and capped.

## What the user sees

### Fixing a bug (Phase 1: Composer and the CLI)

1. Composer gets a **Prove** toggle, off by default. The user asks: "parse_duration panics on empty input".
2. The agent writes a test. PhazeAI writes the approval card: *"Prove, before the fix: run `cargo test --test parse_duration_empty` once. Expected: it fails."* It fails. If it had passed, the model would have to write a sharper test.
3. The agent edits `src/time.rs`. That edit is approved the same way as today.
4. A second card: *"Prove, after the fix: run the same command with your fix, then with src/time.rs put back as it was at the failing run; then restore your version."*
5. The result (numbers illustrative):

```
PROVEN by 1 test: fails without the change to src/time.rs, passes with it
  before the fix   FAILED  panicked at src/time.rs:14   38 s
  with the fix     passed                               41 s
  fix reverted     FAILED  same panic                   37 s
  restored         src/time.rs, SHA-256 matches
Not verified  full suite · each state ran once · README.md (no test)
Spent         8 turns · ≈52k in / 3.4k out tokens (est.) · ≈$0.21 · tests 1m56s
```

The model's own summary stays below the card, unchanged. If no proof ran, the card reads **NO PROOF RAN**. Every Composer run also ends with one line from the change ledger, which costs no tokens: "Changed 2 files · no command ran after the last change".

### Find the holes (Phase 2)

1. **Find the holes…** (palette, or `phazeai --holes PATH`) asks for a scope: a file, a folder, or changes since a git ref. It then shows:
   - which provider may receive the files in scope (files the privacy policy denies are left out);
   - what it looks for: crashes, hangs, wrong results, injection, path escape;
   - that probes run your code with your permissions;
   - a budget.
2. Risky spots ("hotspots") are found without a model. The model then reads around them with read-only tools.
3. The agent writes one probe file per crate or package. Each probe is a test that asserts the correct behaviour. The user reviews the file as one diff and approves each exact command.
4. Each suspect ends as **Reproduced**, **Ruled out** or **Couldn't test** (with the reason).
   - Findings are ranked by severity, each with its probe, command and output. Suspects that were not reproduced are collapsed under "Leads, not findings".
   - **Fix it with proof** uses the probe as the already-failing test. The probe is frozen so the fix cannot edit it.
   - When nothing is found, the panel says: "No holes reproduced. This is not a security audit."

## Engine pieces

A new module, `crates/phazeai-core/src/proof/`, plus hooks in `agent/core.rs`. No new dependencies: `similar`, `sha2`, `libc` and tree-sitter are already in `phazeai-core`.

- **`ChangeLedger`**: records every committed write as `WriteRecord { path, before, after, tool, call_id }`, plus each `bash` command and its exit code.
  - Writes come through `FileWrite` (PR #14) and are recorded even when no diff hook is attached.
  - Changes made outside the file tools are found by comparing `git status` and file hashes at the start and end of the run, and are listed as unreviewed.
- **`ProveTool`** (tool name `prove`), with parameters `{stage: before_fix | after_fix, test_command, test_files}`:
  - `before_fix` runs the test once and checkpoints the files in the ledger.
  - `after_fix` must use the same command. It runs the test with the change, then with every non-test file reverted to the checkpoint (or to its bytes from before the run), then restores the change.
  - The command runs without a shell. Shell metacharacters are refused.
- **Runner** (`runner.rs`, `outcome.rs`):
  - reuses `bash`'s process-group kill, moved into `process.rs`;
  - removes provider API-key variables from the test environment;
  - parses the full output into `TestOutcome { Passed, NoTestsRan, Failed, BuildFailed, TimedOut, Crashed }` for cargo, pytest and `go test`. Other test runners count as "exit code only".
  - The model gets back a summary of about 1.5 KB.
- **`RevertGuard`**, which makes the temporary revert safe:
  - Before touching a file, it writes the bytes to a journal under `$XDG_DATA_HOME/phazeai/proof-journal/`.
  - It writes through `writable_target`, so the sandbox, privacy and protected-file checks apply.
  - Writes are compare-and-swap: a file is only written if it still holds the bytes PhazeAI expects.
  - It also restores in `Drop`, because Stop drops the tool's future.
  - `recover_interrupted()` runs at startup, after a crash.
- **`ProofVerdict`**:
  - `Proven`: an assertion failure before the fix and again with it reverted, a pass with it, and a verified restore.
  - `UsesChange`: the test does not compile without the change. This is normal for a new feature. The card says "the test needs the new code; it may not check it".
  - `PassesOnly`.
  - `NotProven(reason)`.
  - `Evidence` keeps the runs, usage and file hashes. If those files are edited later, the card is marked stale.
- **`ProofSession`**: `Agent::with_proof` registers `prove`, raises `max_iterations` (15 today; a proof run needs about 8 turns) and adds a protocol prompt of at most 250 tokens. If files changed and nothing was proven, PhazeAI adds one `[PhazeAI]` message asking the model to prove it.
- **`BudgetMeter`** (`agent/budget.rs`): shared counters for tokens, test runs and test seconds, checked before every model call and every test run. New events: `AgentEvent::Usage` and `AgentEvent::ProofStep`.
- **`holes/`** (Phase 2):
  - A pattern table run over the files `search::scan::discover` returns, so `.gitignore` and the privacy policy apply.
  - A finder agent on a new `ToolRegistry::strictly_read_only()`, holding `read_file`, `grep`, `glob`, `list_files`, `code_search` and `report_suspect` (in memory only, classed ReadOnly).
  - It is not built on `read_only()`, whose `memory` tool writes a file. It leaves out `find_path`, which the classifier treats as Execute, and has no `fetch`, `web_search`, MCP or `bash`.

## Phased plan

Every test calls production code and is shown to fail with the fix removed (temporary commit or `git checkout <base> -- <file>`).

**Phase 0: prerequisites, one PR each.**
- PR #14 (`FileWrite`: the approved diff is exactly what is written) is merged.
- `bash` output: cut it on a character boundary and keep both the start and the end.
  - Today `String::truncate` at byte 30,000 panics when that byte falls inside a multi-byte character (checked on a copy of the code). ✓ marks in test output can cause this.
  - `diagnostics.rs` cuts its output the same way, and its `cargo check` keeps running after Stop.
- Token usage:
  - send `TokenUsage` on every exit, not only on success;
  - Ollama's tool path stops discarding usage;
  - OpenAI-compatible streams send `stream_options.include_usage`, only to providers verified to accept it;
  - Composer shows usage;
  - `estimate_cost` moves to `llm/pricing.rs` and returns `Option<f64>`, because unknown cloud models are priced at zero today.
- Editor: an unmodified tab reloads when its file changes on disk. By reading `panels/editor.rs`, auto-save can write a stale tab over a change the agent made.
- Small models:
  - `-p` passes `settings.context_budget()`; the in-progress work on STATUS #20 adds this.
  - The CLI prompt's `edit_file` examples switch from `search`/`replace` to the real `old_text`/`new_text`. Small models copy these examples.

**Phase 1: Proven changes in Composer and `phazeai -p --prove`** (the MVP; about 10 working days, estimated).
- Ships:
  - the engine, the toggle and the card;
  - changed-file cards built from the ledger, replacing `git diff HEAD` (which misses new files and includes the user's own edits);
  - a banner while files are temporarily reverted;
  - recovery at startup;
  - `[proof]` settings;
  - `prove` cases in `approval.rs`, `oneshot.rs` and `tool_view.rs`.
- `--prove` prints the evidence to stderr and exits with code 3 when changed files have no Proven or UsesChange verdict.
- *Tests:*
  - parser tests on captured cargo, pytest and go output;
  - `tests/proof_engine.rs`, using temporary git repos, a shell-script test runner and `ScriptedLlm`. Cases: the Proven path; a test that still passes when the fix is reverted; a test inside the fixed file; a feature; a changed command; Stop during the revert (bytes exact, process group dead, journal empty); crash recovery; privacy refusals; a budget stop;
  - one real `cargo test`.
- *Measurement:* `tests/proof_eval.rs` (`#[ignore]`, run by a human with keys). It plants 10 bugs in Rust, Python and Go and runs on two cloud models, qwen2.5-coder 7B on Ollama, and the built-in 1.5B. Results go in the PR.

**Phase 2: Find the holes v1** (about 2.5 weeks, estimated).
- Ships: one agent at a time; jest and vitest parsers; `panels/holes.rs`. `--holes` exits with code 1 on a reproduced High finding. Without `--allow all` it lists leads only.
- A suspect counts as reproduced only when the probe:
  - compiles;
  - fails with an assertion or panic that prints its `PHAZEAI-REPRO <id>` marker;
  - calls a symbol defined in the suspect file (tree-sitter check).
  A failure located outside the suspect's lines is labelled "weak".
- Severity comes from a rubric shown in the panel: impact (proposed by the model), reachability (worked out by PhazeAI: public API or internal) and strength of the evidence.
- *Tests:*
  - a fixture repo with one planted panic and one decoy: the panic is reproduced, the decoy ends up a lead, the probes are removed, and the tree is unchanged;
  - the finder's tool set holds no tool that writes;
  - calls that are not ReadOnly are refused.

**Phase 3: everywhere.**
- Chat and TUI support.
- "Prove it" without a model: `--prove --mine`, and a button on the NO PROOF RAN card.
- A baseline run plus a run of related tests.
- Reverting only the changed parts of a file (hunk-aware revert).
- Mutation spot-checks for features.
- Parallel read-only finders, built new. The orchestrator removed in `0859aed` never applied an edit.
- Saved evidence and "Copy as PR description".
- *Tests:* the same rules as Phase 1. Chat first needs the streaming tests it does not have today.

## Honest limits, and what the card says

| Situation | The card says |
|---|---|
| The model never calls `prove` | NO PROOF RAN |
| A model known to handle tools badly (`supports_tools == false`, built-in 1.5B) | a warning; the sweep shows "signals, not findings" |
| Unknown test runner | "exit codes only", never Proven |
| 0 tests ran | "not evidence" |
| The test passes with the fix reverted | "doesn't detect the bug" |
| Test and fix in one file, no checkpoint | PassesOnly, with the reason |
| A file was edited during the check | "nothing overwritten", saved copy offered |
| Fix from a finding | "only the probe's inputs were tested" |
| Always | this machine only; each state ran once; full suite not run |

## Costs and controls

- **Limits:** `[proof]` (`ProofSettings`). Starting values are guesses to be measured: `max_iterations = 25`, `max_tokens_per_run = 300000`, `max_proofs_per_run = 4`, `run_timeout_secs = 600`, `max_test_secs_per_run = 1800`. `[holes]` has a token cap, `max_reproductions = 8` and `max_minutes = 30`.
- **Live status line:** `turn 6/25 · ≈38k in / 2.9k out (est.) · ≈$0.16 · tests 2m10s`.
  - "≈" marks an estimate (characters ÷ 4).
  - An unknown price shows tokens only.
  - Local models show "$0 · local".
- **At a cap:** the run stops before the next model call and answers any pending tool calls with "not run: budget reached". The card still shows.
- **Approval:**
  - `prove` is classed Execute. It asks wherever Execute asks today; "Auto-approve all" runs it without asking, as it runs `bash`.
  - The card names the command, the number of runs, the timeout and the files that will be reverted.
  - The CLI's "always allow" for `prove` is tied to the exact command, not to the tool name.
  - Sweeps never run under "Auto-approve all".
- **Privacy:**
  - `test_command` is checked with `privacy::command_denial`.
  - `prove` makes no model calls, and every model request still goes through `GuardedClient`.
  - Keeping private files out of test output is best effort, the same as for `bash` output.

## Risks

- Models follow the protocol unevenly. The Phase 1 eval measures this before anyone claims it works.
- A test can fail without the fix and still check the wrong thing. The user reviews every test, and test edits made after the failing run are shown.
- Probes run project code with the user's permissions. There is no sandbox (MISSION Principle 4: nothing extra to install).
- Swept code can contain instructions aimed at the model. Read-only finders and reviewed probes limit what that can do.
- cargo's build lock, which rust-analyzer also takes, eats into test timeouts. Prices are hard-coded and may be stale.

## Choices between the three designs

| Question | Chosen | Why |
|---|---|---|
| Who drives the proof | the model calls `prove`; PhazeAI runs it and gives the verdict (MVP) | fits today's agent loop; the architecture design's automatic check after each turn can come later |
| Who writes the test command | the model, run without a shell | commands built by PhazeAI need runner detection first |
| First place it ships | Composer and the CLI | chat asks before every call except writes |
| How many runs per state | once, and the card says so | three each (user design) triples the time on slow suites |
| `--allow test` for the CLI | not added | file writes plus test runs can run anything |
| Sweep | one agent at a time | cost; cargo's build lock makes probes run one at a time anyway |

## Decisions for you

1. **Temporary revert.** During a proof, PhazeAI briefly puts approved files back to older bytes. Each write is journaled and compare-and-swap, under the Execute approval. I don't think this loosens anything, but MISSION makes approval changes your call.
2. **Default.** Should Prove stay opt-in until the eval numbers exist?
3. **Sweeps running code.** Probes are reviewed and each command is approved. Is that enough, or do we want a sandbox? Bubblewrap or landlock would mean a new runtime or dependency.
4. **Read-only calls without asking in sweeps.** Composer's "Approve destructive" mode already allows this; chat asks before every read. OK for sweeps?
5. **Parallel finders and network analyzers** (`cargo audit`, `npm audit`). Both stay off in v1?
6. **Pricing.** A proof or a sweep costs as much as several plain runs (unmeasured), on the user's own key, and needs no PhazeAI server.
   - Should we ask again when the estimate is above some amount on a paid model?
   - Whether any of this is ever paid depends on the undecided cloud tier.
7. **Claims and names.** Nothing public before the eval, and no comparison with other editors without sources. Call it "Find the holes" or "Findings"?
8. **Storage.** Where evidence and findings are kept, and whether they are committed.