---
date: "2026-04-25 11:51:07 EST"
repo: "git@github.com:jmagar/lab.git"
branch: "bd-security/marketplace-p1-fixes"
head: "f168964b"
plan: "/home/jmagar/workspace/lab/docs/superpowers/plans/2026-04-25-lab-5yzk1-completion.md"
agent: "Codex Worker for bead lab-5yzk.1"
working_directory: "/home/jmagar/workspace/lab"
worktree: "/home/jmagar/workspace/lab                                   f168964b [bd-security/marketplace-p1-fixes]"
pr: '{"number":29,"title":"fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation","url":"https://github.com/jmagar/lab/pull/29"}'
---

# lab-5yzk.1 Completion Session Report

## User Request

Complete bead `lab-5yzk.1` in `/home/jmagar/workspace/lab`: fix all 17 service CLI shims so action help lists complete possible values, use `action_parser`, correct `action` field type/default, route CLI through dispatch instead of MCP, and enforce destructive gates. Create an implementation plan and a session report without overwriting existing files.

## Session Overview

Bead details were gathered with `bd show lab-5yzk.1`. Current code already had most of the bead implemented: `action_parser()` existed, 16 of 17 target shims used `default_value = "help"` and `action_parser(ACTIONS)`, no target shim used `crate::mcp::services`, and destructive-action shims already used `run_confirmable_action_command`.

The remaining bead-scope code gap was `crates/lab/src/cli/arcane.rs`: `ArcaneArgs.action` was still a required positional without `default_value = "help"` and without `action_parser(ACTIONS)`. I fixed that single remaining shim and verified all 17 target services.

## Required Context

### `TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'`

```text
2026-04-25 11:51:07 EST
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

### `git worktree list | grep /home/jmagar/workspace/lab | head -1`

```text
/home/jmagar/workspace/lab                                   f168964b [bd-security/marketplace-p1-fixes]
```

### `gh pr view --json number,title,url 2>/dev/null || echo "none"`

```json
{"number":29,"title":"fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation","url":"https://github.com/jmagar/lab/pull/29"}
```

### Transcript/session source

Unavailable in the execution environment.

### Active plan path

```text
/home/jmagar/workspace/lab/docs/superpowers/plans/2026-04-25-lab-5yzk1-completion.md
```

## Sequence of Events

1. Loaded `superpowers:using-superpowers`, `superpowers:writing-plans`, `superpowers:test-driven-development`, `superpowers:executing-plans`, and `superpowers:systematic-debugging` when applicable.
2. Ran `bd show lab-5yzk.1` and captured the bead requirements, service list, reference implementations, and validation checklist.
3. Gathered required repo/session context with the user-specified git/date/worktree/PR commands.
4. Read the target CLI shims, helper file, reference shims, and nearest `CLAUDE.md` rules under `crates/lab/src` and `crates/lab/src/cli`.
5. Determined that `crates/lab/src/cli/arcane.rs` was the only remaining incomplete target shim.
6. Created the implementation plan at `docs/superpowers/plans/2026-04-25-lab-5yzk1-completion.md`.
7. Ran the red check `cargo run --manifest-path crates/lab/Cargo.toml --all-features -- arcane --help`; the output lacked action possible values for `arcane`.
8. Patched `crates/lab/src/cli/arcane.rs` to import `action_parser` and set `#[arg(default_value = "help", value_parser = action_parser(ACTIONS))]` on `ArcaneArgs.action`.
9. Ran static and behavior verification across all 17 target shims.
10. Ran `cargo check --workspace --all-features`, `cargo build --workspace --all-features`, `just check`, and `just build`.
11. Marked all plan checkboxes complete.
12. Created this session report.

## Key Findings

- `crates/lab/src/cli/helpers.rs:18` already contained `action_parser(actions: &'static [ActionSpec])`.
- `crates/lab/src/cli/arcane.rs:11` needed the `action_parser` import.
- `crates/lab/src/cli/arcane.rs:20` needed the clap action parser/default attribute.
- All 17 target shims now have `#[arg(default_value = "help", value_parser = action_parser(ACTIONS))]` on the action field.
- No target shim currently contains `crate::mcp::services`, `action: Option<String>`, or the old `unwrap_or_else(|| "help"...)` fallback.
- All target services whose dispatch catalog contains `destructive: true` use `run_confirmable_action_command`.
- The workspace was already heavily dirty before this task, and more unrelated changes appeared during the session; bead-scope writes were limited to `crates/lab/src/cli/arcane.rs`, the plan, and this report.

## Technical Decisions

- Reused the already-present helper `action_parser(ACTIONS)` rather than inlining `PossibleValuesParser` in `arcane`.
- Kept the existing `arcane` key/value param parser, dry-run behavior, dispatch-layer call, and confirmable destructive helper unchanged.
- Used `--manifest-path crates/lab/Cargo.toml` for one-off `cargo run` checks because `cargo run -p lab` is ambiguous in this workspace.
- Used `target/debug/lab` after building for CLI behavior checks to avoid repeated cargo warning noise.
- Did not edit unrelated marketplace/scaffold warnings because the user limited write scope to CLI shims/helpers and required dispatch/destructive-gate fixes.

## Files Modified

- `crates/lab/src/cli/arcane.rs:11` imported `action_parser` with `run_confirmable_action_command`.
- `crates/lab/src/cli/arcane.rs:20` added `#[arg(default_value = "help", value_parser = action_parser(ACTIONS))]` to `ArcaneArgs.action`.
- `docs/superpowers/plans/2026-04-25-lab-5yzk1-completion.md` created and then marked complete.
- `docs/sessions/2026-04-25-lab-5yzk1-completion.md` created.

## Commands Executed

```bash
bd show lab-5yzk.1
sed -n '1,220p' /home/jmagar/.codex/superpowers/skills/using-superpowers/SKILL.md
sed -n '1,260p' /home/jmagar/.codex/superpowers/skills/writing-plans/SKILL.md
sed -n '1,240p' /home/jmagar/.codex/superpowers/skills/test-driven-development/SKILL.md
sed -n '1,260p' /home/jmagar/.codex/superpowers/skills/executing-plans/SKILL.md
sed -n '1,260p' /home/jmagar/.codex/superpowers/skills/systematic-debugging/SKILL.md
TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'
git remote get-url origin
git branch --show-current
git rev-parse --short HEAD
git log --oneline -5
git status --short
git log --oneline --name-only -10
pwd
git worktree list | grep "$(pwd)" | head -1
gh pr view --json number,title,url 2>/dev/null || echo "none"
rg -n 'destructive: true|pub const ACTIONS|ActionSpec' crates/lab/src/dispatch/...
rg -n 'mcp::services|action: Option<String>|unwrap_or_else\(\|\| "help"|run_confirmable_action_command|run_action_command|action_parser\(' crates/lab/src/cli/...
cargo run -p lab --all-features -- arcane --help
cargo run --manifest-path crates/lab/Cargo.toml --all-features -- arcane --help
cargo check --workspace --all-features
cargo build --workspace --all-features
target/debug/lab apprise invalid_action
target/debug/lab arcane volume.delete
target/debug/lab arcane volume.delete -y --dry-run
target/debug/lab sonarr series.list --dry-run
just check
just build
```

## Errors Encountered

- `cargo run -p lab --all-features -- arcane --help` failed because the package specification `lab` is ambiguous between the local crate and crates.io `lab@0.11.0`. I used `cargo run --manifest-path crates/lab/Cargo.toml --all-features -- arcane --help` instead.
- The first `cargo check --workspace --all-features` failed with `E0583` for missing module `plugin_ui` from `crates/lab-apis/src/core.rs:22`. The file `crates/lab-apis/src/core/plugin_ui.rs` existed by the time I investigated, indicating a concurrent workspace update outside this bead. Rerunning `cargo check --workspace --all-features` passed.
- `cargo build`, `just check`, and `just build` exit 0 but emit unrelated warnings from `crates/lab/src/dispatch/marketplace/update.rs:5` about `dispatch_update_action` being unused. Earlier `cargo build` also emitted unrelated scaffold warnings from `crates/lab/src/scaffold/templates.rs:8` and `crates/lab/src/scaffold/templates/adapters.rs:7`; the final `just build` warning set only showed the marketplace warning.

## Behavior Changes

- `lab arcane --help` now shows `[default: help]` and `[possible values: ...]` for the action positional.
- `lab arcane` action is now optional and defaults to `help`, matching the bead’s reference shape.
- Invalid `arcane` actions are rejected by clap before dispatch, consistent with the other 16 target shims.
- Existing destructive gate behavior for `arcane` remains in place and now benefits from clap action validation.

## Verification Evidence

### Red check before code fix

Command: `cargo run --manifest-path crates/lab/Cargo.toml --all-features -- arcane --help`

Result: exit 0, but the `ACTION` argument was required and did not include `[possible values: ...]`.

```text
Usage: lab arcane [OPTIONS] <ACTION> [KEY=VALUE]...
Arguments:
  <ACTION>        Action to run, e.g. `help`, `system.health`, `container.list`
```

### Static old-pattern check

Command:

```bash
rg -n 'mcp::services|action: Option<String>|unwrap_or_else\(\|\| "help"|PossibleValuesParser::new\(ACTIONS' crates/lab/src/cli/{apprise,arcane,linkding,memos,openai,overseerr,paperless,plex,prowlarr,qbittorrent,qdrant,sabnzbd,sonarr,tailscale,tautulli,tei,unraid}.rs
```

Result: exit 1 with no matches, as expected.

### Static parser count

Command:

```bash
rg -n 'default_value = "help", value_parser = action_parser\(ACTIONS\)' crates/lab/src/cli/{apprise,arcane,linkding,memos,openai,overseerr,paperless,plex,prowlarr,qbittorrent,qdrant,sabnzbd,sonarr,tailscale,tautulli,tei,unraid}.rs
```

Result: exit 0 with 17 matches, one for each target shim.

### Dispatch target check

Command:

```bash
rg -n 'crate::dispatch::(apprise|arcane|linkding|memos|openai|overseerr|paperless|plex|prowlarr|qbittorrent|qdrant|sabnzbd|sonarr|tailscale|tautulli|tei|unraid)::dispatch' crates/lab/src/cli/{apprise,arcane,linkding,memos,openai,overseerr,paperless,plex,prowlarr,qbittorrent,qdrant,sabnzbd,sonarr,tailscale,tautulli,tei,unraid}.rs
```

Result: exit 0 with 17 dispatch-layer matches.

### Compile/build checks

Command: `cargo check --workspace --all-features`

Result: exit 0 on rerun.

Command: `cargo build --workspace --all-features`

Result: exit 0 with unrelated warnings from marketplace/scaffold code.

Command: `just check`

Result: exit 0 with one unrelated warning from `crates/lab/src/dispatch/marketplace/update.rs:5`.

Command: `just build`

Result: exit 0 with one unrelated warning from `crates/lab/src/dispatch/marketplace/update.rs:5`.

### All 17 help pages

Command:

```bash
set -e
for svc in apprise arcane linkding memos openai overseerr paperless plex prowlarr qbittorrent qdrant sabnzbd sonarr tailscale tautulli tei unraid; do
  target/debug/lab "$svc" --help | grep -q 'possible values:'
  echo "$svc: possible values ok"
done
```

Result: exit 0.

```text
apprise: possible values ok
arcane: possible values ok
linkding: possible values ok
memos: possible values ok
openai: possible values ok
overseerr: possible values ok
paperless: possible values ok
plex: possible values ok
prowlarr: possible values ok
qbittorrent: possible values ok
qdrant: possible values ok
sabnzbd: possible values ok
sonarr: possible values ok
tailscale: possible values ok
tautulli: possible values ok
tei: possible values ok
unraid: possible values ok
```

### Invalid action parser check

Command: `target/debug/lab apprise invalid_action`

Result: exit 2 with clap possible-values error.

```text
error: invalid value 'invalid_action' for '[ACTION]'
  [possible values: help, schema, server.health, notify.send, notify.key.send, config.add, config.get, config.delete, config.urls, server.details]
```

### Destructive gate check

Command: `target/debug/lab arcane volume.delete`

Result: exit 1, refused before dispatch in non-interactive stdin.

```text
WARN  destructive action blocked: non-interactive stdin, pass -y  action=volume.delete  service=arcane  surface=cli
ERROR  pass -y / --yes to confirm destructive action `volume.delete`
```

Command: `target/debug/lab arcane volume.delete -y --dry-run`

Result: exit 0.

```text
[dry-run] would dispatch arcane action `volume.delete` with params: {}
```

### Non-destructive action field regression check

Command: `target/debug/lab sonarr series.list --dry-run`

Result: exit 0.

```text
[dry-run] would dispatch sonarr action `series.list` with params: {}
```

### Arcane final help sample

Command: `target/debug/lab arcane --help | sed -n '1,25p'`

Result: exit 0 and includes default plus possible values.

```text
Usage: lab arcane [OPTIONS] [ACTION] [KEY=VALUE]...

Arguments:
  [ACTION]        Action to run, e.g. `help`, `system.health`, `container.list` [default: help] [possible values: help, schema, health, environment.list, environment.get, container.list, container.get, container.start, container.stop, container.restart, container.redeploy, project.list, project.create, project.up, project.down, project.redeploy, volume.list, volume.delete, volume.prune, image.list, image.pull, image.prune, image.update-summary]
```

## Risks and Rollback

- Risk: the workspace contains many unrelated modified, deleted, and untracked files from other concurrent work. This session intentionally did not revert or edit them.
- Risk: `just check` and `just build` currently emit an unrelated marketplace warning. This is not caused by the CLI shim change but may matter for a separate zero-warning policy.
- Rollback for this bead-scope code change is limited to removing the `action_parser` import and the `#[arg(default_value = "help", value_parser = action_parser(ACTIONS))]` attribute from `crates/lab/src/cli/arcane.rs`, but that would reopen `lab-5yzk.1`.

## Decisions Not Taken

- Did not edit `gotify` or `bytestash` reference shims because they are outside the 17-service bead list.
- Did not change unrelated marketplace/scaffold warnings because they are outside the requested CLI shim/helper/dispatch/destructive-gate scope.
- Did not close the bead with `bd close`; the request asked to finish the work so it can be closed, not to mutate bead state.
- Did not normalize every shim’s dry-run implementation beyond the bead requirements.

## References

- `bd show lab-5yzk.1`
- `crates/lab/src/CLAUDE.md:14` documents `cli -> dispatch -> lab-apis` as the intended dependency direction.
- `crates/lab/src/CLAUDE.md:21` forbids `cli -> mcp`.
- `crates/lab/src/cli/CLAUDE.md:10` defines CLI files as thin shims.
- `crates/lab/src/cli/CLAUDE.md:14` requires `-y` / `--yes`, `--no-confirm`, and `--dry-run` for destructive actions.
- `crates/lab/src/cli/helpers.rs:18` defines `action_parser`.
- `crates/lab/src/cli/arcane.rs:20` now applies `action_parser(ACTIONS)`.

## Open Questions

- Transcript/session source was not exposed in the execution environment.
- The workspace contains many concurrent non-bead changes; ownership of those changes is outside this session.
- The unrelated marketplace warning from `crates/lab/src/dispatch/marketplace/update.rs:5` remains for another bead/owner if zero-warning builds are required globally.

## Next Steps

1. Close bead `lab-5yzk.1` if the bead owner accepts exit-0 verification with unrelated warnings documented.
2. Handle the unrelated marketplace warning under the bead that owns `crates/lab/src/dispatch/marketplace/update.rs` if strict zero-warning policy is required before merge.
