# Nodes Surface Unification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace all public `device` and `fleet` terminology with a single `nodes` surface across CLI, HTTP, websocket transport, config, logs, docs, and OpenAPI with a clean break and no backward-compatibility aliases.

**Architecture:** This is a public-surface rename with strict scope control. Public product surfaces become `node` or `nodes` only, with `node_id` as the canonical external identifier everywhere. Internal module paths may remain under `device/` temporarily, but no public CLI command, route, websocket method, config section, OpenAPI path, response body, or human-facing message may expose `device` or `fleet` after this plan lands.

**Tech Stack:** Rust 2024, `clap`, `axum`, `rmcp`, existing device runtime/websocket transport, existing OpenAPI builder, Markdown docs, existing test suites under `crates/lab/tests`.

---

## Non-Goals And Hard Contracts

### Hard contracts
- External identifier is `node_id` everywhere.
- `device_id` must not appear in fresh product output.
- `lab device` must cease to exist.
- `lab fleet` must cease to exist.
- Canonical HTTP routes are `/v1/nodes`, `/v1/nodes/{node_id}`, `/v1/nodes/enrollments`, `/v1/nodes/logs/search`, `/v1/nodes/hello`, `/v1/nodes/oauth/relay/start`, `/v1/nodes/syslog/batch`, and `/v1/nodes/ws`.
- Canonical websocket methods are `nodes/status.push`, `nodes/metadata.push`, and `nodes/log.event`.
- Canonical initialize metadata key is `lab.node_id`.
- Canonical config section is `[node]` with `controller = ...`.
- OpenAPI must describe only the canonical `nodes` surface.
- Serialization must follow `docs/design/SERIALIZATION.md`: `lab-apis` owns SDK wire types, `lab` owns product-surface envelopes and presentation.

### Non-goals
- No backward compatibility aliases.
- No deprecation telemetry.
- No on-disk queue migration layer.
- No internal filesystem/module rename from `device/` to `node/` in this plan.
- No deploy bugfix work unrelated to the naming unification.

---

## File Structure

### Public CLI surface
- Modify: `crates/lab/src/cli/device.rs`
  - Replace the public command group and all outward-facing strings with `nodes` / `node_id` language.
- Modify: `crates/lab/src/cli.rs`
  - Re-register the top-level command tree so `nodes` is canonical and `device` no longer parses.
- Modify: `crates/lab/src/main.rs`
  - Ensure top-level help text prefers `nodes` only.

### HTTP/API surface
- Modify: `crates/lab/src/api/device.rs`
  - Expose canonical `/nodes` resource routes and rename outward-facing helpers/messages to `node` language.
- Modify: `crates/lab/src/api/device/fleet.rs`
  - Rename outward-facing API/log/error text, websocket methods, and initialize metadata from `fleet` / `device` to `nodes` / `node`.
- Modify: `crates/lab/src/api/router.rs`
  - Mount canonical `/v1/nodes/*` and `/v1/nodes/ws` routes only.
- Modify: `crates/lab/src/api/state.rs`
  - Update externally visible labels and docstrings.
- Modify: `crates/lab/src/api/openapi.rs`
  - Ensure generated OpenAPI only contains canonical `nodes` routes, examples, and field names.

### Runtime and client naming
- Modify: `crates/lab/src/device/master_client.rs`
  - Resolve controller host using canonical config accessors and `nodes` path semantics.
- Modify: `crates/lab-apis/src/device_runtime/client.rs`
  - Point SDK client calls at canonical `/v1/nodes*` endpoints.
- Modify: `crates/lab/src/device/ws_client.rs`
  - Derive websocket URL from `/v1/nodes/ws` and emit only canonical websocket method names and metadata keys.
- Modify: `crates/lab/src/device/runtime.rs`
  - Emit `node_id` payloads and `node` language in runtime-generated product data.
- Modify: `crates/lab/src/device/identity.rs`
  - Rename public-facing identifiers/messages from `device_id` to `node_id` where they cross a product boundary.
- Modify: `crates/lab/src/device/store.rs`
  - Shape public JSON output to use `node_id` only.
- Modify: `crates/lab/src/device/checkin.rs`
  - Rename product-surface structs/serde fields to `node_id` where they are part of product API or websocket contracts.
- Modify: `crates/lab/src/device/queue.rs`
  - Keep queue internals minimal; emit only canonical payloads without compatibility shims.

### Configuration and shared types
- Modify: `crates/lab/src/config.rs`
  - Introduce canonical `[node] controller = ...` config and remove public reliance on `[device] master`.
- Modify: `docs/CONFIG.md`
  - Document the canonical `[node]` section only.
- Reference: `docs/design/SERIALIZATION.md`
  - Keep SDK-wire vs product-surface boundaries correct while renaming fields.

### Docs and product references
- Modify: `docs/DEPLOY.md`
- Modify: `docs/DEVICE_RUNTIME.md`
- Modify: `docs/OPERATIONS.md`
- Modify: `docs/README.md`
- Create: `docs/NODES.md`
  - Rewrite operator-facing terminology to `nodes` only.

### Tests
- Modify: `crates/lab/tests/device_cli.rs`
- Modify: `crates/lab/tests/device_api.rs`
- Modify: `crates/lab/tests/device_runtime.rs`
- Modify: `crates/lab/tests/device_scan.rs`
- Modify: `crates/lab/src/api/device/fleet.rs` test module
- Modify: `crates/lab/src/config.rs` test module
  - Update expectations to canonical `nodes` surface only. Remove legacy alias expectations.

---

### Task 1: Define the clean-break nodes contract in docs

**Files:**
- Create: `docs/NODES.md`
- Modify: `docs/README.md`
- Modify: `docs/CONFIG.md`
- Reference: `docs/design/SERIALIZATION.md`

- [ ] **Step 1: Write the canonical vocabulary section**

```md
Canonical terms:
- node: one managed runtime
- nodes: operator control surface
- controller: coordinating node
- node_id: the only public runtime identifier
```

- [ ] **Step 2: Write the clean-break policy explicitly**

```md
This rename is a clean break:
- `device` is not a supported public term
- `fleet` is not a supported public term
- old CLI commands, old routes, old websocket methods, and old config names are removed
```

- [ ] **Step 3: Document the target config shape**

```toml
[node]
controller = "100.88.16.79"

[mcp]
host = "127.0.0.1"
port = 8765
```

- [ ] **Step 4: Add serialization boundary guidance**

```md
`lab-apis` keeps SDK wire types.
`lab` owns product-surface routes, envelopes, websocket payloads, and CLI JSON output.
```

- [ ] **Step 5: Commit**

```bash
git add docs/NODES.md docs/README.md docs/CONFIG.md
git commit -m "docs: define clean-break nodes contract"
```

### Task 2: Replace the CLI surface with `nodes`

**Files:**
- Modify: `crates/lab/src/cli/device.rs`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/main.rs`
- Modify: `crates/lab/tests/device_cli.rs`

- [ ] **Step 1: Write failing CLI parser/help assertions**

```rust
assert!(help_output.contains("nodes"));
assert!(!help_output.contains("device"));
```

- [ ] **Step 2: Rename the top-level command variant to `Nodes`**

```rust
pub enum Command {
    Nodes(device::DeviceArgs),
}
```

- [ ] **Step 3: Rename subcommand docs and arg names to `node` / `node_id`**

```rust
pub enum DeviceCommand {
    /// List all registered nodes visible from the controller.
    List,
    /// Get details for a specific node by `node_id`.
    Get { node_id: String },
}
```

- [ ] **Step 4: Remove `device` as a public CLI command**

```rust
// no alias = "device"
```

- [ ] **Step 5: Update CLI integration tests to expect canonical `/v1/nodes` requests**

```rust
.and(wiremock::matchers::path("/v1/nodes"))
.and(wiremock::matchers::path("/v1/nodes/enrollments"))
```

- [ ] **Step 6: Run focused CLI tests**

Run: `cargo test -p lab --test device_cli -- --nocapture`
Expected: CLI tests pass with only canonical `nodes` command and canonical `/v1/nodes*` requests

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/cli/device.rs crates/lab/src/cli.rs crates/lab/src/main.rs crates/lab/tests/device_cli.rs
git commit -m "feat: replace public device cli with nodes"
```

### Task 3: Replace the HTTP product API with canonical `/v1/nodes*` routes

**Files:**
- Modify: `crates/lab/src/api/device.rs`
- Modify: `crates/lab/src/api/device/fleet.rs`
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/tests/device_api.rs`

- [ ] **Step 1: Write failing route tests for canonical nodes paths**

```rust
let response = app.oneshot(Request::get("/v1/nodes/enrollments")).await?;
assert_eq!(response.status(), StatusCode::OK);
```

- [ ] **Step 2: Rename canonical resource routes from `/devices` to `/nodes`**

```rust
.route("/nodes", axum::routing::get(fleet::list_devices))
.route("/nodes/{node_id}", axum::routing::get(fleet::get_device))
```

- [ ] **Step 3: Move public node routes under `/v1/nodes` only**

```rust
let mut v1 = Router::new().nest("/nodes", super::device::routes(state.clone()));
```

- [ ] **Step 4: Move unauthenticated product routes to `/v1/nodes` only**

```rust
.nest("/v1/nodes", super::device::public_routes(state.clone()))
.route("/v1/nodes/ws", get(crate::api::device::fleet::websocket_upgrade))
```

- [ ] **Step 5: Rename request path params and outward-facing messages to `node_id` / `node`**

```rust
message: "node control queries are only available on the controller".to_string()
```

- [ ] **Step 6: Remove old `/v1/device/*` and `/v1/fleet/ws` route coverage from tests**

```rust
assert_eq!(app.oneshot(Request::get("/v1/device/enrollments")).await?.status(), StatusCode::NOT_FOUND);
```

- [ ] **Step 7: Run focused product API tests**

Run: `cargo test -p lab --test device_api -- --nocapture`
Expected: canonical `/v1/nodes*` routes pass and old `/v1/device*` routes no longer exist

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/api/device.rs crates/lab/src/api/device/fleet.rs crates/lab/src/api/router.rs crates/lab/tests/device_api.rs
git commit -m "feat: replace public device api with nodes routes"
```

### Task 4: Move REST clients and runtime traffic to canonical nodes endpoints

**Files:**
- Modify: `crates/lab-apis/src/device_runtime/client.rs`
- Modify: `crates/lab/src/device/master_client.rs`
- Modify: `crates/lab/src/device/runtime.rs`
- Modify: `crates/lab/tests/device_cli.rs`
- Modify: `crates/lab/tests/device_runtime.rs`

- [ ] **Step 1: Write failing client expectations for canonical nodes paths**

```rust
.and(wiremock::matchers::path("/v1/nodes"))
```

- [ ] **Step 2: Update SDK client paths to canonical nodes endpoints**

```rust
self.http.get_json("/v1/nodes")
self.http.get_json(&format!("/v1/nodes/{encoded_id}"))
self.http.get_json("/v1/nodes/enrollments")
self.http.post_json("/v1/nodes/logs/search", &request)
```

- [ ] **Step 3: Keep `MasterClient` thin and drive controller resolution through canonical config accessors**

```rust
let host = config.controller_host().unwrap_or_else(resolve_local_hostname)?;
```

- [ ] **Step 4: Emit canonical runtime payload keys**

```rust
let payload = serde_json::json!({
    "node_id": self.resolved.local_host.clone(),
    "events": events,
});
```

- [ ] **Step 5: Run focused client/runtime tests**

Run: `cargo test -p lab --test device_cli --test device_runtime -- --nocapture`
Expected: runtime queue payloads and client requests use canonical `node_id` and `/v1/nodes*`

- [ ] **Step 6: Commit**

```bash
git add crates/lab-apis/src/device_runtime/client.rs crates/lab/src/device/master_client.rs crates/lab/src/device/runtime.rs crates/lab/tests/device_cli.rs crates/lab/tests/device_runtime.rs
git commit -m "refactor: move node runtime traffic to canonical nodes endpoints"
```

### Task 5: Replace websocket transport names with canonical nodes wire contracts

**Files:**
- Modify: `crates/lab/src/device/ws_client.rs`
- Modify: `crates/lab/src/api/device/fleet.rs`
- Modify: `crates/lab/src/device/checkin.rs`
- Modify: `crates/lab/src/device/queue.rs`
- Modify: `crates/lab/src/api/device/fleet.rs` test module

- [ ] **Step 1: Write failing websocket tests for canonical path and methods**

```rust
assert_eq!(websocket_url_from_master_base("http://master:8765")?.path(), "/v1/nodes/ws");
assert_eq!(response["result"]["_meta"]["lab.node_id"], "node-1");
```

- [ ] **Step 2: Change websocket URL derivation to `/v1/nodes/ws`**

```rust
url.set_path("/v1/nodes/ws");
```

- [ ] **Step 3: Rename outbound websocket methods to canonical `nodes/*` names**

```rust
"method": "nodes/status.push"
"method": "nodes/metadata.push"
"method": "nodes/log.event"
```

- [ ] **Step 4: Rename initialize metadata to `lab.node_id`**

```rust
"lab.node_id": node_id,
```

- [ ] **Step 5: Update the server-side websocket dispatcher to accept only canonical methods**

```rust
match request.method.as_str() {
    "nodes/status.push" => { ... }
    "nodes/metadata.push" => { ... }
    "nodes/log.event" => { ... }
    other => error_response(...),
}
```

- [ ] **Step 6: Rename websocket payload structs and validation paths to `node_id`**

```rust
#[serde(rename = "lab.node_id")]
node_id: String,
```

- [ ] **Step 7: Remove legacy websocket route/method tests and replace them with canonical coverage**

```rust
.route("/v1/nodes/ws", get(websocket_upgrade))
```

- [ ] **Step 8: Run focused websocket tests**

Run: `cargo test -p lab api::device::fleet::tests -- --nocapture`
Expected: websocket tests pass using only `/v1/nodes/ws`, `nodes/*` methods, and `lab.node_id`

- [ ] **Step 9: Commit**

```bash
git add crates/lab/src/device/ws_client.rs crates/lab/src/api/device/fleet.rs crates/lab/src/device/checkin.rs crates/lab/src/device/queue.rs
git commit -m "refactor: replace fleet websocket contract with nodes"
```

### Task 6: Replace config with canonical `[node] controller`

**Files:**
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab/src/device/master_client.rs`
- Modify: `crates/lab/src/device/identity.rs`
- Modify: `crates/lab/src/device/runtime.rs`
- Modify: `crates/lab/src/config.rs` test module
- Modify: `crates/lab/tests/device_cli.rs`
- Modify: `docs/CONFIG.md`

- [ ] **Step 1: Write failing config tests for canonical `[node]` parsing**

```rust
let cfg = parse_config(r#"
[node]
controller = "100.88.16.79"
"#)?;
assert_eq!(cfg.node.as_ref().and_then(|n| n.controller.as_deref()), Some("100.88.16.79"));
```

- [ ] **Step 2: Add canonical `NodePreferences` and `LabConfig::controller_host()`**

```rust
pub struct NodePreferences {
    pub controller: Option<String>,
}
```

- [ ] **Step 3: Update runtime and client call sites to use canonical controller accessors**

```rust
let host = config.controller_host().unwrap_or_else(resolve_local_hostname)?;
let resolved = resolve_runtime_role(local_host, config.controller_host())?;
```

- [ ] **Step 4: Remove public config tests and docs that reference `[device].master`**

```rust
assert!(rendered_config.contains("[node]"));
assert!(!rendered_config.contains("[device]"));
```

- [ ] **Step 5: Run focused config tests**

Run: `cargo test -p lab config::tests -- --nocapture`
Expected: config tests pass for `[node].controller` and do not mention `[device].master`

- [ ] **Step 6: Run config-driven client tests**

Run: `cargo test -p lab --test device_cli -- --nocapture`
Expected: CLI tests pass using canonical node config helper setup

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/config.rs crates/lab/src/device/master_client.rs crates/lab/src/device/identity.rs crates/lab/src/device/runtime.rs crates/lab/tests/device_cli.rs docs/CONFIG.md
git commit -m "feat: replace device master config with node controller"
```

### Task 7: Replace public JSON, logs, and product wording with `node_id`

**Files:**
- Modify: `crates/lab/src/device/store.rs`
- Modify: `crates/lab/src/device/checkin.rs`
- Modify: `crates/lab/src/api/device/*.rs`
- Modify: `crates/lab/src/device/*.rs`
- Modify: `crates/lab/tests/device_api.rs`
- Modify: `crates/lab/tests/device_runtime.rs`

- [ ] **Step 1: Write failing assertions that fresh product output uses `node_id`**

```rust
assert!(body.get("node_id").is_some());
assert!(body.get("device_id").is_none());
```

- [ ] **Step 2: Rename product-surface structs and responses to `node_id`**

```rust
json!({
    "node_id": snapshot.node_id,
    "connected": snapshot.connected,
})
```

- [ ] **Step 3: Rename outward-facing validation error text to `node_id`**

```rust
message: "node_id must be 1-256 non-whitespace characters".to_string()
```

- [ ] **Step 4: Rename product logs and human-facing messages to `node` language**

```rust
tracing::info!(surface = "api", service = "nodes", action = "ws.log.event", node_id = %node_id, ...);
```

- [ ] **Step 5: Verify serialization boundary discipline while renaming**

```md
- SDK wire models stay in `lab-apis`
- product JSON shape changes stay in `lab`
```

- [ ] **Step 6: Run focused JSON/output tests**

Run: `cargo test -p lab --test device_api --test device_runtime -- --nocapture`
Expected: fresh product output uses `node_id` only

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/device/store.rs crates/lab/src/device/checkin.rs crates/lab/src/api/device crates/lab/src/device crates/lab/tests/device_api.rs crates/lab/tests/device_runtime.rs
git commit -m "refactor: replace product device ids with node ids"
```

### Task 8: Make OpenAPI fully accurate for the canonical nodes surface

**Files:**
- Modify: `crates/lab/src/api/openapi.rs`
- Modify: `crates/lab/src/api/router.rs`
- Modify: any OpenAPI-specific tests or snapshots under `crates/lab/tests`
- Modify: `docs/README.md`
- Modify: `docs/NODES.md`

- [ ] **Step 1: Identify all OpenAPI examples and paths that still mention `device` or `fleet`**

```rust
assert!(!openapi_json.contains("/v1/device"));
assert!(!openapi_json.contains("fleet"));
```

- [ ] **Step 2: Regenerate route/path metadata so only canonical `/v1/nodes*` paths are described**

```rust
assert!(openapi_json.contains("/v1/nodes"));
assert!(openapi_json.contains("/v1/nodes/ws"));
```

- [ ] **Step 3: Update schema field names and examples to `node_id`**

```rust
assert!(openapi_json.contains("node_id"));
assert!(!openapi_json.contains("device_id"));
```

- [ ] **Step 4: Run focused OpenAPI verification**

Run: `cargo test -p lab openapi -- --nocapture`
Expected: OpenAPI generation/tests pass and spec output contains only canonical nodes terminology

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api/openapi.rs crates/lab/src/api/router.rs docs/README.md docs/NODES.md
git commit -m "docs: align openapi with canonical nodes surface"
```

### Task 9: Sweep tests and docs so the clean break is enforced everywhere

**Files:**
- Modify: `crates/lab/tests/device_cli.rs`
- Modify: `crates/lab/tests/device_api.rs`
- Modify: `crates/lab/tests/device_runtime.rs`
- Modify: `crates/lab/tests/device_scan.rs`
- Modify: `crates/lab/src/api/device/fleet.rs` test module
- Modify: `crates/lab/src/config.rs` test module
- Modify: `docs/DEPLOY.md`
- Modify: `docs/DEVICE_RUNTIME.md`
- Modify: `docs/OPERATIONS.md`
- Modify: `docs/README.md`
- Modify: `docs/NODES.md`

- [ ] **Step 1: Remove all test expectations that reference public `device` / `fleet` names**

```rust
assert!(!rendered_help.contains("device"));
assert!(!openapi_json.contains("fleet"));
```

- [ ] **Step 2: Update docs and examples to canonical nodes paths only**

```bash
lab nodes list
curl http://127.0.0.1:8765/v1/nodes
```

- [ ] **Step 3: Run focused integration coverage**

Run: `cargo test -p lab --test device_cli --test device_api --test device_runtime --test device_scan -- --nocapture`
Expected: all targeted tests pass using only canonical nodes naming

- [ ] **Step 4: Run websocket module tests directly**

Run: `cargo test -p lab api::device::fleet::tests -- --nocapture`
Expected: websocket coverage passes using only canonical `nodes` wire contracts

- [ ] **Step 5: Run all-features verification**

Run: `cargo test --all-features`
Expected: pass, or only pre-existing unrelated failures with explicit notes

- [ ] **Step 6: Commit**

```bash
git add crates/lab/tests/device_cli.rs crates/lab/tests/device_api.rs crates/lab/tests/device_runtime.rs crates/lab/tests/device_scan.rs crates/lab/src/api/device/fleet.rs crates/lab/src/config.rs docs/DEPLOY.md docs/DEVICE_RUNTIME.md docs/OPERATIONS.md docs/README.md docs/NODES.md
git commit -m "test: enforce clean-break nodes surface"
```

## Notes for the implementing engineer

- This is a clean break. Do not add aliases, serde compatibility shims, fallback routes, fallback websocket methods, or deprecated CLI names.
- Internal module paths may remain under `device/` in this phase. Public surface first; filesystem refactor later if still desired.
- `crates/lab-apis/src/device_runtime/client.rs` owns the HTTP path strings used by `MasterClient`. Route migration is incomplete if that file is not updated.
- `crates/lab/src/api/router.rs` currently splits protected routes, unauthenticated `hello`, and websocket upgrade into different mounts. Preserve the auth boundary while renaming them to `/v1/nodes`.
- Follow `docs/design/SERIALIZATION.md`: keep SDK wire types in `lab-apis`, and keep product API, websocket payloads, CLI JSON, and OpenAPI shaping in `lab`.
- Fresh product output must use `node_id` only. If a test still expects `device_id` in output, update the test rather than preserving the old field.
- OpenAPI is part of the contract. Do not treat it as a documentation afterthought; verify it in the same change as the route and schema rename.
