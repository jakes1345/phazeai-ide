# Coding tasks for `phazeai eval`

Each folder is one task: `task.toml` holds the prompt and the command that
grades it, `repo/` is the code the agent starts from, `check/` holds hidden
tests that are copied in only after the agent finishes, and `solution/` is a
known answer.

```bash
phazeai eval --verify                          # every task fails as given and passes with its solution
phazeai eval                                   # Phaze with your configured provider
phazeai eval --provider grok                    # Phaze on another model
phazeai eval --agent claude-code               # Claude Code, for comparison
phazeai eval --agent 'my-agent "$PHAZE_EVAL_PROMPT"' --runs 3 --out results.jsonl
```

The tasks need `git`, `python3`, `node` and `cargo`.
