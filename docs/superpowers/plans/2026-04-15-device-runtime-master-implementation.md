# Device Runtime Master Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn `lab serve` into the always-on device runtime on every Linux `x86_64` machine, with one named `master` owning the Web/API/MCP/syslog control plane and non-master devices reporting status, metadata, AI CLI MCP config inventory, and buffered logs to it over `/v1/device/*`.

**Architecture:** Add a new `device` subsystem in `crates/lab/src/device/` instead of overloading the existing upstream MCP gateway code. `serve` starts the device runtime on every machine, mounts a new `/v1/device/*` namespace, gates operator-facing surfaces to the configured master, and wires a master-owned fleet state store for device inventory and log ingestion.

**Tech Stack:** Rust 2024, tokio, axum, reqwest, serde/serde_json, toml, tracing, sha2, tempfile, existing `lab_auth`, existing OAuth relay helpers in `crates/lab/src/oauth/`.

---

## File Structure

### Existing files to modify

- Modify: `crates/lab/src/config.rs`
  - Add `[device]` config model and master resolution helpers.
- Modify: `crates/lab/src/cli/serve.rs`
  - Start device runtime, derive local device identity, and gate master-only surfaces.
- Modify: `crates/lab/src/cli.rs`
  - Register new `device` and `logs` CLI groups.
- Modify: `crates/lab/src/api.rs`
  - Export new `api::device` module.
- Modify: `crates/lab/src/api/state.rs`
  - Add device runtime state and fleet store handles.
- Modify: `crates/lab/src/api/router.rs`
  - Mount `/v1/device/*`, add master-only gating for operator surfaces, keep `/health` and `/ready` reachable.
- Modify: `crates/lab/src/api/web.rs`
  - Return a non-master response when Web UI is disabled on non-master devices.
- Modify: `crates/lab/src/catalog.rs`
  - Add catalog entries for new device/log CLI/MCP surfaces when introduced.
- Modify: `crates/lab/src/registry.rs`
  - Register any new master-only MCP tools/resources for fleet device and logs actions.
- Modify: `crates/lab/src/mcp/server.rs`
  - Gate fleet MCP tools to master-only runtime state.
- Modify: `crates/lab/src/mcp/resources.rs`
  - Add any fleet resources introduced by the new device/log surface.
- Modify: `crates/lab/src/oauth/local_relay.rs`
  - Expose a reusable invocation entrypoint the device runtime can call without starting a standalone CLI-only path.
- Modify: `docs/README.md`
- Modify: `docs/CONFIG.md`
- Modify: `docs/CLI.md`
- Modify: `docs/OAUTH.md`
- Modify: `docs/OPERATIONS.md`
- Modify: `docs/TRANSPORT.md`
- Modify: `docs/MCP.md`
- Modify: `docs/ARCH.md`
- Modify: `docs/OBSERVABILITY.md`
- Modify: `docs/ERRORS.md`
- Modify: `docs/GATEWAY.md`

### New Rust files to create

- Create: `crates/lab/src/device.rs`
  - Top-level module exports.
- Create: `crates/lab/src/device/identity.rs`
  - Resolve local hostname, current role, and master target.
- Create: `crates/lab/src/device/config_scan.rs`
  - Scan and parse `~/.claude.json`, `~/.codex/config.toml`, and `~/.gemini/settings.json`.
- Create: `crates/lab/src/device/checkin.rs`
  - Typed hello/status/metadata payloads.
- Create: `crates/lab/src/device/log_event.rs`
  - Normalized log event schema.
- Create: `crates/lab/src/device/queue.rs`
  - Durable local outbound queue.
- Create: `crates/lab/src/device/master_client.rs`
  - Non-master HTTP client for `/v1/device/*` uploads.
- Create: `crates/lab/src/device/runtime.rs`
  - Runtime bootstrap, background loops, and master/non-master orchestration.
- Create: `crates/lab/src/device/store.rs`
  - Master-owned in-process store for device inventory, last-seen, metadata, and ingested logs.
- Create: `crates/lab/src/device/oauth.rs`
  - Device-runtime wrapper around the existing local OAuth relay capability.
- Create: `crates/lab/src/device/log_collect.rs`
  - Linux log collection and normalization entrypoints.
- Create: `crates/lab/src/api/device.rs`
  - Route builder for `/v1/device/*`.
- Create: `crates/lab/src/api/device/hello.rs`
- Create: `crates/lab/src/api/device/status.rs`
- Create: `crates/lab/src/api/device/metadata.rs`
- Create: `crates/lab/src/api/device/syslog.rs`
- Create: `crates/lab/src/api/device/oauth.rs`
- Create: `crates/lab/src/cli/device.rs`
  - Master-routed fleet device commands.
- Create: `crates/lab/src/cli/logs.rs`
  - Master-routed fleet log commands.

### New tests to create

- Create: `crates/lab/tests/device_config.rs`
- Create: `crates/lab/tests/device_identity.rs`
- Create: `crates/lab/tests/device_scan.rs`
- Create: `crates/lab/tests/device_queue.rs`
- Create: `crates/lab/tests/device_api.rs`
- Create: `crates/lab/tests/device_runtime.rs`
- Create: `crates/lab/tests/device_master_only.rs`
- Create: `crates/lab/tests/device_cli.rs`

### New docs to create

- Create: `docs/DEVICE_RUNTIME.md`
- Create: `docs/FLEET_LOGS.md`
- Create: `docs/DEPLOY.md`

## Task 1: Add device config and master resolution

**Files:**
- Modify: `crates/lab/src/config.rs`
- Create: `crates/lab/src/device.rs`
- Create: `crates/lab/src/device/identity.rs`
- Test: `crates/lab/tests/device_config.rs`
- Test: `crates/lab/tests/device_identity.rs`

- [ ] **Step 1: Write the failing config tests**

```rust
#[test]
fn parses_device_master_config_block() {
    let raw = r#"
        [device]
        master = "tootie"
    "#;

    let parsed: lab::config::LabConfig = toml::from_str(raw).unwrap();
    assert_eq!(parsed.device.as_ref().unwrap().master.as_deref(), Some("tootie"));
}

#[test]
fn defaults_device_config_when_block_missing() {
    let parsed: lab::config::LabConfig = toml::from_str("").unwrap();
    assert!(parsed.device.is_none() || parsed.device.as_ref().unwrap().master.is_none());
}
```

- [ ] **Step 2: Write the failing identity tests**

```rust
#[test]
fn resolves_master_role_when_master_matches_local_hostname() {
    let resolved = resolve_runtime_role("tootie", Some("tootie")).unwrap();
    assert!(matches!(resolved.role, DeviceRole::Master));
}

#[test]
fn resolves_non_master_role_when_master_differs_from_local_hostname() {
    let resolved = resolve_runtime_role("dookie", Some("tootie")).unwrap();
    assert!(matches!(resolved.role, DeviceRole::NonMaster));
    assert_eq!(resolved.master_host, "tootie");
}

#[test]
fn defaults_first_device_to_master_when_master_is_missing() {
    let resolved = resolve_runtime_role("tootie", None).unwrap();
    assert!(matches!(resolved.role, DeviceRole::Master));
    assert_eq!(resolved.master_host, "tootie");
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test --test device_config --test device_identity --all-features`

Expected: FAIL with missing `device` config fields and unresolved `device::identity` module/functions.

- [ ] **Step 4: Add the config model**

Implement in `crates/lab/src/config.rs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevicePreferences {
    #[serde(default)]
    pub master: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRole {
    Master,
    NonMaster,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDeviceRuntime {
    pub local_host: String,
    pub master_host: String,
    pub role: DeviceRole,
}
```

Add to `LabConfig`:

```rust
#[serde(default)]
pub device: Option<DevicePreferences>,
```

- [ ] **Step 5: Add the identity helper**

Implement in `crates/lab/src/device/identity.rs`:

```rust
pub fn resolve_runtime_role(
    local_host: &str,
    configured_master: Option<&str>,
) -> anyhow::Result<ResolvedDeviceRuntime> {
    let master_host = configured_master.unwrap_or(local_host).to_string();
    let role = if master_host == local_host {
        DeviceRole::Master
    } else {
        DeviceRole::NonMaster
    };

    Ok(ResolvedDeviceRuntime {
        local_host: local_host.to_string(),
        master_host,
        role,
    })
}
```

- [ ] **Step 6: Export the device module**

Update `crates/lab/src/device.rs`:

```rust
pub mod identity;
```

Update `crates/lab/src/main.rs` or the nearest module root if needed so the new module compiles.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --test device_config --test device_identity --all-features`

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/config.rs crates/lab/src/device.rs crates/lab/src/device/identity.rs crates/lab/tests/device_config.rs crates/lab/tests/device_identity.rs
git commit -m "feat: add master device config resolution"
```

## Task 2: Add typed device payloads and master fleet store

**Files:**
- Create: `crates/lab/src/device/checkin.rs`
- Create: `crates/lab/src/device/store.rs`
- Modify: `crates/lab/src/api/state.rs`
- Test: `crates/lab/tests/device_runtime.rs`

- [ ] **Step 1: Write the failing fleet store tests**

```rust
#[tokio::test]
async fn device_store_tracks_last_seen_status_and_metadata() {
    let store = DeviceFleetStore::default();
    store.record_hello(DeviceHello {
        device_id: "tootie".into(),
        role: "master".into(),
        version: "1.0.0".into(),
    }).await;

    store.record_status(DeviceStatus {
        device_id: "tootie".into(),
        connected: true,
        cpu_percent: Some(3.5),
        memory_used_bytes: Some(1024),
        storage_used_bytes: Some(2048),
        os: Some("linux".into()),
        ips: vec!["100.64.0.1".into()],
    }).await;

    let snapshot = store.device("tootie").await.unwrap();
    assert!(snapshot.connected);
    assert_eq!(snapshot.device_id, "tootie");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test device_runtime --all-features`

Expected: FAIL with missing `DeviceFleetStore` and payload types.

- [ ] **Step 3: Add typed payload structs**

Implement in `crates/lab/src/device/checkin.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceHello {
    pub device_id: String,
    pub role: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub device_id: String,
    pub connected: bool,
    pub cpu_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    pub storage_used_bytes: Option<u64>,
    pub os: Option<String>,
    pub ips: Vec<String>,
}
```

- [ ] **Step 4: Add the in-memory master store**

Implement in `crates/lab/src/device/store.rs`:

```rust
#[derive(Debug, Clone, Default)]
pub struct DeviceFleetStore {
    inner: Arc<RwLock<BTreeMap<String, DeviceSnapshot>>>,
}

#[derive(Debug, Clone)]
pub struct DeviceSnapshot {
    pub device_id: String,
    pub connected: bool,
    pub last_seen: std::time::SystemTime,
    pub role: Option<String>,
    pub status: Option<DeviceStatus>,
}
```

Provide async methods:
- `record_hello`
- `record_status`
- `device`
- `list_devices`

- [ ] **Step 5: Thread the store into `AppState`**

Extend `crates/lab/src/api/state.rs` with:

```rust
pub device_store: Option<Arc<crate::device::store::DeviceFleetStore>>,

pub fn with_device_store(
    mut self,
    store: Arc<crate::device::store::DeviceFleetStore>,
) -> Self {
    self.device_store = Some(store);
    self
}
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test --test device_runtime --all-features`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/device/checkin.rs crates/lab/src/device/store.rs crates/lab/src/api/state.rs crates/lab/tests/device_runtime.rs
git commit -m "feat: add device fleet state store"
```

## Task 3: Add AI CLI MCP config discovery

**Files:**
- Create: `crates/lab/src/device/config_scan.rs`
- Modify: `crates/lab/src/device.rs`
- Test: `crates/lab/tests/device_scan.rs`

- [ ] **Step 1: Write the failing discovery tests**

```rust
#[test]
fn scans_claude_codex_and_gemini_configs_when_present() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join(".codex")).unwrap();
    std::fs::write(
        temp.path().join(".claude.json"),
        r#"{"mcpServers":{"lab":{"command":"lab","args":["serve"]}}}"#,
    ).unwrap();
    std::fs::write(
        temp.path().join(".codex/config.toml"),
        r#"[mcp_servers.lab]
command = "lab"
"#,
    ).unwrap();
    std::fs::create_dir_all(temp.path().join(".gemini")).unwrap();
    std::fs::write(
        temp.path().join(".gemini/settings.json"),
        r#"{"mcpServers":{"lab":{"url":"http://127.0.0.1:8765/mcp"}}}"#,
    ).unwrap();

    let inventory = discover_ai_cli_configs(temp.path()).unwrap();
    assert_eq!(inventory.len(), 3);
    assert!(inventory.iter().all(|entry| !entry.content_hash.is_empty()));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test device_scan --all-features`

Expected: FAIL with missing discovery module and helpers.

- [ ] **Step 3: Implement tolerant parsers**

Implement in `crates/lab/src/device/config_scan.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredMcpConfigFile {
    pub source: String,
    pub path: PathBuf,
    pub modified_unix_secs: u64,
    pub content_hash: String,
    pub servers: BTreeMap<String, serde_json::Value>,
}

pub fn discover_ai_cli_configs(home: &Path) -> anyhow::Result<Vec<DiscoveredMcpConfigFile>> {
    // probe ~/.claude.json
    // probe ~/.codex/config.toml
    // probe ~/.gemini/settings.json
    // parse only the mcp server sections
    // ignore unknown keys
}
```

Use:
- `serde_json` for Claude/Gemini
- `toml` for Codex
- `sha2::Sha256` for content hashing

- [ ] **Step 4: Export the module**

Update `crates/lab/src/device.rs`:

```rust
pub mod config_scan;
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test device_scan --all-features`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/device.rs crates/lab/src/device/config_scan.rs crates/lab/tests/device_scan.rs
git commit -m "feat: add ai cli mcp config discovery"
```

## Task 4: Add durable outbound queue and normalized log event schema

**Files:**
- Create: `crates/lab/src/device/log_event.rs`
- Create: `crates/lab/src/device/queue.rs`
- Test: `crates/lab/tests/device_queue.rs`

- [ ] **Step 1: Write the failing queue tests**

```rust
#[tokio::test]
async fn queue_persists_and_reloads_entries() {
    let temp = tempfile::tempdir().unwrap();
    let queue = DeviceOutboundQueue::open(temp.path().join("queue.jsonl")).await.unwrap();

    queue.push(QueuedEnvelope::status(serde_json::json!({"device_id":"tootie"}))).await.unwrap();
    drop(queue);

    let reopened = DeviceOutboundQueue::open(temp.path().join("queue.jsonl")).await.unwrap();
    let drained = reopened.drain_batch(10).await.unwrap();
    assert_eq!(drained.len(), 1);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test device_queue --all-features`

Expected: FAIL with missing queue and log event types.

- [ ] **Step 3: Add the normalized log schema**

Implement in `crates/lab/src/device/log_event.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceLogEvent {
    pub device_id: String,
    pub source: String,
    pub timestamp_unix_ms: i64,
    pub level: Option<String>,
    pub message: String,
    pub fields: serde_json::Map<String, serde_json::Value>,
}
```

- [ ] **Step 4: Add the durable queue**

Implement in `crates/lab/src/device/queue.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedEnvelope {
    pub kind: String,
    pub payload: serde_json::Value,
}

pub struct DeviceOutboundQueue { /* file-backed jsonl queue */ }
```

Required methods:
- `open(path)`
- `push(envelope)`
- `drain_batch(limit)`
- `ack_drained(count)`

Implementation rule:
- keep v1 simple with JSONL rewrite after ack
- do not introduce sqlite yet

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test device_queue --all-features`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/device/log_event.rs crates/lab/src/device/queue.rs crates/lab/tests/device_queue.rs
git commit -m "feat: add device outbound queue"
```

## Task 5: Add `/v1/device/*` API surface on the master

**Files:**
- Modify: `crates/lab/src/api.rs`
- Create: `crates/lab/src/api/device.rs`
- Create: `crates/lab/src/api/device/hello.rs`
- Create: `crates/lab/src/api/device/status.rs`
- Create: `crates/lab/src/api/device/metadata.rs`
- Create: `crates/lab/src/api/device/syslog.rs`
- Modify: `crates/lab/src/api/router.rs`
- Test: `crates/lab/tests/device_api.rs`

- [ ] **Step 1: Write the failing API tests**

```rust
#[tokio::test]
async fn hello_endpoint_updates_master_store() {
    let app = test_device_router();
    let response = app
        .oneshot(hello_request(r#"{"device_id":"dookie","role":"non-master","version":"1.0.0"}"#))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn syslog_batch_endpoint_accepts_normalized_events() {
    let app = test_device_router();
    let response = app
        .oneshot(syslog_request(r#"{"device_id":"dookie","events":[{"device_id":"dookie","source":"journald","timestamp_unix_ms":1,"message":"hello","fields":{}}]}"#))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test device_api --all-features`

Expected: FAIL because `/v1/device/*` routes are not mounted.

- [ ] **Step 3: Create the route modules**

Implement:
- `crates/lab/src/api/device.rs`
- `crates/lab/src/api/device/hello.rs`
- `crates/lab/src/api/device/status.rs`
- `crates/lab/src/api/device/metadata.rs`
- `crates/lab/src/api/device/syslog.rs`

Route contract:
- `POST /v1/device/hello`
- `POST /v1/device/status`
- `POST /v1/device/metadata`
- `POST /v1/device/syslog/batch`

Return `200 OK` with a small JSON ack:

```json
{ "ok": true }
```

- [ ] **Step 4: Mount the device router**

Update `crates/lab/src/api.rs`:

```rust
pub mod device;
```

Update `crates/lab/src/api/router.rs` inside `build_v1_router`:

```rust
.nest("/device", super::device::routes(state.clone()))
```

- [ ] **Step 5: Use the fleet store from handlers**

Each handler must fail clearly if `state.device_store` is missing:

```rust
let store = state.device_store.clone().ok_or_else(|| {
    ToolError::internal_message("device store is not configured")
})?;
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test device_api --all-features`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/api.rs crates/lab/src/api/router.rs crates/lab/src/api/device.rs crates/lab/src/api/device crates/lab/tests/device_api.rs
git commit -m "feat: add master device ingest api"
```

## Task 6: Add non-master -> master client and runtime loops

**Files:**
- Create: `crates/lab/src/device/master_client.rs`
- Create: `crates/lab/src/device/runtime.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Test: `crates/lab/tests/device_runtime.rs`

- [ ] **Step 1: Write the failing runtime tests**

```rust
#[tokio::test]
async fn non_master_runtime_posts_hello_to_master() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/v1/device/hello"))
        .respond_with(wiremock::ResponseTemplate::new(200))
        .mount(&server)
        .await;

    let runtime = DeviceRuntime::non_master_for_test("dookie", server.uri());
    runtime.send_initial_hello().await.unwrap();
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test device_runtime --all-features`

Expected: FAIL with missing `DeviceRuntime` and `MasterClient`.

- [ ] **Step 3: Implement the master client**

Implement in `crates/lab/src/device/master_client.rs`:

```rust
pub struct MasterClient {
    http: reqwest::Client,
    base_url: String,
}

impl MasterClient {
    pub async fn post_hello(&self, payload: &DeviceHello) -> anyhow::Result<()>;
    pub async fn post_status(&self, payload: &DeviceStatus) -> anyhow::Result<()>;
    pub async fn post_metadata(&self, payload: &serde_json::Value) -> anyhow::Result<()>;
    pub async fn post_syslog_batch(&self, payload: &serde_json::Value) -> anyhow::Result<()>;
}
```

- [ ] **Step 4: Implement the runtime skeleton**

Implement in `crates/lab/src/device/runtime.rs`:

```rust
pub struct DeviceRuntime {
    resolved: ResolvedDeviceRuntime,
    master_client: Option<MasterClient>,
}

impl DeviceRuntime {
    pub async fn send_initial_hello(&self) -> anyhow::Result<()> { /* non-master only */ }
}
```

- [ ] **Step 5: Start the runtime from `serve`**

Update `crates/lab/src/cli/serve.rs` to:
- resolve local hostname
- resolve current role from config
- create a shared `DeviceFleetStore`
- attach the store to `AppState`
- initialize `DeviceRuntime`

Do not start background loops yet beyond the first hello/status path needed by tests.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test device_runtime --all-features`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/device/master_client.rs crates/lab/src/device/runtime.rs crates/lab/src/cli/serve.rs crates/lab/tests/device_runtime.rs
git commit -m "feat: add device runtime master client"
```

## Task 7: Make Web/API/MCP surfaces master-only

**Files:**
- Modify: `crates/lab/src/cli/serve.rs`
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/api/web.rs`
- Modify: `crates/lab/src/mcp/server.rs`
- Test: `crates/lab/tests/device_master_only.rs`

- [ ] **Step 1: Write the failing master-only tests**

```rust
#[tokio::test]
async fn non_master_router_rejects_gateway_api_surface() {
    let app = test_non_master_router();
    let response = app.oneshot(gateway_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn non_master_router_rejects_web_ui_surface() {
    let app = test_non_master_router();
    let response = app.oneshot(web_request()).await.unwrap();
    assert!(matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::FORBIDDEN));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test device_master_only --all-features`

Expected: FAIL because all current surfaces are still mounted everywhere.

- [ ] **Step 3: Add role-aware state**

Add a `device_role` field to `AppState`:

```rust
pub device_role: Option<crate::config::DeviceRole>,
```

and a builder:

```rust
pub fn with_device_role(mut self, role: crate::config::DeviceRole) -> Self
```

- [ ] **Step 4: Gate router mounting**

Update `crates/lab/src/api/router.rs` so that:
- `/v1/device/*`
- `/health`
- `/ready`
remain available on all devices

and:
- `/v1/<service>` operator routes
- `/mcp`
- static web assets
only mount when role is `Master`

- [ ] **Step 5: Gate Web UI behavior**

Update `crates/lab/src/api/web.rs` so a non-master instance returns a clear non-master response instead of serving the app shell.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test device_master_only --all-features`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/cli/serve.rs crates/lab/src/api/router.rs crates/lab/src/api/web.rs crates/lab/src/mcp/server.rs crates/lab/tests/device_master_only.rs
git commit -m "feat: gate central surfaces to master"
```

## Task 8: Report AI CLI config inventory and periodic status to the master

**Files:**
- Modify: `crates/lab/src/device/runtime.rs`
- Modify: `crates/lab/src/device/master_client.rs`
- Modify: `crates/lab/src/device/checkin.rs`
- Modify: `crates/lab/src/api/device/metadata.rs`
- Test: `crates/lab/tests/device_runtime.rs`

- [ ] **Step 1: Write the failing metadata upload test**

```rust
#[tokio::test]
async fn non_master_runtime_uploads_discovered_ai_cli_inventory() {
    // mock /v1/device/metadata and assert one request body contains source path + content hash
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test device_runtime --all-features`

Expected: FAIL because metadata upload loop does not exist.

- [ ] **Step 3: Add metadata payloads**

Extend `crates/lab/src/device/checkin.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceMetadataUpload {
    pub device_id: String,
    pub discovered_configs: Vec<DiscoveredMcpConfigFile>,
}
```

- [ ] **Step 4: Add the runtime upload path**

Update `crates/lab/src/device/runtime.rs` so the non-master startup path:
- scans the current home directory
- builds `DeviceMetadataUpload`
- posts it to `/v1/device/metadata`

Keep v1 simple: upload once at startup, then add periodic refresh later if needed.

- [ ] **Step 5: Persist metadata in the master store**

Update `crates/lab/src/device/store.rs` and `crates/lab/src/api/device/metadata.rs` to keep the uploaded config inventory on the device snapshot.

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test --test device_runtime --all-features`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/device/runtime.rs crates/lab/src/device/master_client.rs crates/lab/src/device/checkin.rs crates/lab/src/device/store.rs crates/lab/src/api/device/metadata.rs crates/lab/tests/device_runtime.rs
git commit -m "feat: report device ai cli config inventory"
```

## Task 9: Add log collection, queue flush, and master log ingest

**Files:**
- Create: `crates/lab/src/device/log_collect.rs`
- Modify: `crates/lab/src/device/runtime.rs`
- Modify: `crates/lab/src/device/store.rs`
- Modify: `crates/lab/src/api/device/syslog.rs`
- Test: `crates/lab/tests/device_runtime.rs`
- Test: `crates/lab/tests/device_api.rs`

- [ ] **Step 1: Write the failing log ingest tests**

```rust
#[tokio::test]
async fn master_store_keeps_uploaded_logs_by_device() {
    let store = DeviceFleetStore::default();
    store.record_logs("dookie", vec![DeviceLogEvent {
        device_id: "dookie".into(),
        source: "journald".into(),
        timestamp_unix_ms: 1,
        level: Some("info".into()),
        message: "hello".into(),
        fields: Default::default(),
    }]).await;

    assert_eq!(store.logs_for_device("dookie").await.len(), 1);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test device_runtime --test device_api --all-features`

Expected: FAIL because master log persistence and runtime flush do not exist.

- [ ] **Step 3: Add minimal Linux log collector**

Implement in `crates/lab/src/device/log_collect.rs` a minimal collector abstraction:

```rust
pub fn collect_bootstrap_logs(device_id: &str) -> anyhow::Result<Vec<DeviceLogEvent>> {
    Ok(Vec::new())
}
```

Keep v1 intentionally narrow:
- no journald cursor protocol yet
- produce an empty vector by default behind a stable abstraction
- unblock queue + ingest plumbing first

- [ ] **Step 4: Add runtime queue flush**

Update `crates/lab/src/device/runtime.rs`:
- enqueue log/status envelopes
- POST queued syslog batches to `/v1/device/syslog/batch`
- ack queue entries only after `200 OK`

- [ ] **Step 5: Persist logs on the master**

Update `crates/lab/src/device/store.rs` with:
- `record_logs(device_id, events)`
- `logs_for_device(device_id)`

Update `crates/lab/src/api/device/syslog.rs` to call `record_logs`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test device_runtime --test device_api --all-features`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/device/log_collect.rs crates/lab/src/device/runtime.rs crates/lab/src/device/store.rs crates/lab/src/api/device/syslog.rs crates/lab/tests/device_runtime.rs crates/lab/tests/device_api.rs
git commit -m "feat: add device log queue flush and ingest"
```

## Task 10: Integrate OAuth relay as a device capability

**Files:**
- Create: `crates/lab/src/device/oauth.rs`
- Modify: `crates/lab/src/oauth/local_relay.rs`
- Create: `crates/lab/src/api/device/oauth.rs`
- Modify: `crates/lab/src/api/device.rs`
- Test: `crates/lab/tests/device_api.rs`

- [ ] **Step 1: Write the failing OAuth device capability test**

```rust
#[tokio::test]
async fn device_oauth_route_calls_runtime_wrapper() {
    // exercise POST /v1/device/oauth/relay/start against a fake wrapper
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test device_api --all-features`

Expected: FAIL because the OAuth device route does not exist.

- [ ] **Step 3: Extract a reusable runtime wrapper**

Implement in `crates/lab/src/device/oauth.rs`:

```rust
pub async fn start_local_oauth_relay(
    bind_addr: std::net::SocketAddr,
    resolved_target: crate::oauth::target::ResolvedTarget,
    request_timeout: std::time::Duration,
) -> anyhow::Result<()> {
    crate::oauth::local_relay::run_local_relay(LocalRelayConfig {
        bind_addr,
        resolved_target,
        request_timeout,
    }).await.map_err(anyhow::Error::from)
}
```

- [ ] **Step 4: Add the device route**

Implement `POST /v1/device/oauth/relay/start` in `crates/lab/src/api/device/oauth.rs`.

Keep v1 response minimal:

```json
{ "ok": true }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test --test device_api --all-features`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/device/oauth.rs crates/lab/src/oauth/local_relay.rs crates/lab/src/api/device/oauth.rs crates/lab/src/api/device.rs crates/lab/tests/device_api.rs
git commit -m "feat: expose oauth relay as device capability"
```

## Task 11: Add master-routed CLI surfaces for devices and fleet logs

**Files:**
- Create: `crates/lab/src/cli/device.rs`
- Create: `crates/lab/src/cli/logs.rs`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/catalog.rs`
- Test: `crates/lab/tests/device_cli.rs`

- [ ] **Step 1: Write the failing CLI tests**

```rust
#[tokio::test]
async fn device_list_command_reads_from_master_api() {
    // wiremock master + run CLI helper against it
}

#[tokio::test]
async fn logs_search_command_reads_from_master_api() {
    // wiremock master + assert request path/body
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test device_cli --all-features`

Expected: FAIL because the CLI groups do not exist.

- [ ] **Step 3: Implement thin CLI shims**

Create `crates/lab/src/cli/device.rs` and `crates/lab/src/cli/logs.rs` with thin request/formatting code only.

Commands:
- `lab device list`
- `lab device get <device_id>`
- `lab logs search --device <device_id> --query <term>`

Do not add deploy yet in this slice.

- [ ] **Step 4: Register the CLI groups**

Update `crates/lab/src/cli.rs`:

```rust
pub mod device;
pub mod logs;
```

Add enum variants and dispatch arms.

- [ ] **Step 5: Update the catalog**

Add matching discoverable catalog entries in `crates/lab/src/catalog.rs`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --test device_cli --all-features`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/cli/device.rs crates/lab/src/cli/logs.rs crates/lab/src/cli.rs crates/lab/src/catalog.rs crates/lab/tests/device_cli.rs
git commit -m "feat: add master-routed device and logs cli"
```

## Task 12: Update docs and final verification

**Files:**
- Create: `docs/DEVICE_RUNTIME.md`
- Create: `docs/FLEET_LOGS.md`
- Create: `docs/DEPLOY.md`
- Modify: `docs/README.md`
- Modify: `docs/CONFIG.md`
- Modify: `docs/CLI.md`
- Modify: `docs/OAUTH.md`
- Modify: `docs/OPERATIONS.md`
- Modify: `docs/TRANSPORT.md`
- Modify: `docs/MCP.md`
- Modify: `docs/ARCH.md`
- Modify: `docs/OBSERVABILITY.md`
- Modify: `docs/ERRORS.md`
- Modify: `docs/GATEWAY.md`

- [ ] **Step 1: Write the new docs**

Required contents:
- `docs/DEVICE_RUNTIME.md`
  - master/non-master model
  - startup behavior
  - `/v1/device/*` routes
  - local queue behavior
- `docs/FLEET_LOGS.md`
  - device -> master log flow
  - normalized event model
  - storage and query behavior
- `docs/DEPLOY.md`
  - future-facing `lab deploy` direction
  - Linux `x86_64` assumption
  - SSH/Tailscale onboarding direction

- [ ] **Step 2: Update existing docs**

Make each doc consistent with the new master model:
- `docs/CONFIG.md`: add `[device] master = "tootie"`
- `docs/CLI.md`: document `lab device` and `lab logs`
- `docs/OAUTH.md`: explain that relay is a device capability
- `docs/TRANSPORT.md`: add `/v1/device/*`
- `docs/OBSERVABILITY.md`: add device upload, queue, and master ingest boundaries
- `docs/ERRORS.md`: add device-runtime error envelope kinds if introduced
- `docs/GATEWAY.md`: clarify this is not the same as the new `master` runtime

- [ ] **Step 3: Run the targeted test suites**

Run:

```bash
cargo test --test device_config --test device_identity --test device_scan --test device_queue --test device_api --test device_runtime --test device_master_only --test device_cli --all-features
```

Expected: all PASS.

- [ ] **Step 4: Run the crate-wide verification**

Run:

```bash
cargo test -p lab --all-features
cargo build -p lab --all-features
```

Expected: PASS.

- [ ] **Step 5: Run formatting and lint**

Run:

```bash
cargo fmt --all --check
cargo clippy -p lab --all-features -- -D warnings
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add docs/DEVICE_RUNTIME.md docs/FLEET_LOGS.md docs/DEPLOY.md docs/README.md docs/CONFIG.md docs/CLI.md docs/OAUTH.md docs/OPERATIONS.md docs/TRANSPORT.md docs/MCP.md docs/ARCH.md docs/OBSERVABILITY.md docs/ERRORS.md docs/GATEWAY.md
git commit -m "docs: document device runtime master model"
```

## Notes for the implementing agent

- Do not put this work under `dispatch/gateway/`. That module is for upstream MCP gateway management and already has a different meaning.
- Use `master` terminology in new code and docs. Keep `gateway` reserved for the existing upstream MCP gateway feature.
- Prefer simple first implementations:
  - in-memory master store first
  - file-backed JSONL queue first
  - startup metadata upload first
  - minimal log collector abstraction first
- Do not add Tailscale device discovery to the runtime path in this plan.
- Do not add `lab deploy` in this plan; document it in `docs/DEPLOY.md` only.
- Keep CLI and API layers thin. Business logic belongs in `crates/lab/src/device/`.
