# Phase 3 — Testing and Documentation Review

Review target: `e6d761f91466905b435b253497b5d4077882fba8`
Scope: entire tracked Lab project

## Verification baseline

- `just test`: 2,035/2,035 runnable Rust tests passed; 14 ignored.
- Gateway Admin: 336 unit tests and 2 installer-sync tests passed.
- Gateway Admin browser tests: 5/5 failed during opaque preview startup.
- Palette contains 58 renderer cases and 37 Rust test attributes, but neither suite is run by repository CI.
- `just docs-check` exposed drift in five checked-in generated artifacts.

## Testing findings

### P1 — Doctor's advertised probe contract has no successful behavioral test

`crates/labby/src/dispatch/doctor/service.rs:45-58,89-126,156-170` has empty known/configured service lists, so the advertised named probe and `audit.full` service coverage cannot succeed. No test proves a successful named probe, inclusion in `audit.full`, or a non-empty health report.

Fix: implement registry-derived probes (or remove the unsupported surface), then add fake-client success, auth failure, transport failure, multi-instance, and CLI/API/MCP end-to-end tests. Empty probe sets must not report healthy.

### P1 — Setup authorization tests codify the vulnerable policy

Mutation-capable actions remain non-admin in `crates/labby/src/dispatch/setup/catalog.rs:58-127,205-247,321-379`. Existing tests cover only three settings mutations and explicitly treat `draft.commit` as unrestricted (`crates/labby/src/api/services/setup.rs:171-186,227-279`).

Fix: registry-drive authorization tests over every Setup action for read-only, admin, and trusted-local callers. Cover bootstrap, drafts, plugins, finalize, secret reads, invalid environment keys, and connectivity probes.

### P1 — MCP admin testing is not registry-wide and missed Doctor

`crates/labby/src/mcp/context.rs:172-203` hard-codes service names and omits Doctor. MCP tests cover Gateway and Snippets only (`crates/labby/src/mcp/context/tests.rs:55-137`); Doctor is tested only through HTTP.

Fix: derive authorization from the live action registry and test every registered `requires_admin` action with read-only/admin/trusted-local callers. Unknown actions must fail closed.

### P1 — Gateway/protected-route tests cover success but not partial failure

`apps/gateway-admin/components/gateway/gateway-form-dialog.test.tsx:275-351` verifies only successful ordering. Production persists the gateway before the route and parent callbacks close during the first step.

Fix: use one backend transaction or compensation, then test route add/update/remove failure, abort, rollback, retry, and dialog lifetime.

### P1 — Full gateway reload cancellation is untested

`crates/labby-gateway/src/gateway/manager/pool_lifecycle.rs:208-340` drains the current pool before probing/installing the replacement. Existing tests cover completed reloads only.

Fix: build and validate the replacement before publication and add a blocking connector test that aborts after rebuild starts, including HTTP timeout cancellation, while proving the old pool remains available.

### P1 — Code Mode has no aggregate-limit or cancellation-leak tests

Steps accumulate in `crates/labby-codemode/src/runner_drive.rs:1199-1204`, and request journals append in `crates/labby/src/mcp/code_mode_host.rs:126-175` until normal execution cleanup.

Fix: impose count/byte limits, add stress tests at and beyond each boundary, and use a drop-safe request guard tested by aborting after the first step.

### P1 — Secret scanning accepts real secrets through baselines

`.gitleaksignore:1-11` suppresses first-party credential findings while CI runs ordinary Gitleaks at `.github/workflows/ci.yml:91-104`.

Fix: rotate and redact the credentials, remove blanket baselines, scan the current tree and history, and policy-test that any remaining ignores are restricted to documented synthetic fixtures.

### P2 — Filesystem and subprocess failures lack injected tests

Uncovered paths include swallowed Claude failures, ignored draft deletion, and corrupt runtime state replacement. Add injectable command/filesystem seams and tests for malformed JSON, spawn/nonzero failures, permissions, corruption, and evidence preservation.

### P2 — Gateway-test cancellation is dropped and untested

The form creates a controller, but `apps/gateway-admin/lib/hooks/use-gateways.ts:411-426` accepts no signal. Thread the signal through hook, mock, and API client, and test close/unmount cancellation.

### P2 — HTTP stable-kind mapping is incomplete

`crates/labby/src/api/error.rs:42-114` defaults unknown kinds to 500. Tests omit `audit_timeout`, `merge_write_conflict`, and `workspace_not_configured`, though `docs/dev/ERRORS.md:180-192` specifies conflict/timeout semantics.

Fix: use a complete typed status table and endpoint-level tests.

### P2 — Palette is absent from CI

`.github/workflows/ci.yml` and `scripts/ci/changed_paths.py:68-69` do not schedule Palette. Add renderer tests, coverage, typecheck, Vite build, Tauri-side Rust tests, an independent lockfile audit, and Windows smoke coverage.

### P2 — No budgets cover the identified performance risks

There are no benchmarks for offset pagination, reprobe concurrency, Code Mode growth, OAuth address churn, or Gateway route bundle size.

Fix: add large-row database benchmarks, deterministic concurrency tests, memory stress limits, TTL eviction tests, and compressed bundle budgets.

### P2 — Post-pivot tests remain ignored

At least nine ignored tests cite missing fixtures in `crates/labby-gateway/src/gateway/dispatch_tests.rs:999-1226`, manager config tests, and virtual-server tests.

Fix: add a test-only registered service with required/secret environment metadata and restore the tests to CI.

### P2 — Browser tests are excluded and opaque on failure

The browser suite hard-codes port 3101 and suppresses preview output; all five tests timed out during the review. Allocate a free port, capture early exit/stderr, build once, and run the suite in CI.

### P2 — Windows secret ACL behavior has no tests

Add Windows tests for owner-only ACLs on env, draft, backup, auth DB/WAL/SHM, private keys, and usage stores.

### P3 — No Rust coverage trend or test-pyramid report exists

The high test count concealed multiple P1 defects. Add `cargo llvm-cov` artifacts and initially gate security, dispatch, and runtime-critical modules while reporting unit/integration/live-test proportions.

## Documentation and API-contract findings

### P0 — An active secret and other credential material are committed to public documentation

- `docs/sessions/2026-05-04-acp-session-persistence-chat-polish.md:98-100` contains an HMAC secret that a non-printing equality check confirmed still matches the active `LAB_ACP_HMAC_SECRET`.
- `docs/superpowers/plans/2026-04-12-backup-node-live-test-services.md:30-53` contains additional credential material.
- `.gitleaksignore:1-11` baselines these detections.
- The GitHub repository is public.

Impact: published credentials are available from the repository and history; the active HMAC material can be used to forge accepted state.

Fix: immediately rotate/revoke affected credentials, redact the current tree, purge published plaintext from history, remove the baselines, and add current-tree plus history scanning. Secret values must never be reproduced in logs or review artifacts.

### P1 — Doctor and health documentation promises probes that are empty

`docs/OPERATIONS.md:218-245`, `docs/surfaces/CLI.md:140-169`, and `docs/runtime/OAUTH.md:783-793` promise meaningful probes. Current Doctor service discovery is empty and `crates/labby/src/cli/health.rs:21-30` returns an empty successful report.

Fix: implement and test real registry-derived probes or remove/mark the contract unsupported. Empty coverage must warn or fail.

### P1 — Generated API contracts omit action-level admin requirements

Authorization depends on `ActionSpec.requires_admin`, but `crates/labby/src/docs/types.rs:60-72`, the projection/renderers, and OpenAPI omit that property and its required `lab:admin`/403 contract.

Fix: project `requires_admin` into JSON, Markdown, MCP, and OpenAPI, document transport differences, and correct Setup mutation flags.

### P2 — The RSA advisory exception rationale contradicts production signing

`deny.toml:4-10` claims Lab does no RSA signing, while `crates/labby-auth/src/jwt.rs:77-105` signs RS256 access tokens and affected RSA versions remain locked.

Fix: migrate signing away from the affected implementation or write an accurate time-bounded accepted-risk record with real compensating controls.

### P2 — Windows security documentation exceeds the implementation

Windows support and sensitive-file permission validation are documented, but non-Unix auth permission enforcement is a no-op and Doctor checks permissions only on Unix.

Fix: implement restrictive Windows DACL creation/validation; until then emit explicit Doctor warnings and document the limitation consistently.

### P2 — Generated documentation fails its freshness gate

`just docs-check` found drift in action-catalog Markdown/JSON, MCP-help Markdown/JSON, and OpenAPI. The checked-in docs still expose a removed `snippets.promote.confirm` parameter.

Fix: reconcile the intended confirmation contract, regenerate all artifacts, and make the freshness gate mandatory before merge.

### P2 — OpenWiki is materially stale

`openwiki/domain.md` describes obsolete MCP tool naming, a nonexistent polling store, and outdated environment variables. `openwiki/development.md` contains invalid workspace commands, and broken links remain in the quickstart/development docs.

Fix: regenerate from current main and add link, command, and contract validation to the OpenWiki workflow.

### P3 — A duplicate changelog is stale

`docs/CHANGELOG.md` disagrees with the release-managed root `CHANGELOG.md`.

Fix: remove it or replace it with a redirect to the root changelog.
