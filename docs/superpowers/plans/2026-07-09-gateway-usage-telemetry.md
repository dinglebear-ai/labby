# Gateway Usage Telemetry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist a bounded SQLite record of every tool/resource/prompt call proxied through the gateway's `UpstreamPool`, and expose `gateway.usage.metrics` / `gateway.usage.calls` actions (CLI + MCP + HTTP, for free, via the existing action-dispatch pattern) so the usage dashboard has a real backend again.

**Architecture:** A new `UsageStore` (SQLite, connection-pooled, mirrors `labby-auth`'s `SqliteStore`) lives in a new top-level `crates/labby-gateway/src/usage/` module — a sibling of `gateway`/`upstream`, not nested under either, so both can depend on it without a layering cycle. `UpstreamPool` gets an optional `usage_store` field; the single choke point all tool/resource/prompt calls already funnel through (`timed_capability_call` in `pool/capability_call.rs`) fire-and-forget writes a call record after every outcome. `GatewayManager` owns the canonical `Arc<UsageStore>` and threads it into every pool it builds; the `labby` binary opens the store once at startup (mirroring how it already opens the OAuth SQLite store) and passes it to both the long-lived `UpstreamPool` and `GatewayManagerConfig`. Query/aggregation lives behind two new `gateway.usage.*` actions, following the exact `gateway.enrich.preview`/`gateway.enrich.apply` precedent (catalog entry → `dispatch.rs` route → `GatewayManager` method → CLI subcommand). This plan is backend-only — it does not touch `apps/gateway-admin`; the frontend's dead `/v1/logs/*` calls stay broken until a follow-up plan rewires them to these new actions.

**Tech Stack:** Rust 2024, Tokio, `rusqlite` (already a workspace dependency, used today by `labby-auth`), `serde`/`serde_json`, existing `labby-gateway`/`labby` crate split.

## Global Constraints

- Scope is calls proxied through the gateway to upstreams only (tools, resources, prompts) — not CLI/HTTP/MCP dispatch-level events for the `gateway` service itself (e.g. `gateway.add`). That distinction was locked in conversation with the repo owner before this plan was written.
- The call-record schema must not hardcode "upstream" as the only source: use a `capability`/`operation` shape (already what `UpstreamRequestLog` carries) rather than baking in an "external upstream only" assumption, because Labby's own tools may become callable through the same `host.call_tool` plane later. Do not build that path now — just don't paint the schema into a corner.
- Recording a call must never add latency or failure risk to the tool-call path it's observing. Every write is fire-and-forget (`tokio::spawn`, log-and-drop on error).
- Per `crates/labby-gateway/src/upstream/CLAUDE.md`: do not read env vars outside `pool/helpers.rs` and the connect modules. `usage_db_path()` / the disable-telemetry env var are resolved in `crates/labby/src/config.rs` (the binary), never inside `labby-gateway`.
- Per the same doc: keep new `pool/` files under ~500 LOC.
- `ToolError::Sdk { sdk_kind, message }` is the error shape for new storage-layer errors (mirrors `crates/labby-gateway/src/gateway/manager/enrichment.rs`'s `sdk_kind: "invalid_hint"` precedent).
- New catalog actions follow the existing `gateway.enrich.*` shape exactly: `ActionSpec` in `crates/labby-gateway/src/gateway/catalog.rs`, routed in `crates/labby-gateway/src/gateway/dispatch.rs`, params in `crates/labby-gateway/src/gateway/params.rs`, response views in `crates/labby-gateway/src/gateway/types.rs`.
- Do not touch `apps/gateway-admin/**` in this plan.

---

### Task 1: `UsageStore` — SQLite-backed call-record store

**Files:**
- Create: `crates/labby-gateway/src/usage.rs`
- Create: `crates/labby-gateway/src/usage/types.rs`
- Create: `crates/labby-gateway/src/usage/store.rs`
- Modify: `crates/labby-gateway/src/lib.rs`
- Modify: `crates/labby-gateway/Cargo.toml`

**Interfaces:**
- Produces: `UpstreamCallRecord { ts_unix: i64, upstream_name: String, tool_name: String, capability: String, operation: String, subject_scoped: bool, actor: Option<String>, outcome: String, error_kind: Option<String>, elapsed_ms: i64, response_bytes: Option<i64> }`
- Produces: `UsageStore::open(path: PathBuf) -> Result<Self, ToolError>` (async)
- Produces: `UsageStore::record_call(&self, record: UpstreamCallRecord) -> Result<(), ToolError>` (async)
- Produces: `UsageStore::prune_older_than(&self, cutoff_unix: i64) -> Result<u64, ToolError>` (async)

- [ ] **Step 1: Add the `rusqlite` dependency**

In `crates/labby-gateway/Cargo.toml`, in the `[dependencies]` block, add (alphabetically near `regex`):

```toml
rusqlite = { workspace = true }
```

- [ ] **Step 2: Write the record type**

Create `crates/labby-gateway/src/usage/types.rs`:

```rust
//! Types shared between the usage-telemetry writer (`UpstreamPool`) and the
//! query/aggregation side (`gateway.usage.*` actions).

/// One recorded call proxied through the gateway's `UpstreamPool`.
///
/// `capability`/`operation` mirror `UpstreamRequestLog` (`upstream/pool/logging.rs`)
/// deliberately, so this schema is not hardcoded to "external upstream only" —
/// a future in-process source (Labby's own tools reachable from Code Mode)
/// can populate the same shape without a migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamCallRecord {
    /// Unix seconds when the call finished (success or failure).
    pub ts_unix: i64,
    pub upstream_name: String,
    pub tool_name: String,
    /// `"tools"` | `"resources"` | `"prompts"`.
    pub capability: String,
    /// `"tool.call"` | `"resource.read"` | `"prompt.get"`.
    pub operation: String,
    pub subject_scoped: bool,
    /// OAuth subject for subject-scoped calls; `None` for the non-OAuth pool
    /// path (bearer-auth callers are not yet individually attributed).
    pub actor: Option<String>,
    /// `"ok"` | `"upstream_error"` | `"timeout"` | `"response_too_large"` | `"upstream_connect_error"`.
    pub outcome: String,
    pub error_kind: Option<String>,
    pub elapsed_ms: i64,
    pub response_bytes: Option<i64>,
}
```

- [ ] **Step 3: Write a failing open+record+prune round-trip test**

Create `crates/labby-gateway/src/usage/store.rs` with just the test module first:

```rust
//! `UsageStore`: a small connection-pooled SQLite store for gateway call
//! telemetry. Mirrors `labby-auth`'s `SqliteStore` (`crates/labby-auth/src/sqlite.rs`)
//! but carries no secrets, so there is no at-rest encryption or restrictive
//! file-permission enforcement here.

#[cfg(test)]
mod tests {
    use super::UsageStore;
    use crate::usage::types::UpstreamCallRecord;

    fn sample_record(ts_unix: i64) -> UpstreamCallRecord {
        UpstreamCallRecord {
            ts_unix,
            upstream_name: "github".to_string(),
            tool_name: "search_repos".to_string(),
            capability: "tools".to_string(),
            operation: "tool.call".to_string(),
            subject_scoped: false,
            actor: None,
            outcome: "ok".to_string(),
            error_kind: None,
            elapsed_ms: 42,
            response_bytes: Some(128),
        }
    }

    #[tokio::test]
    async fn record_call_persists_and_is_queryable_by_count() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        store.record_call(sample_record(1_000)).await.unwrap();
        store.record_call(sample_record(1_001)).await.unwrap();

        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM upstream_calls", [], |row| row.get(0))
                    .map_err(super::sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn prune_older_than_deletes_only_stale_rows() {
        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        store.record_call(sample_record(100)).await.unwrap();
        store.record_call(sample_record(200)).await.unwrap();

        let deleted = store.prune_older_than(150).await.unwrap();
        assert_eq!(deleted, 1);

        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM upstream_calls", [], |row| row.get(0))
                    .map_err(super::sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p labby-gateway --all-features usage::store::tests -- --nocapture`

Expected: FAIL to compile — `UsageStore` does not exist yet.

- [ ] **Step 3: Implement `UsageStore`**

Add above the `#[cfg(test)]` block in `crates/labby-gateway/src/usage/store.rs`:

```rust
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};

use labby_runtime::error::ToolError;

use super::types::UpstreamCallRecord;

const SQLITE_BUSY_TIMEOUT_MS: u64 = 5_000;
const SQLITE_POOL_SIZE: usize = 4;
const SCHEMA_VERSION: i64 = 1;

#[derive(Clone)]
pub struct UsageStore {
    conns: Arc<Vec<Mutex<Connection>>>,
    next_conn: Arc<AtomicUsize>,
    path: Arc<PathBuf>,
}

impl std::fmt::Debug for UsageStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl UsageStore {
    pub async fn open(path: PathBuf) -> Result<Self, ToolError> {
        let path_for_open = path.clone();
        let conns = tokio::task::spawn_blocking(move || {
            open_connections(path_for_open.as_path(), SQLITE_POOL_SIZE)
        })
        .await
        .map_err(|error| storage_error(format!("sqlite open task failed: {error}")))??;
        Ok(Self {
            conns: Arc::new(conns.into_iter().map(Mutex::new).collect()),
            next_conn: Arc::new(AtomicUsize::new(0)),
            path: Arc::new(path),
        })
    }

    pub async fn record_call(&self, record: UpstreamCallRecord) -> Result<(), ToolError> {
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO upstream_calls (
                    ts_unix, upstream_name, tool_name, capability, operation,
                    subject_scoped, actor, outcome, error_kind, elapsed_ms, response_bytes
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    record.ts_unix,
                    record.upstream_name,
                    record.tool_name,
                    record.capability,
                    record.operation,
                    i64::from(record.subject_scoped),
                    record.actor,
                    record.outcome,
                    record.error_kind,
                    record.elapsed_ms,
                    record.response_bytes,
                ],
            )
            .map_err(sqlite_error)?;
            Ok(())
        })
        .await
    }

    /// Delete rows older than `cutoff_unix`. Returns the number of deleted rows.
    pub async fn prune_older_than(&self, cutoff_unix: i64) -> Result<u64, ToolError> {
        self.with_conn(move |conn| {
            let deleted = conn
                .execute(
                    "DELETE FROM upstream_calls WHERE ts_unix < ?1",
                    params![cutoff_unix],
                )
                .map_err(sqlite_error)?;
            Ok(deleted as u64)
        })
        .await
    }

    pub(crate) async fn with_conn<T, F>(&self, op: F) -> Result<T, ToolError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, ToolError> + Send + 'static,
    {
        let conns = Arc::clone(&self.conns);
        let len = conns.len();
        let idx = self.next_conn.fetch_add(1, Ordering::Relaxed) % len;
        tokio::task::spawn_blocking(move || {
            let guard = conns[idx]
                .lock()
                .map_err(|_| storage_error("sqlite mutex poisoned".to_string()))?;
            op(&guard)
        })
        .await
        .map_err(|error| storage_error(format!("sqlite task failed: {error}")))?
    }
}

fn open_connections(path: &Path, count: usize) -> Result<Vec<Connection>, ToolError> {
    (0..count).map(|_| open_connection(path)).collect()
}

fn open_connection(path: &Path) -> Result<Connection, ToolError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            storage_error(format!(
                "create usage database directory `{}`: {error}",
                parent.display()
            ))
        })?;
    }
    let conn = Connection::open(path).map_err(sqlite_error)?;
    conn.busy_timeout(std::time::Duration::from_millis(SQLITE_BUSY_TIMEOUT_MS))
        .map_err(sqlite_error)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(sqlite_error)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS upstream_calls (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            ts_unix INTEGER NOT NULL,
            upstream_name TEXT NOT NULL,
            tool_name TEXT NOT NULL,
            capability TEXT NOT NULL,
            operation TEXT NOT NULL,
            subject_scoped INTEGER NOT NULL,
            actor TEXT,
            outcome TEXT NOT NULL,
            error_kind TEXT,
            elapsed_ms INTEGER NOT NULL,
            response_bytes INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_upstream_calls_ts ON upstream_calls(ts_unix);
        CREATE INDEX IF NOT EXISTS idx_upstream_calls_upstream ON upstream_calls(upstream_name, ts_unix);",
    )
    .map_err(sqlite_error)?;
    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))
        .map_err(sqlite_error)?;
    Ok(conn)
}

pub(super) fn sqlite_error(error: rusqlite::Error) -> ToolError {
    storage_error(format!("sqlite error: {error}"))
}

fn storage_error(message: String) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "usage_store_error".to_string(),
        message,
    }
}
```

Add `tempfile` as a dev-dependency if not already present — check `crates/labby-gateway/Cargo.toml`'s `[dev-dependencies]`; `tempfile.workspace = true` is already listed under `[dependencies]` (used elsewhere in the crate), so the test can use it without changes.

- [ ] **Step 4: Declare the module**

Create `crates/labby-gateway/src/usage.rs`:

```rust
//! Gateway call-usage telemetry: a small SQLite-backed store recording every
//! tool/resource/prompt call proxied through the upstream pool, plus the
//! aggregation queries backing the `gateway.usage.*` actions.

pub mod store;
pub mod types;

pub use store::UsageStore;
pub use types::UpstreamCallRecord;
```

In `crates/labby-gateway/src/lib.rs`, add (alphabetically, after `registry`, before `security`):

```rust
pub mod usage;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p labby-gateway --all-features usage::store::tests -- --nocapture`

Expected: PASS (2 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/labby-gateway/Cargo.toml crates/labby-gateway/src/lib.rs crates/labby-gateway/src/usage.rs crates/labby-gateway/src/usage/types.rs crates/labby-gateway/src/usage/store.rs
git commit -m "feat(gateway): add UsageStore for call telemetry"
```

---

### Task 2: Wire `UsageStore` into `UpstreamPool`'s call path

**Files:**
- Modify: `crates/labby-gateway/src/upstream/pool.rs`
- Create: `crates/labby-gateway/src/upstream/pool/usage_record.rs`
- Modify: `crates/labby-gateway/src/upstream/pool/capability_call.rs`

**Interfaces:**
- Consumes: `UsageStore::record_call` (Task 1), `UpstreamRequestLog` fields (`crates/labby-gateway/src/upstream/pool/logging.rs`).
- Produces: `UpstreamPool::with_usage_store(self, store: Option<Arc<UsageStore>>) -> Self`.
- Produces: `pool/usage_record.rs::record_usage_call(pool, event, subject, outcome, error_kind, elapsed_ms, response_bytes)`.

- [ ] **Step 1: Write a failing test asserting a `call_tool` produces a usage row**

Add to the `#[cfg(test)] mod tests` block at the bottom of `crates/labby-gateway/src/upstream/pool/tools_call.rs` (alongside the existing `call_tool_times_out_slow_upstream_response` test):

```rust
    /// Usage telemetry: a successful `call_tool` through the pool writes one
    /// row to the wired `UsageStore`, with capability/tool/upstream/outcome set.
    #[tokio::test]
    async fn call_tool_records_usage_when_store_is_wired() {
        use crate::usage::UsageStore;

        struct EchoServer;
        impl ServerHandler for EchoServer {
            fn get_info(&self) -> ServerInfo {
                ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            }
            async fn list_tools(
                &self,
                _: Option<PaginatedRequestParams>,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<ListToolsResult, ErrorData> {
                Ok(ListToolsResult::with_all_items(vec![rmcp::model::Tool::new(
                    "echo",
                    "echo tool",
                    Arc::new(serde_json::Map::new()),
                )]))
            }
            async fn call_tool(
                &self,
                _: CallToolRequestParams,
                _: rmcp::service::RequestContext<RoleServer>,
            ) -> Result<CallToolResult, ErrorData> {
                Ok(CallToolResult::success(vec![]))
            }
        }

        let upstream_name = "usage-upstream";
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = EchoServer.serve(server_transport).await.expect("server starts");
            running.waiting().await.ok();
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> =
            ().serve(client_transport).await.expect("client starts");
        let peer = client_service.peer().clone();

        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(UsageStore::open(dir.path().join("usage.db")).await.unwrap());
        let pool = Arc::new(UpstreamPool::new().with_usage_store(Some(Arc::clone(&store))));
        let upstream_name_arc: Arc<str> = Arc::from(upstream_name);
        pool.catalog.write().await.insert(
            upstream_name.to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name_arc), HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            UpstreamConnection {
                _client_service: client_service,
                _server_task: Some(server_task),
                peer,
                runtime: UpstreamRuntimeMetadata::default(),
            },
        );

        pool.call_tool(upstream_name, CallToolRequestParams::new("echo"))
            .await
            .expect("upstream is connected")
            .expect("echo call succeeds");

        // The write is fire-and-forget (`tokio::spawn`); give it a beat to land.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let count: i64 = store
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM upstream_calls WHERE upstream_name = ?1 AND tool_name = ?2 AND outcome = 'ok'",
                    rusqlite::params!["usage-upstream", "echo"],
                    |row| row.get(0),
                )
                .map_err(crate::usage::store::sqlite_error)
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
```

`with_conn` is `pub(crate)`, so this test (inside the `labby-gateway` crate) can call it directly; `sqlite_error` needs `pub(super)` widened to `pub(crate)` for this cross-module test access — make that change now in `usage/store.rs`:

```rust
pub(crate) fn sqlite_error(error: rusqlite::Error) -> ToolError {
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p labby-gateway --all-features call_tool_records_usage_when_store_is_wired -- --nocapture`

Expected: FAIL to compile — `UpstreamPool::with_usage_store` does not exist.

- [ ] **Step 3: Add the field and builder to `UpstreamPool`**

In `crates/labby-gateway/src/upstream/pool.rs`, add a field to the `UpstreamPool` struct (after `shared_http_client`):

```rust
    /// Optional call-usage recorder. `None` (the default) disables telemetry
    /// capture entirely — most tests and any pool built without an explicit
    /// `.with_usage_store(...)` call never touch SQLite.
    pub(super) usage_store: Option<Arc<crate::usage::UsageStore>>,
```

In `UpstreamPool::new()`, add to the struct literal (after `shared_http_client`):

```rust
            usage_store: None,
```

Add a builder method near `with_relay_timeout`:

```rust
    /// Attach a call-usage recorder. `None` explicitly disables capture even
    /// if the caller previously wired one — used by tests that want a clean
    /// pool without reconstructing it.
    #[must_use]
    pub fn with_usage_store(mut self, store: Option<Arc<crate::usage::UsageStore>>) -> Self {
        self.usage_store = store;
        self
    }
```

- [ ] **Step 4: Add the record-write helper**

Create `crates/labby-gateway/src/upstream/pool/usage_record.rs`:

```rust
//! Fire-and-forget usage-record write, called from `capability_call.rs` after
//! every tool/resource/prompt call outcome. Never blocks or fails the call
//! path: if `pool.usage_store` is `None`, this is a no-op; if the write
//! itself fails, it is logged and dropped.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::usage::UpstreamCallRecord;

use super::UpstreamPool;
use super::logging::UpstreamRequestLog;

#[allow(clippy::too_many_arguments)]
pub(super) fn record_usage_call(
    pool: &UpstreamPool,
    event: UpstreamRequestLog<'_>,
    subject: Option<&str>,
    outcome: &'static str,
    error_kind: Option<&'static str>,
    elapsed_ms: u128,
    response_bytes: Option<usize>,
) {
    let Some(store) = pool.usage_store.clone() else {
        return;
    };
    let record = UpstreamCallRecord {
        ts_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        upstream_name: event.upstream.to_string(),
        tool_name: event.item.unwrap_or_default().to_string(),
        capability: event.capability.to_string(),
        operation: event.operation.to_string(),
        subject_scoped: event.subject_scoped,
        actor: subject.map(str::to_string),
        outcome: outcome.to_string(),
        error_kind: error_kind.map(str::to_string),
        elapsed_ms: i64::try_from(elapsed_ms).unwrap_or(i64::MAX),
        response_bytes: response_bytes.map(|b| i64::try_from(b).unwrap_or(i64::MAX)),
    };
    tokio::spawn(async move {
        if let Err(error) = store.record_call(record).await {
            tracing::warn!(error = %error, "usage store record_call failed");
        }
    });
}
```

Declare the module in `crates/labby-gateway/src/upstream/pool.rs`'s `mod` list (find the existing `mod tools_call;` line and add nearby, alphabetically):

```rust
mod usage_record;
```

(No `pub` — it's only called from `capability_call.rs`, a sibling in the same `pool` module tree.)

- [ ] **Step 5: Call it from `timed_capability_call`**

In `crates/labby-gateway/src/upstream/pool/capability_call.rs`, add the import:

```rust
use super::usage_record::record_usage_call;
```

Update each of the three outcome arms in `timed_capability_call` to call `record_usage_call` right after the existing `log_upstream_request_*` call. The size-cap branch:

```rust
            if response_size > max_bytes {
                pool.record_failure_for(
                    upstream_name,
                    capability,
                    format!("response too large: {response_size} bytes"),
                )
                .await;
                log_upstream_request_error(
                    event,
                    start.elapsed().as_millis(),
                    "response_too_large",
                    None,
                    Some(response_size),
                    Some(max_bytes),
                );
                record_usage_call(
                    pool,
                    event,
                    subject,
                    "response_too_large",
                    Some("response_too_large"),
                    start.elapsed().as_millis(),
                    Some(response_size),
                );
                return Err(format!(
                    "upstream response too large ({response_size} bytes, max {max_bytes})"
                ));
            }
```

The success branch:

```rust
            pool.record_success_for(upstream_name, capability).await;
            log_upstream_request_finish(event, start.elapsed().as_millis(), Some(response_size));
            record_usage_call(
                pool,
                event,
                subject,
                "ok",
                None,
                start.elapsed().as_millis(),
                Some(response_size),
            );
            Ok(result)
```

The upstream-error branch:

```rust
        RawCallOutcome::UpstreamError(error) => {
            pool.record_failure_for(upstream_name, capability, error_message_fn(&error))
                .await;
            if let Some(subj) = subject {
                pool.evict_subject_connection(upstream_name, subj).await;
            }
            log_upstream_request_error(
                event,
                start.elapsed().as_millis(),
                "upstream_error",
                Some(&error),
                None,
                None,
            );
            record_usage_call(
                pool,
                event,
                subject,
                "upstream_error",
                Some("upstream_error"),
                start.elapsed().as_millis(),
                None,
            );
            Err(error_message_fn(&error))
        }
```

The timeout branch:

```rust
        RawCallOutcome::Timeout => {
            pool.record_failure_for(upstream_name, capability, timeout_message.clone())
                .await;
            if let Some(subj) = subject {
                pool.evict_subject_connection(upstream_name, subj).await;
            }
            log_upstream_request_error(
                event,
                start.elapsed().as_millis(),
                "timeout",
                None,
                None,
                None,
            );
            record_usage_call(
                pool,
                event,
                subject,
                "timeout",
                Some("timeout"),
                start.elapsed().as_millis(),
                None,
            );
            Err(timeout_message)
        }
```

`subject_scoped_call_tool`'s early connect-failure path in `crates/labby-gateway/src/upstream/pool/tools_call.rs` also emits a log event outside `timed_capability_call` (the `upstream_connect_error` case). Add a matching `record_usage_call` call there too, right after its `log_upstream_request_error(...)` call:

```rust
                log_upstream_request_error(
                    event,
                    elapsed_ms,
                    "upstream_connect_error",
                    Some(&error),
                    None,
                    None,
                );
                super::usage_record::record_usage_call(
                    self,
                    event,
                    Some(subject),
                    "upstream_connect_error",
                    Some("upstream_connect_error"),
                    elapsed_ms,
                    None,
                );
                return Err(error.to_string());
```

(`self` here is `&UpstreamPool` — `subject_scoped_call_tool` is an `impl UpstreamPool` method, so `self` is in scope.)

- [ ] **Step 6: Run tests to verify they pass**

Run:

```bash
cargo test -p labby-gateway --all-features call_tool_records_usage_when_store_is_wired -- --nocapture
cargo test -p labby-gateway --all-features upstream::pool -- --nocapture
```

Expected: PASS. The second command is a regression sweep over the whole `pool` module — confirms the four edited branches in `capability_call.rs` didn't change existing behavior (log fields, error strings, timing) for pools with no `usage_store` wired (`record_usage_call` no-ops immediately on `None`).

- [ ] **Step 7: Commit**

```bash
git add crates/labby-gateway/src/upstream/pool.rs crates/labby-gateway/src/upstream/pool/usage_record.rs crates/labby-gateway/src/upstream/pool/capability_call.rs crates/labby-gateway/src/upstream/pool/tools_call.rs crates/labby-gateway/src/usage/store.rs
git commit -m "feat(gateway): record usage telemetry on every upstream call outcome"
```

---

### Task 3: Wire `UsageStore` construction into `GatewayManager` and `labby` startup

**Files:**
- Modify: `crates/labby-gateway/src/gateway/manager.rs`
- Modify: `crates/labby-gateway/src/gateway/manager/core.rs`
- Modify: `crates/labby/src/config.rs`
- Modify: `crates/labby/src/cli/serve.rs`
- Modify: `crates/labby/src/cli/gateway.rs`

**Interfaces:**
- Consumes: `UpstreamPool::with_usage_store` (Task 2), `UsageStore::open` (Task 1).
- Produces: `GatewayManager::with_usage_store(self, store: Arc<UsageStore>) -> Self`.
- Produces: `GatewayManagerConfig.usage_store: Option<Arc<UsageStore>>`.
- Produces: `crate::config::usage_db_path() -> PathBuf`, `crate::config::usage_telemetry_enabled() -> bool`.

- [ ] **Step 1: Add the config helpers**

In `crates/labby/src/config.rs`, add near `registry_db_path` (after it):

```rust
/// Path to the SQLite gateway usage-telemetry database: `~/.labby/usage.db`.
///
/// Creates no files — callers are responsible for opening/creating the store.
pub fn usage_db_path() -> PathBuf {
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".labby")
        .join("usage.db")
}

/// Whether gateway call-usage telemetry capture is enabled.
///
/// Set `LABBY_GATEWAY_USAGE_DISABLED=1` to opt out — e.g. for a throwaway
/// dev instance where nobody looks at the usage dashboard.
pub fn usage_telemetry_enabled() -> bool {
    std::env::var("LABBY_GATEWAY_USAGE_DISABLED")
        .ok()
        .as_deref()
        != Some("1")
}
```

- [ ] **Step 2: Write a failing test that `GatewayManagerConfig.usage_store` reaches pools it builds**

Add to `crates/labby-gateway/src/gateway/manager/tests.rs` (this file already has a `SqliteStore::open` test fixture at line ~182 to model the setup on):

```rust
    #[tokio::test]
    async fn new_base_pool_carries_the_manager_usage_store() {
        let dir = tempfile::tempdir().unwrap();
        let usage_store = Arc::new(
            crate::usage::UsageStore::open(dir.path().join("usage.db"))
                .await
                .unwrap(),
        );
        let manager = GatewayManager::new(
            dir.path().join("config.toml"),
            GatewayRuntimeHandle::default(),
        )
        .with_usage_store(Arc::clone(&usage_store));

        let pool = manager.new_base_pool(
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(5),
        );

        assert!(
            pool.usage_store.is_some(),
            "pools built by a manager with a usage store must inherit it"
        );
    }
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `cargo test -p labby-gateway --all-features new_base_pool_carries_the_manager_usage_store -- --nocapture`

Expected: FAIL to compile — `GatewayManager::with_usage_store` does not exist.

- [ ] **Step 4: Add the field, builder, and config wiring to `GatewayManager`**

In `crates/labby-gateway/src/gateway/manager.rs`, add a field to the `GatewayManager` struct (after `pub(super) oauth_redirect_uri: Option<Arc<String>>,`):

```rust
    pub(super) usage_store: Option<Arc<crate::usage::UsageStore>>,
```

In `crates/labby-gateway/src/gateway/manager/core.rs`:

Add to `GatewayManagerConfig` (after `pub oauth: Option<GatewayOauthConfig>,`):

```rust
    /// Optional call-usage recorder, shared with every `UpstreamPool` the
    /// manager builds. `None` disables telemetry capture.
    pub usage_store: Option<Arc<crate::usage::UsageStore>>,
```

In `GatewayManager::from_config`, after the `if let Some(oauth) = cfg.oauth { ... }` block:

```rust
        if let Some(store) = cfg.usage_store {
            manager = manager.with_usage_store(store);
        }
```

In `GatewayManager::try_with_store`'s struct literal, add (after `oauth_redirect_uri: None,`):

```rust
            usage_store: None,
```

Add a builder method near `with_openapi`:

```rust
    /// Attach a call-usage recorder, shared with every `UpstreamPool` this
    /// manager builds via `new_base_pool`.
    #[must_use]
    pub fn with_usage_store(mut self, store: Arc<crate::usage::UsageStore>) -> Self {
        self.usage_store = Some(store);
        self
    }
```

Update `new_base_pool` to thread it through:

```rust
    pub(crate) fn new_base_pool(
        &self,
        request_timeout: std::time::Duration,
        relay_timeout: std::time::Duration,
    ) -> UpstreamPool {
        match &self.oauth_client_cache {
            Some(cache) => UpstreamPool::new().with_oauth_client_cache(cache.clone()),
            None => UpstreamPool::new(),
        }
        .with_request_timeout(request_timeout)
        .with_relay_timeout(relay_timeout)
        .with_usage_store(self.usage_store.clone())
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p labby-gateway --all-features new_base_pool_carries_the_manager_usage_store -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Wire the store into `labby serve`'s long-lived pool**

In `crates/labby/src/cli/serve.rs`, immediately before the existing `let mut pool_builder = crate::dispatch::upstream::pool::UpstreamPool::new()` line, add:

```rust
    let usage_store = if crate::config::usage_telemetry_enabled() {
        match labby_gateway::usage::UsageStore::open(crate::config::usage_db_path()).await {
            Ok(store) => Some(Arc::new(store)),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to open gateway usage store; usage telemetry disabled for this run"
                );
                None
            }
        }
    } else {
        None
    };
```

Change the `pool_builder` chain to thread it in:

```rust
    let mut pool_builder = crate::dispatch::upstream::pool::UpstreamPool::new()
        .with_request_timeout(config.upstream_request_timeout())
        .with_relay_timeout(config.upstream_relay_timeout())
        .with_in_process_connector(crate::mcp::in_process_peer::connector())
        .with_usage_store(usage_store.clone());
```

Then in the `GatewayManagerConfig { ... }` struct literal further down (the one at line ~965), add:

```rust
            usage_store: usage_store.clone(),
```

- [ ] **Step 7: Wire the store into the one-shot CLI pool builder**

In `crates/labby/src/cli/gateway.rs`, inside `build_manager_with_upstream_oauth_runtime`, immediately before `let mut pool_builder = UpstreamPool::new()`, add:

```rust
    let usage_store = if crate::config::usage_telemetry_enabled() {
        labby_gateway::usage::UsageStore::open(crate::config::usage_db_path())
            .await
            .ok()
            .map(Arc::new)
    } else {
        None
    };
```

Change the `pool_builder` chain (inside the `if discover_upstreams { ... }` block) to add `.with_usage_store(usage_store.clone())` after `.with_in_process_connector(...)`, matching Step 6's shape.

In the `GatewayManagerConfig { ... }` struct literal in the same function, add `usage_store: usage_store.clone(),`.

Note: this function currently early-returns a manager without a pool at all when `!discover_upstreams` (see the surrounding `if discover_upstreams { ... }` guard) — `usage_store` must be computed *before* that branch so it's available for the `GatewayManagerConfig` construction that happens after the branch either way. Read the full function body before editing to place the `usage_store` computation correctly relative to that control flow.

- [ ] **Step 8: Full workspace build + targeted tests**

Run:

```bash
cargo check -p labby-gateway -p labby --all-features
cargo test -p labby-gateway --all-features usage -- --nocapture
cargo test -p labby-gateway --all-features new_base_pool_carries_the_manager_usage_store -- --nocapture
```

Expected: builds clean, tests PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/labby-gateway/src/gateway/manager.rs crates/labby-gateway/src/gateway/manager/core.rs crates/labby-gateway/src/gateway/manager/tests.rs crates/labby/src/config.rs crates/labby/src/cli/serve.rs crates/labby/src/cli/gateway.rs
git commit -m "feat(gateway): construct and thread UsageStore through manager and CLI startup"
```

---

### Task 4: Aggregation queries and `gateway.usage.*` actions

**Files:**
- Create: `crates/labby-gateway/src/usage/query.rs`
- Create: `crates/labby-gateway/src/gateway/manager/usage.rs`
- Modify: `crates/labby-gateway/src/usage.rs`
- Modify: `crates/labby-gateway/src/gateway/manager.rs`
- Modify: `crates/labby-gateway/src/gateway/catalog.rs`
- Modify: `crates/labby-gateway/src/gateway/dispatch.rs`
- Modify: `crates/labby-gateway/src/gateway/params.rs`
- Modify: `crates/labby-gateway/src/gateway/types.rs`
- Test: `crates/labby-gateway/src/gateway/dispatch_tests.rs`

**Interfaces:**
- Consumes: `UsageStore::with_conn` (Task 1).
- Produces: `UsageStore::metrics(&self, query: UsageMetricsQuery) -> Result<UsageMetrics, ToolError>` (async).
- Produces: `UsageStore::list_calls(&self, query: UsageCallsQuery) -> Result<(Vec<UpstreamCallRecordView>, i64), ToolError>` (async) — second element is `total_matching`, ignoring `limit`/`offset`.
- Produces: `GatewayManager::usage_metrics(&self, params: GatewayUsageMetricsParams) -> Result<GatewayUsageMetricsView, ToolError>`.
- Produces: `GatewayManager::usage_calls(&self, params: GatewayUsageCallsParams) -> Result<GatewayUsageCallsView, ToolError>`.
- Produces actions: `gateway.usage.metrics`, `gateway.usage.calls`.

- [ ] **Step 1: Write failing store-level aggregation tests**

Add to `crates/labby-gateway/src/usage/store.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[tokio::test]
    async fn metrics_aggregates_totals_and_top_tools() {
        use super::super::query::UsageMetricsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        let mut ok = sample_record(1_000);
        ok.tool_name = "search_repos".to_string();
        store.record_call(ok.clone()).await.unwrap();
        store.record_call(ok).await.unwrap();

        let mut failed = sample_record(1_001);
        failed.outcome = "timeout".to_string();
        failed.tool_name = "search_repos".to_string();
        store.record_call(failed).await.unwrap();

        let metrics = store
            .metrics(UsageMetricsQuery {
                since_unix: None,
                until_unix: None,
                upstream: None,
            })
            .await
            .unwrap();

        assert_eq!(metrics.total_calls, 3);
        assert_eq!(metrics.error_calls, 1);
        assert_eq!(metrics.top_tools.len(), 1);
        assert_eq!(metrics.top_tools[0].tool, "search_repos");
        assert_eq!(metrics.top_tools[0].calls, 3);
    }

    #[tokio::test]
    async fn list_calls_respects_limit_and_reports_total_matching() {
        use super::super::query::UsageCallsQuery;

        let dir = tempfile::tempdir().unwrap();
        let store = UsageStore::open(dir.path().join("usage.db")).await.unwrap();

        for ts in 0..5 {
            store.record_call(sample_record(ts)).await.unwrap();
        }

        let (page, total) = store
            .list_calls(UsageCallsQuery {
                since_unix: None,
                until_unix: None,
                upstream: None,
                limit: 2,
                offset: 0,
            })
            .await
            .unwrap();

        assert_eq!(page.len(), 2);
        assert_eq!(total, 5);
        // Newest first.
        assert_eq!(page[0].ts_unix, 4);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p labby-gateway --all-features usage::store::tests -- --nocapture`

Expected: FAIL to compile — `query` module and `UsageStore::metrics`/`list_calls` do not exist.

- [ ] **Step 3: Implement the query types and SQL**

Create `crates/labby-gateway/src/usage/query.rs`:

```rust
//! Aggregation query parameters and result shapes for `gateway.usage.*`.

#[derive(Debug, Clone, Default)]
pub struct UsageMetricsQuery {
    pub since_unix: Option<i64>,
    pub until_unix: Option<i64>,
    pub upstream: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UsageCallsQuery {
    pub since_unix: Option<i64>,
    pub until_unix: Option<i64>,
    pub upstream: Option<String>,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageToolCount {
    pub upstream: String,
    pub tool: String,
    pub calls: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageActorCount {
    /// `"unattributed"` for calls with no OAuth subject.
    pub actor: String,
    pub calls: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageMetrics {
    pub total_calls: i64,
    pub error_calls: i64,
    pub avg_elapsed_ms: f64,
    pub top_tools: Vec<UsageToolCount>,
    pub top_actors: Vec<UsageActorCount>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpstreamCallRecordView {
    pub ts_unix: i64,
    pub upstream: String,
    pub tool: String,
    pub actor: Option<String>,
    pub outcome: String,
    pub elapsed_ms: i64,
}

pub(super) const TOP_N: usize = 10;
```

Add to `crates/labby-gateway/src/usage/store.rs`, above the `#[cfg(test)]` block:

```rust
impl UsageStore {
    pub async fn metrics(
        &self,
        query: super::query::UsageMetricsQuery,
    ) -> Result<super::query::UsageMetrics, ToolError> {
        self.with_conn(move |conn| {
            let (where_clause, bind) = usage_where_clause(&query.since_unix, &query.until_unix, &query.upstream);

            let (total_calls, error_calls, avg_elapsed_ms): (i64, i64, f64) = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*), SUM(CASE WHEN outcome != 'ok' THEN 1 ELSE 0 END), \
                         COALESCE(AVG(elapsed_ms), 0.0) FROM upstream_calls {where_clause}"
                    ),
                    rusqlite::params_from_iter(bind.iter()),
                    |row| Ok((row.get(0)?, row.get::<_, Option<i64>>(1)?.unwrap_or(0), row.get(2)?)),
                )
                .map_err(sqlite_error)?;

            let mut top_tools_stmt = conn
                .prepare(&format!(
                    "SELECT upstream_name, tool_name, COUNT(*) as calls FROM upstream_calls {where_clause} \
                     GROUP BY upstream_name, tool_name ORDER BY calls DESC LIMIT {}",
                    super::query::TOP_N
                ))
                .map_err(sqlite_error)?;
            let top_tools = top_tools_stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok(super::query::UsageToolCount {
                        upstream: row.get(0)?,
                        tool: row.get(1)?,
                        calls: row.get(2)?,
                    })
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;

            let mut top_actors_stmt = conn
                .prepare(&format!(
                    "SELECT COALESCE(actor, 'unattributed'), COUNT(*) as calls FROM upstream_calls {where_clause} \
                     GROUP BY COALESCE(actor, 'unattributed') ORDER BY calls DESC LIMIT {}",
                    super::query::TOP_N
                ))
                .map_err(sqlite_error)?;
            let top_actors = top_actors_stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok(super::query::UsageActorCount {
                        actor: row.get(0)?,
                        calls: row.get(1)?,
                    })
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;

            Ok(super::query::UsageMetrics {
                total_calls,
                error_calls,
                avg_elapsed_ms,
                top_tools,
                top_actors,
            })
        })
        .await
    }

    /// Returns the requested page plus the total row count matching the
    /// filter (ignoring `limit`/`offset`), newest calls first.
    pub async fn list_calls(
        &self,
        query: super::query::UsageCallsQuery,
    ) -> Result<(Vec<super::query::UpstreamCallRecordView>, i64), ToolError> {
        self.with_conn(move |conn| {
            let (where_clause, mut bind) =
                usage_where_clause(&query.since_unix, &query.until_unix, &query.upstream);

            let total: i64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM upstream_calls {where_clause}"),
                    rusqlite::params_from_iter(bind.iter()),
                    |row| row.get(0),
                )
                .map_err(sqlite_error)?;

            bind.push(rusqlite::types::Value::Integer(query.limit as i64));
            bind.push(rusqlite::types::Value::Integer(query.offset as i64));
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT ts_unix, upstream_name, tool_name, actor, outcome, elapsed_ms \
                     FROM upstream_calls {where_clause} \
                     ORDER BY ts_unix DESC, id DESC LIMIT ?{} OFFSET ?{}",
                    bind.len() - 1,
                    bind.len()
                ))
                .map_err(sqlite_error)?;
            let rows = stmt
                .query_map(rusqlite::params_from_iter(bind.iter()), |row| {
                    Ok(super::query::UpstreamCallRecordView {
                        ts_unix: row.get(0)?,
                        upstream: row.get(1)?,
                        tool: row.get(2)?,
                        actor: row.get(3)?,
                        outcome: row.get(4)?,
                        elapsed_ms: row.get(5)?,
                    })
                })
                .map_err(sqlite_error)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(sqlite_error)?;

            Ok((rows, total))
        })
        .await
    }
}

/// Build a `WHERE ...` clause (or empty string) plus its positional bind
/// values for the optional since/until/upstream filters shared by `metrics`
/// and `list_calls`.
fn usage_where_clause(
    since_unix: &Option<i64>,
    until_unix: &Option<i64>,
    upstream: &Option<String>,
) -> (String, Vec<rusqlite::types::Value>) {
    let mut clauses = Vec::new();
    let mut bind = Vec::new();
    if let Some(since) = since_unix {
        clauses.push(format!("ts_unix >= ?{}", bind.len() + 1));
        bind.push(rusqlite::types::Value::Integer(*since));
    }
    if let Some(until) = until_unix {
        clauses.push(format!("ts_unix <= ?{}", bind.len() + 1));
        bind.push(rusqlite::types::Value::Integer(*until));
    }
    if let Some(upstream) = upstream {
        clauses.push(format!("upstream_name = ?{}", bind.len() + 1));
        bind.push(rusqlite::types::Value::Text(upstream.clone()));
    }
    if clauses.is_empty() {
        (String::new(), bind)
    } else {
        (format!("WHERE {}", clauses.join(" AND ")), bind)
    }
}
```

Add `pub mod query;` to `crates/labby-gateway/src/usage.rs` (alongside the existing `pub mod store;`/`pub mod types;`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p labby-gateway --all-features usage::store::tests -- --nocapture`

Expected: PASS (4 tests total in this module now).

- [ ] **Step 5: Add `GatewayManager` query methods**

Create `crates/labby-gateway/src/gateway/manager/usage.rs`:

```rust
//! `GatewayManager` facade over `UsageStore`'s query side, backing the
//! `gateway.usage.metrics` / `gateway.usage.calls` actions. Read-only: this
//! module never writes — writes happen inline in `UpstreamPool` (see
//! `upstream/pool/usage_record.rs`).

use labby_runtime::error::ToolError;

use crate::usage::query::{UsageCallsQuery, UsageMetricsQuery};

use super::GatewayManager;
use crate::gateway::params::{GatewayUsageCallsParams, GatewayUsageMetricsParams};
use crate::gateway::types::{
    GatewayUsageActorCount, GatewayUsageCallView, GatewayUsageCallsView, GatewayUsageMetricsView,
    GatewayUsageToolCount,
};

const DEFAULT_CALLS_LIMIT: usize = 100;
const MAX_CALLS_LIMIT: usize = 1000;

impl GatewayManager {
    pub async fn usage_metrics(
        &self,
        params: GatewayUsageMetricsParams,
    ) -> Result<GatewayUsageMetricsView, ToolError> {
        let Some(store) = &self.usage_store else {
            return Err(ToolError::Sdk {
                sdk_kind: "usage_store_unavailable".to_string(),
                message: "gateway usage telemetry is disabled for this instance".to_string(),
            });
        };
        let metrics = store
            .metrics(UsageMetricsQuery {
                since_unix: params.since_unix,
                until_unix: params.until_unix,
                upstream: params.upstream,
            })
            .await?;
        Ok(GatewayUsageMetricsView {
            total_calls: metrics.total_calls,
            error_calls: metrics.error_calls,
            avg_elapsed_ms: metrics.avg_elapsed_ms,
            top_tools: metrics
                .top_tools
                .into_iter()
                .map(|t| GatewayUsageToolCount {
                    upstream: t.upstream,
                    tool: t.tool,
                    calls: t.calls,
                })
                .collect(),
            top_actors: metrics
                .top_actors
                .into_iter()
                .map(|a| GatewayUsageActorCount {
                    actor: a.actor,
                    calls: a.calls,
                })
                .collect(),
        })
    }

    pub async fn usage_calls(
        &self,
        params: GatewayUsageCallsParams,
    ) -> Result<GatewayUsageCallsView, ToolError> {
        let Some(store) = &self.usage_store else {
            return Err(ToolError::Sdk {
                sdk_kind: "usage_store_unavailable".to_string(),
                message: "gateway usage telemetry is disabled for this instance".to_string(),
            });
        };
        let limit = params
            .limit
            .unwrap_or(DEFAULT_CALLS_LIMIT)
            .min(MAX_CALLS_LIMIT);
        let (rows, total_matching) = store
            .list_calls(UsageCallsQuery {
                since_unix: params.since_unix,
                until_unix: params.until_unix,
                upstream: params.upstream,
                limit,
                offset: params.offset.unwrap_or(0),
            })
            .await?;
        Ok(GatewayUsageCallsView {
            calls: rows
                .into_iter()
                .map(|r| GatewayUsageCallView {
                    ts_unix: r.ts_unix,
                    upstream: r.upstream,
                    tool: r.tool,
                    actor: r.actor,
                    outcome: r.outcome,
                    elapsed_ms: r.elapsed_ms,
                })
                .collect(),
            total_matching,
        })
    }
}
```

Add `mod usage;` to `crates/labby-gateway/src/gateway/manager.rs` (alongside the existing `mod enrichment;`).

- [ ] **Step 6: Add param and view types**

In `crates/labby-gateway/src/gateway/params.rs`, add (near `GatewayEnrichPreviewParams`):

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GatewayUsageMetricsParams {
    #[serde(default)]
    pub since_unix: Option<i64>,
    #[serde(default)]
    pub until_unix: Option<i64>,
    #[serde(default)]
    pub upstream: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct GatewayUsageCallsParams {
    #[serde(default)]
    pub since_unix: Option<i64>,
    #[serde(default)]
    pub until_unix: Option<i64>,
    #[serde(default)]
    pub upstream: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}
```

In `crates/labby-gateway/src/gateway/types.rs`, add (near `GatewayHintProposalView`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayUsageToolCount {
    pub upstream: String,
    pub tool: String,
    pub calls: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayUsageActorCount {
    pub actor: String,
    pub calls: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayUsageMetricsView {
    pub total_calls: i64,
    pub error_calls: i64,
    pub avg_elapsed_ms: f64,
    pub top_tools: Vec<GatewayUsageToolCount>,
    pub top_actors: Vec<GatewayUsageActorCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayUsageCallView {
    pub ts_unix: i64,
    pub upstream: String,
    pub tool: String,
    pub actor: Option<String>,
    pub outcome: String,
    pub elapsed_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GatewayUsageCallsView {
    pub calls: Vec<GatewayUsageCallView>,
    pub total_matching: i64,
}
```

- [ ] **Step 7: Register the catalog actions**

In `crates/labby-gateway/src/gateway/catalog.rs`, add after the `gateway.enrich.apply` entry:

```rust
    ActionSpec {
        name: "gateway.usage.metrics",
        description: "Aggregate gateway upstream call telemetry: totals, error rate, top tools, top actors",
        destructive: false,
        requires_admin: true,
        returns: "GatewayUsageMetricsView",
        params: &[
            ParamSpec {
                name: "since_unix",
                ty: "integer",
                required: false,
                description: "Only include calls at or after this Unix timestamp",
            },
            ParamSpec {
                name: "until_unix",
                ty: "integer",
                required: false,
                description: "Only include calls at or before this Unix timestamp",
            },
            ParamSpec {
                name: "upstream",
                ty: "string",
                required: false,
                description: "Restrict to one upstream name",
            },
        ],
    },
    ActionSpec {
        name: "gateway.usage.calls",
        description: "List raw gateway upstream call records, newest first",
        destructive: false,
        requires_admin: true,
        returns: "GatewayUsageCallsView",
        params: &[
            ParamSpec {
                name: "since_unix",
                ty: "integer",
                required: false,
                description: "Only include calls at or after this Unix timestamp",
            },
            ParamSpec {
                name: "until_unix",
                ty: "integer",
                required: false,
                description: "Only include calls at or before this Unix timestamp",
            },
            ParamSpec {
                name: "upstream",
                ty: "string",
                required: false,
                description: "Restrict to one upstream name",
            },
            ParamSpec {
                name: "limit",
                ty: "integer",
                required: false,
                description: "Max rows to return, capped at 1000 (default 100)",
            },
            ParamSpec {
                name: "offset",
                ty: "integer",
                required: false,
                description: "Row offset for pagination",
            },
        ],
    },
```

Find the match arm that lists every `gateway.*` action name for admin-gating (the one containing `"gateway.enrich.preview" | "gateway.enrich.apply"` seen at `catalog.rs:946-947`) and add the two new names to that same arm:

```rust
                    | "gateway.enrich.preview"
                    | "gateway.enrich.apply"
                    | "gateway.usage.metrics"
                    | "gateway.usage.calls"
```

- [ ] **Step 8: Route the actions in dispatch**

In `crates/labby-gateway/src/gateway/dispatch.rs`, add the import (extend the existing `use crate::gateway::params::{...}` line):

```rust
    GatewayUsageCallsParams, GatewayUsageMetricsParams,
```

Add match arms alongside the existing `"gateway.enrich.preview" => { ... }` arm:

```rust
        "gateway.usage.metrics" => {
            let params: GatewayUsageMetricsParams = parse_params(params_value)?;
            serde_json::to_value(manager.usage_metrics(params).await?)
                .map_err(|error| ToolError::Sdk {
                    sdk_kind: "serialize_error".to_string(),
                    message: error.to_string(),
                })
        }
        "gateway.usage.calls" => {
            let params: GatewayUsageCallsParams = parse_params(params_value)?;
            serde_json::to_value(manager.usage_calls(params).await?)
                .map_err(|error| ToolError::Sdk {
                    sdk_kind: "serialize_error".to_string(),
                    message: error.to_string(),
                })
        }
```

Match the exact serialization/error-wrapping style already used by the neighboring `gateway.enrich.*` arms — read them first and mirror their pattern precisely rather than the sketch above if they differ (e.g. if there's already a shared `to_value_or_sdk_error` helper, use that instead of duplicating the `serde_json::to_value(...).map_err(...)` inline).

- [ ] **Step 9: Write dispatch-level tests**

Add to `crates/labby-gateway/src/gateway/dispatch_tests.rs`, modeled on the existing enrichment dispatch tests in that file:

```rust
    #[tokio::test]
    async fn gateway_usage_metrics_returns_zeroed_view_with_no_calls() {
        let (manager, _store) = fixture_manager_with_config(
            r#"
[[upstream]]
name = "github"
url = "https://example.invalid/mcp"
"#,
        )
        .await;
        let usage_store = Arc::new(
            labby_gateway::usage::UsageStore::open(
                tempfile::tempdir().unwrap().path().join("usage.db"),
            )
            .await
            .unwrap(),
        );
        let manager = manager.with_usage_store(usage_store);

        let result = dispatch(&manager, "gateway.usage.metrics", serde_json::json!({}))
            .await
            .expect("dispatch succeeds");
        assert_eq!(result["total_calls"], 0);
        assert_eq!(result["error_calls"], 0);
    }

    #[tokio::test]
    async fn gateway_usage_metrics_fails_closed_when_store_not_wired() {
        let (manager, _store) = fixture_manager_with_config(
            r#"
[[upstream]]
name = "github"
url = "https://example.invalid/mcp"
"#,
        )
        .await;

        let error = dispatch(&manager, "gateway.usage.metrics", serde_json::json!({}))
            .await
            .expect_err("no usage store wired must fail, not silently return empty data");
        assert_eq!(error.kind(), "usage_store_unavailable");
    }
```

Adjust the `dispatch(...)` call signature and `fixture_manager_with_config` import path to match whatever helper the existing tests in this file actually use — read the top of `dispatch_tests.rs` first (it already has this pattern for `gateway.enrich.preview`; copy its exact call shape rather than guessing).

- [ ] **Step 10: Run the full test sweep**

Run:

```bash
cargo test -p labby-gateway --all-features gateway_usage -- --nocapture
cargo test -p labby-gateway --all-features usage -- --nocapture
cargo check -p labby --all-features
```

Expected: all PASS, workspace builds clean.

- [ ] **Step 11: Commit**

```bash
git add crates/labby-gateway/src/usage.rs crates/labby-gateway/src/usage/query.rs crates/labby-gateway/src/usage/store.rs crates/labby-gateway/src/gateway/manager.rs crates/labby-gateway/src/gateway/manager/usage.rs crates/labby-gateway/src/gateway/catalog.rs crates/labby-gateway/src/gateway/dispatch.rs crates/labby-gateway/src/gateway/params.rs crates/labby-gateway/src/gateway/types.rs crates/labby-gateway/src/gateway/dispatch_tests.rs
git commit -m "feat(gateway): add gateway.usage.metrics and gateway.usage.calls actions"
```

---

### Task 5: CLI — `labby gateway usage metrics|calls`

**Files:**
- Modify: `crates/labby/src/cli/gateway/args.rs`
- Modify: `crates/labby/src/cli/gateway/dispatch.rs`
- Modify: `crates/labby/src/cli/gateway.rs`

**Interfaces:**
- Consumes: `gateway.usage.metrics` / `gateway.usage.calls` actions (Task 4).
- Produces CLI: `labby gateway usage metrics [--since-unix N] [--until-unix N] [--upstream NAME] [--json]`, `labby gateway usage calls [--since-unix N] [--until-unix N] [--upstream NAME] [--limit N] [--offset N] [--json]`.

- [ ] **Step 1: Write a failing CLI parse test**

Find the existing parse tests for `GatewayCommand::Enrich` (likely in `crates/labby/src/cli/gateway/args.rs` or a sibling `tests.rs` — grep `GatewayEnrichArgs` for the existing test to model this on) and add:

```rust
    #[test]
    fn gateway_usage_metrics_parses_with_upstream_filter() {
        let args = GatewayArgs::parse_from([
            "gateway",
            "usage",
            "metrics",
            "--upstream",
            "github",
        ]);
        match args.command {
            GatewayCommand::Usage(usage) => match usage.command {
                GatewayUsageCommand::Metrics(metrics) => {
                    assert_eq!(metrics.upstream.as_deref(), Some("github"));
                }
                _ => panic!("expected Metrics subcommand"),
            },
            _ => panic!("expected Usage command"),
        }
    }

    #[test]
    fn gateway_usage_calls_parses_limit_and_offset() {
        let args = GatewayArgs::parse_from([
            "gateway", "usage", "calls", "--limit", "50", "--offset", "10",
        ]);
        match args.command {
            GatewayCommand::Usage(usage) => match usage.command {
                GatewayUsageCommand::Calls(calls) => {
                    assert_eq!(calls.limit, Some(50));
                    assert_eq!(calls.offset, Some(10));
                }
                _ => panic!("expected Calls subcommand"),
            },
            _ => panic!("expected Usage command"),
        }
    }
```

Adjust the exact `GatewayArgs::parse_from` / module path to whatever the neighboring `Enrich` test actually uses (top-level clap struct name, module it lives in) — copy its shape.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p labby --all-features gateway_usage_metrics_parses gateway_usage_calls_parses -- --nocapture`

Expected: FAIL to compile — `GatewayCommand::Usage` does not exist.

- [ ] **Step 3: Add the CLI args**

In `crates/labby/src/cli/gateway/args.rs`, add a variant to `GatewayCommand` (alongside `Enrich(GatewayEnrichArgs)`):

```rust
    /// Query gateway upstream call-usage telemetry.
    Usage(GatewayUsageArgs),
```

Add the arg structs (mirroring `GatewayEnrichArgs`/`GatewayEnrichCommand`):

```rust
#[derive(Debug, Args)]
pub struct GatewayUsageArgs {
    #[command(subcommand)]
    pub command: GatewayUsageCommand,
}

#[derive(Debug, Subcommand)]
pub enum GatewayUsageCommand {
    /// Aggregated totals, error rate, top tools, top actors.
    Metrics(GatewayUsageMetricsArgs),
    /// Raw call records, newest first.
    Calls(GatewayUsageCallsArgs),
}

#[derive(Debug, Args)]
pub struct GatewayUsageMetricsArgs {
    #[arg(long)]
    pub since_unix: Option<i64>,
    #[arg(long)]
    pub until_unix: Option<i64>,
    #[arg(long)]
    pub upstream: Option<String>,
}

#[derive(Debug, Args)]
pub struct GatewayUsageCallsArgs {
    #[arg(long)]
    pub since_unix: Option<i64>,
    #[arg(long)]
    pub until_unix: Option<i64>,
    #[arg(long)]
    pub upstream: Option<String>,
    #[arg(long)]
    pub limit: Option<usize>,
    #[arg(long)]
    pub offset: Option<usize>,
}
```

- [ ] **Step 4: Wire dispatch**

In `crates/labby/src/cli/gateway/dispatch.rs`, add a match arm alongside the existing `GatewayCommand::Enrich(args) => match args.command { ... }`:

```rust
                GatewayCommand::Usage(args) => match args.command {
                    GatewayUsageCommand::Metrics(m) => (
                        "gateway.usage.metrics".to_string(),
                        json!({
                            "since_unix": m.since_unix,
                            "until_unix": m.until_unix,
                            "upstream": m.upstream,
                        }),
                    ),
                    GatewayUsageCommand::Calls(c) => (
                        "gateway.usage.calls".to_string(),
                        json!({
                            "since_unix": c.since_unix,
                            "until_unix": c.until_unix,
                            "upstream": c.upstream,
                            "limit": c.limit,
                            "offset": c.offset,
                        }),
                    ),
                },
```

Match this arm's exact shape (tuple vs. struct return, surrounding match) to the neighboring `GatewayCommand::Enrich` arm at `crates/labby/src/cli/gateway/dispatch.rs:410` — read it first and copy its pattern rather than the sketch above if the real shape differs (e.g. if it returns through a shared `dispatch_action(action, params)` call instead of a `(String, Value)` tuple).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p labby --all-features gateway_usage_metrics_parses gateway_usage_calls_parses -- --nocapture`

Expected: PASS.

- [ ] **Step 6: Manual smoke test**

```bash
cargo run -p labby --all-features -- gateway usage metrics --json
cargo run -p labby --all-features -- gateway usage calls --limit 5 --json
```

Expected: both print `{"kind":"usage_store_unavailable",...}` structured errors if run against a fresh dev instance with no `LABBY_MCP_HTTP_TOKEN`/gateway not yet serving; against a running `labby serve` instance (Task 3 wired), `metrics` returns zeroed totals on a fresh `usage.db` and `calls` returns an empty list. Confirm the command doesn't panic either way.

- [ ] **Step 7: Commit**

```bash
git add crates/labby/src/cli/gateway/args.rs crates/labby/src/cli/gateway/dispatch.rs crates/labby/src/cli/gateway.rs
git commit -m "feat(gateway): add labby gateway usage metrics|calls CLI"
```

---

### Task 6: Retention pruning, disable switch verification, and docs

**Files:**
- Modify: `crates/labby/src/cli/serve.rs`
- Modify: `docs/dev/OBSERVABILITY.md`
- Modify: `docs/runtime/CONFIG.md`

**Interfaces:**
- Consumes: `UsageStore::prune_older_than` (Task 1).

- [ ] **Step 1: Add a periodic prune task to `labby serve`**

In `crates/labby/src/cli/serve.rs`, after the `usage_store` is constructed (Task 3, Step 6) and the long-lived pool/manager are set up, spawn a background prune loop:

```rust
    if let Some(store) = usage_store.clone() {
        tokio::spawn(async move {
            const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);
            const RETENTION_SECS: i64 = 30 * 24 * 60 * 60; // 30 days
            let mut interval = tokio::time::interval(PRUNE_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let cutoff = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64 - RETENTION_SECS)
                    .unwrap_or(0);
                match store.prune_older_than(cutoff).await {
                    Ok(deleted) if deleted > 0 => {
                        tracing::info!(deleted, "pruned stale gateway usage records");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(error = %error, "gateway usage prune failed");
                    }
                }
            }
        });
    }
```

Place this spawn after the `gateway_manager` is constructed and before the server actually starts accepting connections, so the task is visibly part of startup rather than buried mid-function — find the natural "background maintenance tasks" section of `serve.rs` (there is likely already at least one other periodic `tokio::spawn(async move { loop { interval.tick()... } })` pattern in this file, e.g. for probe/reprobe or catalog refresh — place this alongside it and follow its exact logging-field conventions if they differ from the sketch above).

- [ ] **Step 2: Verify it compiles and the interval logic is sound with a unit test**

The interval/spawn loop itself isn't unit-testable in isolation without extracting the cutoff math. Extract that one piece so it is:

In `crates/labby/src/cli/serve.rs` (or a small private helper near the spawn site), add:

```rust
fn usage_prune_cutoff(now_unix: i64, retention_secs: i64) -> i64 {
    now_unix - retention_secs
}

#[cfg(test)]
mod usage_prune_tests {
    use super::usage_prune_cutoff;

    #[test]
    fn cutoff_is_now_minus_retention() {
        assert_eq!(usage_prune_cutoff(1_000_000, 100), 999_900);
    }

    #[test]
    fn cutoff_saturates_at_zero_for_small_now() {
        // Not a realistic production input, but confirms no panic/underflow
        // on i64 subtraction for a clock that hasn't reached epoch+retention yet.
        assert_eq!(usage_prune_cutoff(50, 100), -50);
    }
}
```

Use `usage_prune_cutoff(now, RETENTION_SECS)` in place of the inline subtraction in Step 1's spawned task.

Run: `cargo test -p labby --all-features usage_prune_tests -- --nocapture`

Expected: PASS.

- [ ] **Step 3: Document the new store and actions in `docs/dev/OBSERVABILITY.md`**

Add a new subsection (find the section describing `upstream.pool` request logging — it documents `UpstreamRequestLog` fields — and add this immediately after it):

```markdown
### Gateway usage telemetry (`UsageStore`)

Every upstream tool/resource/prompt call outcome recorded by `upstream.request.finish`/`upstream.request.error` (above) is also durably persisted to a small SQLite store at `~/.labby/usage.db`, via `UpstreamPool`'s `timed_capability_call` choke point (`crates/labby-gateway/src/upstream/pool/capability_call.rs`). This is a fire-and-forget write (`tokio::spawn`) — it never adds latency or failure risk to the call it's observing, and a write failure is logged (`usage store record_call failed`) and dropped, never surfaced to the caller.

Query it via the `gateway.usage.metrics` (aggregated totals/top-tools/top-actors) and `gateway.usage.calls` (raw paginated records) actions — both admin-gated, same as `gateway.enrich.*`. CLI: `labby gateway usage metrics` / `labby gateway usage calls`.

Set `LABBY_GATEWAY_USAGE_DISABLED=1` to disable capture entirely (no store is opened at startup). Retained rows are pruned on a 6-hour cycle to a 30-day retention window by a background task started in `labby serve`.

This store intentionally does not capture CLI/HTTP/MCP dispatch-level events for the `gateway` service's own actions (e.g. `gateway.add`, `gateway.enrich.preview`) — only calls proxied through to upstreams. See `docs/superpowers/plans/2026-07-09-gateway-usage-telemetry.md` for the full design rationale, including why the schema's `capability`/`operation` fields are not hardcoded to "external upstream only".
```

- [ ] **Step 4: Document the config surface in `docs/runtime/CONFIG.md`**

Find the section listing environment variables read by the `labby` binary (near `LABBY_LOG`, `LABBY_LOG_FORMAT`, etc.) and add a row:

```markdown
| `LABBY_GATEWAY_USAGE_DISABLED` | `labby serve`, `labby gateway *` | unset | Set to `1` to disable gateway call-usage telemetry capture. When unset, calls proxied through the gateway to upstreams are recorded to `~/.labby/usage.db`. |
```

Find the section documenting `~/.labby/` file layout (near where `registry.db` is documented, if it is) and add:

```markdown
- `~/.labby/usage.db` — SQLite store of gateway upstream call telemetry (see `docs/dev/OBSERVABILITY.md`). Pruned automatically to a 30-day retention window.
```

If no such file-layout section exists yet, add a short one near the top-level `~/.labby/` description instead of inventing new document structure — read the file's current top-level sections first.

- [ ] **Step 5: Full workspace verification**

Run:

```bash
cargo fmt --all
cargo clippy --workspace --all-features --all-targets -- -D warnings
cargo nextest run --workspace --all-features
```

Expected: clean formatting, zero clippy warnings, all tests pass (including every new test from Tasks 1–6).

- [ ] **Step 6: Commit**

```bash
git add crates/labby/src/cli/serve.rs docs/dev/OBSERVABILITY.md docs/runtime/CONFIG.md
git commit -m "feat(gateway): prune stale usage records and document usage telemetry"
```

---

## Post-plan follow-up (explicitly out of scope here)

- `apps/gateway-admin/lib/api/metrics-client.ts` and the `usage` dashboard page still call the dead `/v1/logs/*` routes deleted by commit `fdb23858`. Rewiring the frontend to `gateway.usage.metrics`/`gateway.usage.calls` (via `POST /v1/gateway` with `{action, params}`, same as every other gateway action) is a separate plan — the response shapes here (`GatewayUsageMetricsView`, `GatewayUsageCallsView`) are deliberately much smaller than the old `logs.metrics` contract in `apps/gateway-admin/lib/types/metrics.ts`, so the frontend will need real changes, not just a URL swap.
- GitHub issue #115 and beads `lab-sohnl`/`lab-2r5br` (reopened 2026-07-08) should be updated once this plan lands, and closed once the frontend follow-up also lands.
- If/when Labby's own tools become callable through Code Mode (mentioned in conversation as a near-term follow-on), extend `UpstreamCallRecord`'s capture point to that new call site using the same `capability`/`operation`/`outcome` shape — no schema migration should be needed, per the Global Constraints above.
