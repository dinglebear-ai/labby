# Complete Labby LiveView Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Full real-product mock parity, then bundled supervised Phoenix delivery,
full-PR review, correction of all findings, and final live artifact qualification.

**Architecture:** Port authored presentation while replacing sample behavior with
caller-bound Rust APIs. Keep domain behavior in Rust. Complete the UI migration
before implementing the embedded OTP release lifecycle.

**Tech Stack:** Existing Rust/Axum dispatch, Phoenix LiveView/Req, Aurora HEEx/CSS,
ExUnit, Node tests, Playwright and native OTP releases.

**Spec:** [full-migration-spec.md](full-migration-spec.md)

**Status:** units 0-3 complete at `5cb9dd2f7` (2026-11-10): reference inventory, caller-bound action
transport, shared mock shell and Projects lifecycle. Unit 4 is partially done - Gateway CRUD/OAuth
handoff and the Observe workspace landed, while charts, start/stop/reload, configuration editor and
traces remain. Access (unit 5) landed ahead of sequence. Units 6-11 are untouched.

## Global Constraints

- Full mock parity and same or better visual quality are mandatory.
- Use TDD, regression tests and Playwright E2Es for every reasonable workflow.
- Live tests, not mocked success, close acceptance requirements.
- Rust owns domain state, validation and authority; no preview-domain simulation.
- Bound requests, response bytes, concurrency, timeouts and retry budgets.
- Never automatically replay a mutation with uncertain side effects.
- Preserve unrelated work, private guidance, secrets and existing services.
- Complete mock functionality before implementing packaging.
- Review all touched PR files with `vibin:full-review` and fix all findings.

## Review Focus

1. A revoked/forbidden caller cannot mutate or observe another caller's state.
2. Lost responses, double submits and stale async replies cannot duplicate writes.
3. Untrusted artifact/file/archive names cannot escape storage or expose secrets.
4. Missing API data/errors remain visible and never become sample success or zero.
5. Restart, extraction failure and rollback leave working state and process ownership intact.

## Work units

Every implementation unit follows RED regression -> observed expected failure ->
minimal implementation -> GREEN unit/contract checks -> live Playwright -> task
review -> commit. Record exact receipts and open scope in the execution ledger.

### 0. Reference and coverage inventory

- [x] Freeze checksummed mock source and enumerate all eighteen areas' workflows.
- [x] Map each workflow to current dispatch/dedicated HTTP contracts and tests.
- [x] Record live parity, fixture-only coverage and missing behavior separately.

Files: this plan/spec, `mock-source-manifest.json`, `workflow-coverage.json`.

### 1. Caller-bound action transport

Files: `lib/labby_console/auth.ex`, `gateway_action.ex`, auth/transport tests.
Interface: `Auth.action(session_id, service, action, params)` accepts only reviewed
actions; returns decoded success or a bounded typed error with side-effect state.
Initially admit Projects lifecycle and Team membership reads for unit 3.
Exact initial actions: `projects.list/get/create/update/archive/activate` and
`access.team.list`. Auth and existing read adapters must preserve a valid session
on dispatch 403 while removing authority on a confirmed session 401.

- [x] RED: forbidden keeps the session; unauthorized revokes it; disallowed action
  is never sent; CSRF is forwarded; timeout sends once and reports uncertain write.
- [x] Implement transport using the existing per-caller vault and bounded Req path.
- [x] GREEN: validate structured Rust envelopes and malformed/oversized failures.
- [x] Test exact 32 KiB request/64 KiB response limits, 5-second whole-operation
  deadline (12 seconds for host sampling), four-per-session/32-global admission,
  caller death/cancellation and no replay. Preserve bounded kind, context,
  recovery and side effects; malformed/oversized write success is uncertain.

### 2. Shared mock shell and visual primitives

Files: `components/workspace_shell.ex`, authored `assets/css/`, `assets/js/`,
`components/layouts.ex`, mock-shell ExUnit and browser canaries.
Interface: shell receives real identity/count/current page and implemented routes.
Shared authenticated mounts own expiry, scope generations and routing/history;
existing read-model routes remain valid. Caller/Team/Project generation changes
fence stale results and repeated submissions before domain pages migrate.

- [x] RED: native desktop/mobile nav, profile menus, keyboard dismissal, collapse,
  no sample labels/counts and no unavailable route presented as working.
- [x] Port mock-authored shell/styles/icons with production identity and routing.
- [x] Live-compare desktop/mobile renders and preserve existing read-model tests.

### 3. Projects lifecycle

Files: `projects.ex`, `projects_live.ex`, `projects_components.ex`, Projects tests.
Consumes `Auth.action/4`; exposes list/get/create/update/archive/activate backed by
the existing Team-scoped Rust service. Preserve mock search/filter/layout/dialogs.
`access.team.list` supplies Team membership. Project snapshots contain
`project_id`, `team_id`, `name`, `status`, `role`, `policy_epoch`, `can_manage`.
Workspace selection is a caller-isolated UI preference, not lifecycle activation.
Active-list reconciliation and explicit archived lookup retain only authoritative
records and refresh permissions before writes.

- [x] RED: creation validation, Team permission denial, read/update/archive/restore,
  filter persistence, double-submit fencing and lookup of missing project.
- [x] Implement real projections, async operations, safe confirmation/error states.
- [x] Live E2E: create/rename/archive/restore, reload persistence and caller isolation.
- [x] Pin selection vs restore semantics, archived lookup/reconciliation, stale
  permission changes and caller-isolated filter/layout/selection preferences.

### 4. Overview, Gateway and Observe parity

Files: existing read models/LiveViews plus mock-shaped presentation and tests.
Complete window charts/rankings/layout persistence, gateway setup/config/catalog/
OAuth/action dialogs, and unified Activity/Logs/Traces with inspectors.
Expand into one task brief per named workflow in `workflow-coverage.json` before
dispatch; each brief must name exact actions/fields, persistent effects, owned
live prerequisites/cleanup, test names and success/error assertions.

- [ ] Test-first each mock workflow against actual generated action schemas.
- [ ] Implement and verify refresh/reconnect/permission/error/unknown-result states.
- [ ] Live E2E with configured local peers and authorized external providers where required.

### 5. Access, Settings, Setup, Diagnostics, Browser and Environments

Files: one domain adapter/LiveView/component/test family per area.
Consume existing access, settings, setup, doctor, browser and dev-container APIs.
Expand each coverage-manifest workflow into its own concrete test-first brief.

- [ ] Test-first all reference dialogs/forms, roles, last-owner protections, secret
  redaction, preflight/apply modes, pairing consent and instance lifecycle.
- [ ] Implement native workflows rather than generic action forms.
- [ ] Live E2E persistence, provider/runtime failures, keyboard/mobile, revocation.

### 6. Stash, Tasks and Agents

Files: per-domain adapters/LiveViews/components and transfer/run E2Es.
Consume Stash dedicated transfer HTTP routes and task/agent dispatch APIs.
Expand each coverage-manifest workflow into its own concrete test-first brief.

- [ ] Test-first file boundaries/grants, uploads/downloads, schedule/run/cancel,
  immutable revisions/pins, validation and provider failure/uncertain outcomes.
- [ ] Implement the mock's complete collections, editors, inspectors and actions.
- [ ] Live E2E real files, durable jobs/revisions and owned runtime cleanup.

### 7. Discover, Create, Library, Depot and Tools

Files: domain-focused workspace/editor/depot modules and workflow tests.
Consume sources/artifacts/bundles/snippets/jobs/uploads/publishing contracts.
Expand each coverage-manifest workflow into its own concrete test-first brief.

- [ ] Test-first connected discover/fork/edit/save/compose/test/publish workflows,
  exact acquisition, revisions, licensing, archive validation and one-use approval.
- [ ] Implement editors, file dialogs, catalog filters and durable transaction state.
- [ ] Live E2E real Depot/provider calls when authorized, artifact bytes and persistence.

### 8. Complete UI qualification

- [ ] Close every coverage-manifest item with real-source/live/visual evidence.
- [ ] Compare every mock area and major interaction at 1440, 740 and 320px.
- [ ] Verify keyboard/history/reconnect, long data, empty/error/permission states.
- [ ] No placeholder workflow or incomplete mock capability may pass this unit.

### 9. Bundled release extraction

Files: focused Rust runtime module/crate, release build inputs and extraction tests.
Interface: verified embedded manifest -> atomically published immutable runtime path.
Before starting, bind native target/ABI matrix, public route and cookie namespaces,
proxy ownership and artifact identity. Include origin/proxy trust, cookie/header
collision, authenticated child access and streaming/backpressure contracts.

- [ ] RED: tampering, path/symlink attacks, truncated archives, concurrent launch,
  bounded disk use, interrupted extraction and unsupported target.
- [ ] Implement target-native OTP builds and safe versioned extraction.
- [ ] Verify actual packaged artifact first/warm launch and rollback paths.

### 10. Supervision, HTTP/WebSocket routing and delivery

Files: existing serve composition, route adapter, process supervisor, installer/CI.

- [ ] RED: readiness failure, crash budget, PID reuse, signal/shutdown, scoped
  headers/cookies, assets and upgraded WebSocket traffic through the public listener.
- [ ] Implement owned-process lifecycle and authenticated transparent routing.
- [ ] Live-test each supported native artifact and installation/update/rollback.

### 11. Full review and final acceptance

- [ ] Run requested full-review across every touched file in the current PR diff.
- [ ] Fix every finding with regression coverage and scoped re-review.
- [ ] Rerun full live browser/canary and native artifact acceptance on final source.
- [ ] Verify PR head/base, conflict status, CI, clean owned worktree and retained evidence.
- [ ] Mark goal complete only after every original requirement has current proof.
