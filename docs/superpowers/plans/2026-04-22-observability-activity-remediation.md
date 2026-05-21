# Observability Activity Feed Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make local-master logging semantically queryable and trustworthy enough to power the gateway-admin Activity view, including stable activity classification, actor scoping, and aligned observability documentation.

**Architecture:** Normalize all operator-facing logging around a single event contract: valid `surface`, explicit `subsystem`, stable `service`/`action`, and a non-secret actor correlation key. Add a backend-owned activity query shape on top of the log store so the UI stops reverse-engineering raw logs. Update `docs/OBSERVABILITY.md` in the same change so the contract and implementation stay aligned.

**Tech Stack:** Rust, tracing, axum, rmcp, SQLite log store, Next.js/TypeScript, gateway-admin.

---

## File Structure

### Existing files to modify

- `docs/OBSERVABILITY.md`
  - Canonical observability contract. Must be updated to define valid surfaces, required subsystem semantics for operator-facing events, stable actor-correlation fields, and the distinction between activity-worthy versus operational-noise logs.
- `crates/lab/src/dispatch/logs/types.rs`
  - Shared persisted log event schema. Likely needs new field(s) for stable actor correlation and possibly clarified comments around valid `surface`/`subsystem` values.
- `crates/lab/src/dispatch/logs/ingest.rs`
  - Tracing ingest normalization. Must stop silently collapsing valid operator-facing events into `core_runtime` when the caller should have assigned a subsystem.
- `crates/lab/src/api/services/helpers.rs`
  - Shared API dispatch wrapper. Must emit subsystem-aware logs and support caller-provided activity metadata without inventing route-local formats.
- `crates/lab/src/api/auth_helpers.rs`
  - Shared auth HTTP dispatch logging. Must align with the same subsystem and actor-key contract.
- `crates/lab/src/api/browser_session.rs`
  - Browser-session activity currently logs outside the shared API dispatch shape. Needs normalization and consistent actor tagging.
- `crates/lab/src/api/upstream_oauth.rs`
  - Upstream OAuth product routes. Must emit valid subsystem values and stable actor metadata.
- `crates/lab/src/api/device/oauth.rs`
  - Device relay route. Needs explicit subsystem classification and failure-path observability alignment.
- `crates/lab/src/api/device/syslog.rs`
  - Device syslog ingest route. Needs explicit failure-path observability and consistent API dispatch shape.
- `crates/lab/src/mcp/server.rs`
  - High-volume MCP dispatch emitter. Must consistently attach `subsystem = "mcp_server"` and stable actor metadata to dispatch events.
- `crates/lab/src/oauth/local_relay.rs`
  - Local OAuth relay currently uses invalid `surface` values. Must be normalized to valid persisted values while preserving relay-specific semantics.
- `crates/lab/src/cli/serve.rs`
  - Startup logs currently use ad hoc subsystem values. Must be classified as operational-noise and brought into the documented taxonomy.
- `apps/gateway-admin/lib/dashboard/admin-insights.ts`
  - Current client-side activity derivation. Must be simplified or retired once the backend exposes a semantic activity feed.
- `apps/gateway-admin/lib/api/logs-client.ts`
  - May need a new client function if a backend activity-specific query or endpoint is added.
- `apps/gateway-admin/app/(admin)/activity/page.tsx`
  - Must stop filtering raw log rows by `fields_json.subject === session.user.sub` and use the backend-owned activity contract instead.

### Existing tests to modify or extend

- `crates/lab/tests/logs_dispatch.rs`
  - Add semantic-ingest assertions for subsystem classification and actor-key persistence.
- `crates/lab/tests/logs_api.rs`
  - Add end-to-end assertions for the new activity query shape.
- `apps/gateway-admin/lib/dashboard/admin-insights.test.ts`
  - Update or replace tests once the UI no longer reconstructs semantics from raw log rows.

### New files likely to create

- `crates/lab/src/observability/activity.rs`
  - Shared event-contract helpers: valid subsystem mapping, actor-key derivation, and typed helpers for activity-worthy dispatch logs.
- `crates/lab/src/dispatch/logs/activity.rs`
  - Backend-owned activity query builder on top of the log store.
- `crates/lab/tests/logs_activity.rs`
  - Focused regression tests for activity semantics: MCP, auth-web, upstream OAuth, device OAuth relay, and actor scoping.
- `apps/gateway-admin/lib/types/activity.ts`
  - UI-facing activity item contract if the backend returns semantic rows instead of raw log events.
- `apps/gateway-admin/lib/api/activity-client.ts`
  - Client fetch wrapper for the backend activity query if separated from raw log search.

## Implementation Notes

- Do not broaden the Activity page by simply adding `core_runtime` to the existing query. That is a symptom patch and will pollute the page with startup and operational-noise logs.
- Prefer adding a stable `actor_key` over exposing raw `subject` to the UI or persisting raw `subject` in the store.
- Preserve the existing dispatch boundary guarantees from `docs/OBSERVABILITY.md`; the change is to make the event taxonomy and actor-scoping semantics explicit and testable.
- The docs update is part of the implementation, not a follow-up.
- If a helper can enforce valid `surface`/`subsystem` combinations centrally, prefer that over patching every callsite manually.

### Task 1: Define the corrected observability contract

**Files:**
- Modify: `docs/OBSERVABILITY.md`
- Reference: `apps/gateway-admin/app/(admin)/activity/page.tsx`
- Reference: `crates/lab/src/dispatch/logs/types.rs`

- [ ] **Step 1: Write the failing documentation checklist in the plan branch notes**

Document the current mismatches to close:
- `surface = "http"` example is stale and should be `api`
- operator-facing logs need explicit `subsystem`
- actor correlation needs a stable non-secret field
- activity UI must consume semantic activity logs, not raw log heuristics
- startup/operational logs must be distinguishable from activity logs

Expected: a short markdown scratch note or commit message draft listing the contract deltas before editing the doc.

- [ ] **Step 2: Update `docs/OBSERVABILITY.md` to define the new contract**

Add or revise sections covering:
- valid `surface` values actually used by persisted logs
- required `subsystem` for operator-facing events
- `actor_key` and optional redacted display tag semantics
- `activity-worthy` vs `operational-noise` event classes
- Activity view guidance: backend-owned semantic feed, not raw log reconstruction
- corrected example payloads using `surface = "api"`

Expected diff themes:
- stricter field contract
- clearer taxonomy
- explicit doc language that undefined subsystem values are a bug, not a convenience

- [ ] **Step 3: Review the updated doc for internal consistency**

Check that:
- examples use real enum values
- route examples match current product terminology
- the new actor-field wording does not violate the redaction section

Run: `rg -n 'surface": "http"|actor_key|activity-worthy|operational-noise|subsystem' docs/OBSERVABILITY.md`
Expected: `surface": "http"` absent; new sections present.

- [ ] **Step 4: Commit the docs contract update**

```bash
git add docs/OBSERVABILITY.md
git commit -m "docs: tighten observability contract for activity feed"
```

### Task 2: Add a shared activity taxonomy helper

**Files:**
- Create: `crates/lab/src/observability/activity.rs`
- Modify: `crates/lab/src/main.rs` (only if module registration is needed)
- Modify: `crates/lab/src/lib.rs` or nearest module root if needed
- Test: `crates/lab/tests/logs_activity.rs`

- [ ] **Step 1: Write the failing Rust test for actor-key derivation and taxonomy helpers**

Create tests that assert:
- actor key is deterministic for the same subject
- actor key is not the raw subject
- helper rejects or remaps invalid operator-facing subsystem values

Run: `cargo test -p lab logs_activity -- --nocapture`
Expected: FAIL because helper module does not exist yet.

- [ ] **Step 2: Implement `observability::activity` helper module**

Include minimal helpers such as:
- `fn actor_key(subject: &str) -> String`
- `fn subject_tag(subject: &str) -> String` if a display-safe tag is still needed
- `fn operator_subsystem(...) -> &'static str` or typed enum mapper for activity-worthy events
- comments tying behavior back to `docs/OBSERVABILITY.md`

Implementation constraints:
- one-way stable derivation for `actor_key`
- no raw subject persistence requirement
- no new ad hoc field names beyond what the doc now defines

- [ ] **Step 3: Run the focused helper tests**

Run: `cargo test -p lab logs_activity -- --nocapture`
Expected: PASS for actor-key and taxonomy helper tests.

- [ ] **Step 4: Commit the helper layer**

```bash
git add crates/lab/src/observability/activity.rs crates/lab/tests/logs_activity.rs
git commit -m "feat: add shared observability activity taxonomy helpers"
```

### Task 3: Normalize MCP dispatch logging

**Files:**
- Modify: `crates/lab/src/mcp/server.rs`
- Test: `crates/lab/tests/logs_activity.rs`

- [ ] **Step 1: Write the failing MCP ingest classification test**

Add a test that emits a representative MCP dispatch event through the tracing ingest path and asserts:
- `surface == "mcp"`
- `subsystem == "mcp_server"`
- `action` preserved
- `actor_key` present when subject exists

Run: `cargo test -p lab logs_activity::mcp -- --nocapture`
Expected: FAIL because MCP dispatch events currently land as `core_runtime` and lack `actor_key`.

- [ ] **Step 2: Patch MCP dispatch emitters**

Update dispatch start/ok/error callsites and any high-volume MCP lifecycle events so they consistently include:
- `subsystem = "mcp_server"`
- valid `surface = "mcp"`
- `actor_key` derived from subject when subject exists
- optional redacted subject tag only if still useful for human diagnosis

Do not change the outward dispatch result contract; this is an observability-only change.

- [ ] **Step 3: Run focused MCP activity tests**

Run: `cargo test -p lab logs_activity::mcp -- --nocapture`
Expected: PASS.

- [ ] **Step 4: Commit MCP logging normalization**

```bash
git add crates/lab/src/mcp/server.rs crates/lab/tests/logs_activity.rs
git commit -m "fix: normalize mcp activity logging"
```

### Task 4: Normalize API auth and upstream OAuth logging

**Files:**
- Modify: `crates/lab/src/api/auth_helpers.rs`
- Modify: `crates/lab/src/api/browser_session.rs`
- Modify: `crates/lab/src/api/upstream_oauth.rs`
- Modify: `crates/lab/src/api/device/oauth.rs`
- Test: `crates/lab/tests/logs_activity.rs`

- [ ] **Step 1: Write failing tests for auth-web and auth-upstream subsystem classification**

Add tests for representative flows:
- `session.get` / `session.logout` should classify as `auth_webui`
- upstream OAuth `probe/start/status/clear/callback` should classify as `auth_upstream` or `oauth_relay` per the new doc
- device OAuth relay start should classify consistently with the chosen contract

Run: `cargo test -p lab logs_activity::auth -- --nocapture`
Expected: FAIL due to missing subsystem and actor-key normalization.

- [ ] **Step 2: Normalize shared auth logging helpers**

Update `auth_helpers.rs` and any auth-adjacent route logging to emit:
- valid API dispatch fields
- explicit subsystem
- `actor_key` where subject exists

- [ ] **Step 3: Normalize route-local emitters that currently bypass the shared wrapper**

Patch `browser_session.rs`, `upstream_oauth.rs`, and `device/oauth.rs` to use the same subsystem and actor-key semantics. If repeated code appears, extract a small helper rather than duplicating field assembly.

- [ ] **Step 4: Run focused auth activity tests**

Run: `cargo test -p lab logs_activity::auth -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit auth activity normalization**

```bash
git add crates/lab/src/api/auth_helpers.rs crates/lab/src/api/browser_session.rs crates/lab/src/api/upstream_oauth.rs crates/lab/src/api/device/oauth.rs crates/lab/tests/logs_activity.rs
git commit -m "fix: normalize auth and oauth activity logging"
```

### Task 5: Normalize device syslog and operational-noise logging boundaries

**Files:**
- Modify: `crates/lab/src/api/device/syslog.rs`
- Modify: `crates/lab/src/oauth/local_relay.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Test: `crates/lab/tests/logs_activity.rs`

- [ ] **Step 1: Write failing tests for device-ingest and startup-noise classification**

Add tests or fixture-based assertions that:
- device syslog validation/rejection paths emit consistent API/device observability
- local relay no longer invents `surface = "oauth_relay"`
- startup logs do not masquerade as activity-worthy subsystems

Run: `cargo test -p lab logs_activity::operational -- --nocapture`
Expected: FAIL.

- [ ] **Step 2: Patch device syslog logging**

Ensure both success and key validation/failure paths emit structured logs aligned with the API dispatch contract.

- [ ] **Step 3: Patch local relay and serve bootstrap logging**

Normalize `local_relay.rs` to valid persisted `surface` values and explicit relay-appropriate subsystem semantics.

Normalize `serve.rs` startup logs so they:
- use documented subsystem values
- are clearly operational-noise, not user activity
- remain useful for startup diagnosis

- [ ] **Step 4: Run focused operational classification tests**

Run: `cargo test -p lab logs_activity::operational -- --nocapture`
Expected: PASS.

- [ ] **Step 5: Commit operational logging normalization**

```bash
git add crates/lab/src/api/device/syslog.rs crates/lab/src/oauth/local_relay.rs crates/lab/src/cli/serve.rs crates/lab/tests/logs_activity.rs
git commit -m "fix: align device and startup logging taxonomy"
```

### Task 6: Add a backend-owned activity query

**Files:**
- Create: `crates/lab/src/dispatch/logs/activity.rs`
- Modify: `crates/lab/src/dispatch/logs/dispatch.rs`
- Modify: `crates/lab/src/dispatch/logs/catalog.rs`
- Modify: `crates/lab/src/api/services/logs.rs` or add a dedicated API route if cleaner
- Modify: `crates/lab/tests/logs_api.rs`
- Modify: `crates/lab/tests/logs_dispatch.rs`

- [ ] **Step 1: Write the failing backend activity-query tests**

Add tests asserting a semantic activity query returns:
- MCP dispatch rows
- auth-web rows
- upstream OAuth rows
- optional `mine_only` scoping by `actor_key`
- no startup-noise rows by default

Run: `cargo test -p lab logs_api -- --nocapture`
Expected: FAIL because no activity query exists yet.

- [ ] **Step 2: Implement backend activity query translation**

Implement a backend module that queries the log store and returns semantic activity items. Prefer one of:
- `logs.activity` action under the existing `logs` service
- or a dedicated API endpoint if the contract is cleaner that way

Include filtering inputs such as:
- `limit`
- `mine_only`
- `actor_key`
- optional subsystem/category filters if needed

- [ ] **Step 3: Wire the new action/endpoint into the dispatch and API catalog**

Update the logs catalog/schema/help output so the activity query is discoverable.

- [ ] **Step 4: Run focused backend activity tests**

Run: `cargo test -p lab logs_dispatch logs_api -- --nocapture`
Expected: PASS for the new semantic activity behavior.

- [ ] **Step 5: Commit backend activity query support**

```bash
git add crates/lab/src/dispatch/logs/activity.rs crates/lab/src/dispatch/logs/dispatch.rs crates/lab/src/dispatch/logs/catalog.rs crates/lab/src/api/services/logs.rs crates/lab/tests/logs_api.rs crates/lab/tests/logs_dispatch.rs
 git commit -m "feat: add semantic activity query over log store"
```

### Task 7: Switch gateway-admin to the semantic activity feed

**Files:**
- Create: `apps/gateway-admin/lib/types/activity.ts`
- Create: `apps/gateway-admin/lib/api/activity-client.ts`
- Modify: `apps/gateway-admin/app/(admin)/activity/page.tsx`
- Modify: `apps/gateway-admin/lib/dashboard/admin-insights.ts`
- Modify: `apps/gateway-admin/lib/dashboard/admin-insights.test.ts`

- [ ] **Step 1: Write the failing UI/client tests**

Add or update tests so the Activity page behavior expects:
- semantic activity rows from the backend
- `mine_only` based on backend scoping, not `fields_json.subject === sub`
- no dependency on raw log subsystem guessing in client code

Run: `pnpm --filter gateway-admin test -- admin-insights`
Expected: FAIL until the new client contract is wired.

- [ ] **Step 2: Add typed activity client contract**

Create a dedicated activity client and types module rather than reusing raw log-search types.

- [ ] **Step 3: Simplify the Activity page**

Replace:
- raw `fetchLogs(...)`
- hardcoded `ACTIVITY_SUBSYSTEMS`
- client-side raw subject filter

With:
- backend semantic feed request
- backend-provided rows/counts
- viewer-driven `mine_only` parameter passed to the server

- [ ] **Step 4: Run focused gateway-admin tests**

Run: `pnpm --filter gateway-admin test -- activity`
Expected: PASS.

- [ ] **Step 5: Commit the UI contract switch**

```bash
git add apps/gateway-admin/lib/types/activity.ts apps/gateway-admin/lib/api/activity-client.ts apps/gateway-admin/app/'(admin)'/activity/page.tsx apps/gateway-admin/lib/dashboard/admin-insights.ts apps/gateway-admin/lib/dashboard/admin-insights.test.ts
git commit -m "feat: switch activity page to semantic activity feed"
```

### Task 8: Final verification pass and documentation sync check

**Files:**
- Modify as needed based on test output
- Review: `docs/OBSERVABILITY.md`
- Review: all touched Rust and TypeScript files

- [ ] **Step 1: Run focused backend verification**

Run: `cargo test -p lab logs_activity logs_dispatch logs_api -- --nocapture`
Expected: PASS.

- [ ] **Step 2: Run focused frontend verification**

Run: `pnpm --filter gateway-admin test -- activity`
Expected: PASS.

- [ ] **Step 3: Run grep-based consistency checks**

Run:
```bash
rg -n 'surface = "oauth_relay"|subsystem = "startup"|subsystem = "cli"|fields_json.subject ===|surface": "http"' crates apps docs
```
Expected:
- no invalid persisted surfaces
- no stale UI raw-subject filter
- no stale `surface": "http"` doc example

- [ ] **Step 4: Review the final touched-file set for contract drift**

Check that:
- all activity-worthy emitters use the shared taxonomy
- docs match the implemented field names
- tests cover the new semantics

- [ ] **Step 5: Commit final cleanup**

```bash
git add docs/OBSERVABILITY.md crates/lab/src apps/gateway-admin
 git commit -m "chore: finalize observability activity remediation"
```

## Notes for the implementing agent

- Do not collapse this into one large refactor. The point is to make each layer verifiable independently.
- If the chosen backend activity shape differs from `logs.activity`, update the plan and docs together before implementation drifts.
- If introducing `actor_key` requires exposing it in browser session JSON, ensure the field is documented and treated as non-secret but stable.
- Keep the raw log search API intact for the logs console; the activity feed is a separate semantic contract.
- `OBSERVABILITY.md` must be updated in the same branch as the code changes. Do not defer the docs update.
