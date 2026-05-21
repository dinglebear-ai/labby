# Local Master Log Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a dispatch-owned local-master logging subsystem with persistent indexed history, live SSE streaming for HTTP/WebUI, and shared search/query access from CLI, MCP, API, and WebUI.

**Architecture:** Add a new always-on product-local `logs` subsystem under `crates/lab/src/dispatch/` that owns one explicit `LogSystem` runtime, normalized event ingestion, redaction, indexed persistence, retention, and live subscriber fanout. Keep CLI, MCP, API, and WebUI as thin adapters over one shared query model; use true live streaming only on HTTP SSE, and keep MCP to bounded search/tail polling shapes while preserving explicit commented extension seams for future fleet/syslog ingestion.

**Tech Stack:** Rust 2024, `tracing`/`tracing-subscriber`, axum SSE, existing dispatch registry/catalog patterns, embedded indexed local store, Next.js 16 + React 19 gateway-admin UI, cargo-nextest, node test runner.

---

## File Map

### New Rust files

- Create: `crates/lab/src/dispatch/logs.rs`
  Purpose: thin entrypoint exporting actions and dispatch for the new product-local `logs` service.
- Create: `crates/lab/src/dispatch/logs/catalog.rs`
  Purpose: single source of truth for `logs` action metadata and parameter documentation.
- Create: `crates/lab/src/dispatch/logs/client.rs`
  Purpose: resolve the shared log subsystem handle for CLI/MCP fallback paths and surface a consistent "not wired" error.
- Create: `crates/lab/src/dispatch/logs/params.rs`
  Purpose: query coercion, filter parsing, retention config parsing, and stream subscription param helpers.
- Create: `crates/lab/src/dispatch/logs/dispatch.rs`
  Purpose: built-in `help` / `schema` actions and shared action routing.
- Create: `crates/lab/src/dispatch/logs/types.rs`
  Purpose: normalized `LogEvent`, query, stream subscription, stats, retention, and future-ingest extension fields.
- Create: `crates/lab/src/dispatch/logs/store.rs`
  Purpose: indexed persistence, historical query execution, retention enforcement, and store stats.
- Create: `crates/lab/src/dispatch/logs/stream.rs`
  Purpose: in-process live subscriber fanout and stream backpressure policy.
- Create: `crates/lab/src/dispatch/logs/ingest.rs`
  Purpose: normalization and redaction from tracing-aware runtime events into `LogEvent`.
- Create: `crates/lab/src/api/services/logs.rs`
  Purpose: `/v1/logs` action route plus dedicated SSE endpoint(s) for live push.
- Create: `crates/lab/src/mcp/services/logs.rs`
  Purpose: thin MCP wrapper over `dispatch::logs` for bounded search/tail actions only.
- Create: `crates/lab/tests/logs_dispatch.rs`
  Purpose: end-to-end dispatch/search action tests.
- Create: `crates/lab/tests/logs_api.rs`
  Purpose: API action and SSE route tests.
- Create: `crates/lab/tests/logs_cli.rs`
  Purpose: CLI local log search/stream contract tests.

### Modified Rust files

- Modify: `crates/lab/src/dispatch.rs`
  Purpose: register the new `logs` dispatch module.
- Modify: `crates/lab/src/registry.rs`
  Purpose: register the `logs` service in the shared runtime catalog and MCP/API registry.
- Modify: `crates/lab/src/api/state.rs`
  Purpose: attach the shared log subsystem handle to `AppState`.
- Modify: `crates/lab/src/api/router.rs`
  Purpose: mount `/v1/logs` routes and any dedicated SSE endpoint.
- Modify: `crates/lab/src/api.rs`
  Purpose: expose the new API service module.
- Modify: `crates/lab/src/api/services.rs`
  Purpose: register the `logs` API service module in the parent service list.
- Modify: `crates/lab/src/cli.rs`
  Purpose: keep `lab logs` wired while expanding the local-master subcommands.
- Modify: `crates/lab/src/cli/logs.rs`
  Purpose: preserve fleet log search while adding `local` search/stream/stats shims.
- Modify: `crates/lab/src/main.rs`
  Purpose: bootstrap the `LogSystem` runtime once and initialize tracing with the new log-ingest layer attached.
- Modify: `crates/lab/src/mcp/services.rs`
  Purpose: register the `logs` MCP service module in the parent service list.
- Modify: `crates/lab/src/catalog.rs`
  Purpose: no logic changes expected, but verify `logs` appears correctly via `registry`.
- Modify: `crates/lab/src/api/openapi.rs`
  Purpose: document the new local log endpoints if the HTTP API is covered there.
- Modify: `crates/lab/src/cli/serve.rs`
  Purpose: wire the subsystem into the HTTP server startup path if the handle is created there.

### New gateway-admin files

- Create: `apps/gateway-admin/app/(admin)/logs/page.tsx`
  Purpose: first-class log console page for the current master process.
- Create: `apps/gateway-admin/components/logs/log-console.tsx`
  Purpose: main log console composition.
- Create: `apps/gateway-admin/components/logs/log-timeline.tsx`
  Purpose: unified live + historical timeline rendering.
- Create: `apps/gateway-admin/components/logs/log-filters.tsx`
  Purpose: subsystem, level, time-range, and identifier filters.
- Create: `apps/gateway-admin/components/logs/log-stream-status.tsx`
  Purpose: live stream state, pause/resume, jump-to-newest controls.
- Create: `apps/gateway-admin/lib/api/logs-client.ts`
  Purpose: typed historical query calls from the browser.
- Create: `apps/gateway-admin/lib/api/logs-stream.ts`
  Purpose: base-URL-aware EventSource/SSE wrapper for live updates under hosted same-origin auth.
- Create: `apps/gateway-admin/lib/types/logs.ts`
  Purpose: shared frontend types for events, filters, stats, and stream state.
- Create: `apps/gateway-admin/lib/api/logs-client.test.ts`
  Purpose: browser/client fetch contract tests.

### Modified gateway-admin files

- Modify: `apps/gateway-admin/components/app-sidebar.tsx`
  Purpose: add navigation to the new log console page.
- Modify: `apps/gateway-admin/app/(admin)/activity/page.tsx`
  Purpose: optionally link operators from summary activity to the full log console rather than treating activity as the endpoint.

### Documentation files

- Modify: `docs/OBSERVABILITY.md`
  Purpose: document where the new local store ingestion boundary fits and reiterate that redaction happens before persistence and live fanout.
- Modify: `docs/CLI.md`
  Purpose: document the expanded `lab logs` command group.
- Modify: `docs/CONFIG.md`
  Purpose: document retention, store location, and any local log configuration knobs.
- Modify: `docs/README.md`
  Purpose: link the new logging docs if needed.
- Create: `docs/LOCAL_LOGS.md`
  Purpose: operator-facing documentation for local-master logging, search, streaming, and future fleet/syslog extension seams.

## Implementation Decisions Locked In

- Service name is `logs` for MCP/API/catalog consistency.
- `logs` is an always-on product-local service registered manually like `gateway`, not a `lab-apis` feature-gated service.
- CLI keeps existing fleet search compatibility and adds a nested local-master surface under `lab logs local ...`.
- API uses `/v1/logs` for action-style requests plus a dedicated SSE stream route under the same service namespace.
- True live streaming is HTTP SSE only; MCP gets bounded `logs.search`, `logs.tail` or `logs.poll`, and `logs.stats`.
- WebUI is a first-class `/logs` page in gateway-admin, not an extension of `/activity`.
- The runtime is owned by one explicit `LogSystem` bootstrap path, not by adapter-local handles.
- Browser SSE in v1 assumes hosted same-origin session auth. Standalone bearer mode does not get implicit live streaming support.
- Future fleet/syslog fields stay visible in code and docs with explicit comments explaining why they exist.

## Task 1: Scaffold The Shared `logs` Dispatch Service

**Files:**
- Create: `crates/lab/src/dispatch/logs.rs`
- Create: `crates/lab/src/dispatch/logs/catalog.rs`
- Create: `crates/lab/src/dispatch/logs/client.rs`
- Create: `crates/lab/src/dispatch/logs/params.rs`
- Create: `crates/lab/src/dispatch/logs/dispatch.rs`
- Modify: `crates/lab/src/dispatch.rs`
- Modify: `crates/lab/src/registry.rs`
- Modify: `crates/lab/src/mcp/services.rs`
- Modify: `crates/lab/src/api/services.rs`
- Test: `crates/lab/tests/logs_dispatch.rs`

- [ ] **Step 1: Write the failing registry and dispatch smoke tests**

```rust
#[test]
fn default_registry_includes_logs_service() {
    let registry = crate::registry::build_default_registry();
    let service = registry.service("logs").expect("logs service registered");
    assert_eq!(service.status, "available");
}

#[tokio::test]
async fn logs_dispatch_help_and_schema_exist() {
    let help = crate::dispatch::logs::dispatch("help", serde_json::json!({})).await.unwrap();
    let schema = crate::dispatch::logs::dispatch("schema", serde_json::json!({"action":"logs.search"})).await.unwrap();
    assert!(help.is_object());
    assert!(schema.is_object());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab logs_dispatch -- --nocapture`
Expected: FAIL because `dispatch::logs` and the registry entry do not exist yet.

- [ ] **Step 3: Add the dispatch service skeleton**

```rust
// crates/lab/src/dispatch/logs.rs
pub mod catalog;
pub mod client;
pub mod dispatch;
pub mod ingest;
pub mod params;
pub mod store;
pub mod stream;
pub mod types;

pub use catalog::ACTIONS;
pub use dispatch::dispatch;
```

- [ ] **Step 4: Implement the minimal `help` / `schema` dispatch path and register `logs`**

```rust
pub async fn dispatch(action: &str, params: Value) -> Result<Value, ToolError> {
    match action {
        "help" => Ok(help_payload("logs", ACTIONS)),
        "schema" => {
            let requested = require_str(&params, "action")?;
            action_schema(ACTIONS, requested)
        }
        _ => Err(ToolError::UnknownAction {
            service: "logs".to_string(),
            action: action.to_string(),
            valid: ACTIONS.iter().map(|a| a.name.to_string()).collect(),
        }),
    }
}
```

- [ ] **Step 4a: Register `logs` as an always-on product-local service**

Do not route this through the normal `lab_apis::<service>::META` helper path. Add an explicit manual `RegisteredService` entry in `registry.rs` and parent-module registrations in `mcp/services.rs` and `api/services.rs`, matching the always-on pattern used for `gateway` and `extract`.

- [ ] **Step 5: Re-run the tests to verify they pass**

Run: `cargo test -p lab logs_dispatch -- --nocapture`
Expected: PASS for registry presence and built-in action discovery.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch.rs crates/lab/src/dispatch/logs.rs crates/lab/src/dispatch/logs crates/lab/src/registry.rs crates/lab/src/mcp/services.rs crates/lab/src/api/services.rs crates/lab/tests/logs_dispatch.rs
git commit -m "feat: scaffold local logs dispatch service"
```

## Task 2: Define The Runtime Ownership And Bootstrap Path

**Files:**
- Modify: `crates/lab/src/main.rs`
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/dispatch/logs/client.rs`
- Test: `crates/lab/tests/logs_dispatch.rs`

- [ ] **Step 1: Write failing tests for runtime ownership expectations**

```rust
#[test]
fn log_system_bootstrap_is_single_owner() {
    let runtime = bootstrap_log_system_for_test();
    assert!(runtime.is_ok());
}

#[tokio::test]
async fn local_live_commands_fail_cleanly_without_long_lived_runtime() {
    let error = crate::dispatch::logs::dispatch("logs.tail", serde_json::json!({"limit": 10})).await.unwrap_err();
    assert_eq!(error.kind(), "internal_error");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab log_system_bootstrap_is_single_owner local_live_commands_fail_cleanly_without_long_lived_runtime -- --nocapture`
Expected: FAIL because the runtime owner/bootstrap path is not defined yet.

- [ ] **Step 3: Implement one explicit `LogSystem` bootstrap path**

Document and code one owner:

- `main.rs` bootstraps the long-lived `Arc<LogSystem>`
- `AppState` carries that handle for HTTP/MCP/WebUI
- `dispatch/logs/client.rs` exposes the on-disk store/bootstrap helper used by one-shot CLI search/stats paths
- unsupported live capabilities in one-shot contexts return a clear structured error

- [ ] **Step 4: Re-run the tests to verify they pass**

Run: `cargo test -p lab log_system_bootstrap_is_single_owner local_live_commands_fail_cleanly_without_long_lived_runtime -- --nocapture`
Expected: PASS, with ownership and failure semantics defined up front.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/main.rs crates/lab/src/api/state.rs crates/lab/src/dispatch/logs/client.rs crates/lab/tests/logs_dispatch.rs
git commit -m "feat: define local logs runtime ownership"
```

## Task 3: Define The Normalized Event Model And Future-Ingest Seams

**Files:**
- Create: `crates/lab/src/dispatch/logs/types.rs`
- Modify: `crates/lab/src/dispatch/logs/catalog.rs`
- Test: `crates/lab/tests/logs_dispatch.rs`

- [ ] **Step 1: Write failing serialization and taxonomy tests**

```rust
#[test]
fn log_event_serialization_preserves_future_ingest_fields() {
    let event = LogEvent::fixture();
    let json = serde_json::to_value(&event).unwrap();
    assert!(json.get("source_kind").is_some());
    assert!(json.get("source_device_id").is_some());
    assert!(json.get("ingest_path").is_some());
}

#[test]
fn subsystem_enum_includes_local_master_taxonomy() {
    assert_eq!(Subsystem::Gateway.as_str(), "gateway");
    assert_eq!(Subsystem::OauthRelay.as_str(), "oauth_relay");
    assert_eq!(Subsystem::AuthUpstream.as_str(), "auth_upstream");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab log_event_serialization_preserves_future_ingest_fields subsystem_enum_includes_local_master_taxonomy -- --nocapture`
Expected: FAIL because the types do not exist yet.

- [ ] **Step 3: Implement `LogEvent`, query, retention, and stats types with explicit comments**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    pub event_id: String,
    pub ts: DateTime<Utc>,
    pub level: LogLevel,
    pub subsystem: Subsystem,
    pub surface: Surface,
    pub action: Option<String>,
    pub message: String,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
    pub correlation_id: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub instance: Option<String>,
    pub auth_flow: Option<String>,
    pub outcome_kind: Option<String>,
    pub fields_json: serde_json::Value,
    // Reserved for future remote device and syslog ingestion.
    pub source_kind: Option<String>,
    pub source_node_id: Option<String>,
    pub source_device_id: Option<String>,
    pub ingest_path: Option<String>,
    pub upstream_event_id: Option<String>,
}
```

- [ ] **Step 4: Add action specs for `logs.search`, `logs.tail`, and `logs.stats`**

```rust
pub const ACTIONS: &[ActionSpec] = &[
    action!("logs.search", "Search local-master persisted logs"),
    action!("logs.tail", "Read a bounded follow-up window of local-master log events"),
    action!("logs.stats", "Inspect local log store retention and health"),
];
```

- [ ] **Step 5: Re-run the tests to verify they pass**

Run: `cargo test -p lab log_event_serialization_preserves_future_ingest_fields subsystem_enum_includes_local_master_taxonomy -- --nocapture`
Expected: PASS, with explicit future-ingest fields visible in serialized output.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/logs/types.rs crates/lab/src/dispatch/logs/catalog.rs crates/lab/tests/logs_dispatch.rs
git commit -m "feat: define local logs event model"
```

## Task 4: Build The Indexed Store And Retention Engine

**Files:**
- Create: `crates/lab/src/dispatch/logs/store.rs`
- Modify: `crates/lab/src/dispatch/logs/client.rs`
- Modify: `crates/lab/src/dispatch/logs/params.rs`
- Test: `crates/lab/tests/logs_dispatch.rs`
- Modify: `docs/CONFIG.md`

- [ ] **Step 1: Write failing store tests for insert, search, and combined retention**

```rust
#[tokio::test]
async fn store_search_filters_by_subsystem_and_level() {
    let store = test_store().await;
    seed(store.clone()).await;
    let result = store.search(LogQuery::builder().subsystems(["gateway"]).levels([LogLevel::Warn]).build()).await.unwrap();
    assert_eq!(result.events.len(), 1);
}

#[tokio::test]
async fn retention_enforces_age_and_size_limits() {
    let store = test_store_with_limits(days(7), bytes(1024)).await;
    seed_large_and_old(store.clone()).await;
    store.run_maintenance().await.unwrap();
    let stats = store.stats().await.unwrap();
    assert!(stats.on_disk_bytes <= 1024);
    assert!(stats.oldest_retained_ts >= Utc::now() - chrono::Duration::days(7));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab store_search_filters_by_subsystem_and_level retention_enforces_age_and_size_limits -- --nocapture`
Expected: FAIL because the persistent store and maintenance behavior are not implemented.

- [ ] **Step 3: Implement the embedded indexed store and retention stats**

```rust
pub struct LogStore {
    // embedded database handle
}

impl LogStore {
    pub async fn insert(&self, event: &LogEvent) -> Result<(), ToolError> { /* ... */ }
    pub async fn search(&self, query: LogQuery) -> Result<LogSearchResult, ToolError> { /* ... */ }
    pub async fn stats(&self) -> Result<LogStoreStats, ToolError> { /* ... */ }
    pub async fn run_maintenance(&self) -> Result<(), ToolError> { /* ... */ }
}
```

- [ ] **Step 4: Wire config-backed retention and store path resolution**

Run the implementation through one resolver path, not scattered env reads. Document the knobs in `docs/CONFIG.md`.

- [ ] **Step 5: Re-run the tests to verify they pass**

Run: `cargo test -p lab store_search_filters_by_subsystem_and_level retention_enforces_age_and_size_limits -- --nocapture`
Expected: PASS, with deterministic coverage for age and size pressure.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/logs/store.rs crates/lab/src/dispatch/logs/client.rs crates/lab/src/dispatch/logs/params.rs crates/lab/tests/logs_dispatch.rs docs/CONFIG.md
git commit -m "feat: add local logs indexed store"
```

## Task 5: Add Redacting Ingest, Queueing, And Live Fanout

**Files:**
- Create: `crates/lab/src/dispatch/logs/ingest.rs`
- Create: `crates/lab/src/dispatch/logs/stream.rs`
- Modify: `crates/lab/src/main.rs`
- Modify: `docs/OBSERVABILITY.md`
- Test: `crates/lab/tests/logs_dispatch.rs`

- [ ] **Step 1: Write failing tests for redaction and subscriber fanout**

```rust
#[tokio::test]
async fn ingest_redacts_sensitive_fields_before_store_and_stream() {
    let system = test_log_system().await;
    system.ingest(raw_event_with_bearer_token()).await.unwrap();
    let stored = system.search(LogQuery::default()).await.unwrap();
    assert!(!stored.events[0].message.contains("Bearer "));
}

#[tokio::test]
async fn stream_subscribers_receive_new_events_without_querying_store() {
    let system = test_log_system().await;
    let mut sub = system.subscribe(StreamSubscription::default()).await.unwrap();
    system.ingest(raw_gateway_event()).await.unwrap();
    let next = sub.recv().await.unwrap();
    assert_eq!(next.subsystem, Subsystem::Gateway);
}

#[tokio::test]
async fn full_ingest_queue_records_overflow_without_blocking_caller() {
    let system = test_log_system_with_small_queue().await;
    for _ in 0..100 {
        let _ = system.try_ingest(raw_gateway_event());
    }
    let stats = system.stats().await.unwrap();
    assert!(stats.dropped_event_count > 0);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab ingest_redacts_sensitive_fields_before_store_and_stream stream_subscribers_receive_new_events_without_querying_store -- --nocapture`
Expected: FAIL because ingestion and live fanout are not wired.

- [ ] **Step 3: Implement the redacting ingest pipeline, bounded queue, and subscriber hub**

```rust
pub fn try_ingest(&self, raw: RawLogEvent) -> Result<(), ToolError> {
    self.queue.try_send(raw).map_err(map_queue_error)
}

async fn worker_loop(&self) -> Result<(), ToolError> {
    while let Some(raw) = self.queue.recv().await {
        let event = normalize_and_redact(raw)?;
        self.store.insert(&event).await?;
        self.stream.publish(event).await;
    }
    Ok(())
}
```

- [ ] **Step 4: Define and document failure-isolation behavior**

Lock these decisions in code comments and docs:

- bounded ingest queue with overflow counters
- slow-subscriber drop or resync policy
- persistence failures surface via health/stats without recursively flooding logs

- [ ] **Step 5: Attach the logging layer to tracing initialization**

Keep `main.rs` as the only tracing setup owner. Add the new layer there so runtime components feed one ingestion boundary.

- [ ] **Step 6: Re-run the tests to verify they pass**

Run: `cargo test -p lab ingest_redacts_sensitive_fields_before_store_and_stream stream_subscribers_receive_new_events_without_querying_store full_ingest_queue_records_overflow_without_blocking_caller -- --nocapture`
Expected: PASS, proving redaction occurs before persistence and fanout and that queue pressure is isolated.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/logs/ingest.rs crates/lab/src/dispatch/logs/stream.rs crates/lab/src/main.rs crates/lab/tests/logs_dispatch.rs docs/OBSERVABILITY.md
git commit -m "feat: add local logs ingest and fanout pipeline"
```

## Task 6: Expose Shared Search, Tail, And Stats Actions In Dispatch And MCP

**Files:**
- Modify: `crates/lab/src/dispatch/logs/dispatch.rs`
- Create: `crates/lab/src/mcp/services/logs.rs`
- Modify: `crates/lab/src/registry.rs`
- Test: `crates/lab/tests/logs_dispatch.rs`

- [ ] **Step 1: Write failing dispatch tests for `logs.search`, `logs.tail`, and `logs.stats`**

```rust
#[tokio::test]
async fn logs_search_returns_filtered_results() {
    let value = crate::dispatch::logs::dispatch("logs.search", serde_json::json!({
        "query": { "subsystems": ["gateway"], "levels": ["warn"] }
    })).await.unwrap();
    assert!(value.get("events").is_some());
}

#[tokio::test]
async fn logs_stats_returns_retention_metadata() {
    let value = crate::dispatch::logs::dispatch("logs.stats", serde_json::json!({})).await.unwrap();
    assert!(value.get("on_disk_bytes").is_some());
}

#[tokio::test]
async fn logs_tail_returns_bounded_follow_up_window() {
    let value = crate::dispatch::logs::dispatch("logs.tail", serde_json::json!({
        "after_ts": "2026-04-16T00:00:00Z",
        "limit": 50
    })).await.unwrap();
    assert!(value.get("events").is_some());
    assert!(value.get("next_cursor").is_some());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab logs_search_returns_filtered_results logs_stats_returns_retention_metadata -- --nocapture`
Expected: FAIL because service-specific action routing is not implemented.

- [ ] **Step 3: Implement shared dispatch handlers and MCP wrapper**

```rust
match action {
    "logs.search" => Ok(serde_json::to_value(system.search(parse_query(&params)?).await?)?),
    "logs.tail" => Ok(serde_json::to_value(system.tail(parse_tail_request(&params)?).await?)?),
    "logs.stats" => Ok(serde_json::to_value(system.stats().await?)?),
    _ => unknown_action(...),
}
```

- [ ] **Step 4: Re-run the tests to verify they pass**

Run: `cargo test -p lab logs_search_returns_filtered_results logs_stats_returns_retention_metadata -- --nocapture`
Expected: PASS, and `logs` appears in the MCP/registry catalog.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/logs/dispatch.rs crates/lab/src/mcp/services/logs.rs crates/lab/src/registry.rs crates/lab/tests/logs_dispatch.rs
git commit -m "feat: expose local logs search and tail actions"
```

## Task 7: Add The HTTP API And SSE Transport

**Files:**
- Create: `crates/lab/src/api/services/logs.rs`
- Modify: `crates/lab/src/api.rs`
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/api/services.rs`
- Modify: `crates/lab/src/api/openapi.rs`
- Test: `crates/lab/tests/logs_api.rs`

- [ ] **Step 1: Write failing API tests for historical search and live SSE**

```rust
#[tokio::test]
async fn post_logs_search_route_exists() {
    let response = post_json("/v1/logs", json!({"action":"logs.search","params":{"query":{}}})).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn logs_stream_sse_route_emits_event_stream_content_type() {
    let response = get("/v1/logs/stream").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab logs_api -- --nocapture`
Expected: FAIL because the logs API routes are not mounted.

- [ ] **Step 3: Implement the HTTP handlers and wire the shared subsystem into `AppState`**

```rust
pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new()
        .route("/", post(handle_action_route))
        .route("/stream", get(handle_sse_stream))
}
```

- [ ] **Step 4: Re-run the tests to verify they pass**

Run: `cargo test -p lab logs_api -- --nocapture`
Expected: PASS, including SSE content type and action route behavior.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api.rs crates/lab/src/api/state.rs crates/lab/src/api/router.rs crates/lab/src/api/services.rs crates/lab/src/api/openapi.rs crates/lab/src/api/services/logs.rs crates/lab/tests/logs_api.rs
git commit -m "feat: add local logs HTTP API and SSE stream"
```

## Task 8: Expand The CLI Without Breaking Fleet Log Search

**Files:**
- Modify: `crates/lab/src/cli/logs.rs`
- Modify: `crates/lab/src/cli.rs`
- Test: `crates/lab/tests/logs_cli.rs`
- Modify: `docs/CLI.md`

- [ ] **Step 1: Write failing CLI parsing and shim tests**

```rust
#[test]
fn logs_cli_parses_local_search() {
    let cli = Cli::parse_from(["lab", "logs", "local", "search", "--subsystem", "gateway"]);
    assert!(matches!(cli.command, Command::Logs(_)));
}

#[tokio::test]
async fn logs_local_search_uses_shared_dispatch_contract() {
    let value = crate::cli::logs::run_local_search_for_test(/* ... */).await.unwrap();
    assert!(value.get("events").is_some());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p lab logs_cli -- --nocapture`
Expected: FAIL because `local` subcommands do not exist yet.

- [ ] **Step 3: Implement nested `local` CLI commands while preserving existing fleet `search`**

```rust
pub enum LogsCommand {
    Search { device: String, query: String },
    Local(LocalLogsArgs),
}
```

- [ ] **Step 4: Re-run the tests to verify they pass**

Run: `cargo test -p lab logs_cli -- --nocapture`
Expected: PASS, with the old fleet search path still accepted.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/cli.rs crates/lab/src/cli/logs.rs crates/lab/tests/logs_cli.rs docs/CLI.md
git commit -m "feat: add local logs CLI commands"
```

## Task 9: Build The Gateway-Admin Log Console

**Files:**
- Create: `apps/gateway-admin/app/(admin)/logs/page.tsx`
- Create: `apps/gateway-admin/components/logs/log-console.tsx`
- Create: `apps/gateway-admin/components/logs/log-timeline.tsx`
- Create: `apps/gateway-admin/components/logs/log-filters.tsx`
- Create: `apps/gateway-admin/components/logs/log-stream-status.tsx`
- Create: `apps/gateway-admin/lib/api/logs-client.ts`
- Create: `apps/gateway-admin/lib/api/logs-stream.ts`
- Create: `apps/gateway-admin/lib/types/logs.ts`
- Modify: `apps/gateway-admin/components/app-sidebar.tsx`
- Modify: `apps/gateway-admin/app/(admin)/activity/page.tsx`
- Test: `apps/gateway-admin/lib/api/logs-client.test.ts`

- [ ] **Step 1: Write failing frontend API and route tests**

```ts
test('fetchLogs posts logs.search to the backend', async () => {
  const result = await fetchLogs({ subsystems: ['gateway'] })
  assert.equal(result.events.length, 1)
})
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd apps/gateway-admin && pnpm test`
Expected: FAIL because the log client and route do not exist.

- [ ] **Step 3: Implement the log console page and base-URL-aware SSE client hooks**

```ts
export function connectLogStream(onEvent: (event: LogEvent) => void) {
  const url = `${normalizeGatewayApiBase()}/logs/stream`
  const source = new EventSource(url, { withCredentials: true })
  source.onmessage = (message) => onEvent(JSON.parse(message.data) as LogEvent)
  return () => source.close()
}
```

Hosted same-origin session auth is the only required SSE auth mode in v1. If standalone bearer mode needs live streaming later, plan that separately instead of trying to force bearer headers through `EventSource`.

- [ ] **Step 4: Re-run the tests to verify they pass**

Run: `cd apps/gateway-admin && pnpm test`
Expected: PASS, with typed search requests and EventSource client coverage.

- [ ] **Step 5: Smoke-test the page manually**

Run: `cd apps/gateway-admin && pnpm dev`
Expected: hosted same-origin `/logs` renders, filters update the query, and live events append without full-page refresh.

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/app/(admin)/logs/page.tsx apps/gateway-admin/components/logs apps/gateway-admin/lib/api/logs-client.ts apps/gateway-admin/lib/api/logs-stream.ts apps/gateway-admin/lib/types/logs.ts apps/gateway-admin/lib/api/logs-client.test.ts apps/gateway-admin/components/app-sidebar.tsx apps/gateway-admin/app/(admin)/activity/page.tsx
git commit -m "feat: add gateway admin log console"
```

## Task 10: Publish Docs For Operators And Future Extension Points

**Files:**
- Create: `docs/LOCAL_LOGS.md`
- Modify: `docs/README.md`
- Modify: `docs/OBSERVABILITY.md`
- Modify: `docs/CONFIG.md`
- Modify: `docs/CLI.md`

- [ ] **Step 1: Write failing doc checklist assertions in the PR description or task tracker**

Checklist:
- local-master scope stated clearly
- retention knobs documented
- CLI/API/WebUI surfaces documented
- future fleet/syslog extension-point rationale documented

- [ ] **Step 2: Write the operator-facing docs and update the index pages**

Include explicit code-comment guidance for the reserved future-ingest fields and explain why they must remain in place even before fleet ingest ships.

- [ ] **Step 3: Verify the docs cover the new contract**

Run: `rtk rg -n "future remote device and syslog ingestion|lab logs local|/v1/logs|SSE|retention" docs`
Expected: matches in the new and updated docs.

- [ ] **Step 4: Commit**

```bash
git add docs/LOCAL_LOGS.md docs/README.md docs/OBSERVABILITY.md docs/CONFIG.md docs/CLI.md
git commit -m "docs: add local logs operator guide"
```

## Task 11: Run Full Verification And Clean Up Gaps

**Files:**
- Modify: any files above only as needed to fix failing verification

- [ ] **Step 1: Run targeted Rust tests**

Run: `cargo test -p lab logs_dispatch logs_api logs_cli -- --nocapture`
Expected: PASS

- [ ] **Step 2: Run restart persistence and fleet-regression tests**

Run: `cargo test -p lab local_logs_persist_across_restart existing_fleet_logs_search_still_works -- --nocapture`
Expected: PASS

- [ ] **Step 3: Run SSE reconnect and MCP parity tests**

Run: `cargo test -p lab logs_sse_reconnect_resumes_stream logs_mcp_tail_matches_api_query_semantics -- --nocapture`
Expected: PASS

- [ ] **Step 4: Run gateway-admin tests**

Run: `cd apps/gateway-admin && pnpm test`
Expected: PASS

- [ ] **Step 5: Run format and lint**

Run: `cargo fmt --all --check`
Expected: PASS

Run: `cargo clippy --workspace --all-features -- -D warnings`
Expected: PASS

- [ ] **Step 6: Run the all-features workspace tests**

Run: `cargo test --workspace --all-features --tests --no-fail-fast`
Expected: PASS

- [ ] **Step 7: Manual smoke test**

Run: `cargo run --all-features -- serve --transport http`
Expected: the process starts, `/v1/logs` responds, `/v1/logs/stream` emits `text/event-stream`, existing `/v1/device/logs/search` still works, and the gateway-admin `/logs` page can search and receive live updates.

- [ ] **Step 8: Final commit**

```bash
git add crates/lab/src apps/gateway-admin docs
git commit -m "feat: ship local master log foundation"
```

## Notes For The Implementer

- Do not hide or collapse the reserved future-ingest fields during implementation. Keep the comments near those fields explicit.
- Do not move search or retention logic into CLI, API, MCP, or React code. The shared execution semantics belong in `dispatch::logs`.
- Keep `lab logs search <device> <query>` working. The new local-master workflow must be additive.
- Keep MCP bounded. Do not try to return a long-lived stream handle through the JSON action contract.
- Prefer SSE for browser live delivery in v1. Do not introduce WebSockets unless a concrete blocker appears.
- Use hosted same-origin auth for SSE in v1. Do not assume `EventSource` can reuse standalone bearer-header behavior.
- Redaction must happen before persistence and before live fanout. A UI-only mask is a security bug.
