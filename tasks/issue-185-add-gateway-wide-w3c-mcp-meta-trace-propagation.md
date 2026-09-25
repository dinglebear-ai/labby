# Issue #185 — Add gateway-wide W3C / MCP _meta trace propagation

Source issue: https://github.com/dinglebear-ai/labby/issues/185
Mapped against `origin/main` at `41ef5fb4936a49993bc36fc95ee95760100a9e30`.

## Current architecture

- Direct MCP tool proxying enters `crates/labby/src/mcp/call_tool_upstream.rs::call_tool_upstream_impl`; `prepare_upstream_tool_request` already preserves incoming request metadata and has regression coverage for `_meta.traceparent`.
- Code Mode calls enter `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs::call_tool`, then `execute_upstream_tool_checked`, which constructs `CallToolRequestParams` and dispatches through `UpstreamPool::checked_call_tool`.
- The pool's tool-call spine lives in `crates/labby-gateway/src/upstream/pool/tools_call.rs` and `checked_call.rs`; both reach `call_tool_with_header_recovery`, whose retry reuses the same cloned request.
- Code Mode already threads `ExecCtx { seq, execution_id, step_ordinal }` from `crates/labby-codemode/src/runner_drive.rs` into host calls, while `response.calls[]` is built from host-brokered external calls. There is no dedicated `call_ordinal` yet.
- Existing upstream request logs inherit outer `request_id` span context in `crates/labby-gateway/src/upstream/pool/logging.rs`; the server-log projection currently correlates `trace_id`, `request_id`, and `execution_id` in `crates/labby/src/dispatch/server_logs/dispatch.rs`.
- `tracestate` and `baggage` have no implementation on current main. `traceparent` exists only as transparent metadata pass-through coverage today.

## Dependency order

T01 → T02 → T03 → T04 → T05
                  ├────→ T06
                  └────→ T07
T05 + T06 + T07 → T08 → T09

- T04 can start once T03's outbound-context API is stable.
- T06 and T07 can proceed in parallel after T03.
- T08 is the explicit policy/configuration checkpoint after functionality exists and overhead can be measured.
- T09 is verification/closure only; it must not become an implementation catch-all.

## Tasks

- [ ] **T01 — Define the bounded SEP-414 trace-context primitive**
  - **Acceptance:**
    - One neutral implementation represents a validated W3C trace id/span id, `traceparent`, optional `tracestate`, optional bounded `baggage`, and the chosen namespaced Labby execution/call extension.
    - Standard MCP keys remain exactly `traceparent`, `tracestate`, and `baggage`; Labby extensions cannot shadow them.
    - Maximum lengths/counts and malformed-input behavior are constants with tests.
    - Invalid inbound `traceparent` never causes its accompanying `tracestate` to be continued.
    - Inject → extract round trips preserve the validated context.
    - No auth, route, tenant, capability, redaction, or storage policy is embedded in the primitive.
  - **Verify:** focused unit tests for W3C valid/invalid/boundary vectors plus `just check` and `just lint`.
  - **Files:** a single reusable trace-context module/crate chosen during implementation; `Cargo.toml` only if an internal workspace crate is warranted.
  - **Depends on:** none.
  - **Scope:** M, target 2–4 files.
  - **🔒 Gate:** adding any new third-party dependency requires maintainer approval; prefer the existing dependency set/current rmcp 3.3 APIs.

- [ ] **T02 — Establish inbound MCP trace creation/continuation policy**
  - **Acceptance:**
    - Applicable inbound gateway requests continue an allowed valid W3C context or mint a fresh trace id.
    - Invalid, oversized, or untrusted metadata is rejected/replaced deterministically according to documented policy.
    - Request-local trace state is attached to host-owned context, never tool arguments or authorization state.
    - Existing transparent metadata preservation in direct tool proxying remains intact.
    - Tests cover absent metadata, valid continuation, malformed/oversized metadata, and authorization invariance.
  - **Verify:** focused MCP handler/context tests; existing `traceparent` preservation regression remains green; `just check`.
  - **Files:** `crates/labby/src/mcp/context.rs` or the equivalent request-context boundary, `crates/labby/src/mcp/call_tool.rs`, trace primitive tests.
  - **Depends on:** T01.
  - **Scope:** M, target 3–5 files.

- [ ] **T03 — Inject child trace metadata at the shared outbound tool-call spine**
  - **Acceptance:**
    - Direct, OAuth-subject-scoped, checked Code Mode, and ordinary pooled tool calls all use one canonical outbound metadata-preparation path.
    - One inbound causal request keeps one trace id across sequential and concurrent fan-out while every outbound operation receives a distinct span/parent id.
    - The SEP-2243 header-recovery retry path documents and tests whether a retry represents the same logical operation and therefore reuses the same outbound trace metadata.
    - Existing request `_meta` fields unrelated to tracing survive injection.
    - Peers that ignore unknown metadata continue to work.
  - **Verify:** pool tests for direct/subject-scoped/checked calls, concurrent fan-out, retry, metadata preservation, and peer ignorance; `just test` for touched crates.
  - **Files:** `crates/labby-gateway/src/upstream/pool/tools_call.rs`, `crates/labby-gateway/src/upstream/pool/checked_call.rs`, trace primitive/integration module, focused tests.
  - **Depends on:** T02.
  - **Scope:** M, target 3–5 files.

- [ ] **T04 — Add a host-owned Code Mode call ordinal**
  - **Acceptance:**
    - `ExecCtx` gains a monotonic external `call_ordinal` distinct from runner protocol `seq` and `step_ordinal`.
    - Ordinals are assigned only to host-brokered upstream calls that appear in `response.calls[]`; internal/local pseudo-operations do not consume them.
    - Concurrent fan-out produces deterministic ordinals aligned 1:1 with final `response.calls[]` order.
    - Standalone/write-free execution remains supported when `execution_id` is absent.
  - **Verify:** `labby-codemode` unit tests for sequential calls, `Promise.all` fan-out, local pseudo-calls, failures, and response ordering; `just check`.
  - **Files:** `crates/labby-codemode/src/host.rs`, `crates/labby-codemode/src/runner_drive.rs`, `crates/labby-codemode/src/types.rs`, focused tests.
  - **Depends on:** T03's context API may be stubbed, but no gateway enrichment yet.
  - **Scope:** M, target 3–4 files.

- [ ] **T05 — Enrich Code Mode upstream calls with execution/call correlation**
  - **Acceptance:**
    - `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs::call_tool` passes host-owned `execution_id` + `call_ordinal` into the canonical outbound trace path.
    - The correlation fields use the namespaced extension fixed in T01 and never replace W3C parent/span relationships.
    - Sandbox JavaScript cannot author, override, or grant trusted correlation metadata through ordinary tool params.
    - In-process authorization metadata and trace metadata merge without either shadowing the other.
    - Direct and Code Mode traffic use the same standard trace format.
  - **Verify:** gateway Code Mode tests proving forged params cannot set trusted correlation, real context is injected, in-process auth is unchanged, and ordinal/execution id match the recorded call.
  - **Files:** `crates/labby-gateway/src/gateway/code_mode/code_mode_host.rs`, trace integration helper, Code Mode gateway tests.
  - **Depends on:** T03, T04.
  - **Scope:** M, target 2–4 files.

- [ ] **T06 — Correlate upstream structured logs with trace/span context**
  - **Acceptance:**
    - Upstream `request.start` / finish / error records can be correlated by trace id and outbound span id without flattening arbitrary or untrusted baggage into logs.
    - Existing inherited `request_id` behavior remains intact.
    - `server_logs.query` correlation recognizes the canonical trace fields needed to locate direct and Code Mode call timelines.
    - Secret/redaction tests cover trace-adjacent values and malicious extension/baggage content.
  - **Verify:** structured-log capture tests in the pool plus server-log projection tests; `just test` for `labby-gateway` and `labby` focused modules.
  - **Files:** `crates/labby-gateway/src/upstream/pool/logging.rs`, `crates/labby/src/dispatch/server_logs/dispatch.rs`, observability tests.
  - **Depends on:** T03.
  - **Scope:** S–M, target 2–4 files.

- [ ] **T07 — Extract propagated context on Labby server/upstream handler entry**
  - **Acceptance:**
    - A Labby/rmcp server receiving SEP-414 metadata validates/extracts the same canonical context and enters the matching tracing span/context.
    - Nested Labby gateway hops preserve trace id and derive the next child relationship correctly.
    - Malicious metadata cannot inject trusted auth/log fields or escape redaction.
    - Authorization results are identical with tracing enabled, disabled, absent, or forged.
  - **Verify:** nested gateway/upstream integration test plus denied-operation tests with forged trace metadata.
  - **Files:** MCP request/context handler boundary in `crates/labby/src/mcp/`, trace helper, focused integration tests.
  - **Depends on:** T03.
  - **Scope:** M, target 3–5 files.

- [ ] **T08 — Measure overhead and settle propagation/default configuration** 🔒
  - **Acceptance:**
    - Representative sequential and concurrent fan-out benchmarks record latency/allocation impact with propagation enabled.
    - The issue records a concrete decision: default-on propagation vs configurable propagation.
    - Sampling/export is explicitly separate from context propagation; correlation works with no OpenTelemetry exporter.
    - If configuration is needed, its schema/default/docs/tests are added through Labby's existing configuration surface. If it is not needed, no ornamental flag is introduced.
    - Default behavior does not materially reduce Code Mode concurrency or upstream compatibility.
  - **Verify:** reproducible benchmark/smoke commands and config/schema tests; `just docs-check` if documented config changes.
  - **Files:** benchmark/test location selected by existing conventions; if required, `crates/labby-runtime/src/gateway_config.rs`, `.env.example`, generated/product docs.
  - **Depends on:** T05, T06, T07.
  - **Scope:** M.
  - **🔒 Gate:** maintainer review before adding a new public config/env surface or exporter dependency.

- [ ] **T09 — Prove end-to-end interoperability and close the inherited scope**
  - **Acceptance:**
    - A live diagnostic flow demonstrates inbound request → gateway → at least two concurrent upstream calls with one trace id, distinct outbound spans, and Code Mode execution/call correlation.
    - Tests cover direct + Code Mode paths, nested hops, retry semantics, peer ignorance, redaction, and authorization invariance.
    - Current `main` is re-audited for every rmcp tool-call construction/forwarding path so no applicable path bypasses canonical injection.
    - #190, #191, and #192 are re-read and every still-valid inherited requirement is represented by merged code/tests/docs.
    - Final verification records merged PR/commit SHAs, rmcp version, performance numbers, and exact commands/results before #185 can close.
  - **Verify:** `just check`, `just test`, `just lint`, `just docs-check`, focused trace/gateway/Code Mode tests, and the live fan-out smoke.
  - **Files:** focused test/docs files only; implementation gaps discovered here must be returned to the owning earlier task rather than expanded inside T09.
  - **Depends on:** T08.
  - **Scope:** S–M, verification/documentation only.

## Checkpoints

### After T01–T03: canonical propagation foundation
- W3C/SEP-414 vectors pass.
- Direct and pooled tool calls use one bounded metadata contract.
- Existing `traceparent` pass-through tests still pass.
- Header-retry semantics are explicit.

### After T04–T07: Code Mode + observability integration
- Code Mode ordinals align with `response.calls[]`.
- Execution/call metadata cannot be forged through sandbox params.
- Direct and Code Mode traffic correlate through the same trace format.
- Nested handler extraction and authorization-invariance tests pass.

### After T08–T09: release/closure gate
- Propagation/default policy is recorded from measured behavior.
- Full repo quality gates are green.
- Live fan-out evidence satisfies issue #185 sections A–I.
- Only then is #185 eligible to close.
