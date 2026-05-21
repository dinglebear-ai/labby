# PR #40 Main Delta Report

Generated: 2026-05-04

## Scope

This report answers: what does `origin/main` have that the current PR branch does not?

Compared refs:

| Ref | SHA |
| --- | --- |
| local `HEAD` | `6a596eef` |
| remote PR branch | `a50e7e2e` |
| `origin/main` | `52c48d4c` |
| merge base with main | `fd8a49ac` |

Important local caveat: the working tree is dirty and the local branch is `ahead 1, behind 3` relative to `origin/bd-work/mcp-gateway-review-remediation`. The tree-level comparisons below use committed refs only, not uncommitted local files.

## Executive Finding

At the commit-patch level, `origin/main` does not appear to have any unique non-merge patches missing from this branch:

```bash
git log --right-only --cherry-pick --no-merges HEAD...origin/main | wc -l
# 0

git log --right-only --cherry-pick --no-merges origin/bd-work/mcp-gateway-review-remediation...origin/main | wc -l
# 0
```

Without patch-equivalence filtering, `main` has many commits not in the branch by hash:

```bash
git log --right-only --no-merges HEAD...origin/main | wc -l
# 333
```

Interpretation: this branch appears to contain patch-equivalent copies of `main` work under different commit hashes, plus additional branch-only changes that moved/deleted/reorganized the tree after those changes landed. So "cherry-pick main into this branch" is unlikely to be the clean fix. Git already sees the main-side patches as duplicated; the breakage is tree divergence on top.

## Tree Delta Summary

To make local `HEAD` look exactly like `origin/main`:

```bash
git diff --shortstat HEAD..origin/main
# 16173 files changed, 5679 insertions(+), 11677633 deletions(-)
```

To make the remote PR branch look exactly like `origin/main`:

```bash
git diff --shortstat origin/bd-work/mcp-gateway-review-remediation..origin/main
# 16173 files changed, 5682 insertions(+), 11677366 deletions(-)
```

Status counts from `HEAD..origin/main`:

| Status | Count | Meaning |
| --- | ---: | --- |
| `A` | 15 | files present on `main` but absent from branch |
| `M` | 297 | files present in both with different content |
| `R` | 74 | paths Git sees as renamed between branch and main |
| `D` | 15787 | files present on branch but absent from `main` |

The enormous number is not because `main` added 16k files. It is mostly because the branch contains large branch-only trees that `main` does not have.

Top changed path buckets:

| Bucket | Path Count |
| --- | ---: |
| `docs` | 14707 |
| `plugins` | 922 |
| `crates` | 385 |
| `apps` | 131 |
| `config` | 4 |
| `.github` | 3 |
| `scripts` | 2 |

## Files Main Has That The Branch Lacks

These are the direct `A` entries from `git diff --name-status HEAD..origin/main`:

```text
apps/gateway-admin/app/(admin)/setup/page.tsx
docs/CICD.md
docs/DEPLOY.md
docs/DEVICE_RUNTIME.md
docs/FLEET_LOGS.md
plugins/.claude-plugin/plugin.json
plugins/.mcp.json
plugins/bin/AGENTS.md
plugins/bin/CLAUDE.md
plugins/bin/GEMINI.md
plugins/commands/quick-push.md
plugins/commands/save-to-md.md
plugins/monitors/monitors.json
plugins/skills/gh-address-comments/scripts/__pycache__/_bd_utils.cpython-314.pyc
plugins/skills/gh-address-comments/scripts/__pycache__/fetch_comments.cpython-314.pyc
```

The `.pyc` files should probably not be treated as meaningful code to port.

## Main Path Layout Differences

Main has several important path-layout choices that differ from the PR branch:

```text
plugins/acp/.mcp.json -> .mcp.json
config/config.example.toml -> config.example.toml
scripts/health-check -> plugins/bin/health-check
scripts/link-claude-mds -> plugins/bin/link-claude-mds
plugins/vibin/skills/gh-address-comments/... -> plugins/skills/gh-address-comments/...
plugins/lab/skills/lab-service-onboarding/... -> plugins/skills/lab-service-onboarding/...
plugins/lab/skills/using-lab-cli/... -> plugins/skills/using-lab-cli/...
```

Docs also moved in the opposite direction from the PR branch's newer organization. Examples:

```text
docs/surfaces/CLI.md -> docs/CLI.md
docs/runtime/CONFIG.md -> docs/CONFIG.md
docs/runtime/ENV.md -> docs/ENV.md
docs/dev/ERRORS.md -> docs/ERRORS.md
docs/services/GATEWAY.md -> docs/GATEWAY.md
docs/surfaces/MCP.md -> docs/MCP.md
docs/dev/OBSERVABILITY.md -> docs/OBSERVABILITY.md
docs/dev/TESTING.md -> docs/TESTING.md
```

This is a major reason the merge conflict set is ugly: both sides reorganized the same docs/plugin surfaces, but not to the same final paths.

## Modified Areas On Main Relative To Branch

The 297 modified files cluster here:

| Area | Modified File Count |
| --- | ---: |
| `crates/lab/src` | 122 |
| `apps/gateway-admin/components` | 52 |
| `apps/gateway-admin/lib` | 27 |
| `docs` | 25 |
| `crates/lab/tests` | 25 |
| `crates/lab-apis/src` | 14 |
| `crates/lab-auth/src` | 3 |
| `.github` | 3 |

Representative files:

```text
.github/workflows/ci.yml
.github/workflows/release.yml
Cargo.lock
Cargo.toml
README.md
apps/gateway-admin/lib/server/gateway-adapter.ts
apps/gateway-admin/lib/chat/session-events.ts
apps/gateway-admin/lib/dashboard/logs-console-state.ts
crates/lab/src/api/nodes/fleet.rs
crates/lab/src/dispatch/stash/catalog.rs
crates/lab/src/cli/serve.rs
crates/lab/src/mcp/server.rs
crates/lab/src/registry.rs
docs/README.md
docs/CHANGELOG.md
```

## What This Means For The Fix Strategy

There is no useful "main commit set" to cherry-pick into the current branch. Patch-equivalence says main's non-merge work is already represented somewhere on the branch. The problem is that the branch then continued with a different final tree layout and large branch-only content.

The practical recovery strategy should be:

1. Use `origin/main` as the base.
2. Identify branch-only changes that are still valuable.
3. Cherry-pick or manually port those branch-only changes onto a clean branch.
4. Force-push the cleaned branch over PR #40 only after verification.

Do not try to resolve the current merge directly. That means accepting a huge branch-only tree deletion and reconciling hundreds of duplicate/reorganized files.

## Commands Used

```bash
git status --short --branch
git rev-parse --short HEAD origin/main origin/bd-work/mcp-gateway-review-remediation
git merge-base HEAD origin/main
git merge-base origin/bd-work/mcp-gateway-review-remediation origin/main
git log --right-only --cherry-pick --no-merges HEAD...origin/main
git log --right-only --no-merges HEAD...origin/main
git diff --shortstat HEAD..origin/main
git diff --shortstat origin/bd-work/mcp-gateway-review-remediation..origin/main
git diff --name-status --find-renames HEAD..origin/main
git diff --name-only HEAD..origin/main
```
