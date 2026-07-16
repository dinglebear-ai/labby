# Phase 1: Code Quality & Architecture Review

Review commit: `e6d761f91466905b435b253497b5d4077882fba8`

## Summary

| Area | Critical | High | Medium | Low | Total |
|---|---:|---:|---:|---:|---:|
| Code quality | 0 | 2 | 4 | 2 | 8 |
| Architecture | 0 | 1 | 3 | 0 | 4 |
| Combined | 0 | 3 | 7 | 2 | 12 |

The repository-wide `just lint` gate passed, including wrapper/skill drift checks, all-feature workspace Clippy with `-D warnings`, and rustfmt. The findings below are behavioral or structural issues not detected by that baseline.

## Code Quality Findings

### High

#### Q-H1: Advertised Doctor service probes cannot probe any service

- Evidence: `crates/labby/src/dispatch/doctor/catalog.rs:34-60` advertises `service.probe` and all-service probing; `crates/labby/src/dispatch/doctor/service.rs:50-58,72-86,112-126,156-170` constructs empty service-name lists and always returns unknown service; `crates/labby/src/dispatch/clients.rs:32-45` has no service clients; `crates/labby/src/cli/doctor.rs:353-371` exposes the empty result.
- Impact: every named probe fails, while `doctor services` and `audit.full` can report success without probing anything.
- Fix: either remove the obsolete post-slim surface or rebuild it from live registry/gateway health, with success and unknown-service end-to-end tests.

#### Q-H2: Gateway and protected-route save is partially committed on second-step failure

- Evidence: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx:867-935` awaits gateway persistence and then mutates the protected route; parent callbacks in `gateway-list-content.tsx:470-479` and `gateway-detail-content.tsx:256-261` close the dialog during the first callback.
- Impact: route failure leaves persisted gateway state inconsistent with public access configuration, after success UI has already closed the form.
- Fix: add a backend atomic action under the config lock, or keep the dialog open and compensate on failure; test add/update/remove failures after the first mutation.

### Medium

#### Q-M1: Gateway test cancellation drops its `AbortSignal`

- Evidence: `gateway-form-dialog.tsx:823-842,1081-1087` creates/aborts a controller, but `lib/hooks/use-gateways.ts:411-426` accepts no signal despite `lib/api/gateway-client.ts:484-492` supporting one.
- Impact: closing the dialog does not cancel the backend test/probe requests.
- Fix: thread the signal through the mutation hook and test close-time cancellation.

#### Q-M2: `services.status` converts Claude CLI errors into false absence

- Evidence: `crates/labby/src/dispatch/setup/claude_plugins.rs:64-75,145-177` discards typed command/parse failures with `unwrap_or_default()`.
- Impact: callers receive successful `installed: false` values when installation state is actually unknown.
- Fix: propagate `ToolError`, or return an explicit unknown/error state; test command and malformed-JSON failures.

#### Q-M3: Draft commit reports success when secret-bearing draft deletion fails

- Evidence: `crates/labby/src/dispatch/setup/dispatch.rs:696-728` calls `remove_file(&draft).ok()` after merging environment values.
- Impact: credentials can remain in `.env.draft`, and stale values can be replayed despite a success response.
- Fix: return a structured partial-commit failure or explicit `draft_cleared: false`; add a deletion-failure test.

#### Q-M4: Corrupt/unreadable gateway runtime state is silently replaced

- Evidence: `crates/labby-gateway/src/gateway/runtime.rs:72-93,153-194,227-258` maps read and decode failures to default state and later persists it.
- Impact: PID/PGID, ownership, age, and stale-process evidence can be erased without diagnosis.
- Fix: distinguish not-found from I/O/decode errors, quarantine or fail visibly, and test malformed/unreadable state.

### Low

#### Q-L1: `GatewayFormDialog` concentrates an oversized synchronization state machine

- Evidence: `gateway-form-dialog.tsx:267-1912` owns gateway state, OAuth, service config, JSON/env synchronization, protected routes, testing, and rendering through 30 state hooks, 15 effects, 11 refs, dependency suppressions, and timer-based synchronization.
- Impact: the two gateway behavioral defects above are harder to isolate and regressions span unrelated modes.
- Fix: introduce a typed reducer and extract OAuth, protected-route, and config-editor hooks plus separate service/custom forms.

#### Q-L2: Several production modules exceed established decomposition guidance

- Evidence: `labby-codemode/src/runner_drive.rs`, `labby/src/config.rs`, `labby/src/api/router.rs`, and `labby-auth/src/sqlite.rs` each combine multiple independently testable responsibilities and materially exceed the Code Mode guide's 500-line rule.
- Impact: increased review surface and difficult isolated testing; no direct runtime failure is asserted.
- Fix: mechanically split along protocol-handler, config, router/middleware, and persistence-domain seams while preserving tests.

## Architecture Findings

### High

#### A-H1: MCP admin authorization omits an action explicitly marked `requires_admin`

- Evidence: `dispatch/doctor/catalog.rs:107` marks `oauth.relay.check` admin-only; `mcp/context.rs:172-203` checks metadata only for a hard-coded service allowlist that omits `doctor`; `mcp/call_tool.rs:369` relies on that helper. The API's generic gate at `api/services/helpers.rs:156` correctly uses action metadata.
- Impact: an authenticated MCP caller without `lab:admin` can invoke an admin-only doctor action, creating a cross-surface authorization bypass.
- Fix: resolve metadata for every registered service, exempt only built-ins, fail closed for unknown actions, and add a registry-wide non-admin test for every admin action.

### Medium

#### A-M1: MCP production code depends on the API adapter

- Evidence: `mcp/context.rs:110` and `mcp/server.rs:77` use `crate::api::oauth::AuthContext`, violating `crates/labby/src/CLAUDE.md:29`; the API module only re-exports the neutral `labby_auth` type.
- Impact: MCP compilation/refactoring is coupled to a sibling transport surface.
- Fix: import the neutral auth type directly and add a production cross-surface import guard.

#### A-M2: CLI gateway construction reaches into MCP internals

- Evidence: `cli/gateway.rs:63-71` injects `crate::mcp::in_process_peer::connector()`, violating `crates/labby/src/CLAUDE.md:35`.
- Impact: gateway composition is split across protocol adapters and CLI cannot be isolated.
- Fix: accept an injected neutral connector at a product composition root and guard against future cross-surface imports.

#### A-M3: Stable setup/FS error kinds fall through to HTTP 500

- Evidence: `api/error.rs:42-114` has a catch-all 500; active code emits `audit_timeout` (`dispatch/setup/dispatch.rs:652`), `merge_write_conflict` (`config/env_merge.rs:186`), and `workspace_not_configured` (`dispatch/fs/client.rs:23`); `docs/dev/ERRORS.md:180` requires 504 and 409 for the first two.
- Impact: clients cannot distinguish retryable timeouts, conflicts, or configuration errors from internal faults.
- Fix: map every active stable kind and add a table-driven contract test, moving toward a typed audited status registry.

## Critical Issues for Phase 2 Context

No Critical-severity Phase 1 findings were found. Phase 2 must focus on:

1. The MCP admin authorization bypass for `doctor.oauth.relay.check` and whether other services/actions are omitted.
2. Partial/non-atomic gateway and draft-secret persistence behavior.
3. Silent corruption recovery for gateway runtime process state.
4. Error-kind mapping gaps that may hide security, timeout, or concurrency semantics.
5. The empty Doctor probe surface, especially whether it creates false operational assurance.
