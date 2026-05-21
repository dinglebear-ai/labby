# Branch Cleanup Audit - 2026-05-04

Current canonical branch:

- `main` / `origin/main`: `6ef4c1a7`
- Working tree status before cleanup: clean, `main...origin/main`

## Summary

All non-backup local and remote feature branches reviewed here are stale after the PR #40 recovery and main promotion. No missing feature work was identified that needs to be ported from those branches into current `main`.

Backup refs were intentionally kept:

- `backup/local-main-48448d4c-20260504T220219Z`
- `origin/backup/pr40-main-20260504T211942Z`
- `origin/backup/pr40-head-20260504T211942Z`
- `origin/backup/pr40-intended-main-20260504T215025Z`

## Evidence Checked

Fresh current-main file evidence exists for the feature surfaces that appeared unique on stale branches:

- Command palette:
  - `apps/gateway-admin/components/app-command-palette.tsx`
  - `apps/gateway-admin/lib/app-command-palette.ts`
- Beads service and UI:
  - `crates/lab-apis/src/beads/client.rs`
  - `crates/lab/src/dispatch/beads/dispatch.rs`
  - `apps/gateway-admin/app/(admin)/tasks/page.tsx`
  - `apps/gateway-admin/lib/api/beads-client.ts`
- Floating chat and page context:
  - `apps/gateway-admin/components/floating-chat-shell.tsx`
  - `crates/lab/src/dispatch/acp/page_context.rs`
- Marketplace v2:
  - `apps/gateway-admin/app/dev/marketplace/page.tsx`
  - `apps/gateway-admin/components/marketplace/marketplace-state.ts`
  - `crates/lab/src/dispatch/marketplace/dispatch.rs`
- OAuth email allowlist and admin UI:
  - `crates/lab-auth/src/authorize.rs`
  - `crates/lab/src/api/services/auth_admin.rs`
  - `apps/gateway-admin/components/allowed-users-panel.tsx`
  - `apps/gateway-admin/lib/api/auth-admin-client.ts`

GitHub PR inventory showed no open PRs. The branch heads below correspond to merged PRs or stale worktree refs.

## Branch Classification

Delete local refs:

- `bd-work/mcp-gateway-review-remediation` - local ref equals current main.
- `feat/acp-beads` - PR #34 merged; current main has ACP/Beads surfaces.
- `feat/beads-webui` - PR #36 merged into the command-palette stack; current main has Beads UI and service surfaces.
- `feat/lab-aid2.1` - PR #39 merged; no missing patch-unique work needed.
- `feat/lab-gych` - PR #38 merged; current main has floating chat and page-context surfaces.
- `feat/marketplace-v2-setup` - PR #32 merged; current main has marketplace v2 surfaces.
- `feat/oauth-email-allowlist` - PR #33 merged; current main has email allowlist and auth-admin surfaces.
- `feat/stash-implementation` - PR #35 merged; no missing patch-unique work needed.
- `worktree-agent-ac9c6933` - no patch-unique commits versus current main.

Delete remote refs:

- `origin/bd-work/mcp-gateway-review-remediation`
- `origin/feat/acp-beads`
- `origin/feat/beads-webui`
- `origin/feat/gateway-admin-command-palette`
- `origin/feat/lab-aid2.1`
- `origin/feat/lab-gych`
- `origin/feat/marketplace-v2-setup`
- `origin/feat/oauth-email-allowlist`
- `origin/feat/stash-implementation`
- `origin/worktree-agent-ac9c6933`

Keep backup refs:

- `backup/local-main-48448d4c-20260504T220219Z`
- `origin/backup/pr40-main-20260504T211942Z`
- `origin/backup/pr40-head-20260504T211942Z`
- `origin/backup/pr40-intended-main-20260504T215025Z`

## Remaining Issue

The `upstream` remote is misconfigured as `jmagar/lab`, so `git fetch --all --prune` fails on that remote after successfully updating `origin`. This is remote configuration drift, not missing branch work.
