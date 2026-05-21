---
date: 2026-04-25 23:56:31 EST
repo: git@github.com:jmagar/lab.git
branch: bd-security/marketplace-p1-fixes
head: 2f6d76c6
agent: Codex
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab                                   2f6d76c6 [bd-security/marketplace-p1-fixes]
pr: #29 fix(marketplace): P1 security fixes - path traversal, symlink following, installPath validation https://github.com/jmagar/lab/pull/29
---

# Session: Marketplace V2 Migration And Component Development Process

## User Request

The session started with Marketplace redesign work. The user corrected the interpretation of ACP Agents: ACP Agents are implementations of the Agent Client Protocol, not Lab plugin components. The user then asked for Marketplace v2 to use the Gateway page filter/sidebar patterns, remove the old tab sets, support card/table density modes, wire live data, and eventually migrate the redesign from `/dev/marketplace` to `/marketplace`.

The current request was to save the entire session as a Markdown document with concrete repo and git context.

## Session Overview

This session covered Marketplace v2 design, live preview setup, dev-route safety, component-development documentation, production migration, MCP Registry aggregation fixes, MCP install target redesign, ACP/Claude/Codex wiring concerns, and verification.

By the end of the session, Marketplace v2 had been moved toward the production `/marketplace` route, the `/dev/marketplace` preview route had been removed from the app tree, catalog state had been normalized so plugin packages and bundled components are counted separately, MCP install targets had been changed from arbitrary Lab services to Lab Gateway plus Claude/Codex device targets, and backend MCP listing was changed to use the local aggregate registry store instead of only the upstream registry default page.

## Sequence of Events

1. The user clarified that ACP Agents are Agent Client Protocol implementations such as Codex CLI, Gemini CLI, Cline, Goose, OpenHands, and similar clients/agents.
2. The user asked what installing an agent does, whether it becomes available in `/chat`, why Codex did not show installed, and whether richer ACP Registry data could enrich Marketplace agent cards.
3. The user asked to copy the Gateway page pattern for Marketplace: a left filter rail with checkbox filters, removal of both existing tab sets, and a card/table display switch.
4. A live mockup workflow was started at `/dev/marketplace`.
5. The user reported `/dev/marketplace` was showing the Overview page or an unrelated mockup error. The lab serve and web build/watch behavior was investigated.
6. The user asked whether `lab serve` was running the intended command: `cargo run --all-features --bin lab -- serve --host 0.0.0.0 --port 8765`.
7. The user identified a systemd setup for the binary and asked to disable it during active development.
8. The user confirmed `/dev/marketplace` showed the Marketplace preview.
9. The user asked for a component development process document based on the Marketplace workflow.
10. `docs/design/component-development.md` was created or updated to define the design-spec, `/dev/<feature>`, read-only preview, implementation, review, and design-system deviation workflow.
11. The user approved returning to Marketplace redesign work.
12. Marketplace v2 development continued, including design-system alignment questions around typography, spacing, dropdowns, checkboxes, scrollbars, and colors.
13. Explicit typography and badge tokens were added to the design system contract, and the user confirmed `docs/design/design-system-contract.md` was updated.
14. The user approved documenting Gateway-style checkbox filter rows as an accepted Marketplace/Gateway filter-rail pattern, cleaning spacing, using Aurora scrollbars where applicable, and keeping v2 on `/dev/marketplace` until mutation flows were production-ready.
15. The user corrected component counting: bundled agents, skills, and commands should remain their own item kinds, not be counted as plugins.
16. The user warned not to remove Axum routes during router work.
17. The user asked to migrate Marketplace v1 to Marketplace v2 and then clarified that the original Marketplace code should be updated directly.
18. The user stated `/dev/marketplace` should go away once `/marketplace` is migrated.
19. MCP install, ACP wiring, and bundled component action flows were discussed. The user asked why those were not wired and clarified that clicking Marketplace cards should open the appropriate components/actions.
20. MCP install target UX was corrected: MCP servers should install to the Lab Gateway, Claude/Codex on the local device, or Claude/Codex on remote devices, not to every Lab service.
21. The user reported Marketplace showed zero installed agents/commands/skills, 291 packages, and incorrect plugin counts. The user asked to stop counting individual bundled components as plugins.
22. The user asked to remove the nine summary cards showing installed counts.
23. The user asked to debug why MCP servers always showed the same entries regardless of filters/sorting.
24. A first frontend-only change made `mcp.list` request `limit: 100`; the user rejected that as insufficient because Lab should cache and aggregate the full registry.
25. Backend `mcp.list` was changed to read from the aggregate local registry store, sync if empty, aggregate all local pages when no explicit pagination is requested, and return latest-only MCP server rows by default.
26. When a missing `latest_only` initializer required touching `crates/lab/src/api/services/registry_v01.rs`, the user asked whether routes had been changed. The response clarified that only the struct initializer was updated, not route mounts.
27. A reviewer output was provided showing lint fixes, formatting, cargo check, clippy, gateway-admin lint/tests, and diff-check verification. That review output also noted GitHub PR fetch and `nextest`/`tsx` sandbox caveats.
28. The user asked to save the full session as a Markdown document with concrete repo and git context.

## Key Findings

- Marketplace v2 defaults to sorting by recently updated items via `DEFAULT_FILTERS.sort = 'updated'` in `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx:69`.
- Marketplace type filters explicitly include plugins, agents, skills, commands, MCP servers, ACP agents, apps, hooks, assets, files, config, monitors, output styles, and sources in `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx:80`.
- Card and table rows are clickable and call the shared item action handler in `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx:196` and `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx:253`.
- MCP install modal target state now includes Lab Gateway and Claude/Codex client targets in `apps/gateway-admin/components/marketplace/mcp-install-modal.tsx:25`.
- MCP install requests send `gateway_ids` and `client_targets` rather than arbitrary service targets in `apps/gateway-admin/components/marketplace/mcp-install-modal.tsx:101`.
- MCP install UI loads fleet devices from the backend before rendering remote Claude/Codex targets in `apps/gateway-admin/components/marketplace/mcp-install-modal.tsx:49`.
- MCP list dispatch now opens the local registry store, ensures it is populated, and lists from the store rather than directly returning one upstream page in `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs:124`.
- MCP list aggregates all local store pages when the caller does not explicitly pass pagination parameters in `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs:129`.
- MCP store list params gained `latest_only` so default Marketplace MCP results can avoid duplicate historical versions in `crates/lab/src/dispatch/marketplace/store.rs:80`.
- Store-side SQL applies `s.is_latest = 1` when `latest_only` is enabled in `crates/lab/src/dispatch/marketplace/store.rs:489`.
- MCP install dispatch validates that at least one gateway or client target is selected in `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs:277`.
- MCP install dispatch sends `mcp.install` RPCs to selected device nodes for Claude/Codex targets in `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs:339`.
- Node install code contains explicit MCP client enum variants for Claude and Codex in `crates/lab/src/node/install.rs:38`.
- Plugin component classification maps bundled `agents`, `skills`, `commands`, and `mcp_servers` to distinct Marketplace item kinds instead of `plugin` in `apps/gateway-admin/components/marketplace/marketplace-state.ts:176`.
- MCP entries are deduplicated by identifier and prefer latest/newer rows in `apps/gateway-admin/components/marketplace/marketplace-state.ts:215`.
- Plugin rows and plugin component rows are built separately in `apps/gateway-admin/components/marketplace/marketplace-state.ts:296`.
- Summary counts now count only `item.kind === 'plugin'` as plugins in `apps/gateway-admin/components/marketplace/marketplace-state.ts:375`.
- Filtering applies lens, search, type, install state, ecosystem, source, and distribution checks in `apps/gateway-admin/components/marketplace/marketplace-state.ts:435`.

## Technical Decisions

- Marketplace v2 should replace the production `/marketplace` implementation rather than remain a parallel `/dev/marketplace` preview after migration.
- `/dev/<feature>` remains the correct workflow for future mockups and development previews, but `/dev/marketplace` should not survive once Marketplace v2 is the production Marketplace.
- `/dev/*` previews must remain live and read-only because `/dev` is not OAuth-protected.
- Gateway-style checkbox filter rows were accepted as the correct filter-rail pattern for Marketplace instead of forcing all filters into pill controls.
- Marketplace should default to recently updated items across plugins, MCP servers, and ACP agents.
- Individual bundled components should be first-class catalog items of their own kinds, while plugin package counts should only count actual plugin packages.
- MCP Registry data should come from Lab's aggregate local cache/store, not from a single upstream default page.
- MCP Registry duplicate versions should be hidden by default in Marketplace by using latest-only store filtering.
- MCP server installs should target Lab Gateway and Claude/Codex clients on local or remote devices, not arbitrary Lab service integrations.
- Card clicks should open the appropriate action path: plugin install/update/remove, MCP install, ACP wiring, or component install/preview.
- Axum route mounts should not be removed as part of Marketplace refactors.

## Files Modified

The following files were dirty at session save time. Some changes came from this Marketplace session; some were already present or modified by reviewer/other work before this save request. Purpose is recorded only where observed during this session.

| File | Status | Purpose / observed context |
| --- | --- | --- |
| `Cargo.lock` | modified | Dependency lockfile changed before save; exact purpose not re-audited during save. |
| `Cargo.toml` | modified | Workspace/package dependency metadata changed before save; exact purpose not re-audited during save. |
| `apps/gateway-admin/app/dev/marketplace/page.tsx` | deleted | Removed `/dev/marketplace` preview route after migrating Marketplace v2 toward production `/marketplace`. |
| `apps/gateway-admin/app/dev/page.tsx` | modified | Dev index updated during `/dev/<feature>` preview work. |
| `apps/gateway-admin/components/ai/agent.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/artifact.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/attachments.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/code-block.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/commit.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/confirmation.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/context.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/environment-variables.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/file-tree.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/inline-citation.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/package-info.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/prompt-input.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/queue.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/sandbox.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/schema-display.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/snippet.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/stack-trace.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/task.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/test-results.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/tool.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/ai/web-preview.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/aurora/tokens.ts` | modified | Added/updated Aurora typography/badge token helpers used by Marketplace and design-system cleanup. |
| `apps/gateway-admin/components/chat/activity-card.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/chat/chat-input.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/chat/tool-artifact-panels.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/gateway/gateway-detail-content.tsx` | modified | Gateway pattern/reference related work; exact diff not re-audited during save. |
| `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx` | modified | Gateway pattern/reference related work; exact diff not re-audited during save. |
| `apps/gateway-admin/components/gateway/gateway-list-content.tsx` | modified | Gateway pattern/reference related work; exact diff not re-audited during save. |
| `apps/gateway-admin/components/gateway/gateway-table.tsx` | modified | Gateway pattern/reference related work; exact diff not re-audited during save. |
| `apps/gateway-admin/components/logs/log-console.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/marketplace/marketplace-card.tsx` | modified | Marketplace card UI compatibility work. |
| `apps/gateway-admin/components/marketplace/marketplace-list-content.tsx` | modified | Production Marketplace v2 UI, filters, card/table mode, action handling, removal of summary cards. |
| `apps/gateway-admin/components/marketplace/marketplace-v2-content.tsx` | deleted | Removed temporary v2 component after migration into the main Marketplace component. |
| `apps/gateway-admin/components/marketplace/marketplace-v2-state.test.ts` | deleted | Removed temporary v2 state tests after renaming/migrating state into main Marketplace state. |
| `apps/gateway-admin/components/marketplace/marketplace-v2-state.ts` | deleted | Removed temporary v2 state module after renaming/migrating state into main Marketplace state. |
| `apps/gateway-admin/components/marketplace/mcp-install-modal.tsx` | modified | MCP install target UI changed to Lab Gateway plus Claude/Codex fleet device targets. |
| `apps/gateway-admin/components/marketplace/mkt-source-card.tsx` | modified | Marketplace source card compatibility work; exact diff not re-audited during save. |
| `apps/gateway-admin/components/marketplace/plugin-detail-content.tsx` | modified | Marketplace detail/action compatibility work; exact diff not re-audited during save. |
| `apps/gateway-admin/components/marketplace/marketplace-state.ts` | added | Main Marketplace catalog normalization, filtering, sorting, MCP dedupe, and summary logic. |
| `apps/gateway-admin/components/marketplace/marketplace-state.test.ts` | added | Tests for catalog state, component counting, sorting, filtering, and MCP dedupe behavior. |
| `apps/gateway-admin/components/registry/registry-list-content.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/components/registry/server-detail-panel.tsx` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/docs/marketplace-catalog.md` | modified | Marketplace catalog/design documentation updates. |
| `apps/gateway-admin/eslint.config.mjs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/lib/acp/persistence.ts` | modified | ACP persistence/wiring work; exact diff not re-audited during save. |
| `apps/gateway-admin/lib/acp/providers/codex-acp.ts` | modified | Codex ACP provider wiring work; exact diff not re-audited during save. |
| `apps/gateway-admin/lib/api/gateway-request.ts` | modified | Gateway API request support work; exact diff not re-audited during save. |
| `apps/gateway-admin/lib/auth/auth-mode.ts` | modified | OAuth/dev auth mode investigation or support work; exact diff not re-audited during save. |
| `apps/gateway-admin/lib/chat/use-session-events.test.ts` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/lib/chat/use-session-events.ts` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/lib/dev/preview-mode.test.ts` | modified | `/dev/*` read-only preview guard tests. |
| `apps/gateway-admin/lib/fs/client.test.ts` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/lib/hooks/use-marketplace.ts` | modified | Marketplace hooks/data loading updates for v2 and MCP list behavior. |
| `apps/gateway-admin/lib/marketplace/api-client.ts` | modified | Marketplace API client updates, including MCP install payload changes. |
| `apps/gateway-admin/package.json` | modified | Frontend dependency/script metadata changed before save; exact purpose not re-audited during save. |
| `apps/gateway-admin/pnpm-lock.yaml` | modified | Frontend lockfile changed before save; exact purpose not re-audited during save. |
| `apps/gateway-admin/public/apple-icon.png` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/public/icon-dark-32x32.png` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/public/icon-light-32x32.png` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/public/icon.svg` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `apps/gateway-admin/app/favicon.ico` | added | New frontend favicon asset at save time; exact purpose not re-audited during save. |
| `crates/lab-apis/src/acp_registry/client.rs` | modified | ACP Registry client enrichment work. |
| `crates/lab-apis/src/acp_registry/types.rs` | modified | ACP Registry type enrichment work. |
| `crates/lab-apis/src/extract/client.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab-apis/src/extract/parsers/radarr.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab-apis/src/extract/runtime.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab-apis/src/extract/ssh_config.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab-apis/src/extract/transport.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab-apis/tests/gotify_client.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab-apis/tests/http_client.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab-apis/tests/tailscale_client.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab-apis/tests/unifi_client.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab-auth/src/authorize.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab-auth/src/google.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/Cargo.toml` | modified | Lab crate dependency/feature metadata changed before save; exact purpose not re-audited during save. |
| `crates/lab/src/api/dev_mockups.rs` | deleted | Removed old dev mockup module after `/dev/marketplace` migration/removal work. |
| `crates/lab/src/api/nodes/fleet.rs` | modified | Fleet device API support used by MCP install target selection. |
| `crates/lab/src/api/router.rs` | modified | Router touched by prior dev mockup/route work and reviewer formatting; user explicitly warned not to remove routes. |
| `crates/lab/src/api/services/fs.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/api/services/gateway.rs` | modified | Gateway API support work; exact diff not re-audited during save. |
| `crates/lab/src/api/services/marketplace.rs` | modified | Marketplace API surface updates for v2 and read-only/live data paths. |
| `crates/lab/src/api/services/registry_v01.rs` | modified | Added `latest_only: false` when constructing store list params so v0.1 route behavior stays explicit. |
| `crates/lab/src/api/state.rs` | modified | API state support for registry/marketplace data. |
| `crates/lab/src/api/web.rs` | modified | Web serving/dev route behavior touched during serve/mockup work. |
| `crates/lab/src/cli.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/cli/doctor.rs` | modified | Reviewer formatting or dirty pre-existing changes; exact purpose not re-audited during save. |
| `crates/lab/src/cli/gateway.rs` | modified | Gateway CLI support work; exact diff not re-audited during save. |
| `crates/lab/src/cli/serve.rs` | modified | `lab serve`/web-watch behavior investigation and changes. |
| `crates/lab/src/config.rs` | modified | Config/dev serve support work; exact diff not re-audited during save. |
| `crates/lab/src/dispatch/bytestash.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/dispatch/doctor.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/dispatch/doctor/catalog.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/dispatch/doctor/dispatch.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/dispatch/doctor/service.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/dispatch/doctor/system.rs` | modified | Reviewer formatting or dirty pre-existing changes; exact purpose not re-audited during save. |
| `crates/lab/src/dispatch/gateway/dispatch.rs` | modified | Gateway dispatch support used by MCP install to Lab Gateway. |
| `crates/lab/src/dispatch/marketplace/backends/claude.rs` | modified | Claude plugin/component metadata and installed-state Marketplace backend work. |
| `crates/lab/src/dispatch/marketplace/client.rs` | modified | Marketplace dispatch client support. |
| `crates/lab/src/dispatch/marketplace/dispatch.rs` | modified | Marketplace action routing updates. |
| `crates/lab/src/dispatch/marketplace/mcp_catalog.rs` | modified | MCP action catalog updates. |
| `crates/lab/src/dispatch/marketplace/mcp_dispatch.rs` | modified | MCP list aggregation and MCP install target routing. |
| `crates/lab/src/dispatch/marketplace/mcp_params.rs` | modified | MCP param parsing updates. |
| `crates/lab/src/dispatch/marketplace/params.rs` | modified | Marketplace param types updates. |
| `crates/lab/src/dispatch/marketplace/patch.rs` | modified | Marketplace patch/install support. |
| `crates/lab/src/dispatch/marketplace/store.rs` | modified | Aggregate registry store, latest-only filter, listing/sync behavior. |
| `crates/lab/src/dispatch/unifi/client.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/mcp/logging.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/mcp/server.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/node/install.rs` | modified | Node-side component, ACP agent, and MCP Claude/Codex install handling. |
| `crates/lab/src/node/log_store.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/node/ws_client.rs` | modified | Node RPC/error handling used by remote install paths. |
| `crates/lab/src/oauth/local_relay.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/tui/ecosystem.rs` | modified | Dirty at save time; exact purpose not re-audited during save. |
| `crates/lab/src/tui/marketplace.rs` | modified | Marketplace TUI support updates. |
| `docs/OAUTH.md` | modified | OAuth/dev-route documentation updates. |
| `docs/OPERATIONS.md` | modified | Operations documentation updates. |
| `docs/design/design-system-contract.md` | modified | Added explicit typography/badge/token guidance for Marketplace/Gateway UI consistency. |
| `scripts/check-oauth.sh` | added | OAuth check helper at save time; exact purpose not re-audited during save. |
| `docs/sessions/2026-04-25-marketplace-v2-session.md` | added | This session document. |

## Commands Executed

| Command | Result |
| --- | --- |
| `TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'` | Returned `2026-04-25 23:56:31 EST`. |
| `git remote get-url origin` | Returned `git@github.com:jmagar/lab.git`. |
| `git branch --show-current` | Returned `bd-security/marketplace-p1-fixes`. |
| `git rev-parse --short HEAD` | Returned `2f6d76c6`. |
| `git log --oneline -5` | Returned recent commits headed by `2f6d76c6 docs: setup+settings feature design spec + component-development doc update`. |
| `git status --short` | Returned the dirty file list captured in **Files Modified**. |
| `git log --oneline --name-only -10` | Returned recent commit/file history, including Marketplace v2, dev mockup, nodeinfo, and component-development commits. |
| `pwd` | Returned `/home/jmagar/workspace/lab`. |
| `git worktree list | grep $(pwd) | head -1` | Returned `/home/jmagar/workspace/lab                                   2f6d76c6 [bd-security/marketplace-p1-fixes]`. |
| `gh pr view --json number,title,url 2>/dev/null || echo "none"` | Returned PR #29 JSON for the P1 Marketplace security fixes PR. |
| `cargo check --all-features --bin lab` | Passed after MCP install/config changes. |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features node::install::tests::render_ -- --nocapture` | Passed two node install render tests in lib/main targets. |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features dispatch::marketplace::store::tests::list_servers_can_filter_to_latest_versions -- --nocapture` | Passed focused latest-version store test in lib/main targets. |
| `pnpm --dir apps/gateway-admin exec tsx --test components/marketplace/marketplace-state.test.ts lib/dev/preview-mode.test.ts lib/api/marketplace-acp-client.test.ts` | Passed 21 tests. |
| `pnpm --dir apps/gateway-admin exec tsx --test components/marketplace/marketplace-state.test.ts` | Passed 11 tests. |
| `pnpm --dir apps/gateway-admin build` | Passed after retrying when a prior Next build lock/process cleared. |
| `rustfmt --edition 2024 --check crates/lab/src/dispatch/marketplace/store.rs crates/lab/src/dispatch/marketplace/mcp_dispatch.rs crates/lab/src/api/services/registry_v01.rs` | Passed. |
| `git diff --check` | Passed for the touched relevant files when run during verification. |
| `curl http://127.0.0.1:8765/v1/marketplace` | Returned `auth_failed` without bearer/session auth. |
| `curl /dev/api/marketplace` equivalent diagnostic | Succeeded for read-only Marketplace diagnostics and showed pre-fix catalog shape including 291 plugin/package count and empty component kinds. |

## Errors Encountered

- `cargo fmt --all --check` initially failed because existing dirty files such as `crates/lab/src/api/router.rs`, `crates/lab/src/cli/doctor.rs`, and `crates/lab/src/dispatch/doctor/system.rs` needed formatting. At that point, blanket formatting was avoided because the user had warned not to disturb Axum routes.
- Running `rustfmt` without an edition failed on async code because the default edition was too old. The command was corrected to `rustfmt --edition 2024 --check ...`.
- A Cargo test command using `-p lab` was ambiguous because more than one package named `lab` was present. The command was corrected to use `--manifest-path crates/lab/Cargo.toml`.
- `cargo check` failed after `StoreListParams.latest_only` was added because `crates/lab/src/api/services/registry_v01.rs` constructed `StoreListParams` without the new field. The fix was to add `latest_only: false` to that initializer.
- `pnpm --dir apps/gateway-admin build` failed once while another Next build process still held a lock. The lock was not manually deleted; the build was retried later and passed.
- A direct `/v1/marketplace` curl returned `auth_failed`, which was expected without bearer/session auth. `/dev/api/marketplace` was used for read-only diagnostics.

## Behavior Changes (Before/After)

| Area | Before | After |
| --- | --- | --- |
| Marketplace route | V2 existed as a `/dev/marketplace` preview while production Marketplace still had v1 behavior. | V2 work was migrated into the main Marketplace component path and `/dev/marketplace` app route was deleted. |
| Marketplace navigation | Separate tab sets existed for Browse/Installed/Marketplaces and All/Plugins/MCP Servers/ACP Agents. | Marketplace uses a Gateway-style sidebar/filter rail and card/table view controls. |
| Default ordering | Marketplace did not default to recently updated across all item sources. | Default sort is `updated`. |
| Plugin counting | Bundled components could be counted as plugins/packages. | Plugin package rows and bundled component rows are distinct; only `kind === 'plugin'` counts as a plugin. |
| Component item kinds | Agents, skills, commands, and MCP config could be collapsed into plugin/package semantics. | Component kinds map to `agent`, `skill`, `command`, `mcp_server`, and other explicit catalog kinds. |
| Summary cards | Marketplace showed a block of count cards the user did not want. | The summary cards were removed from the v2 Marketplace view. |
| MCP list data | Frontend initially requested a limited upstream page, which repeated the same servers. | Backend `mcp.list` uses the local aggregate store, syncs if empty, aggregates all store pages when unpaginated, and defaults to latest-only rows. |
| MCP install targets | MCP install UI offered Lab services as targets. | MCP install targets are Lab Gateway and Claude/Codex clients on connected fleet devices. |
| Dev preview process | `/dev` preview/read-only rules were implicit. | `docs/design/component-development.md` documents live read-only `/dev/<feature>` previews and frontend/backend read-only guard requirements. |

## Verification Evidence

| Command | Expected | Actual | Status |
| --- | --- | --- | --- |
| `cargo check --all-features --bin lab` | Rust all-features lab binary type-checks. | Passed. | Pass |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features node::install::tests::render_ -- --nocapture` | Node install rendering tests pass. | Passed two focused tests in lib/main targets. | Pass |
| `cargo test --manifest-path crates/lab/Cargo.toml --all-features dispatch::marketplace::store::tests::list_servers_can_filter_to_latest_versions -- --nocapture` | Latest-only store filter test passes. | Passed in lib/main targets. | Pass |
| `pnpm --dir apps/gateway-admin exec tsx --test components/marketplace/marketplace-state.test.ts lib/dev/preview-mode.test.ts lib/api/marketplace-acp-client.test.ts` | Marketplace state, dev preview guard, and ACP client tests pass. | Passed 21 tests. | Pass |
| `pnpm --dir apps/gateway-admin exec tsx --test components/marketplace/marketplace-state.test.ts` | Marketplace state tests pass after final state changes. | Passed 11 tests. | Pass |
| `pnpm --dir apps/gateway-admin build` | Gateway admin production build succeeds. | Passed after retrying past an existing Next build lock/process. | Pass |
| `rustfmt --edition 2024 --check crates/lab/src/dispatch/marketplace/store.rs crates/lab/src/dispatch/marketplace/mcp_dispatch.rs crates/lab/src/api/services/registry_v01.rs` | Touched Rust files are formatted. | Passed. | Pass |
| `git diff --check` | No whitespace errors in diff. | Passed for relevant touched files during verification. | Pass |

## Risks and Rollback

- The worktree is broadly dirty. Not every dirty file was re-audited during this save request, so commits should be reviewed carefully before staging.
- Marketplace v2 has been migrated toward production `/marketplace`; rollback is to restore the prior Marketplace component route and reintroduce `/dev/marketplace` only if an isolated preview is needed.
- MCP install to Claude/Codex devices depends on fleet device connectivity and node RPC behavior. Rollback is to disable client target selection in the modal and keep Lab Gateway install only.
- MCP Registry latest-only behavior is intentional for Marketplace, but `/v0.1/servers` was kept explicit with `latest_only: false` to avoid changing that route's expected behavior.
- Reviewer output reported `cargo nextest run --workspace --all-features` was blocked in that environment by local socket bind denial for OAuth/websocket tests.

## Decisions Not Taken

- Did not keep `/dev/marketplace` as the long-term Marketplace preview route after the user clarified production migration should remove it.
- Did not continue with a frontend-only `limit: 100` MCP Registry workaround after the user clarified Lab should aggregate the entire registry cache.
- Did not silently hide mutation controls in `/dev/*`; the documented process keeps controls reviewable while blocking actual mutation paths.
- Did not convert Gateway-style filter rail rows to pill-only filters after the user approved documenting the rail pattern.
- Did not remove Axum routes after the user explicitly warned against route removal.

## References

- `docs/design/design-system-contract.md`
- `docs/design/component-development.md`
- `apps/gateway-admin/docs/marketplace-catalog.md`
- PR #29: `https://github.com/jmagar/lab/pull/29`
- MCP Registry Marketplace data via local Lab registry store.
- ACP Agents list supplied by the user in the chat transcript.

## Open Questions

- No transcript path or session identifier was exposed by the current environment, so metadata omits `session id` and `transcript`.
- No active plan path was observed during the save request, so metadata omits `plan`.
- Exact ownership of every dirty file was not fully re-audited during the save request.
- ACP agent install semantics still need final product-level confirmation: whether wiring an ACP agent only records/launches an external ACP implementation, whether it appears in `/chat`, and how Codex installed-state should be detected.
- Full end-to-end remote Claude/Codex MCP install behavior still needs live device verification.

## Next Steps

Unfinished work from this session:

- Finish Marketplace production migration on `/marketplace`.
- Remove or clean any remaining temporary v2/dev-only code paths that are no longer needed.
- Complete card click behavior for every item kind: plugin, bundled component, MCP server, ACP agent, and marketplace source.
- Wire bundled component install flows for agents, commands, skills, and MCP config components.
- Complete ACP agent install/wiring semantics and installed-state detection, including Codex.
- Verify MCP Registry filters and sorting in the live UI against aggregate store data.
- Run Chrome DevTools/browser review over `/marketplace` in desktop/mobile and dark/light modes.
- Re-run design-system compliance after final UI changes.
- Re-run the widest feasible repo verification before commit/PR update.

Follow-on tasks not yet started:

- Add stronger UI tests for Marketplace card actions and install modal target selection.
- Add backend integration coverage for MCP install target validation and client-target RPC dispatch.
- Add live fleet-device validation for Claude and Codex MCP config patching.
- Review and clean the broad dirty worktree before staging or committing.
