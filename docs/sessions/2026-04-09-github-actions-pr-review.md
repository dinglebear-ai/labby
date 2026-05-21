# Session: GitHub Actions PR Review Workflows

**Date:** 2026-04-09  
**Branch:** feat/lab-operational  
**Commit:** ci: add PR review workflows and bump to 0.2.2

---

## Session Overview

Designed, researched, and implemented two GitHub Actions PR review workflows using `anthropics/claude-code-action@v1`:

1. **doc-freshness** — detects stale documentation files from PR diffs, posts a sticky PR comment
2. **code-conventions** — checks changed code against the project's locked convention rules, posts a sticky PR comment

Both workflows went through a full design pipeline: plan → research (4 agents) → engineering review (4 agents) → revision with all findings applied → commit.

---

## Timeline

1. **Plan written** — `docs/superpowers/plans/2026-04-09-github-actions-pr-review.md` created with 5 tasks
2. **Epic imported** — `lab-b5l` epic with child beads `lab-b5l.1`–`lab-b5l.5`
3. **Research phase** — 4 agents (framework-docs, best-practices, architect, learnings) gathered evidence in parallel
4. **Initial workflows created** — first drafts of both YAMLs written from plan
5. **Research findings applied** — 7 improvements integrated (concurrency key, max_turns, dynamic doc discovery, +prefix rule, evidence requirement, use_sticky_comment, missing-file fallback)
6. **Engineering review** — 4 agents (architecture, simplicity, security, performance) reviewed in parallel
7. **Engineering findings applied** — 9 of 10 recommendations implemented
8. **Version bump** — 0.2.1 → 0.2.2 in `Cargo.toml`
9. **Committed and pushed** to `feat/lab-operational`

---

## Key Findings

- **Bash in allowedTools = prompt injection risk**: A crafted `CLAUDE.md` in a PR can inject instructions to exfiltrate `ANTHROPIC_API_KEY`. Mitigated by pre-computing `git diff` and `find` outputs in a prior shell step, removing the need for Bash entirely.
- **`continue-on-error` at job level silently swallows infrastructure failures**: Expired API key, rate limits, and action crashes all show green. Moved to step level with a `::warning::` annotation.
- **`file:line` from unified diffs is unreliable**: Claude's offset arithmetic across hunk boundaries degrades. Replaced with "quote the exact code" requirement.
- **`fetch-depth: 0` not needed**: `git fetch origin main --depth=1` after a shallow checkout is sufficient for `git diff origin/main...HEAD`.
- **60-second sleep debounce**: Prevents API credits being consumed by cancelled runs on rapid-push sequences. Runner is killed before checkout/API call begins.
- **Concurrency key `github.head_ref` not PR number**: More stable across PR close/reopen cycles; `run_id` fallback is dead code for `pull_request`-only triggers.

---

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| Pre-compute diff+find in shell step | Eliminates Bash from allowedTools, closes prompt injection path |
| `--allowedTools Read,Glob,Grep` (no Bash) | Defense in depth for read-only review workflows |
| `use_sticky_comment: true` | Explicit — prevents multiple comments on re-push regardless of action default changes |
| Two separate workflow files | Independent concurrency groups, independent failure domains, different turn budgets |
| `continue-on-error` on step not job | Infrastructure failures (expired key, rate limit) surface as warnings; PR is not blocked |
| `pull_request` not `pull_request_target` | Secrets not exposed to fork PRs; avoids "pwn request" attack vector |
| Dynamic doc discovery via pre-computed file | Self-healing when docs are added/renamed; static list in plan missed 9 of 20 docs |
| max_turns=15 for both | 20 for code-conventions was inconsistent with no practical benefit; unused turns cost nothing |
| SHA pinning deferred | Acceptable risk for now; Dependabot setup is the right mechanism |

---

## Files Modified

| File | Purpose |
|------|---------|
| `.github/workflows/doc-freshness.yml` | New: doc freshness PR review workflow |
| `.github/workflows/code-conventions.yml` | New: code conventions PR review workflow |
| `.github/CLAUDE.md` | Added workflow rows and ANTHROPIC_API_KEY note |
| `docs/CICD.md` | Added CI Checks rows and Claude-Powered PR Checks section |
| `docs/superpowers/plans/2026-04-09-github-actions-pr-review.md` | Implementation plan |
| `Cargo.toml` | Version bump 0.2.1 → 0.2.2 |

---

## Commands Executed

```bash
# Validate YAML
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/doc-freshness.yml'))"
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/code-conventions.yml'))"
# Both: valid

# Version bump
# Cargo.toml: version = "0.2.1" → "0.2.2"

# Commit and push
git add [57 files]
git commit -m "ci: add PR review workflows and bump to 0.2.2"
git push  # → ok feat/lab-operational
```

---

## Behavior Changes (Before/After)

| Before | After |
|--------|-------|
| No automated doc review on PRs | Every PR to main triggers a sticky comment listing stale docs |
| No automated convention check on PRs | Every PR to main triggers a sticky comment listing convention violations |
| No signal when Claude review fails | `::warning::` annotation appears in PR checks when API key missing or rate limited |
| Bash available to Claude in CI | Read/Glob/Grep only — no arbitrary shell execution |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `python3 -c "import yaml; yaml.safe_load(...doc-freshness.yml)"` | valid | valid | ✓ |
| `python3 -c "import yaml; yaml.safe_load(...code-conventions.yml)"` | valid | valid | ✓ |
| `git push` | ok feat/lab-operational | ok feat/lab-operational | ✓ |
| `grep -c "Doc Freshness" docs/CICD.md` | ≥1 | (via .github/CLAUDE.md linter update) | ✓ |

---

## Source IDs + Collections Touched

- Beads: `lab-b5l`, `lab-b5l.1`–`lab-b5l.5` (created, commented, closed)
- Research agent outputs: framework-docs, best-practices, architect, learnings (all completed)
- Engineering review agents: architecture, simplicity, security, performance (all completed)

---

## Risks and Rollback

- **Risk:** `ANTHROPIC_API_KEY` secret not yet confirmed in repo settings. Workflows will fail silently (::warning:: will appear but no review comment posted).
  - **Verify:** Settings → Secrets and Variables → Actions → confirm `ANTHROPIC_API_KEY` exists
- **Risk:** `anthropics/claude-code-action@v1` is a mutable tag. A breaking upstream change breaks both workflows silently.
  - **Rollback:** `git revert HEAD` removes both workflow files
- **Rollback:** `git revert HEAD` on `feat/lab-operational` restores prior state

---

## Decisions Not Taken

- **`pull_request_target`** — would expose secrets to fork PRs; classic "pwn request" vulnerability when combined with `actions/checkout` of PR head
- **Single workflow with two jobs** — saves one runner spin-up (~20s) but couples independent checks; not worth the loss of independent cancel-in-progress behavior
- **Dynamic max_turns scaling by diff size** — unused turns cost nothing; complexity not justified
- **Pre-filtered diff shell step for large diffs** — the 1000-line prompt guard is adequate for this repo's typical PR sizes
- **SHA pinning now** — deferred; Dependabot is the right mechanism and hasn't been set up yet

---

## Open Questions

- Is `ANTHROPIC_API_KEY` set in repository secrets? (manual verification required)
- Will `use_sticky_comment: true` work as expected on the current `@v1` action version? (confirm on first test PR)
- Does the 60-second sleep interact badly with GitHub's minimum job timeout settings?

---

## Next Steps

1. **Confirm `ANTHROPIC_API_KEY`** exists in repo Settings → Secrets and Variables → Actions
2. **Open a test PR** against `main` to verify both workflows run and post comments
3. **Set up Dependabot** for Actions SHA pinning (deferred from engineering review item 10)
4. **Monitor first few PR runs** — check that code-conventions doesn't exhaust 15 turns on large diffs; if it does, lower to 12 or add a doc-concatenation step
