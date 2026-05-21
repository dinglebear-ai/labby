---
title: lab-iut1.1 StashMeta foundation completion
bead: lab-iut1.1
repo: /home/jmagar/workspace/lab
date: 2026-04-25
plan: docs/superpowers/plans/2026-04-25-lab-iut11-completion.md
status: complete-with-tooling-note
---

# User Request

Complete bead `lab-iut1.1` in `/home/jmagar/workspace/lab`: add shared marketplace `StashMeta` types plus `stash_meta.rs` I/O helpers, reconcile with existing marketplace update private substitutes without implementing fork/diff/patch action wiring, create a plan, execute it, verify the bead, and write this session report.

# Session Overview

Implemented the marketplace stash metadata foundation as a shared module at `crates/lab/src/dispatch/marketplace/stash_meta.rs`. Exposed it from `crates/lab/src/dispatch/marketplace.rs:24`, added direct dependencies in `crates/lab/Cargo.toml:54` and `crates/lab/Cargo.toml:60`, and left existing `crates/lab/src/dispatch/marketplace/update.rs` action behavior unchanged.

# Sequence of Events

1. Read `superpowers:using-superpowers` and `superpowers:writing-plans` instructions.
2. Ran `bd show lab-iut1.1` and confirmed the bead required a new shared stash metadata module.
3. Read `.omc/research/beads-next-round-definitive-report-2026-04-25.md` section for `lab-iut1.1` and confirmed `stash_meta.rs` was absent and `update.rs` had private substitutes.
4. Inspected `crates/lab/src/dispatch/marketplace/update.rs`, `crates/lab/src/dispatch/marketplace/params.rs`, `crates/lab/src/dispatch/marketplace.rs`, and `crates/lab/Cargo.toml`.
5. Created and completed the active plan at `docs/superpowers/plans/2026-04-25-lab-iut11-completion.md`.
6. Added the shared `stash_meta` module and tests.
7. Ran focused stash metadata tests, relevant marketplace update tests, full `lab` tests, clippy, and grep checks.
8. Documented the Cargo package-name ambiguity that prevents the literal `cargo test -p lab ...` and `cargo clippy -p lab ...` forms from running in this repo.

# Key Findings

- `update.rs` already has private metadata, config, lock, and validation substitutes; this bead intentionally did not rewire those actions.
- Cargo cannot disambiguate `-p lab` because the dependency graph includes crates.io `lab@0.11.0` via `csscolorparser`, while this workspace package is also named `lab@0.11.0`.
- Another process was holding the shared target directory lock, so verification used `CARGO_TARGET_DIR=target/lab-iut11` to avoid interfering with other agents.

# Technical Decisions

- Kept `update.rs` behavior unchanged to avoid implementing later-bead action wiring.
- Added `content_hashes: HashMap<String, String>` to `StashMeta` because the bead comments and research report require content hash preservation even though the initial type sketch omitted it.
- Implemented `read_stash_meta` to return `Ok(None)` for absent, missing-schema, and schema-version-zero metadata at `crates/lab/src/dispatch/marketplace/stash_meta.rs:129`.
- Implemented path validation with empty/null/backslash rejection plus `Component::Normal`-only traversal protection at `crates/lab/src/dispatch/marketplace/stash_meta.rs:213`.
- Used `fs4::FileExt::lock(&file)` at `crates/lab/src/dispatch/marketplace/stash_meta.rs:125` because fs4 `1.0.2` exposes the blocking exclusive lock operation as `lock()`, not `lock_exclusive()`.
- Used `xxhash-rust` xxh3 hashing for drift/base hashes at `crates/lab/src/dispatch/marketplace/stash_meta.rs:388`.

# Files Modified

- `Cargo.lock` - added lock entries for `fs4 v1.0.2` and `xxhash-rust v0.8.15`; Cargo also retained existing crates.io `lab@0.11.0` from dependency resolution.
- `crates/lab/Cargo.toml:54` - added `fs4 = "1"`.
- `crates/lab/Cargo.toml:60` - added `xxhash-rust = { version = "0.8", features = ["xxh3"] }`.
- `crates/lab/src/dispatch/marketplace.rs:24` - exposed `pub(crate) mod stash_meta;`.
- `crates/lab/src/dispatch/marketplace/stash_meta.rs:28` - added durable stash metadata types.
- `crates/lab/src/dispatch/marketplace/stash_meta.rs:115` - added fs4 stash lock helper.
- `crates/lab/src/dispatch/marketplace/stash_meta.rs:129` - added `.stash.json` read helper.
- `crates/lab/src/dispatch/marketplace/stash_meta.rs:148` - added `.stash.json` atomic write helper.
- `crates/lab/src/dispatch/marketplace/stash_meta.rs:167` - added base snapshot write helper using `OpenOptions::create_new(true)`.
- `crates/lab/src/dispatch/marketplace/stash_meta.rs:232` - added drift cache read/write support.
- `crates/lab/src/dispatch/marketplace/stash_meta.rs:248` - added drift checking.
- `docs/superpowers/plans/2026-04-25-lab-iut11-completion.md` - active plan, now checked complete.
- `docs/sessions/2026-04-25-lab-iut11-completion.md` - this report.

# Commands Executed

## Required context commands

```text
bd show lab-iut1.1
sed -n '49,110p;457,466p' .omc/research/beads-next-round-definitive-report-2026-04-25.md
```

## Verification commands

```text
cargo test -p lab --all-features dispatch::marketplace::stash_meta::tests
```

Result: failed before compilation due Cargo package-name ambiguity between workspace `lab` and crates.io `lab@0.11.0`.

```text
CARGO_TARGET_DIR=target/lab-iut11 cargo test -p path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0 --all-features dispatch::marketplace::stash_meta::tests
```

Result: passed. `10 passed` in `src/lib.rs`; `10 passed` in `src/main.rs`.

```text
CARGO_TARGET_DIR=target/lab-iut11 cargo test -p path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0 --all-features dispatch::marketplace::update::tests
```

Result: passed. `19 passed` in `src/lib.rs`; `19 passed` in `src/main.rs`.

```text
CARGO_TARGET_DIR=target/lab-iut11 cargo test -p lab --all-features
```

Result: failed before compilation due Cargo package-name ambiguity.

```text
CARGO_TARGET_DIR=target/lab-iut11 cargo test -p path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0 --all-features
```

Result: passed. `src/lib.rs`: `833 passed`; `src/main.rs`: `838 passed`; integration tests passed; doc-tests ok with 2 ignored.

```text
CARGO_TARGET_DIR=target/lab-iut11 cargo clippy -p lab --all-features -- -D warnings
```

Result: failed before compilation due Cargo package-name ambiguity.

```text
CARGO_TARGET_DIR=target/lab-iut11 cargo clippy -p path+file:///home/jmagar/workspace/lab/crates/lab#0.11.0 --all-features -- -D warnings
```

Result: passed.

```text
rg -n "fs2" --glob '!target/**' .
rg -n "std::fs::copy" crates/lab/src/dispatch/marketplace/stash_meta.rs
rg -n "unwrap\(" crates/lab/src/dispatch/marketplace/stash_meta.rs
```

Result: all returned no matches.

# Errors Encountered

- Initial fs4 import used `fs4::fs_std::FileExt`, but fs4 `1.0.2` exposes the std trait at `fs4::FileExt`.
- Initial fs4 method call used `lock_exclusive()`, but fs4 `1.0.2` names the blocking exclusive operation `lock()`.
- Literal Cargo package selector `-p lab` fails because two packages named `lab` are in the resolved graph.
- A separate `cargo run --manifest-path /home/jmagar/workspace/lab/Cargo.toml --all-features --bin lab --quiet -- serve` process held the shared artifact target lock, so verification used `CARGO_TARGET_DIR=target/lab-iut11`.

# Behavior Changes

- New shared marketplace stash metadata module is available for later beads via `crate::dispatch::marketplace::stash_meta`.
- No `artifact.fork`, `artifact.diff`, `artifact.patch`, or new action wiring was implemented.
- Existing marketplace update tests still pass.

# Verification Evidence

- Focused stash metadata tests: passed with package ID selector.
- Marketplace update tests: passed with package ID selector.
- Full `lab` all-features test suite: passed with package ID selector.
- `lab` all-features clippy with `-D warnings`: passed with package ID selector.
- `fs2` search: no matches outside `target/`.
- `std::fs::copy` search in `stash_meta.rs`: no matches.
- `unwrap()` search in `stash_meta.rs`: no matches.

# Risks and Rollback

- Risk: `write_stash_meta` acquires the stash lock internally. Later read-modify-write callers should avoid acquiring the same lock and then calling `write_stash_meta` in the same thread unless the API is adjusted for explicit lock ownership.
- Risk: The literal `cargo -p lab` validation form remains blocked by an existing dependency naming ambiguity unrelated to this bead.
- Rollback: remove `crates/lab/src/dispatch/marketplace/stash_meta.rs`, remove `pub(crate) mod stash_meta;`, and remove `fs4`/`xxhash-rust` plus corresponding lockfile entries.

# Decisions Not Taken

- Did not migrate `crates/lab/src/dispatch/marketplace/update.rs` to the shared metadata types because that would alter action behavior and belongs to later beads.
- Did not add CLI/API/MCP action wiring.
- Did not replace existing `fd-lock`; the bead only required avoiding `fs2` and adding `fs4` for stash locks.

# References

- Bead: `lab-iut1.1`.
- Research: `.omc/research/beads-next-round-definitive-report-2026-04-25.md`, section `lab-iut1.1`.
- Plan: `docs/superpowers/plans/2026-04-25-lab-iut11-completion.md`.

# Open Questions

- Transcript/session source is unavailable in this environment.
- Should a follow-up infrastructure bead resolve the Cargo `-p lab` ambiguity caused by crates.io `lab@0.11.0` so the documented command can be run literally?

# Next Steps

- Close `lab-iut1.1` if package-ID validation is accepted as equivalent to the ambiguous literal `-p lab` command.
- Later beads can import `crate::dispatch::marketplace::stash_meta` for fork/diff/patch action implementation.

# Required Session Facts

```text
TZ=America/New_York date: 2026-04-25 15:42:51 EST
git remote get-url origin: git@github.com:jmagar/lab.git
git branch --show-current: bd-security/marketplace-p1-fixes
git rev-parse --short HEAD: f168964b
pwd: /home/jmagar/workspace/lab
git worktree list | grep $(pwd) | head -1: /home/jmagar/workspace/lab                                   f168964b [bd-security/marketplace-p1-fixes]
gh pr view --json number,title,url: {"number":29,"title":"fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation","url":"https://github.com/jmagar/lab/pull/29"}
```

## git log --oneline -5

```text
f168964b fix(lab-zxx5.32): R2 P3 roll-up — redact_home in errors, log tiering, sync_all, dead Sized
39266dce refactor(lab-f1t2): address simplify + review findings on the f1t2 wave
b7f488af fix(lab-zxx5.30,lab-zxx5.31): partial-extraction detection + fail-closed walk
7b051062 fix(lab-zxx5.29): validate node install result shape
12eb0ea0 fix(lab-zxx5.28): typed error markers restore install taxonomy
```

## git status --short

```text
 M Cargo.lock
 M Cargo.toml
 M Justfile
 M apps/gateway-admin/app/(admin)/activity/page.tsx
 M apps/gateway-admin/components/chat/chat-shell.tsx
 M apps/gateway-admin/components/chat/message-thread.tsx
 M apps/gateway-admin/components/gateway/delete-gateway-dialog.tsx
 M apps/gateway-admin/components/gateway/gateway-list-content.tsx
 M apps/gateway-admin/components/gateway/gateway-table.test.tsx
 M apps/gateway-admin/components/gateway/gateway-table.tsx
 M apps/gateway-admin/components/logs/log-timeline.tsx
 M apps/gateway-admin/components/marketplace/acp-agent-card.tsx
 M apps/gateway-admin/components/marketplace/acp-agent-install-modal.tsx
 M apps/gateway-admin/components/marketplace/marketplace-list-content.tsx
 M apps/gateway-admin/lib/acp/types.ts
 M apps/gateway-admin/lib/api/gateway-client.test.ts
 M apps/gateway-admin/lib/api/gateway-client.ts
 M apps/gateway-admin/lib/api/marketplace-client.ts
 M apps/gateway-admin/lib/api/service-action-client.ts
 M apps/gateway-admin/lib/chat/session-events.test.ts
 M apps/gateway-admin/lib/chat/session-events.ts
 M apps/gateway-admin/lib/chat/use-chat-session-controller.ts
 M apps/gateway-admin/lib/chat/use-session-events.ts
 M apps/gateway-admin/lib/dashboard/admin-insights.test.ts
 M apps/gateway-admin/lib/dashboard/admin-insights.ts
 M apps/gateway-admin/lib/hooks/use-gateways.ts
 M apps/gateway-admin/lib/hooks/use-marketplace.ts
 M apps/gateway-admin/lib/marketplace/api-client.ts
 M apps/gateway-admin/lib/marketplace/mocks.ts
 M apps/gateway-admin/lib/marketplace/types.ts
 M apps/gateway-admin/lib/server/gateway-adapter.ts
 M apps/gateway-admin/package.json
 M crates/lab-apis/CLAUDE.md
 M crates/lab-apis/src/acp.rs
 M crates/lab-apis/src/acp/persistence.rs
 M crates/lab-apis/src/acp/types.rs
 M crates/lab-apis/src/acp_registry/client.rs
 M crates/lab-apis/src/acp_registry/types.rs
 M crates/lab-apis/src/core.rs
 M crates/lab-apis/src/core/plugin_ui.rs
 M crates/lab-apis/src/device_runtime/client.rs
 M crates/lab-apis/src/extract/CLAUDE.md
 M crates/lab-apis/src/extract/client.rs
 M crates/lab-apis/src/mcpregistry.rs
 M crates/lab-apis/src/mcpregistry/types.rs
 M crates/lab-apis/src/qbittorrent.rs
 M crates/lab/Cargo.toml
 M crates/lab/src/acp.rs
 M crates/lab/src/acp/persistence.rs
 M crates/lab/src/acp/registry.rs
 M crates/lab/src/acp/runtime.rs
 M crates/lab/src/acp/types.rs
 M crates/lab/src/api/error.rs
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
 M crates/lab/src/dispatch/gateway.rs
 M crates/lab/src/dispatch/gateway/catalog.rs
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
 M docs/MCP.md
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
?? apps/gateway-admin/app/dev/
?? apps/gateway-admin/components/marketplace/marketplace-v2-content.tsx
?? apps/gateway-admin/components/marketplace/marketplace-v2-state.test.ts
?? apps/gateway-admin/components/marketplace/marketplace-v2-state.ts
?? apps/gateway-admin/docs/marketplace-catalog.md
?? apps/gateway-admin/lib/api/gateway-degradation.ts
?? apps/gateway-admin/lib/api/marketplace-acp-client.test.ts
?? apps/gateway-admin/lib/api/service-action-client.test.ts
?? apps/gateway-admin/lib/dev/
?? apps/gateway-admin/lib/marketplace/api-client.test.ts
?? crates/lab/src/acp/providers.rs
?? crates/lab/src/audit/checks/ui_schema.rs
?? crates/lab/src/dispatch/marketplace/stash_meta.rs
?? crates/lab/src/dispatch/marketplace/update.rs
?? docs/NODE_RUNTIME_CONTRACT.md
?? docs/design/component-development.md
?? docs/features/
?? plugins/skills/quick-push/
?? plugins/skills/save-to-md/
```

## git log --oneline --name-only -10

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
