# GitHub Actions PR Review Workflows Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add two automated PR workflows: one that flags stale docs based on code changes, one that flags code violating project conventions.

**Architecture:** Both workflows use `anthropics/claude-code-action@v1` in automation mode (prompt-driven, no `@claude` trigger). They run on every PR open/sync against `main`, check out the repo, and post a single PR comment with findings. Claude reads the diff and relevant docs, then reports concretely.

**Tech Stack:** GitHub Actions, `anthropics/claude-code-action@v1`, `ANTHROPIC_API_KEY` repository secret.

---

## File Map

| Path | Action |
|------|--------|
| `.github/workflows/doc-freshness.yml` | Create — doc freshness workflow |
| `.github/workflows/code-conventions.yml` | Create — code conventions workflow |
| `docs/CICD.md` | Modify — document the two new jobs |
| `.github/CLAUDE.md` | Modify — add rows to the workflow table |

---

### Task 1: Create doc-freshness workflow

**Files:**
- Create: `.github/workflows/doc-freshness.yml`

- [ ] **Step 1: Create the workflow file**

```yaml
name: Doc Freshness

on:
  pull_request:
    branches: [main]
    types: [opened, synchronize, reopened]

concurrency:
  group: doc-freshness-${{ github.ref }}
  cancel-in-progress: true

permissions:
  contents: read
  pull-requests: write

jobs:
  doc-freshness:
    name: Check Docs
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - uses: anthropics/claude-code-action@v1
        with:
          anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
          claude_args: "--max-turns 15"
          prompt: |
            You are a documentation freshness checker for the `lab` Rust project.

            Your job: analyze the files changed in this PR and identify which documentation
            files in `docs/` or any `CLAUDE.md` files may be stale or need updating as a
            result of the code changes.

            Steps:
            1. Run `git diff origin/main...HEAD --name-only` to see which files changed.
            2. Run `git diff origin/main...HEAD` to see what changed and how.
            3. Read the docs that are likely affected. Key docs:
               - `docs/CONVENTIONS.md` — module/async/HTTP/error rules
               - `docs/OBSERVABILITY.md` — logging/tracing contract
               - `docs/ERRORS.md` — error kinds and envelopes
               - `docs/DISPATCH.md` — dispatch ownership
               - `docs/SERIALIZATION.md` — serde and output boundary rules
               - `docs/CICD.md` — CI/CD contract
               - `CLAUDE.md` (root) — development instructions
               - Any `CLAUDE.md` under subdirectories near the changed files
            4. For each doc that needs updating: name the file, the section heading,
               and specifically what is now inaccurate or missing.
            5. Post a single PR comment summarising your findings.
               - If docs need updating: list them with concrete descriptions.
               - If nothing needs updating: say "No documentation updates needed."

            Be precise. Only flag docs genuinely affected by changes in this PR.
            Do not suggest stylistic improvements — only flag factual drift.
```

- [ ] **Step 2: Verify YAML syntax**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/doc-freshness.yml'))" && echo "valid"
```

Expected: `valid`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/doc-freshness.yml
git commit -m "ci: add doc-freshness workflow to flag stale docs on PRs"
```

---

### Task 2: Create code-conventions workflow

**Files:**
- Create: `.github/workflows/code-conventions.yml`

- [ ] **Step 1: Create the workflow file**

```yaml
name: Code Conventions

on:
  pull_request:
    branches: [main]
    types: [opened, synchronize, reopened]

concurrency:
  group: code-conventions-${{ github.ref }}
  cancel-in-progress: true

permissions:
  contents: read
  pull-requests: write

jobs:
  code-conventions:
    name: Check Conventions
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0

      - uses: anthropics/claude-code-action@v1
        with:
          anthropic_api_key: ${{ secrets.ANTHROPIC_API_KEY }}
          claude_args: "--max-turns 15"
          prompt: |
            You are a code conventions checker for the `lab` Rust project.

            Your job: review files changed in this PR against the project's documented
            conventions and report concrete violations.

            Steps:
            1. Run `git diff origin/main...HEAD --name-only` to see which files changed.
            2. Run `git diff origin/main...HEAD` to see the actual changes.
            3. Read all of these authoritative docs — they contain locked rules, not suggestions:
               - `CLAUDE.md` (root) — Golden Rule, module structure, auth, config, error, logging
               - `docs/CONVENTIONS.md` — module style, async trait, HTTP client, error taxonomy,
                 action metadata, batch ops
               - `docs/OBSERVABILITY.md` — mandatory dispatch fields, level conventions, secret
                 redaction, per-surface requirements
               - `docs/ERRORS.md` — stable `kind` tags, MCP/HTTP envelope shapes, status mapping
               - `docs/DISPATCH.md` — dispatch ownership and adapter direction
               - `docs/SERIALIZATION.md` — serde ownership, stable envelopes, output boundaries
               - Any `CLAUDE.md` in subdirectories under paths that were changed
            4. For each violation found:
               - File path and line number
               - The rule being violated (quote it from the doc)
               - What the code does instead
               - What it should do
            5. Post a single PR comment with your findings.
               - If violations exist: list them with the detail above.
               - If no violations: say "No convention violations found."

            Only report real violations — rules explicitly stated in the docs above.
            Do not report clippy/fmt issues (CI already catches those).
            Do not report speculative improvements or style preferences.
            Focus on structural rules: mod.rs usage, async-trait, clap/rmcp in lab-apis,
            business logic in CLI/MCP shims, missing observability fields, wrong error kinds,
            hardcoded auth, println! for debug output, etc.
```

- [ ] **Step 2: Verify YAML syntax**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/code-conventions.yml'))" && echo "valid"
```

Expected: `valid`

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/code-conventions.yml
git commit -m "ci: add code-conventions workflow to flag rule violations on PRs"
```

---

### Task 3: Update CICD.md

**Files:**
- Modify: `docs/CICD.md`

- [ ] **Step 1: Read the current CI Checks section**

```bash
grep -n "CI Checks" docs/CICD.md
```

- [ ] **Step 2: Add the two new jobs to the CI Checks table**

Find the CI Checks table (starts after `## CI Checks`). It currently lists Compile, Format, Lint, Deny, Tests, Docs. Append two rows:

```markdown
| Doc Freshness | Claude Code action — flags stale docs based on PR diff |
| Code Conventions | Claude Code action — flags convention violations in changed code |
```

The full table should look like:

```markdown
| Check | Command |
|-------|---------|
| Compile | `cargo check --workspace --all-features` |
| Format | `cargo fmt --all -- --check` |
| Lint | `cargo clippy --workspace --all-features -- -D warnings` |
| Deny | `cargo deny check` |
| Tests | `cargo nextest run --workspace --all-features` |
| Docs | `cargo doc --no-deps --all-features` (must be warning-free) |
| Doc Freshness | Claude Code action — flags stale docs based on PR diff |
| Code Conventions | Claude Code action — flags convention violations in changed code |
```

- [ ] **Step 3: Add a new section at the end of CICD.md for Claude-powered checks**

Append before `## Non-Goals`:

```markdown
## Claude-Powered PR Checks

Two additional jobs run on every PR against `main`. They use `anthropics/claude-code-action@v1`
in automation mode (prompt-driven, no `@claude` trigger required).

Both require `ANTHROPIC_API_KEY` set as a repository secret.

### Doc Freshness (`doc-freshness.yml`)

Compares the PR diff against `docs/` and per-directory `CLAUDE.md` files.
Posts a single comment listing which docs are stale and what changed.
Reports "No documentation updates needed" when nothing is affected.

### Code Conventions (`code-conventions.yml`)

Compares the PR diff against the locked rules in `docs/CONVENTIONS.md`,
`docs/OBSERVABILITY.md`, `docs/ERRORS.md`, `docs/DISPATCH.md`,
`docs/SERIALIZATION.md`, `CLAUDE.md`, and any nearby `CLAUDE.md` files.
Reports violations with file path, line number, rule citation, and fix guidance.
Reports "No convention violations found" when the diff is clean.

Both workflows use `concurrency` groups to cancel stale runs when new commits
are pushed to the same PR branch.
```

- [ ] **Step 4: Commit**

```bash
git add docs/CICD.md
git commit -m "docs: document doc-freshness and code-conventions CI jobs"
```

---

### Task 4: Update .github/CLAUDE.md

**Files:**
- Modify: `.github/CLAUDE.md`

- [ ] **Step 1: Add the two new workflows to the Workflows table**

Find the table:

```markdown
| File | Trigger | Purpose |
|------|---------|---------|
| `workflows/ci.yml` | push/PR to `main` | Fast correctness checks |
| `workflows/release.yml` | push of `v*` tag | Release builds + GitHub Release |
```

Replace with:

```markdown
| File | Trigger | Purpose |
|------|---------|---------|
| `workflows/ci.yml` | push/PR to `main` | Fast correctness checks |
| `workflows/release.yml` | push of `v*` tag | Release builds + GitHub Release |
| `workflows/doc-freshness.yml` | PR to `main` | Flag stale docs based on diff |
| `workflows/code-conventions.yml` | PR to `main` | Flag convention violations in changed code |
```

- [ ] **Step 2: Add a note about the required secret**

After the Workflows table, add:

```markdown
## Claude-Powered Checks

`doc-freshness.yml` and `code-conventions.yml` require `ANTHROPIC_API_KEY` set as a
repository secret. Both post a single PR comment with findings.

See `docs/CICD.md` for the full contract and prompt rationale.
```

- [ ] **Step 3: Commit**

```bash
git add .github/CLAUDE.md
git commit -m "docs: add doc-freshness and code-conventions to .github/CLAUDE.md"
```

---

### Task 5: Verify end-to-end

- [ ] **Step 1: Confirm all four files exist and are syntactically valid**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/doc-freshness.yml'))" && echo "doc-freshness: valid"
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/code-conventions.yml'))" && echo "code-conventions: valid"
grep -c "Doc Freshness" docs/CICD.md && echo "CICD.md: updated"
grep -c "doc-freshness" .github/CLAUDE.md && echo ".github/CLAUDE.md: updated"
```

Expected: four lines each confirming valid/updated.

- [ ] **Step 2: Confirm `ANTHROPIC_API_KEY` is set in the repo**

This is a manual step. Navigate to: `Settings → Secrets and variables → Actions` and confirm `ANTHROPIC_API_KEY` exists. If it doesn't, add it before opening a test PR.

- [ ] **Step 3: Open a test PR to verify the workflows run**

Push this branch and open a PR against `main`. In the PR's "Checks" tab, you should see:
- `Doc Freshness / Check Docs` — runs and posts a comment
- `Code Conventions / Check Conventions` — runs and posts a comment

Both should post a comment even if findings are empty ("No ... needed" / "No ... found").

- [ ] **Step 4: Confirm comment content is useful**

Read the posted comments. They should:
- Reference specific files/line numbers when violations or stale docs are found
- Not be empty or generic
- Finish in under 5 minutes per workflow
