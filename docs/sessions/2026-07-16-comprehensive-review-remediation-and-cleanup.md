---
date: 2026-07-16 11:10:19 EST
repo: git@github.com:jmagar/labby.git
branch: main
head: e9c6577ac310fa65c9e391aca78d88c262cd8006
session id: 2120645e-b2e9-4faf-8e34-dcb428e9102e
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/2120645e-b2e9-4faf-8e34-dcb428e9102e.jsonl
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
pr: "#248 fix: remediate comprehensive project review (merged)"
beads: lab-5j67j, lab-fpsq9
---

# Comprehensive review, remediation, merge, and repository cleanup

## User Request

Audit the entire project with the full comprehensive-review workflow without stopping, then dispatch parallel agents to fix every P0, P1, P2, and P3 finding. Finally, stage, commit, push, merge into <code>main</code>, synchronize the intentional <code>marketplace-no-mcp</code> branch, and leave only those two clean branches/worktrees.

## Session Overview

The review covered the entire tracked Lab project and validated 49 unique findings: 1 P0, 10 P1, 31 P2, and 7 P3. Parallel remediation resolved every code-, test-, documentation-, CI-, release-, performance-, and repository-controlled item, plus 13 additional delivery findings discovered during validation. The result landed through [PR #248](https://github.com/jmagar/labby/pull/248) as <code>e9c6577a</code>, with post-merge CI, Incus publication, and no-MCP synchronization green.

One operator-controlled security action remains: rotate the MCP Registry DNS publishing key and replace the GitHub secret. That work is tracked in <code>lab-fpsq9</code>; a published-history rewrite was deliberately not attempted without separate destructive authorization.

The injected transcript is a 2,209-line prior Claude session covering OAuth routing and release work from July 14–15. It was read during this closeout, but current-review facts below come from the live Git repository, review artifacts, GitHub, CI, and bead state rather than treating that older transcript as evidence for this Codex session.

## Sequence of Events

1. Established the live target, repository state, long-lived branch rules, project instructions, and all-features verification contract.
2. Ran the full-project review across quality, architecture, security, performance, testing, documentation/contracts, Rust/React practices, CI/CD, and operations; consolidated overlaps into 49 findings.
3. Dispatched parallel agents across independent remediation surfaces, integrated their changes, and repeatedly reviewed the combined tree.
4. Fixed all P0–P3 repository-controlled issues and 13 additional delivery findings exposed by tests, automated review, Windows, browser, release, and Incus validation.
5. Ran the full Rust, frontend, browser, Windows, security, docs, CI-policy, release, and image-publication verification matrix.
6. Pushed the remediation branch, merged PR #248 into <code>main</code>, deleted the merged feature refs/worktree, synchronized <code>marketplace-no-mcp</code>, and verified both retained worktrees were clean.
7. Closed review bead <code>lab-5j67j</code>, created follow-up bead <code>lab-fpsq9</code> for the external key rotation, and saved this docs-only artifact to <code>main</code>.

## Key Findings

- The final review report records the 49-finding distribution and baseline at [.full-review/05-final-report.md](../../.full-review/05-final-report.md#L7-L19); the remediation contract explicitly kept F-01 through F-49 in scope at [.full-review/05-final-report.md](../../.full-review/05-final-report.md#L243-L247).
- Active credential material had been published and suppressed by the leak baseline; the current tree was redacted and scanning policy repaired, while DNS publisher key rotation remains an external operator action ([report lines 37–43](../../.full-review/05-final-report.md#L37-L43)).
- Authorization and admin-policy drift existed across Setup, Doctor, MCP, HTTP, generated contracts, and the local bootstrap capability ([report lines 45–85](../../.full-review/05-final-report.md#L45-L85)).
- Cancellation and partial-commit hazards existed in gateway reload/save and Code Mode execution; runtime replacement now preserves the live pool until a ready replacement can be swapped, and request state has bounded/drop-safe cleanup ([report lines 59–73](../../.full-review/05-final-report.md#L59-L73)).
- CI and release surfaces could report success or publish before all critical gates completed; the final workflows aggregate required tests, pin actions/tools, validate release inputs, preserve rollback targets, and publish through a validated draft/tag handoff.
- The later delivery pass found and fixed 13 more issues, including restored-inspector minimization, iframe shrinking, Windows ACL module inheritance, cargo-deny drift, Node runtime pins, distrobuilder probing, bootstrap proxy scope, zero-limit pagination, cursor exposure, rollback preservation, and release-draft promotion ([report lines 21–35](../../.full-review/05-final-report.md#L21-L35)).

## Technical Decisions

- Preserved <code>marketplace-no-mcp</code> as an intentional long-lived variant and synchronized it from <code>main</code>; it was neither merged into <code>main</code> nor deleted.
- Kept business and policy logic in neutral/shared layers, with CLI, MCP, HTTP, and web surfaces remaining adapters; new architecture tests enforce those boundaries.
- Used fail-closed, metadata-derived admin policy and generated-contract parity instead of service-specific authorization allowlists.
- Used build-beside-live plus atomic swap for gateway reload, bounded guards for Code Mode, keyset/cursor pagination for usage, and backup-first atomic secret/config writes.
- Kept destructive external actions out of scope: key rotation requires Cloudflare/GitHub authority, and rewriting published history requires explicit separate authorization.
- Landed remediation by squash merge through PR #248, then performed explicit branch/worktree and remote ancestry checks before cleanup.

## Files Changed

The remediation squash changed 157 paths: 39 created, 117 modified, and 1 deleted.

| status | path | previous path | purpose | evidence |
|---|---|---|---|---|
| created | <code>.full-review/00-scope.md</code> | — | Review evidence and remediation tracking | <code>e9c6577a</code> |
| created | <code>.full-review/01-quality-architecture.md</code> | — | Review evidence and remediation tracking | <code>e9c6577a</code> |
| created | <code>.full-review/02-security-performance.md</code> | — | Review evidence and remediation tracking | <code>e9c6577a</code> |
| created | <code>.full-review/03-testing-documentation.md</code> | — | Review evidence and remediation tracking | <code>e9c6577a</code> |
| created | <code>.full-review/04-best-practices.md</code> | — | Review evidence and remediation tracking | <code>e9c6577a</code> |
| created | <code>.full-review/05-final-report.md</code> | — | Review evidence and remediation tracking | <code>e9c6577a</code> |
| created | <code>.full-review/06-remediation.md</code> | — | Review evidence and remediation tracking | <code>e9c6577a</code> |
| created | <code>.full-review/state.json</code> | — | Review evidence and remediation tracking | <code>e9c6577a</code> |
| modified | <code>.github/actions/build-gateway-admin/action.yml</code> | — | CI, release, or branch-sync integrity | <code>e9c6577a</code> |
| modified | <code>.github/workflows/build-incus-image.yml</code> | — | CI, release, or branch-sync integrity | <code>e9c6577a</code> |
| modified | <code>.github/workflows/check-no-mcp-drift.yml</code> | — | CI, release, or branch-sync integrity | <code>e9c6577a</code> |
| modified | <code>.github/workflows/ci.yml</code> | — | CI, release, or branch-sync integrity | <code>e9c6577a</code> |
| modified | <code>.github/workflows/openwiki-update.yml</code> | — | CI, release, or branch-sync integrity | <code>e9c6577a</code> |
| modified | <code>.github/workflows/release-please.yml</code> | — | CI, release, or branch-sync integrity | <code>e9c6577a</code> |
| modified | <code>.github/workflows/release.yml</code> | — | CI, release, or branch-sync integrity | <code>e9c6577a</code> |
| modified | <code>.github/workflows/sync-marketplace-no-mcp.yml</code> | — | CI, release, or branch-sync integrity | <code>e9c6577a</code> |
| modified | <code>.gitleaksignore</code> | — | Secret-scanning policy | <code>e9c6577a</code> |
| modified | <code>Cargo.lock</code> | — | Dependency and advisory policy | <code>e9c6577a</code> |
| modified | <code>Cargo.toml</code> | — | Dependency and advisory policy | <code>e9c6577a</code> |
| modified | <code>apps/gateway-admin/components/code-mode-app/code-mode-inspector.test.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| modified | <code>apps/gateway-admin/components/code-mode-app/code-mode-inspector.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-config-editor.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-custom-connection-form.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| modified | <code>apps/gateway-admin/components/gateway/gateway-detail-content.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-enabled-setting.test.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-enabled-setting.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| modified | <code>apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| modified | <code>apps/gateway-admin/components/gateway/gateway-form-dialog.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-form-oauth.ts</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-form-state.test.ts</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-form-state.ts</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-lab-service-form.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| modified | <code>apps/gateway-admin/components/gateway/gateway-list-content.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-save-transaction.test.ts</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-save-transaction.ts</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/gateway-service-fields.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/use-stable-tool-exposure.test.tsx</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/components/gateway/use-stable-tool-exposure.ts</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| modified | <code>apps/gateway-admin/lib/browser/gateway-detail.browser.test.ts</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| modified | <code>apps/gateway-admin/lib/hooks/use-gateways.ts</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| modified | <code>apps/gateway-admin/package.json</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| created | <code>apps/gateway-admin/scripts/check-route-bundle-budgets.mjs</code> | — | Gateway Admin correctness, UX, testing, or bundle budgets | <code>e9c6577a</code> |
| modified | <code>apps/palette-tauri/.gitignore</code> | — | Palette correctness, testing, or dependency security | <code>e9c6577a</code> |
| modified | <code>apps/palette-tauri/src-tauri/Cargo.lock</code> | — | Palette correctness, testing, or dependency security | <code>e9c6577a</code> |
| created | <code>apps/palette-tauri/src-tauri/deny.toml</code> | — | Palette correctness, testing, or dependency security | <code>e9c6577a</code> |
| modified | <code>apps/palette-tauri/src-tauri/src/labby_bridge.rs</code> | — | Palette correctness, testing, or dependency security | <code>e9c6577a</code> |
| modified | <code>apps/palette-tauri/src-tauri/src/lib.rs</code> | — | Palette correctness, testing, or dependency security | <code>e9c6577a</code> |
| modified | <code>apps/palette-tauri/src-tauri/src/oauth.rs</code> | — | Palette correctness, testing, or dependency security | <code>e9c6577a</code> |
| modified | <code>apps/palette-tauri/src-tauri/src/oauth_tests.rs</code> | — | Palette correctness, testing, or dependency security | <code>e9c6577a</code> |
| modified | <code>apps/palette-tauri/src-tauri/src/persistence.rs</code> | — | Palette correctness, testing, or dependency security | <code>e9c6577a</code> |
| modified | <code>apps/palette-tauri/src/components/palette/MarkdownBody.test.tsx</code> | — | Palette correctness, testing, or dependency security | <code>e9c6577a</code> |
| modified | <code>apps/palette-tauri/vite.config.ts</code> | — | Palette correctness, testing, or dependency security | <code>e9c6577a</code> |
| modified | <code>crates/labby-apis/src/acp_registry/installer.rs</code> | — | Async SDK installation boundary | <code>e9c6577a</code> |
| modified | <code>crates/labby-auth/Cargo.toml</code> | — | OAuth, JWT, persistence, and authorization hardening | <code>e9c6577a</code> |
| modified | <code>crates/labby-auth/src/authorize.rs</code> | — | OAuth, JWT, persistence, and authorization hardening | <code>e9c6577a</code> |
| modified | <code>crates/labby-auth/src/jwt.rs</code> | — | OAuth, JWT, persistence, and authorization hardening | <code>e9c6577a</code> |
| modified | <code>crates/labby-auth/src/sqlite.rs</code> | — | OAuth, JWT, persistence, and authorization hardening | <code>e9c6577a</code> |
| created | <code>crates/labby-auth/src/sqlite/migrations.rs</code> | — | OAuth, JWT, persistence, and authorization hardening | <code>e9c6577a</code> |
| created | <code>crates/labby-auth/src/sqlite/rows.rs</code> | — | OAuth, JWT, persistence, and authorization hardening | <code>e9c6577a</code> |
| created | <code>crates/labby-auth/src/sqlite/tokens.rs</code> | — | OAuth, JWT, persistence, and authorization hardening | <code>e9c6577a</code> |
| modified | <code>crates/labby-auth/src/state.rs</code> | — | OAuth, JWT, persistence, and authorization hardening | <code>e9c6577a</code> |
| modified | <code>crates/labby-auth/src/util.rs</code> | — | OAuth, JWT, persistence, and authorization hardening | <code>e9c6577a</code> |
| modified | <code>crates/labby-codemode/src/runner_drive.rs</code> | — | Code Mode budgets, cancellation, and module boundaries | <code>e9c6577a</code> |
| created | <code>crates/labby-codemode/src/runner_drive/artifacts.rs</code> | — | Code Mode budgets, cancellation, and module boundaries | <code>e9c6577a</code> |
| created | <code>crates/labby-codemode/src/runner_drive/finalize.rs</code> | — | Code Mode budgets, cancellation, and module boundaries | <code>e9c6577a</code> |
| created | <code>crates/labby-codemode/src/runner_drive/steps.rs</code> | — | Code Mode budgets, cancellation, and module boundaries | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/codemode_journal/store.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/catalog.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/dispatch_tests.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/manager/pool_lifecycle.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/manager/tests.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/manager/tests/config_ops.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/manager/tests/lifecycle.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/manager/tests/virtual_servers.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/manager/usage.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/params.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/runtime.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/gateway/types.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/upstream/pool.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/upstream/pool/cache_repair.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/upstream/pool/probe.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/usage/query.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby-gateway/src/usage/store.rs</code> | — | Gateway lifecycle, usage, and upstream reliability | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/api.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| created | <code>crates/labby/src/api/app_routes.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| created | <code>crates/labby/src/api/dev_mockup.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/api/error.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/api/openapi.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/api/router.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| created | <code>crates/labby/src/api/router_middleware.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/api/services/catalog.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/api/services/setup.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/catalog.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/cli.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/cli/doctor.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/cli/gateway.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/cli/gateway/args.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/cli/gateway/dispatch.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/cli/serve.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| created | <code>crates/labby/src/composition.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/config.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/config/env_merge.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| created | <code>crates/labby/src/config/env_writer.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| created | <code>crates/labby/src/config/paths.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| created | <code>crates/labby/src/config/secret_files.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/doctor.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/doctor/catalog.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/doctor/dispatch.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/doctor/params.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/doctor/service.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/helpers.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/setup.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/setup/catalog.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/setup/claude_plugins.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/setup/dispatch.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/setup/plugin_hook.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/dispatch/setup/provision.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/docs/action_catalog.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/docs/render.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/docs/types.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/lib.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/mcp/assets/code_mode_app.html</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/mcp/call_tool_codemode.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/mcp/context.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/mcp/context/tests.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/mcp/handlers_resources.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/mcp/handlers_tools/tests.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/mcp/server.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/src/registry.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/tests/architecture_boundaries.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/tests/architecture_orchestrator.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>crates/labby/tests/ci_changed_paths.rs</code> | — | Product composition, adapters, setup, config, docs, and API policy | <code>e9c6577a</code> |
| modified | <code>deny.toml</code> | — | Dependency and advisory policy | <code>e9c6577a</code> |
| modified | <code>docs/ARCH.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| deleted | <code>docs/CHANGELOG.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| modified | <code>docs/OPERATIONS.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| modified | <code>docs/dev/ERRORS.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| modified | <code>docs/generated/action-catalog.json</code> | — | Regenerated public contracts | <code>e9c6577a</code> |
| modified | <code>docs/generated/action-catalog.md</code> | — | Regenerated public contracts | <code>e9c6577a</code> |
| modified | <code>docs/generated/cli-help.md</code> | — | Regenerated public contracts | <code>e9c6577a</code> |
| modified | <code>docs/generated/mcp-help.json</code> | — | Regenerated public contracts | <code>e9c6577a</code> |
| modified | <code>docs/generated/mcp-help.md</code> | — | Regenerated public contracts | <code>e9c6577a</code> |
| modified | <code>docs/generated/openapi.json</code> | — | Regenerated public contracts | <code>e9c6577a</code> |
| modified | <code>docs/generated/service-catalog.json</code> | — | Regenerated public contracts | <code>e9c6577a</code> |
| modified | <code>docs/runtime/CICD.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| modified | <code>docs/runtime/OAUTH.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| modified | <code>docs/sessions/2026-05-04-acp-session-persistence-chat-polish.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| modified | <code>docs/superpowers/plans/2026-04-12-backup-node-live-test-services.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| modified | <code>openwiki/development.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| modified | <code>openwiki/domain.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| modified | <code>openwiki/quickstart.md</code> | — | Documentation correction and redaction | <code>e9c6577a</code> |
| modified | <code>release-please-config.json</code> | — | Validated release handoff | <code>e9c6577a</code> |
| modified | <code>scripts/ci/changed_paths.py</code> | — | CI policy and coverage enforcement | <code>e9c6577a</code> |
| created | <code>scripts/ci/check-openwiki.py</code> | — | CI policy and coverage enforcement | <code>e9c6577a</code> |
| created | <code>scripts/ci/check-rust-coverage.py</code> | — | CI policy and coverage enforcement | <code>e9c6577a</code> |
| created | <code>scripts/ci/test_check_rust_coverage.py</code> | — | CI policy and coverage enforcement | <code>e9c6577a</code> |

## Beads Activity

| bead | title | actions | final status | why it mattered |
|---|---|---|---|---|
| <code>lab-5j67j</code> | Comprehensive full-project review and remediation | Worked throughout the review; closed after merge and verification | closed | Tracked the full F-01–F-49 review/remediation outcome |
| <code>lab-fpsq9</code> | Rotate MCP Registry DNS publishing key | Searched for an existing issue, found none, then created a P1 follow-up | open | Keeps the external secret rotation and workflow verification visible instead of burying it in prose |

The local <code>bd close</code> and <code>bd create</code> operations succeeded. Their automatic MySQL backup/sync attempt timed out against <code>100.75.111.118:3311</code>; local bead state remained authoritative and was verified from command output.

## Repository Maintenance

- Plans: <code>docs/plans/complete/mcp-streamable-http-oauth-proxy.md</code> was already filed as complete. <code>docs/plans/fleet-ws-plan-lab-n07n.md</code> remains active/ambiguous and was deliberately not moved.
- Beads: closed completed review bead <code>lab-5j67j</code>; created open P1 follow-up <code>lab-fpsq9</code> for the only remaining operator-controlled action.
- Worktrees and branches: removed the merged feature worktree/branch and stale proven-safe backup ref during delivery. Final local state retained only clean <code>main</code> and clean <code>marketplace-no-mcp</code>; both matched their remotes before this session-log commit.
- Remote state: deleted the merged feature remote. Preserved the active Release Please branch for [PR #247](https://github.com/jmagar/labby/pull/247), because it is automation-owned and unmerged.
- Stale docs: regenerated action/MCP/OpenAPI/service contracts, refreshed OpenWiki and runtime CI/OAuth docs, removed stale duplicate <code>docs/CHANGELOG.md</code>, and passed docs/OpenWiki checks. No additional safe stale-doc move or rewrite was identified.
- Transparency: no dirty or unmerged user work was discarded. The no-MCP worktree had a temporary local wrapper diff identical to incoming remote content; it was named-stashed, fast-forwarded, hash-compared, then the verified redundant stash was dropped.

## Tools and Skills Used

- Skills: the named <code>comprehensive-review:full-review</code> workflow drove the whole-project audit; <code>vibin:save-to-md</code> drove this maintenance pass, artifact structure, path-limited commit, and default-branch landing.
- Parallel agents: independent agents reviewed and remediated security/auth, gateway/runtime, frontend/Palette, testing/docs, CI/release, and delivery-review findings; shared-tree integration was followed by root-level verification.
- Shell and file tools: <code>rg</code>, Git, Cargo/nextest/clippy/fmt/deny, Bun/TypeScript/Vite, Python policy tests, actionlint, GitHub CLI, Beads, Incus/distrobuilder, and path-limited patching were used for inspection, implementation, orchestration, and verification.
- Browser and platform tooling: Playwright browser scenarios exercised Gateway Admin; Windows self-hosted CI verified ACL and Job Object behavior; Incus jobs verified build, boot, provision, and rolling publication.
- GitHub/external checks: <code>gh</code> inspected PRs, reviews, checks, actions, release automation, and Dependabot alert #77. Official action metadata and release-please schema were consulted when resolving delivery findings.
- Issues encountered: late automated-review findings required additional remediation cycles; GitHub merge changed local branch assumptions; MySQL bead backup timed out; no force push, destructive reset, or unverified cleanup was used.

## Commands Executed

| command | result |
|---|---|
| <code>git status --short --branch</code>, <code>git worktree list --porcelain</code>, <code>git branch -vv</code> | Proved the active checkout and final two-worktree topology |
| <code>rg</code> and targeted source/doc inspection | Built and cross-checked the full review evidence |
| <code>just test</code> | 2,076 passed, 8 skipped |
| <code>just build</code>, <code>just lint</code>, <code>just deny</code> | All-features build, formatting/clippy, and advisory/license gates passed |
| <code>just docs-check</code> plus OpenWiki and CI policy scripts | Generated contracts, links, policy, and changed-path classifiers passed |
| Gateway Admin unit, typecheck, build, installer, browser, and bundle-budget commands | 340 unit tests, 2 installer tests, 5 browser scenarios, and budgets passed |
| Palette renderer, typecheck, Vite, Tauri, and advisory commands | Palette verification passed |
| <code>gh pr create/view/checks/merge</code> and <code>gh run view</code> | PR #248 merged and post-merge workflows were confirmed green |
| no-MCP drift checks plus branch hash/ancestry comparisons | Variant branch synchronized without marketplace drift |
| <code>bd close lab-5j67j</code> and <code>bd create ...</code> | Review closed; key-rotation follow-up created as <code>lab-fpsq9</code> |
| <code>git add -f -- artifact</code> and <code>git commit --only -- artifact</code> | Reserved for the final docs-only session artifact commit |

## Errors Encountered

- <strong>Post-merge checkout assumption:</strong> the PR merge succeeded, but a follow-up checkout path failed because <code>main</code> was already owned by the root worktree. The merged state was confirmed with GitHub and cleanup continued from the correct worktree.
- <strong>Late review findings:</strong> automated reviewers continued to identify P1/P2 delivery defects after the initial remediation. Each was reproduced or disproved against generated output, fixed when valid, and reverified before merge.
- <strong>Patch context collisions:</strong> a few repeated-line patches failed to match uniquely. They were reapplied one occurrence at a time with local context.
- <strong>No-MCP temporary drift:</strong> the local wrapper diff exactly matched incoming remote content. A named stash protected it during fast-forward; hashes proved redundancy before dropping the stash.
- <strong>Beads backup timeout:</strong> automatic MySQL backup sync timed out, but the local close/create operations succeeded. No tracker action was falsely reported as remotely synchronized.
- <strong>Required-check query ambiguity:</strong> <code>gh pr checks --required</code> reported no configured required checks; the full check set and aggregate gates were separately inspected and confirmed green.

## Behavior Changes (Before/After)

| area | before | after |
|---|---|---|
| Authentication and Setup | Mutation/admin/bootstrap policy could drift by surface; RSA/JWT and secret-file handling had security gaps | Metadata-derived fail-closed policy, scoped local bootstrap, modern signing path, redaction, and restrictive secret persistence |
| Gateway runtime | Reload cancellation could leave no live pool; saves could partially commit; reprobes could herd | Build-beside-live atomic swap, transactional/compensated saves, cancellation propagation, bounded jittered reprobes |
| Code Mode and usage | Execution state and journals were unbounded; cancellation leaked state; deep OFFSET pagination and zero-limit panic existed | Aggregate budgets, drop-safe cleanup, bounded artifacts, keyset cursor pagination, zero-limit handling |
| Gateway Admin and Palette | Dialog/editor code shipped eagerly; inspector restore/shrink behavior regressed; Palette gates were incomplete | Lazy route loading, bundle budgets, stable inspector/minimize/resize behavior, full renderer/Tauri/advisory gates |
| Contracts and docs | Admin requirements and stable errors were incomplete/stale across generated surfaces and OpenWiki | Generated policy parity, complete status mapping, regenerated docs, validated OpenWiki |
| CI and releases | Critical jobs were outside <code>ci-gate</code>; mutable actions/tools and early publication weakened integrity | Complete aggregation, pinned actions/tools, strict preflight, draft/tag validation, rollback-safe atomic publication and provenance |
| Repository hygiene | Feature refs/worktrees and review artifacts were in flight | Remediation merged; only <code>main</code> and <code>marketplace-no-mcp</code> remain locally, with intentional automation refs preserved remotely |

## Verification Evidence

| command or signal | expected | actual | status |
|---|---|---|---|
| <code>just test</code> | All runnable all-features Rust tests pass | 2,076 passed; 8 skipped | pass |
| <code>just build</code> | Workspace all-features build succeeds | succeeded | pass |
| <code>just lint</code> | fmt and clippy have no warnings/errors | succeeded | pass |
| <code>just deny</code> | dependency/advisory/license policy succeeds | succeeded | pass |
| Generated docs and OpenWiki checks | no contract or documentation drift | succeeded | pass |
| Gateway Admin verification | unit/install/type/build/browser/budgets green | 340 unit, 2 installer, 5 browser scenarios; all green | pass |
| Palette verification | renderer/type/build/Tauri/advisory green | all green | pass |
| Windows self-hosted validation | tests, ACLs, and Job Object behavior green | succeeded | pass |
| Incus validation | image build, boot, provision, immutable rolling publish green | [run 29506542983](https://github.com/jmagar/labby/actions/runs/29506542983) succeeded | pass |
| Main post-merge CI | every executed job green | [run 29506546131](https://github.com/jmagar/labby/actions/runs/29506546131): 39 success, 1 skipped | pass |
| no-MCP synchronization | remote variant contains main and drift checks pass | [run 29506546642](https://github.com/jmagar/labby/actions/runs/29506546642) succeeded | pass |
| Release Please | release automation remains functional | [run 29507946816](https://github.com/jmagar/labby/actions/runs/29507946816) succeeded | pass |
| Dependabot | vulnerable Palette <code>serde_with</code> alert resolved | alert #77 fixed | pass |
| Final repo status before artifact | clean main; only intended local branches/worktrees | observed exactly <code>main</code> and <code>marketplace-no-mcp</code>, both clean and synced | pass |

## Risks and Rollback

- The squash commit is broad because all 49 priorities were explicitly in scope. Reverting <code>e9c6577a</code> would roll back the remediation as one unit, but would also reintroduce security, correctness, and release-integrity defects; targeted reverts are safer for isolated regressions.
- Release changes intentionally preserve pre-existing public releases and container versions during rollback. The validated stable tag/draft handoff prevents partial public promotion.
- Key rotation remains time-sensitive because current-tree redaction does not revoke material already exposed in history. Rotate first; decide separately whether to rewrite published history.
- The no-MCP variant is automation-synchronized from <code>main</code>; future main commits, including this session log, should be allowed to pass through its sync workflow before declaring both local worktrees current again.

## Decisions Not Taken

- Did not rewrite Git history or force-push to purge historical credential material; that destructive operation needs separate explicit authorization and coordination.
- Did not invent or rotate the MCP Registry DNS signing key without Cloudflare/GitHub secret authority.
- Did not merge <code>marketplace-no-mcp</code> into <code>main</code> or delete it; project instructions define it as intentional and long-lived.
- Did not delete the Release Please branch for PR #247; it is active automation state.
- Did not move <code>docs/plans/fleet-ws-plan-lab-n07n.md</code> into completed plans because its status was not proven complete.

## References

- [Comprehensive final report](../../.full-review/05-final-report.md)
- [Remediation record](../../.full-review/06-remediation.md)
- [PR #248: fix comprehensive project review](https://github.com/jmagar/labby/pull/248)
- [Main post-merge CI run](https://github.com/jmagar/labby/actions/runs/29506546131)
- [Incus validation and publication run](https://github.com/jmagar/labby/actions/runs/29506542983)
- [no-MCP synchronization run](https://github.com/jmagar/labby/actions/runs/29506546642)
- [Release Please run](https://github.com/jmagar/labby/actions/runs/29507946816)
- [Active release PR #247](https://github.com/jmagar/labby/pull/247)
- Beads: <code>lab-5j67j</code> and <code>lab-fpsq9</code>

## Open Questions

- Who will perform the Cloudflare/GitHub-authorized MCP Registry DNS publishing-key rotation tracked by <code>lab-fpsq9</code>?
- After rotation and workflow verification, is a coordinated destructive rewrite of published repository history authorized?
- Is <code>docs/plans/fleet-ws-plan-lab-n07n.md</code> still active, or can a future maintenance pass move it to <code>docs/plans/complete/</code>?

## Next Steps

- <strong>Blocked external action:</strong> complete <code>lab-fpsq9</code> by rotating the DNS publishing key, replacing the GitHub secret, and validating a safe publish.
- <strong>Follow-on decision:</strong> explicitly authorize or decline a coordinated history rewrite only after all exposed credentials are rotated.
- <strong>Automation:</strong> let the docs-only main commit trigger the normal <code>marketplace-no-mcp</code> synchronization, then fast-forward the local no-MCP worktree and rerun its drift/status checks.
- <strong>Immediate verification:</strong> confirm this artifact commit contains only this Markdown path, push it to <code>main</code>, pull fast-forward, and verify both retained worktrees are clean.

