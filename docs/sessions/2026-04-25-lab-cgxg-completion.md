---
date: "2026-04-25 11:54:16 EST"
repo: "git@github.com:jmagar/lab.git"
branch: "bd-security/marketplace-p1-fixes"
head: "f168964b"
plan: "docs/superpowers/plans/2026-04-25-lab-cgxg-completion.md"
agent: "Codex Worker for bead lab-cgxg"
working_directory: "/home/jmagar/workspace/lab"
worktree: "/home/jmagar/workspace/lab                                   f168964b [bd-security/marketplace-p1-fixes]"
pr: "#29 fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation https://github.com/jmagar/lab/pull/29"
---

# lab-cgxg Completion Session Report

## User Request

Complete bead `lab-cgxg` in `/home/jmagar/workspace/lab`: normalize MCP service registration so normal services register directly from `crate::dispatch::<service>`, remove stale `crates/lab/src/mcp/services/` wrappers, preserve tests, verify the result, and write this session report.

## Required Context

### `TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'`

```text
2026-04-25 11:54:16 EST
```

### `git remote get-url origin`

```text
git@github.com:jmagar/lab.git
```

### `git branch --show-current`

```text
bd-security/marketplace-p1-fixes
```

### `git rev-parse --short HEAD`

```text
f168964b
```

### `git log --oneline -5`

```text
f168964b fix(lab-zxx5.32): R2 P3 roll-up — redact_home in errors, log tiering, sync_all, dead Sized
39266dce refactor(lab-f1t2): address simplify + review findings on the f1t2 wave
b7f488af fix(lab-zxx5.30,lab-zxx5.31): partial-extraction detection + fail-closed walk
7b051062 fix(lab-zxx5.29): validate node install result shape
12eb0ea0 fix(lab-zxx5.28): typed error markers restore install taxonomy
```

### `git status --short`

```text
 M Justfile
 M apps/gateway-admin/app/(admin)/activity/page.tsx
 M apps/gateway-admin/components/chat/chat-shell.tsx
 M apps/gateway-admin/components/chat/message-thread.tsx
 M apps/gateway-admin/components/logs/log-timeline.tsx
 M apps/gateway-admin/components/marketplace/acp-agent-card.tsx
 M apps/gateway-admin/components/marketplace/acp-agent-install-modal.tsx
 M apps/gateway-admin/components/marketplace/marketplace-list-content.tsx
 M apps/gateway-admin/lib/acp/types.ts
 M apps/gateway-admin/lib/api/gateway-client.test.ts
 M apps/gateway-admin/lib/api/gateway-client.ts
 M apps/gateway-admin/lib/api/marketplace-client.ts
 M apps/gateway-admin/lib/chat/session-events.test.ts
 M apps/gateway-admin/lib/chat/session-events.ts
 M apps/gateway-admin/lib/chat/use-chat-session-controller.ts
 M apps/gateway-admin/lib/chat/use-session-events.ts
 M apps/gateway-admin/lib/dashboard/admin-insights.test.ts
 M apps/gateway-admin/lib/dashboard/admin-insights.ts
 M apps/gateway-admin/lib/hooks/use-marketplace.ts
 M apps/gateway-admin/lib/marketplace/api-client.ts
 M apps/gateway-admin/lib/marketplace/mocks.ts
 M apps/gateway-admin/lib/marketplace/types.ts
 M apps/gateway-admin/lib/server/gateway-adapter.ts
 M crates/lab-apis/CLAUDE.md
 M crates/lab-apis/src/acp.rs
 M crates/lab-apis/src/acp/persistence.rs
 M crates/lab-apis/src/acp/types.rs
 M crates/lab-apis/src/acp_registry/client.rs
 M crates/lab-apis/src/core.rs
 M crates/lab-apis/src/core/plugin_ui.rs
 M crates/lab-apis/src/device_runtime/client.rs
 M crates/lab-apis/src/extract/CLAUDE.md
 M crates/lab-apis/src/extract/client.rs
 M crates/lab-apis/src/mcpregistry.rs
 M crates/lab-apis/src/mcpregistry/types.rs
 M crates/lab-apis/src/qbittorrent.rs
 M crates/lab/src/acp.rs
 M crates/lab/src/acp/persistence.rs
 M crates/lab/src/acp/registry.rs
 M crates/lab/src/acp/runtime.rs
 M crates/lab/src/acp/types.rs
 M crates/lab/src/api/nodes.rs
 M crates/lab/src/api/nodes/fleet.rs
 M crates/lab/src/api/nodes/syslog.rs
 M crates/lab/src/api/router.rs
 M crates/lab/src/api/services.rs
 M crates/lab/src/api/services/acp.rs
 M crates/lab/src/api/services/gateway.rs
 M crates/lab/src/api/services/marketplace.rs
 M crates/lab/src/api/services/registry_v01.rs
 M crates/lab/src/api/state.rs
 M crates/lab/src/api/upstream_oauth.rs
 M crates/lab/src/api/web.rs
 M crates/lab/src/audit/checks.rs
 M crates/lab/src/audit/checks/files.rs
 M crates/lab/src/audit/checks/registration.rs
 M crates/lab/src/audit/checks/tests.rs
 M crates/lab/src/audit/onboarding.rs
 M crates/lab/src/cli/arcane.rs
 M crates/lab/src/cli/doctor.rs
 M crates/lab/src/cli/gateway.rs
 M crates/lab/src/cli/marketplace.rs
 M crates/lab/src/cli/nodes.rs
 M crates/lab/src/cli/serve.rs
 M crates/lab/src/cli/tei.rs
 M crates/lab/src/config.rs
 M crates/lab/src/dispatch.rs
 M crates/lab/src/dispatch/CLAUDE.md
 M crates/lab/src/dispatch/acp.rs
 M crates/lab/src/dispatch/acp/client.rs
 M crates/lab/src/dispatch/acp/dispatch.rs
 M crates/lab/src/dispatch/acp/params.rs
 M crates/lab/src/dispatch/acp/persistence.rs
 M crates/lab/src/dispatch/deploy/build.rs
 M crates/lab/src/dispatch/deploy/runner.rs
 M crates/lab/src/dispatch/deploy/stages.rs
 M crates/lab/src/dispatch/doctor.rs
 M crates/lab/src/dispatch/doctor/params.rs
 M crates/lab/src/dispatch/doctor/service.rs
 M crates/lab/src/dispatch/doctor/system.rs
 M crates/lab/src/dispatch/doctor/types.rs
 M crates/lab/src/dispatch/fs.rs
 M crates/lab/src/dispatch/fs/client.rs
 M crates/lab/src/dispatch/fs/dispatch.rs
 M crates/lab/src/dispatch/fs/params.rs
 M crates/lab/src/dispatch/gateway/config.rs
 M crates/lab/src/dispatch/gateway/dispatch.rs
 M crates/lab/src/dispatch/gateway/manager.rs
 M crates/lab/src/dispatch/lab_admin.rs
 M crates/lab/src/dispatch/linkding.rs
 M crates/lab/src/dispatch/marketplace.rs
 M crates/lab/src/dispatch/marketplace/acp_client.rs
 M crates/lab/src/dispatch/marketplace/acp_dispatch.rs
 M crates/lab/src/dispatch/marketplace/backend.rs
 M crates/lab/src/dispatch/marketplace/backends/claude.rs
 M crates/lab/src/dispatch/marketplace/backends/codex.rs
 M crates/lab/src/dispatch/marketplace/catalog.rs
 M crates/lab/src/dispatch/marketplace/client.rs
 M crates/lab/src/dispatch/marketplace/dispatch.rs
 M crates/lab/src/dispatch/marketplace/mcp_client.rs
 M crates/lab/src/dispatch/marketplace/mcp_dispatch.rs
 M crates/lab/src/dispatch/marketplace/mcp_params.rs
 M crates/lab/src/dispatch/marketplace/package.rs
 M crates/lab/src/dispatch/marketplace/params.rs
 M crates/lab/src/dispatch/marketplace/runtime.rs
 M crates/lab/src/dispatch/marketplace/service.rs
 M crates/lab/src/dispatch/marketplace/store.rs
 M crates/lab/src/dispatch/marketplace/sync.rs
 M crates/lab/src/dispatch/node.rs
 M crates/lab/src/dispatch/node/send.rs
 M crates/lab/src/dispatch/paperless.rs
 M crates/lab/src/dispatch/plex.rs
 M crates/lab/src/dispatch/radarr.rs
 M crates/lab/src/dispatch/unifi.rs
 M crates/lab/src/dispatch/upstream/pool.rs
 M crates/lab/src/dispatch/upstream/transport/websocket.rs
 M crates/lab/src/lib.rs
 M crates/lab/src/log_fmt/formatter.rs
 M crates/lab/src/main.rs
 M crates/lab/src/mcp/CLAUDE.md
 M crates/lab/src/mcp/server.rs
 M crates/lab/src/mcp/services.rs
 D crates/lab/src/mcp/services/apprise.rs
 D crates/lab/src/mcp/services/arcane.rs
 D crates/lab/src/mcp/services/bytestash.rs
 D crates/lab/src/mcp/services/doctor.rs
 D crates/lab/src/mcp/services/extract.rs
 M crates/lab/src/mcp/services/fs.rs
 D crates/lab/src/mcp/services/gateway.rs
 D crates/lab/src/mcp/services/gotify.rs
 D crates/lab/src/mcp/services/lab_admin.rs
 D crates/lab/src/mcp/services/linkding.rs
 D crates/lab/src/mcp/services/logs.rs
 D crates/lab/src/mcp/services/marketplace.rs
 D crates/lab/src/mcp/services/memos.rs
 M crates/lab/src/mcp/services/nodes.rs
 D crates/lab/src/mcp/services/openai.rs
 D crates/lab/src/mcp/services/overseerr.rs
 D crates/lab/src/mcp/services/paperless.rs
 D crates/lab/src/mcp/services/plex.rs
 D crates/lab/src/mcp/services/prowlarr.rs
 D crates/lab/src/mcp/services/qbittorrent.rs
 D crates/lab/src/mcp/services/qdrant.rs
 D crates/lab/src/mcp/services/radarr.rs
 D crates/lab/src/mcp/services/sabnzbd.rs
 D crates/lab/src/mcp/services/sonarr.rs
 D crates/lab/src/mcp/services/tei.rs
 D crates/lab/src/mcp/services/unifi.rs
 M crates/lab/src/node/enrollment/store.rs
 M crates/lab/src/node/install.rs
 M crates/lab/src/node/log_store.rs
 M crates/lab/src/node/log_store/log_store_tests.rs
 M crates/lab/src/node/queue.rs
 M crates/lab/src/node/runtime.rs
 M crates/lab/src/node/store.rs
 M crates/lab/src/node/token.rs
 M crates/lab/src/node/update.rs
 M crates/lab/src/node/ws_client.rs
 M crates/lab/src/output/render.rs
 M crates/lab/src/output/theme.rs
 M crates/lab/src/registry.rs
 M crates/lab/src/scaffold.rs
 M crates/lab/src/scaffold/patcher.rs
 M crates/lab/src/scaffold/patcher/source.rs
 M crates/lab/src/scaffold/templates.rs
 D crates/lab/src/scaffold/templates/adapter_mcp.tpl
 M crates/lab/src/scaffold/templates/adapters.rs
 M crates/lab/src/scaffold/templates/lab_apis_service.tpl
 M crates/lab/tests/acp_backend_contract.rs
 M crates/lab/tests/api_fs_headers.rs
 M crates/lab/tests/device_api.rs
 M crates/lab/tests/device_cli.rs
 M crates/lab/tests/device_runtime.rs
 M crates/lab/tests/logs_api.rs
 M crates/lab/tests/node_config.rs
 M crates/lab/tests/nodes_api.rs
 M crates/lab/tests/nodes_cli.rs
 M crates/lab/tests/nodes_runtime.rs
 M docs/ARCH.md
 M docs/CONFIG.md
 M docs/ERRORS.md
 M docs/MARKETPLACE.md
 M docs/README.md
 M docs/SCAFFOLD_AND_AUDIT.md
 M docs/SERVICE_ONBOARDING.md
 M docs/TESTING.md
 M docs/coverage/arcane.md
 M docs/coverage/bytestash.md
 M docs/coverage/gotify.md
 M docs/coverage/linkding.md
 M docs/coverage/mcpregistry.md
 M docs/coverage/memos.md
 M docs/coverage/paperless.md
 M docs/coverage/plex.md
 M docs/coverage/qbittorrent.md
 M docs/coverage/qdrant.md
 M docs/coverage/radarr.md
 M docs/coverage/sabnzbd.md
 M docs/coverage/sonarr.md
 M docs/coverage/tailscale.md
 M docs/coverage/tautulli.md
 M docs/coverage/tei.md
 M docs/coverage/unifi.md
 M docs/coverage/unraid.md
?? apps/gateway-admin/lib/api/marketplace-acp-client.test.ts
?? apps/gateway-admin/lib/marketplace/api-client.test.ts
?? crates/lab/src/acp/providers.rs
?? crates/lab/src/audit/checks/ui_schema.rs
?? crates/lab/src/dispatch/marketplace/update.rs
?? docs/NODE_RUNTIME_CONTRACT.md
?? docs/features/
?? plugins/skills/quick-push/
?? plugins/skills/save-to-md/
```

### `git log --oneline --name-only -10`

```text
f168964b fix(lab-zxx5.32): R2 P3 roll-up — redact_home in errors, log tiering, sync_all, dead Sized
crates/lab/src/api/nodes/fleet.rs
crates/lab/src/dispatch/marketplace/acp_dispatch.rs
crates/lab/src/dispatch/marketplace/dispatch.rs
crates/lab/src/dispatch/node/send.rs
crates/lab/src/node/install.rs
39266dce refactor(lab-f1t2): address simplify + review findings on the f1t2 wave
apps/gateway-admin/components/chat/chat-input.tsx
crates/lab/src/api/services/fs.rs
crates/lab/src/dispatch/fs/dispatch.rs
crates/lab/src/mcp/CLAUDE.md
crates/lab/src/mcp/services/fs.rs
b7f488af fix(lab-zxx5.30,lab-zxx5.31): partial-extraction detection + fail-closed walk
crates/lab/src/dispatch/marketplace/acp_dispatch.rs
7b051062 fix(lab-zxx5.29): validate node install result shape
crates/lab/src/dispatch/marketplace/acp_dispatch.rs
crates/lab/src/dispatch/marketplace/dispatch.rs
12eb0ea0 fix(lab-zxx5.28): typed error markers restore install taxonomy
crates/lab/src/node/install.rs
crates/lab/src/node/ws_client.rs
ae302ef6 docs(lab-f1t2.32): document MCP transport auth requirement for fs
crates/lab/src/mcp/CLAUDE.md
crates/lab/src/registry.rs
86e943eb fix(lab-f1t2.26): redact path from deny-list oracle log events
crates/lab/src/api/services/fs.rs
crates/lab/src/dispatch/fs/dispatch.rs
c9be4573 fix(lab-f1t2.30): reset AttachmentChip thumbUrl at effect start
apps/gateway-admin/components/chat/chat-input.tsx
33db1293 fix(lab-f1t2.29): reset loading/truncated when picker closes mid-fetch
apps/gateway-admin/components/chat/workspace-picker.tsx
0e7a569f fix(lab-f1t2.24): handle help/schema before workspace_root resolution
crates/lab/src/dispatch/fs/dispatch.rs
```

### `pwd`

```text
/home/jmagar/workspace/lab
```

### `git worktree list | grep $(pwd) | head -1`

```text
/home/jmagar/workspace/lab                                   f168964b [bd-security/marketplace-p1-fixes]
```

### `gh pr view --json number,title,url 2>/dev/null || echo "none"`

```json
{"number":29,"title":"fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation","url":"https://github.com/jmagar/lab/pull/29"}
```

### Transcript/session source

Unavailable in this environment.

### Active plan path

```text
docs/superpowers/plans/2026-04-25-lab-cgxg-completion.md
```

## Session Overview

Implemented the `lab-cgxg` code/doc migration for MCP registration normalization. Normal service registration now defaults to `crate::dispatch::<service>` through `register_service!`; `mcp/services/` now contains only exception modules with MCP-specific behavior.

Build/test verification is blocked in the current dirty worktree by unrelated compile errors in parallel work under `crates/lab/src/api/web.rs` and transiently under untracked `crates/lab/src/dispatch/marketplace/update.rs`.

## Sequence of Events

1. Ran `bd show lab-cgxg` and captured the bead description, design, and acceptance criteria.
2. Loaded `superpowers:writing-plans` from `/home/jmagar/.codex/superpowers/skills/writing-plans/SKILL.md`.
3. Inspected `crates/lab/src/registry.rs`, `crates/lab/src/mcp/services.rs`, current MCP wrapper files, dispatch entrypoints, scaffold/audit code, and relevant docs.
4. Created implementation plan at `docs/superpowers/plans/2026-04-25-lab-cgxg-completion.md`.
5. Updated `register_service!` in `crates/lab/src/registry.rs` to use `crate::dispatch::<service>::ACTIONS` and `crate::dispatch::<service>::dispatch` by default.
6. Switched normal always-on and feature-gated registrations away from thin `mcp::services` wrappers.
7. Kept MCP exception paths for `deploy`, `fs`, and `nodes` only.
8. Pruned `crates/lab/src/mcp/services.rs` to exception modules only.
9. Moved/preserved useful wrapper tests into dispatch-layer tests.
10. Deleted stale wrapper files under `crates/lab/src/mcp/services/`.
11. Removed MCP adapter generation and MCP adapter audit requirements from scaffold/audit code.
12. Updated MCP/onboarding/scaffold/coverage docs to describe direct registry-to-dispatch wiring.
13. Updated `crates/lab/tests/logs_api.rs` from stale `mcp::services::logs` references to `dispatch::logs`.
14. Ran stale-reference and remaining-file verification.
15. Ran Cargo verification attempts and recorded unrelated blockers.
16. Wrote this report.

## Key Findings

- `registry.rs` was still documenting `mcp::services` as the default macro target while several services already bypassed wrapper modules.
- `mcp/services/` contained many thin forwarder or tests-only modules with no MCP-specific behavior.
- Real MCP-specific behavior remains in exactly three modules: `deploy`, `fs`, and `nodes`.
- Scaffold and audit still encoded the old wrapper model and would have reintroduced stale adapters for new services.
- `mcpregistry` no longer has a standalone MCP service wrapper in the current tree; its actions are absorbed into `marketplace` dispatch.
- The worktree was already heavily dirty before this task, including unrelated API, marketplace, node, frontend, and docs changes.

## Technical Decisions

- Direct `crate::dispatch::<service>` is the default registration path.
- Override registration remains available for services exposing `actions()` instead of a top-level `ACTIONS` const, such as `radarr`, `unifi`, and `marketplace`.
- `deploy` remains in `mcp/services` because it sets MCP elicitation context.
- `fs` remains in `mcp/services` because it filters `fs.preview` from MCP discovery and execution.
- `nodes` remains in `mcp/services` because its enrollment actions are MCP-specific in the current architecture.
- Tests from deleted wrappers were moved only where they added coverage not already present in dispatch tests.
- Unrelated dirty compile blockers were not edited to avoid overwriting parallel work.

## Files Modified

Bead-scoped files changed by this session:

- `crates/lab/src/registry.rs`
- `crates/lab/src/mcp/services.rs`
- `crates/lab/src/mcp/services/apprise.rs` deleted
- `crates/lab/src/mcp/services/arcane.rs` deleted
- `crates/lab/src/mcp/services/bytestash.rs` deleted
- `crates/lab/src/mcp/services/doctor.rs` deleted
- `crates/lab/src/mcp/services/extract.rs` deleted
- `crates/lab/src/mcp/services/gateway.rs` deleted
- `crates/lab/src/mcp/services/gotify.rs` deleted
- `crates/lab/src/mcp/services/lab_admin.rs` deleted
- `crates/lab/src/mcp/services/linkding.rs` deleted
- `crates/lab/src/mcp/services/logs.rs` deleted
- `crates/lab/src/mcp/services/marketplace.rs` deleted
- `crates/lab/src/mcp/services/memos.rs` deleted
- `crates/lab/src/mcp/services/openai.rs` deleted
- `crates/lab/src/mcp/services/overseerr.rs` deleted
- `crates/lab/src/mcp/services/paperless.rs` deleted
- `crates/lab/src/mcp/services/plex.rs` deleted
- `crates/lab/src/mcp/services/prowlarr.rs` deleted
- `crates/lab/src/mcp/services/qbittorrent.rs` deleted
- `crates/lab/src/mcp/services/qdrant.rs` deleted
- `crates/lab/src/mcp/services/radarr.rs` deleted
- `crates/lab/src/mcp/services/sabnzbd.rs` deleted
- `crates/lab/src/mcp/services/sonarr.rs` deleted
- `crates/lab/src/mcp/services/tei.rs` deleted
- `crates/lab/src/mcp/services/unifi.rs` deleted
- `crates/lab/src/dispatch/radarr.rs`
- `crates/lab/src/dispatch/linkding.rs`
- `crates/lab/src/dispatch/paperless.rs`
- `crates/lab/src/dispatch/plex.rs`
- `crates/lab/src/dispatch/unifi.rs`
- `crates/lab/src/dispatch/lab_admin.rs`
- `crates/lab/src/scaffold.rs`
- `crates/lab/src/scaffold/patcher.rs`
- `crates/lab/src/scaffold/patcher/source.rs`
- `crates/lab/src/scaffold/templates.rs`
- `crates/lab/src/scaffold/templates/adapters.rs`
- `crates/lab/src/scaffold/templates/adapter_mcp.tpl` deleted
- `crates/lab/src/audit/checks/files.rs`
- `crates/lab/src/audit/checks/registration.rs`
- `crates/lab/src/audit/checks/tests.rs`
- `crates/lab/tests/logs_api.rs`
- `crates/lab/src/mcp/CLAUDE.md`
- `docs/SERVICE_ONBOARDING.md`
- `docs/SCAFFOLD_AND_AUDIT.md`
- `docs/ARCH.md`
- `docs/TESTING.md`
- `docs/coverage/arcane.md`
- `docs/coverage/bytestash.md`
- `docs/coverage/gotify.md`
- `docs/coverage/linkding.md`
- `docs/coverage/mcpregistry.md`
- `docs/coverage/memos.md`
- `docs/coverage/paperless.md`
- `docs/coverage/plex.md`
- `docs/coverage/qbittorrent.md`
- `docs/coverage/qdrant.md`
- `docs/coverage/radarr.md`
- `docs/coverage/sabnzbd.md`
- `docs/coverage/sonarr.md`
- `docs/coverage/tailscale.md`
- `docs/coverage/tautulli.md`
- `docs/coverage/tei.md`
- `docs/coverage/unifi.md`
- `docs/coverage/unraid.md`
- `docs/superpowers/plans/2026-04-25-lab-cgxg-completion.md`
- `docs/sessions/2026-04-25-lab-cgxg-completion.md`

## Commands Executed

- `bd show lab-cgxg`
- `sed -n '1,240p' /home/jmagar/.codex/superpowers/skills/writing-plans/SKILL.md`
- `git status --short`
- `find crates/lab/src/mcp/services -maxdepth 2 -type f | sort`
- `find crates/lab/src/dispatch -maxdepth 2 -type f | sort`
- `sed -n '1,260p' crates/lab/src/registry.rs`
- `sed -n '261,620p' crates/lab/src/registry.rs`
- `sed -n '621,980p' crates/lab/src/registry.rs`
- `sed -n '1,220p' crates/lab/src/mcp/services.rs`
- `rg -n ... crates/lab/src docs`
- `cargo build --all-features --manifest-path crates/lab/Cargo.toml`
- `cargo test -p lab --all-features --no-fail-fast`
- `cargo test --manifest-path crates/lab/Cargo.toml --all-features --no-fail-fast`
- `TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'`
- `git remote get-url origin`
- `git branch --show-current`
- `git rev-parse --short HEAD`
- `git log --oneline -5`
- `git status --short`
- `git log --oneline --name-only -10`
- `pwd`
- `git worktree list | grep $(pwd) | head -1`
- `gh pr view --json number,title,url 2>/dev/null || echo "none"`

## Errors Encountered

- `cargo test -p lab --all-features --no-fail-fast` failed before running tests because `lab` is ambiguous with a crates.io package in the lockfile. Root cause: package specification ambiguity. Follow-up command used `--manifest-path crates/lab/Cargo.toml`.
- `cargo test --manifest-path crates/lab/Cargo.toml --all-features --no-fail-fast` initially failed on stale `lab::mcp::services::logs` references in `crates/lab/tests/logs_api.rs`; this was bead-related and fixed by switching the test to `lab::dispatch::logs`.
- A later Cargo verification failed in untracked/parallel `crates/lab/src/dispatch/marketplace/update.rs`. The file contents changed after the error output; current status shows it as untracked. No edits were made by this session.
- Final `cargo build --all-features --manifest-path crates/lab/Cargo.toml` failed in unrelated dirty `crates/lab/src/api/web.rs` with missing `SystemTime` import and missing `dev_content_dir` symbol. No edits were made by this session.

## Behavior Changes

- Normal MCP service registration no longer depends on `mcp/services/<service>.rs` wrappers.
- Thin MCP wrapper modules no longer compile for normal services.
- New scaffolded services no longer generate MCP wrapper files or patch `mcp/services.rs`.
- Onboarding audit no longer requires MCP wrapper files for normal services.
- `logs_api` tests now construct a test registry through `lab::dispatch::logs` directly.

## Verification Evidence

Passed:

```text
rg stale concrete wrapper references: no matches
```

Passed:

```text
find crates/lab/src/mcp/services -maxdepth 1 -type f | sort
crates/lab/src/mcp/services/deploy.rs
crates/lab/src/mcp/services/fs.rs
crates/lab/src/mcp/services/nodes.rs
```

Earlier build pass before parallel dirty-work changes surfaced:

```text
cargo build --all-features --manifest-path crates/lab/Cargo.toml
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 34s
```

Current final build blocker:

```text
cargo build --all-features --manifest-path crates/lab/Cargo.toml
error[E0425]: cannot find type `SystemTime` in this scope
  --> crates/lab/src/api/web.rs:99:84
error[E0425]: cannot find function `dev_content_dir` in this scope
   --> crates/lab/src/api/web.rs:147:16
```

Current test blocker:

```text
cargo test --manifest-path crates/lab/Cargo.toml --all-features --no-fail-fast
blocked by unrelated compile errors before tests run
```

## Risks and Rollback

- Risk: broad docs updates in `docs/coverage/*.md` may need wording polish after the branch stabilizes.
- Risk: current worktree contains many unrelated dirty and untracked files; build status can change due to parallel work.
- Rollback for this bead: revert the files listed under “Files Modified” for this session only. Do not revert unrelated dirty files shown in `git status --short`.

## Decisions Not Taken

- Did not edit `crates/lab/src/api/web.rs` because it is unrelated dirty work and outside `lab-cgxg` scope.
- Did not edit untracked `crates/lab/src/dispatch/marketplace/update.rs` because it is unrelated parallel work and changed while verification was running.
- Did not preserve tests-only MCP wrapper files; tests were moved or existing dispatch tests were extended.

## References

- Bead: `lab-cgxg` — “Normalize MCP service registration and retire stale mcp/services wrappers”.
- Plan: `docs/superpowers/plans/2026-04-25-lab-cgxg-completion.md`.
- Registry owner: `crates/lab/src/registry.rs`.
- MCP exception module list: `crates/lab/src/mcp/services.rs`.

## Open Questions

- Transcript/session source was not exposed in this environment.
- Whether the branch owner wants this bead closed before unrelated `api/web.rs` and marketplace update compile blockers are fixed.

## Next Steps

1. Fix or isolate unrelated dirty compile blockers in `crates/lab/src/api/web.rs` and untracked `crates/lab/src/dispatch/marketplace/update.rs`.
2. Rerun `cargo build --all-features --manifest-path crates/lab/Cargo.toml`.
3. Rerun `cargo test --manifest-path crates/lab/Cargo.toml --all-features --no-fail-fast`.
4. Close `lab-cgxg` after those branch-level verification commands pass.
