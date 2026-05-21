# Observability Contract Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every service request in `lab` traceable end-to-end across CLI, MCP, HTTP, and SDK boundaries so integration failures stop behaving like a black box.

**Architecture:** The core change is to make observability a hard contract at the shared boundary, not an optional per-service convention. `lab-apis::core::HttpClient` becomes the mandatory request logging point for every outbound call, while `lab` surfaces add caller context (`surface`, `service`, `action`, `request_id`, `instance`) and propagate it downstream. Documentation is updated so a service is not considered “online” until this instrumentation exists and is verified.

**Tech Stack:** Rust 2024, `tracing`, `tracing-subscriber`, `reqwest`, `axum`, `rmcp`, repo docs in `docs/`

---

### Task 1: Define The Observability Contract In Docs

**Files:**
- Modify: `docs/ARCH.md`
- Modify: `docs/SERVICE_ONBOARDING.md`
- Modify: `docs/README.md`

- [ ] **Step 1: Write the failing doc expectation as a checklist in the plan**

The contract must explicitly answer:

- where logs are mandatory
- which fields are mandatory
- how caller context flows from CLI/MCP/API into the SDK
- what counts as “service is online”
- what must never be logged

- [ ] **Step 2: Update `docs/ARCH.md` to replace the current aspirational logging paragraph**

Add a concrete section under `## Logging Shape` that states:

- `HttpClient` MUST emit one `request.start` event and one `request.finish` or `request.error` event for every outbound call
- caller surfaces MUST emit one dispatch event per user-visible action
- CLI/MCP/API MUST include `surface`, `service`, `action`, and `elapsed_ms`
- HTTP MUST also include `request_id`
- health probes MUST include `operation = "health"`
- destructive actions MUST log intent before execution and result after execution

- [ ] **Step 3: Update `docs/SERVICE_ONBOARDING.md` to make observability a required onboarding step**

Insert a new section after the SDK/client steps:

- “Step X: Add Observability”
- require request instrumentation before CLI/MCP/API verification
- state that a service is not complete until logs prove a request can be traced end-to-end

- [ ] **Step 4: Update `docs/README.md` if needed so the new observability contract is discoverable**

Add one short bullet linking the architecture/onboarding guidance.

- [ ] **Step 5: Verify the docs are internally consistent**

Run:

```bash
rg -n "Logging Shape|observability|HttpClient|service is online" docs
```

Expected:

- updated `ARCH.md`
- updated `SERVICE_ONBOARDING.md`
- no conflicting older wording

- [ ] **Step 6: Commit**

```bash
git add docs/ARCH.md docs/SERVICE_ONBOARDING.md docs/README.md
git commit -m "docs: define mandatory observability contract"
```

### Task 2: Add Shared Request Instrumentation To `HttpClient`

**Files:**
- Modify: `crates/lab-apis/src/core/http.rs`
- Modify: `crates/lab-apis/src/core/error.rs`
- Test: `crates/lab-apis/src/core/http.rs` (unit tests in-module if appropriate)

- [ ] **Step 1: Write failing tests for request classification and logging-friendly error detail**

Add tests that exercise:

- 401 -> `ApiError::Auth`
- 404 -> `ApiError::NotFound`
- 429 -> `ApiError::RateLimited`
- transport failure preserves enough message detail to distinguish DNS/TLS/connect classes

Expected initial failure:

- missing richer network classification helpers and/or missing helper functions for instrumentation

- [ ] **Step 2: Add a small internal request-context model in `HttpClient`**

Introduce a focused helper struct or local variables that derive:

- `method`
- `path`
- `host`
- redacted `url`
- start timestamp

Keep it internal to `http.rs`; do not leak transport concerns to service modules.

- [ ] **Step 3: Emit a `tracing` event before sending each request**

For every shared method (`get_json`, `get_json_query`, `post_json`, `put_json`, `patch_json`, `delete`, `delete_query`, `post_void`, `get_void`):

- emit `request.start`
- include `method`, `path`, `host`
- omit secrets and auth headers

- [ ] **Step 4: Emit completion events after response or failure**

On success:

- event name/message: `request.finish`
- fields: `method`, `path`, `host`, `status`, `elapsed_ms`

On failure:

- event name/message: `request.error`
- fields: `method`, `path`, `host`, `elapsed_ms`, `kind`, `message`

- [ ] **Step 5: Improve network error detail without changing the public taxonomy**

Keep `ApiError::kind()` stable, but preserve raw transport context in the message so failures like:

- DNS resolution failure
- TCP connect refused
- TLS certificate validation failure
- timeout

remain distinguishable in logs and user-facing errors.

- [ ] **Step 6: Run focused tests**

Run:

```bash
cargo test -p lab-apis --lib core::http
cargo test -p lab-apis --lib core::error
```

Expected:

- request helper tests pass
- error taxonomy remains stable

- [ ] **Step 7: Commit**

```bash
git add crates/lab-apis/src/core/http.rs crates/lab-apis/src/core/error.rs
git commit -m "feat: add shared http request instrumentation"
```

### Task 3: Propagate Caller Context From CLI, MCP, And HTTP

**Files:**
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/cli/unifi.rs`
- Modify: `crates/lab/src/cli/bytestash.rs`
- Modify: `crates/lab/src/cli/radarr.rs` if needed for parity
- Modify: `crates/lab/src/cli/serve.rs`
- Modify: `crates/lab/src/api/services/*.rs` via shared pattern or helper
- Modify: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Write down the exact caller-context fields to propagate**

Minimum fields:

- `surface` = `cli` | `mcp` | `http`
- `service`
- `action`
- `instance` when applicable
- `request_id` for HTTP
- `operation = "health"` for health probes

- [ ] **Step 2: Add a thin shared helper for dispatch spans in `lab`**

Prefer one small helper rather than duplicating span construction in every service file.

Candidate location:

- `crates/lab/src/mcp/`
- or a new focused helper module such as `crates/lab/src/observability.rs`

- [ ] **Step 3: Wrap CLI service dispatch in a caller span**

Every service CLI shim should run its shared dispatcher inside a span carrying:

- `surface = "cli"`
- `service`
- `action`

This makes outbound `HttpClient` events inherit CLI context automatically.

- [ ] **Step 4: Make MCP dispatch use the same caller span contract**

Update `serve.rs` so the MCP handler sets:

- `surface = "mcp"`
- `service`
- `action`

before dispatching into a service.

- [ ] **Step 5: Make HTTP dispatch inherit request IDs all the way down**

`api/router.rs` already creates `x-request-id`; ensure service dispatch spans include:

- `surface = "http"`
- `service`
- `action`
- `request_id`

- [ ] **Step 6: Run binary tests**

Run:

```bash
cargo test -p lab --bin lab
```

Expected:

- existing dispatcher tests still pass
- no behavior regressions in CLI/MCP/API envelopes

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/cli.rs crates/lab/src/cli crates/lab/src/cli/serve.rs crates/lab/src/api
git commit -m "feat: propagate caller context into service dispatch"
```

### Task 4: Add Health-Probe And Destructive-Action Logging Semantics

**Files:**
- Modify: `crates/lab/src/cli/health.rs`
- Modify: `crates/lab/src/mcp/services/*.rs` where destructive actions already exist
- Modify: `crates/lab/src/api/services/*.rs` only if shared helper usage requires it

- [ ] **Step 1: Make health operations identifiable in logs**

For `lab health` and `lab doctor`, ensure spans/events include:

- `surface = "cli"`
- `operation = "health"`
- `service`

- [ ] **Step 2: Add explicit pre/post events around destructive dispatch**

For dispatchers with destructive actions, emit:

- `destructive.intent`
- `destructive.result`

Do not log request bodies that may include secrets.

- [ ] **Step 3: Keep the logging rule centralized**

Do not hand-roll this per service if a shared helper can do it from `ActionSpec.destructive`.

- [ ] **Step 4: Verify on one existing destructive-capable service**

Use a safe non-destructive action for smoke plus unit coverage for the intent/result helper path.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/cli/health.rs crates/lab/src/mcp/services crates/lab/src/api/services
git commit -m "feat: add observability semantics for health and destructive actions"
```

### Task 5: Verify End-To-End Traceability On Real And Mocked Paths

**Files:**
- Modify: `docs/SERVICE_ONBOARDING.md`
- Modify: `docs/coverage/unifi.md`
- Modify: `docs/coverage/bytestash.md`
- Optional Test Helpers: service-local mock/test harness files if needed

- [ ] **Step 1: Define a verification recipe in docs**

The recipe should require:

1. run a known read-only CLI action with `LAB_LOG_FORMAT=json`
2. confirm dispatch event exists
3. confirm outbound request start/finish or request/error exists
4. confirm shared fields line up across both events

- [ ] **Step 2: Verify on a mocked service path**

Use ByteStash or another safe local mock:

```bash
LAB_LOG=labby=trace,lab_apis=trace LAB_LOG_FORMAT=json \
BYTESTASH_URL=http://127.0.0.1:8766 BYTESTASH_TOKEN=dummy \
lab bytestash categories.list 2>trace.jsonl
```

Expected:

- one CLI dispatch event
- one `request.start`
- one `request.finish`

- [ ] **Step 3: Verify on a real service path that currently fails**

Use UniFi:

```bash
LAB_LOG=labby=trace,lab_apis=trace LAB_LOG_FORMAT=json \
lab unifi sites.list 2>trace.jsonl
```

Expected:

- one CLI dispatch event
- one `request.start`
- one `request.error`
- failure message precise enough to distinguish cert/TLS issues from generic network errors

- [ ] **Step 4: Update coverage docs with observability verification status**

Add a short note to service coverage docs when traceability has been verified.

- [ ] **Step 5: Commit**

```bash
git add docs/SERVICE_ONBOARDING.md docs/coverage/unifi.md docs/coverage/bytestash.md
git commit -m "docs: add observability verification workflow"
```

### Task 6: Make Observability A Release Gate For New Services

**Files:**
- Modify: `docs/SERVICE_ONBOARDING.md`
- Modify: `docs/ARCH.md`
- Optional: `docs/README.md`

- [ ] **Step 1: Add a hard completion rule**

State explicitly:

- a service is not “implemented” until logs prove requests are observable end-to-end

- [ ] **Step 2: Add the onboarding checklist item**

Checklist must include:

- request instrumentation present
- CLI/MCP/API caller context present
- at least one read-only action traced successfully

- [ ] **Step 3: Run a final consistency grep**

Run:

```bash
rg -n "black box|observability|service is not complete|HttpClient" docs
```

Expected:

- docs consistently describe observability as mandatory

- [ ] **Step 4: Commit**

```bash
git add docs/ARCH.md docs/SERVICE_ONBOARDING.md docs/README.md
git commit -m "docs: make observability a service onboarding gate"
```

### Implementation Notes

- Preserve the stable `ApiError::kind()` taxonomy; improve detail in messages and logs, not the top-level kind strings unless the docs and envelopes are updated together.
- Avoid introducing per-service bespoke logging. The point is to move observability into shared boundaries.
- Redaction is non-negotiable. Never log auth headers, tokens, passwords, cookies, or full URLs if they can embed credentials.
- Prefer shared helpers over copy-paste in individual CLI and API service modules.
- Start with one service path for proof, then fan out.

### Verification Summary

Primary commands once implementation is complete:

```bash
cargo test -p lab-apis --lib
cargo test -p lab --bin lab
LAB_LOG=labby=trace,lab_apis=trace LAB_LOG_FORMAT=json lab bytestash categories.list
LAB_LOG=labby=trace,lab_apis=trace LAB_LOG_FORMAT=json lab unifi sites.list
```

Expected outcomes:

- all tests pass
- successful calls emit `request.start` + `request.finish`
- failing calls emit `request.start` + `request.error`
- every event is attributable to one caller surface and one service action
