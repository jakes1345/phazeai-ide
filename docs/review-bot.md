# The review bot: `phazeai review`

`phazeai review` reads a change the way a careful reviewer would. It reports only real problems, such as bugs, crashes, security holes, data loss, races and broken error handling. Each finding points at a line the change shows.

```sh
phazeai review                    # this branch's commits vs the default branch (what a PR shows)
phazeai review --base release/2.1 # vs another base
phazeai review --uncommitted      # work in progress (new files once `git add`ed)
phazeai review --staged           # only what is staged
phazeai review --focus "security" # look harder at one thing
phazeai review --output-format json
phazeai review --fail-on high     # exit 1 when a high-severity finding exists (for CI)
```

## What it sends, and what it costs

- **What goes in a request:** each changed file's hunks with 3 lines of context and their new line numbers. It never sends whole files, any tools, or a long system prompt.
- **How requests are sized:** they are batched to about 6k tokens each, and up to 3 run at once.
- **What is left out:**
  - lock files and generated or minified files
  - binaries and deleted files
  - any file the project's privacy policy (`.phazeai/privacy.toml`) denies
- **What the summary shows:** every review ends with a line giving the tokens used and the cost, when the model's price is known.
- **How findings are checked:**
  - A finding on a file the model was not shown is dropped.
  - A finding on a line the diff does not show moves to the nearest shown line within 3 lines; if there is none, it is dropped.

  This keeps comments on lines that are actually in the diff.

The model is your configured one, or the one you pass with `--model` and `--provider`. That includes a local model, so a review can run fully offline.

## On a GitHub pull request

`--github-pr N` posts the findings to pull request N as one review, with an inline comment per finding. It needs `GITHUB_TOKEN` (or `GH_TOKEN`) with write access to pull requests. The repository comes from `--github-repo owner/name`, from `GITHUB_REPOSITORY` (set by GitHub Actions) or from the `origin` remote. `GITHUB_API_URL` points it at GitHub Enterprise.

A workflow that reviews every pull request could look like this. It is not added to this repository's own CI:

```yaml
name: PhazeAI review
on: pull_request
permissions:
  contents: read
  pull-requests: write
jobs:
  review:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - run: cargo install --git https://github.com/jakes1345/phazeai-ide phazeai-cli
      - run: phazeai review --provider anthropic --base origin/${{ github.base_ref }} --github-pr ${{ github.event.pull_request.number }}
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
```
