# 2026-05-07 Branch and Worktree Cleanup

## Context

- Repo: `/home/jmagar/workspace/lab`
- Current checkout: `main`
- Current HEAD: `1a9d9369`
- Remote: `origin git@github.com:jmagar/lab.git`
- Request: investigate open branches/worktrees, then clean up what was safe.

## Investigation Summary

Registered worktrees after cleanup:

- `/home/jmagar/workspace/lab` on `main` at `1a9d9369`
- `/home/jmagar/workspace/lab/.claude/worktrees/oauth-integration` on `worktree-lab-auth-extract` at `5dfed192`
- `/home/jmagar/workspace/lab/.worktrees/bd-work/registry-review-fixes` on `bd-work/registry-review-fixes` at `06d21217`

Open local branches after cleanup:

- `main` tracks `origin/main` at `1a9d9369`
- `bd-work/registry-review-fixes` tracks `origin/bd-work/registry-review-fixes` at `06d21217`
- `worktree-lab-auth-extract` is clean but unmerged, with auth extraction commits
- `backup/local-main-48448d4c-20260504T220219Z` is divergent backup/archive history and was left intact

Open remote refs after cleanup:

- `origin/main` at `1a9d9369`
- `origin/bd-work/registry-review-fixes` at `06d21217`
- `origin/backup/pr40-main-20260504T211942Z` at `52c48d4c`

GitHub PR check returned no open PRs at the time of investigation.

## Safe Cleanup Performed

Deleted local merged branch:

- `optimize-mobile-chat`

Deleted remote refs that were verified merged into `origin/main`:

- `origin/work/chat-adapter-model-switching`
- `origin/work/chat-file-attachments`
- `origin/work/chat-interaction-timestamps`
- `origin/work/chat-message-actions`
- `origin/work/chat-mobile-sticky-header`
- `origin/work/chat-prompt-input-scroll`
- `origin/work/chat-session-persistence`
- `origin/work/chat-switch-agents`
- `origin/backup/pr40-head-20260504T211942Z`
- `origin/backup/pr40-intended-main-20260504T215025Z`

## Intentionally Left Alone

- `worktree-lab-auth-extract`: unmerged auth extraction work.
- `bd-work/registry-review-fixes`: active branch/worktree with matching remote.
- `backup/local-main-48448d4c-20260504T220219Z`: divergent local backup ref.
- `origin/backup/pr40-main-20260504T211942Z`: divergent remote backup ref.

## Verification

- Refreshed remote refs with `git fetch --all --prune`.
- Verified merged state before deleting branches.
- Confirmed final `main` status is clean: `## main...origin/main`.
- Confirmed registered worktrees remain present with `git worktree list --porcelain`.
- Confirmed `bd-work/registry-review-fixes` worktree is clean and aligned with `origin/bd-work/registry-review-fixes`.

## Open Questions

- Whether to keep or delete the divergent PR40 backup refs requires an explicit archive/retention decision.
- Whether `worktree-lab-auth-extract` should be pushed, opened as a PR, rebased, or cleaned up remains unresolved.
