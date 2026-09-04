# dispatch/upstream/ — Upstream MCP Proxy Pool

Surface-neutral upstream MCP server proxy. Manages connections to external MCP servers (HTTP or stdio), discovers their tools, and routes `call_tool` / `read_resource` requests.

## Why dispatch/, not mcp/

Both the MCP surface and the HTTP API surface need access to `UpstreamPool`. The layer contract forbids `api -> mcp` dependencies, so shared types must live in the dispatch layer.

Dependency direction:

- `api -> dispatch/upstream`
- `mcp -> dispatch/upstream`
- `cli -> dispatch/upstream`

## Files

| File | Purpose |
|------|---------|
| `upstream.rs` | Module entrypoint. |
| `pool.rs` | Coordinator (~160 LOC): `UpstreamPool` / `UpstreamConnection` struct defs, `InProcessConnector`/`InProcessRegistration` types, builders (`new`/`with_*`/`Default`), `mod` declarations, and `pub`/`pub(crate)` re-exports. **No business logic** — all method bodies live in the `pool/` child modules as additional `impl UpstreamPool` blocks. |
| `types.rs` | `UpstreamEntry`, `UpstreamTool`, `UpstreamHealth` types and the `CIRCUIT_BREAKER_THRESHOLD` / `REPROBE_INTERVAL` constants. |
| `auth.rs`, `http_client.rs`, `process_guard.rs`, `transport.rs` | Bearer/websocket auth, body-capped HTTP client, process-group guard, websocket transport. |

### `pool/` child modules

`pool.rs` keeps the struct definitions; each child module carries method bodies (private fields are visible to descendant modules, so no `pub(super)` is needed for fields — only for cross-module-called private inherent methods).

| Module | Purpose |
|--------|---------|
| `pool/helpers.rs` | Leaf knobs + constants (`DISCOVERY_TIMEOUT`, `DEFAULT_MAX_RESPONSE_BYTES`, …), error classification, naming, redaction, `UpstreamCachedSummary`, prompt/resource merge/rewrite/`cached_upstream_tool`, `max_response_bytes()`, `estimate_response_size`. |
| `pool/logging.rs` | `UpstreamRequestLog` + `log_upstream_request_{start,finish,error}`, `capability_name`, `is_capability_unsupported`. |
| `pool/entries.rs` | `UpstreamEntry` constructors, `health_str`, and the **single** fail-closed exposure-policy compiler (`resolve_named_exposure_policy`) that `expose_tools`/`expose_resources`/`expose_prompts` all resolve through, plus the `prompt_exposed`/`resource_exposed` matchers and the shared `log_exposure_filter` debug event. Never add a second policy implementation. |
| `pool/validate.rs` | `validate_upstream_config` + the `validate_*` tests. |
| `pool/connect.rs` | `connect_upstream` / `_http` / `_websocket`, `runtime_origin_label`, jitter/oauth-log helpers (reads env). All transport fns are generic over the client handler `H: ClientHandler`; `connect_upstream_with_client` passes `()` (the default for pooled connections), while `connect_upstream_with_handler` is the seam the relay path uses to install a `RelayClientHandler`. |
| `pool/http_cancellation.rs` | Builds the bounded HTTP/Unix-socket cancellation side channel, serializes relay-token requests and standard cancellation notifications, and requires an acknowledged relay-token response before treating delivery as correlated. |
| `pool/connect_stdio.rs` | `connect_stdio_upstream` (child-process spawn + process-group guard) + `connect_in_process_service_peer`. |
| `pool/connection.rs` | `UpstreamConnection` `Debug`/`Drop`/`shutdown` + `UpstreamPool::acquire_peer`. |
| `pool/lifecycle.rs` | `drain_for_swap`. |
| `pool/discover.rs` | `discover_all_inner` + `discover_all*` variants + `routable_upstream_peers`. |
| `pool/ensure.rs` | Lazy seeding + on-demand tool discovery; `replace_catalog_tools` shared mutator. |
| `pool/capability.rs` | `discover_capability_counts`. |
| `pool/probe.rs` | `ensure_probe_task` + `reprobe_upstream` background heartbeat/reconnect. |
| `pool/registration.rs` | In-process service-peer registration. |
| `pool/relay.rs` | `RelayClientHandler` — a `ClientHandler` for dedicated upstream connections that mirrors the downstream agent's MRTR input capabilities. `UpstreamPool::call_tool_relayed(config, subject, params, downstream, session_id)` returns a connection from a per-`(upstream, session_id, subject, capability_fingerprint)` cache (`relay_connections`) or opens one through the generic connection seam. Capability generations coexist so renegotiation cannot terminate an older in-flight call. Calls preserve typed MCP errors and `input_required`; the handler does not implement legacy server-initiated callbacks. Both MCP proxy branches select this path automatically when the downstream advertises an input capability. |
| `pool/relay_cache.rs` | Relay capability fingerprints, cache key/value ownership, downstream rebinding, task-notification flushing, and bounded LRU eviction. Keeps connection lifecycle state separate from relay request translation/execution. |
| `pool/relay_cancellation.rs` | Coordinates acknowledgement-aware relay-token cancellation, standard cancellation compatibility delivery, fixed relay-send deadlines, and bounded detached request-handle cleanup. |
| `pool/relay_cancellation_tests.rs` | Focused regressions for early false acknowledgements, blocked relay sends, and stalled request-handle cleanup. |
| `pool/tasks.rs` | Gateway-owned task routing and retained relay-connection lifecycle. |
| `pool/task_route.rs` | Public surface-neutral route-authorization snapshot captured when an upstream task handle is minted. |
| `pool/notifications.rs` | Owns the normalized notification event bus plus generation-guarded `subscriptions/listen` acknowledgment snapshots, retry tasks, concurrent refresh batching, and exact-upstream tool re-listing before downstream catalog publication. |
| `pool/oauth_invalidation.rs` | Single credential-lifecycle boundary that closes subject, relay, and task-retained peers after OAuth replacement, refresh, clear, or shared-provider revocation; returns structured invalidation counts without exposing raw subjects. |
| `pool/notifications_tests.rs` | Focused regressions for acknowledgement visibility, stale-generation isolation, concurrent refresh deadlines, and retry after initial failure. |
| `pool/listing_timeout_tests.rs` | Focused regressions for the per-upstream listing wall-clock bounds: stalled prompt/template listings must degrade to partial data within the `listing_catalog_timeout` budget. |
| `pool/catalog_pagination.rs` | Bounded pagination for catalog listing RPCs (`collect_bounded`): at most `MAX_CATALOG_PAGES` (64) pages per upstream per pass, plus per-page item and cursor-byte caps and a total deadline. A bounds breach is a hard error, never an empty or silently truncated catalog. Every upstream listing path (tools, prompts, resources, templates — connect-time discovery included) must use this instead of rmcp's unbounded `Peer::list_all_*`; `/clippy.toml` bans the `list_all_*` methods outright. The per-upstream listing wall-clock cap (`catalog_listing_timeout` / `CATALOG_LISTING_TIMEOUT`) lives in `pool/resources_list.rs`. |
| `pool/catalog_publication.rs` | Immutable publication of the routable tool, regular-resource, regular-resource-template, and regular-prompt projections; deliberately excludes UI resources/templates, skills, OAuth subjects, local and synthetic rows, and unrelated capability health. Each projection carries an independent generation so one catalog family cannot perturb another's publication identity. Owns the `CatalogWriteGuard` republish invariant: `catalog_write()` hands out the guard and its `Drop` calls `publish_if_changed()`, so no caller has to mark a projection dirty by hand. A `land:` merge silently dropped this module's `mod` declaration, which is what motivated the `xtask` module-reachability gate. |
| `pool/skills_exposure.rs` | Explainable exposure decisions for validated Agent Skills (`SkillExposureReason`: `allow_all` / `matched_pattern` / `not_matched`). Enforcement and the operator explanation evaluate the same compiled `SkillExposurePolicy`, so the operator surface never reverse-engineers a reason from a boolean after the policy context is gone. |
| `pool/tools_call_exact.rs` | Exact publication-bound regular `tools/call` kernel. Crate-private, deliberately unmounted, and freshness-only: it does not authorize AssetUse/admin/destructive execution and does not elicit. Usage telemetry is scheduled only after an exact checked apply and stays unattributed on this path. SEP-2243 recovery treats rmcp's peer-object schema cache as a non-authoritative transport hint only. |
| `pool/tools.rs` | Tool queries (`healthy_tools*`, `find_tool*`, `tool_schema`, exposure rows, summaries, runtime metadata, health). `subject_scoped_tools` applies `expose_tools` from the live `UpstreamConfig`, since a subject-scoped tool list never reaches the catalog and so has no `UpstreamEntry::exposure_policy` to read. |
| `pool/tool_call_cancel.rs` | Cancel-aware `tools/call` (both the `call_tool` and single-shot `call_tool_once` shapes; `Peer::call_tool` is one round trip — the MRTR round-driving loop lives on `RunningService`, which the pool never holds). rmcp has no drop-based cancellation for ordinary requests, so dropping a response future abandons the local await while the upstream keeps executing. The guard here sends `notifications/cancelled` for the in-flight request id if the future is dropped before the response arrives, and disarms on every terminal outcome. Delivery is detached but bounded by `CANCELLATION_DELIVERY_TIMEOUT` and logged, so a wedged upstream cannot accumulate parked tasks holding peer clones. Fires on *any* early drop, which is how the Code Mode path — which cancels by dropping the call future rather than passing a token — is covered without threading anything across the crate boundary. Pooled-path counterpart to `relay_cancellation.rs`. |
| `pool/pooled_cancellation_tests.rs` | Regressions pinning pooled-path cancellation: a cancelled call surfaces `CapabilityCallError::Cancelled`; the upstream observes the cancellation (its `context.ct` fires) and does not run to completion; a call cancelled *before* dispatch never reaches the upstream at all (the `biased` guarantee); a Code-Mode-style dropped future notifies too; a timeout notifies too; repeated cancellations do not trip the circuit breaker; and an uncancelled call is unaffected. |
| `pool/tools_call.rs` | `call_tool` + `subject_scoped_call_tool`. Owns `subject_scoped_tool_is_exposed` — the fail-closed `expose_tools` guard the OAuth execution primitives (here and the subject-scoped arm of `call_tool_relayed`) apply so a hidden tool is uncallable independently of which caller resolved the owner. |
| `pool/tools_exposure_tests.rs` | Focused regressions for `expose_tools` parity between the catalog-backed and OAuth subject-scoped paths: symmetry, fail-closed on a malformed or empty allowlist, per-upstream policy isolation, and hidden-means-uncallable. |
| `pool/usage_record.rs` | `record_usage_call` — fire-and-forget usage-telemetry write after every tool/resource/prompt call outcome, bounded by `UsageStore`'s write semaphore. |
| `pool/health.rs` | Circuit breaker: `record_*`, `should_reprobe*`, `*_last_error`, `filter_collisions`, `upstream_status`/`upstream_count`. |
| `pool/resources_exposure_tests.rs` | Regressions pinning `expose_resources` on both the listing and the read (list-only filtering is a bypass). |
| `pool/prompts_exposure_tests.rs` | Regressions pinning `expose_prompts` on the listing, `prompts/get`, and subject-scoped owner lookup. |
| `pool/resources_list.rs` | Bounded resource listing + synthetic `gateway_*` documents. OAuth listings reuse the per-subject connection cache. Native `ui://` (mcp-ui) resources skip the `lab://upstream/{name}/…` rewrite so they stay addressable by the same URI a tool's `_meta.ui.resourceUri` references. |
| `pool/subscription_schedule.rs` | Coalesces resource-triggered subscription refreshes onto cancellable background batches so `resources/list` never waits for acknowledgement handshakes. |
| `pool/resources_read.rs` | `read_upstream_resource` + `subject_scoped_read_resource` + `read_upstream_ui_resource` (reverse-looks-up the owning upstream by cached native `ui://` `resource_uris`, forwards the read, preserves the native URI — **no** `lab://upstream/` rewrite — for mcp-ui widget resources). |
| `pool/prompts_list.rs` | Prompt listing + ownership lookup (`collect_upstream_prompts`, `find_prompt_owner`, …). |
| `pool/prompts_get.rs` | `subject_scoped_prompts`, `get_prompt`, `subject_scoped_get_prompt`. |
| `pool/prompts_exposure.rs` | `retain_exposed_prompts` — applies incarnation-checked compiled `expose_prompts` policies to an already-merged prompt list. |
| `pool/testsupport.rs` | `#[cfg(test)]` shared fixtures + mock servers (`pub(super)`). |

**The 500-LOC limit (tests included) remains the target and the rule for new
files.** Multiple legacy upstream modules still exceed that target and require
follow-up splits. All new files added to `pool/` must stay under 500 LOC.

## Key Types

- `UpstreamPool` — holds live connections and discovered tool catalogs. Cloneable (Arc internals).
- `UpstreamEntry` — snapshot of a single upstream: name, tools, health state.
- `UpstreamTool` — a discovered tool with its cached input schema and owning upstream name.
- `UpstreamHealth` — `Healthy` or `Unhealthy { consecutive_failures }`.
- `UpstreamConnection` — a live rmcp `Peer<RoleClient>` with its owning `RunningService`.

## Constants

| Constant | Value | Location |
|----------|-------|----------|
| `CIRCUIT_BREAKER_THRESHOLD` | 3 | `types.rs` |
| `REPROBE_INTERVAL` | 30 seconds base | `types.rs` |
| `MAX_REPROBE_INTERVAL` | 30 minutes | `types.rs` |
| `DEFAULT_UPSTREAM_CALL_CONCURRENCY` | 8 per upstream | `pool/helpers.rs` |
| `DISCOVERY_TIMEOUT` | 15 seconds | `pool/helpers.rs` |
| `DEFAULT_MAX_RESPONSE_BYTES` | 24 MiB | `pool/helpers.rs` |

## Rules

- Do not read env vars outside `pool/helpers.rs` (`max_response_bytes()`, `upstream_discovery_concurrency()`) and the connect modules (`pool/connect.rs`, `pool/connect_stdio.rs`, `pool/http_cancellation.rs`). Keep env reads confined to that small, named set of places.
- Do not import MCP-specific types (envelopes, registry) from `mcp/`.
  The `InProcessConnector` IoC seam (`pool.rs`) is the correct boundary: the
  MCP layer (`crate::mcp::in_process_peer`) injects a connector at startup; the
  pool calls it without knowing about `LabMcpServer`. As of A-M6, `connect_stdio.rs`
  no longer has any `crate::mcp` import — the boundary is clean. Do not re-add
  `mcp/` imports to any `dispatch/upstream/` file.
  **PATH/basename-only spawn-guard caveat (S6):** the spawn-guard allowlist check
  in `crate::security::spawn_guard` (`src/security/spawn_guard.rs`) is basename-only
  — `/tmp/x/node` passes because its basename is `node`. This is an accepted
  residual: the trust boundary is admin-write access to the gateway config, and
  no further PATH resolution is performed at spawn time. See
  `src/security/spawn_guard.rs` for the canonical comment.
- Do not import API-specific types (router, state) from `api/`.
- The pool is constructed in `cli/serve.rs` and injected into `AppState` and `LabMcpServer`.
- Circuit breaker state is internal to the pool. Surfaces call `record_failure()` and `record_success()`. Open circuits use exponential quarantine, and every failed reprobe resets the quarantine clock.
- A caller-attributed tool call must carry the downstream cancellation token. `timed_capability_call_with_timeout` takes `cancel: Option<&CancellationToken>` and abandons both the bulkhead wait and the RPC when it fires, letting the RPC's own guard tell the upstream to stop. `None` is correct only for fan-out/discovery passes, which have no downstream request to withdraw from. Do not add a parallel `*_cancellable` method — thread the token through the existing signature.
- `biased;` in those `select!`s is load-bearing. The RPC future is lazy, so polling the token first means an already-cancelled caller never dispatches the request at all; without it tokio's random order writes the request out roughly half the time, executing a side effect for a caller that is already gone.
- A cancelled call is NOT a circuit-breaker failure — the caller withdrew and the upstream is healthy — and it logs at INFO with `event = "cancelled"`, not the WARN `event = "error"`. Client disconnects are routine; upstream error rate is the primary is-my-gateway-sick signal and must not be buried under them. The no-breaker decision depends on the HTTP transport backstop exceeding the upstream deadline (`LabConfig::http_request_timeout`), so a wedged upstream still trips via `Timeout` before any caller withdraws.
- Every caller-attributed upstream RPC must pass through the per-upstream bulkhead. Do not bypass `timed_capability_call` or the relay generation registration when adding tool, prompt, resource, or task paths. The documented exceptions are the fan-out aggregation passes — discovery (`pool/discover.rs`) and prompt/resource listing (`pool/prompts_list.rs`, `pool/resources_list.rs`) — which run under their own discovery concurrency/timeout bounds, keep deliberate partial-result semantics, and record per-upstream failures via the circuit breaker, `*_last_error`, and classified `warn!` logs instead.
- **SEP-2243 tool-header recovery is exact and single-shot.** rmcp derives
  `Mcp-Param-*` mirrors from `x-mcp-header` annotations learned by `tools/list`
  on each transport. Pooled, OAuth-subject, and MRTR relay connections may hold
  independent schema caches. On rmcp's typed `ErrorCode::HEADER_MISMATCH`, the
  tool-call primitive re-lists tools on that exact peer to refresh rmcp's
  transport-local schema cache and retries the same request once under the
  original deadline. The relay path must keep that refresh/replay state in the
  boxed `send_relay_tool_request_with_header_recovery` helper: inlining it into
  `call_tool_relayed` makes the already-large future exceed Tokio worker stack
  headroom when relays nest across multiple Labby hops, even on calls that never
  take the recovery branch. Do not synthesize parameter headers from arbitrary
  arguments, broaden the matcher to ordinary MCP errors, inline the recovery
  state back into the relay future, or add an unbounded retry loop: annotated
  headers are a selective privacy boundary and HeaderMismatch is the
  pre-dispatch signal that makes one replay safe. Recovery observability uses
  `action = "tool.header_mismatch"`, `tool.header_cache.refresh`, and
  `tool.header_cache.retry`; each event carries the upstream name and its
  monotonic counter. `gateway.status` additionally exposes non-zero
  `header_recovery` counters per upstream. The regression test
  `relay_header_recovery_future_stays_structurally_boxed` enforces the durable
  invariant directly: the HeaderMismatch refresh/replay helper returns a
  `BoxFuture`, so unrelated growth in `call_tool_relayed` cannot weaken or
  spuriously trip the guard. Do not inline that recovery state back into the
  outer relay future.
- OAuth credential mutation is an execution barrier, not merely a token-cache
  update. Callback replacement, refresh, clear, and shared-provider revocation
  must route through `pool/oauth_invalidation.rs` so initialized subject peers,
  OAuth relay peers, and task-retained peers are closed before success is
  reported. Never log raw OAuth subjects from this boundary; emit only
  `upstream`, a bounded `reason`, and per-cache invalidation counts.
- **Exposure is enforced on discovery *and* on access.** `expose_tools`,
  `expose_resources`, and `expose_prompts` must each gate the listing path and
  the direct-access path (`tools/call`, `resources/read`, `prompts/get`,
  `completion/complete`) on every routing variant — catalog-backed, OAuth
  subject-scoped, and MRTR relay. Filtering only the list leaves the item
  reachable by name/URI, which is a bypass, not a restriction. Catalog-backed
  paths read the compiled policy off `UpstreamEntry`; subject-scoped and relay
  paths have no catalog entry and resolve it from the live `UpstreamConfig`
  through the same `pool/entries.rs` helpers. The cached
  `prompt_names`/`resource_uris` inspection snapshots stay deliberately
  unfiltered so the admin exposure editor can still see excluded entries.
- Stdio lifecycle ownership lives in `pool/stdio_transport.rs`; it is the single waiter for child exit so PID, generation, exit status, stderr tail, and invalidated requests remain correlated.
