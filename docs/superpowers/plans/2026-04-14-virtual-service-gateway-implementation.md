# Virtual Service Gateway Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a unified gateway control plane where configured Lab services appear in the main server list as visible virtual servers, can be enabled or disabled explicitly, and can be managed alongside custom gateways with selective `CLI`, `API`, `MCP`, and `WebUI` exposure.

**Architecture:** Keep service credentials canonical in Lab config and `.env`, and add a separate virtual-server state layer for enablement, per-surface toggles, and MCP policy. Extend the Rust gateway domain first so the browser can consume stable server view models, then replace the current add-gateway dialog with a metadata-driven two-tab server picker (`Lab Gateways` and `Custom Gateways`).

**Tech Stack:** Rust (`serde`, `tokio`, `axum`, existing `dispatch/gateway`, `config.rs`, `registry.rs`, service `PluginMeta`), Next.js/React in `apps/gateway-admin`, SWR, existing gateway API client/hooks, TOML + `.env` persistence helpers.

---

## File Structure

### Backend domain and persistence

- Create: `crates/lab/src/dispatch/gateway/service_catalog.rs`
  - Build the list of supported Lab-backed services from existing service metadata.
- Create: `crates/lab/src/dispatch/gateway/virtual_servers.rs`
  - Define virtual-server state and helpers for enablement, surfaces, and MCP action policy.
- Create: `crates/lab/src/dispatch/gateway/view_models.rs`
  - Normalize custom gateways and Lab-backed virtual servers into one server view model for the web UI.
- Modify: `crates/lab/src/config.rs`
  - Add persisted virtual-server state and reuse existing `.env` write helpers for canonical service config.
- Modify: `crates/lab/src/dispatch/gateway/{types.rs,params.rs,catalog.rs,dispatch.rs,manager.rs,config.rs}`
  - Add actions, request params, manager logic, and runtime state for virtual servers.
- Modify: `crates/lab/src/dispatch/clients.rs`
  - Add a refresh/rebuild path so service clients pick up updated `.env`/config without process restart.
- Modify: `crates/lab/src/api/services/gateway.rs`
  - Expose the new Rust-owned server setup and detail APIs.
- Modify: `crates/lab/src/registry.rs`
  - Reuse service metadata and later enforce surface/action policy in discovery and dispatch paths.

### Frontend models and UI

- Create: `apps/gateway-admin/components/gateway/lab-service-picker.tsx`
  - Grid of supported Lab service tiles.
- Create: `apps/gateway-admin/components/gateway/service-config-form.tsx`
  - Metadata-driven URL/token/API key form for Lab-backed services.
- Create: `apps/gateway-admin/components/gateway/surface-status-panel.tsx`
  - `CLI`, `API`, `MCP`, `WebUI` enabled/connected indicators and toggles.
- Create: `apps/gateway-admin/components/gateway/action-policy-editor.tsx`
  - Action-level MCP exposure editor for single-tool Lab services.
- Modify: `apps/gateway-admin/lib/types/gateway.ts`
  - Replace the current gateway-only model with a unified server model.
- Modify: `apps/gateway-admin/lib/api/gateway-client.ts`
  - Call the new Rust-owned routes/actions instead of reconstructing the model client-side.
- Modify: `apps/gateway-admin/lib/hooks/use-gateways.ts`
  - Update SWR hooks and mutations to the new contract.
- Modify: `apps/gateway-admin/components/gateway/{gateway-form-dialog.tsx,gateway-list-content.tsx,gateway-detail-content.tsx,gateway-table.tsx}`
  - Replace the old custom-gateway dialog and render unified server rows/details.

### Tests

- Modify: `crates/lab/src/dispatch/gateway/manager.rs`
- Modify: `crates/lab/src/dispatch/gateway/dispatch.rs`
- Modify: `crates/lab/src/api/services/gateway.rs`
- Modify or replace: `apps/gateway-admin/lib/server/gateway-adapter.test.ts`

---

### Task 1: Add persisted virtual-server state to the gateway domain

**Files:**
- Create: `crates/lab/src/dispatch/gateway/virtual_servers.rs`
- Create: `crates/lab/src/dispatch/gateway/view_models.rs`
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab/src/dispatch/gateway/{types.rs,manager.rs}`
- Test: `crates/lab/src/dispatch/gateway/manager.rs`

- [ ] **Step 1: Write the failing tests**

Add these tests to `crates/lab/src/dispatch/gateway/manager.rs`:

```rust
#[tokio::test]
async fn configured_service_appears_in_list_before_virtual_server_enablement() {
    let manager = test_manager();
    manager.seed_config(LabConfig {
        // canonical service config present, virtual server disabled
        ..LabConfig::default()
    }).await;

    let servers = manager.list().await.expect("list");
    assert!(servers.iter().any(|server| server.id == "plex"));
}

#[tokio::test]
async fn disabling_virtual_server_preserves_canonical_service_config() {
    let manager = test_manager();
    // seed canonical service config + enabled virtual server
    // disable virtual server
    // assert canonical service config still exists
}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab configured_service_appears_in_list_before_virtual_server_enablement -- --exact
cargo test -p lab disabling_virtual_server_preserves_canonical_service_config -- --exact
```

Expected: FAIL because `LabConfig` and `GatewayManager` do not yet model virtual servers.

- [ ] **Step 3: Add the virtual-server config types**

In `crates/lab/src/config.rs`, add a new persisted section:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtualServerConfig {
    pub id: String,
    pub service: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub surfaces: VirtualServerSurfacesConfig,
    #[serde(default)]
    pub mcp_policy: Option<VirtualServerMcpPolicyConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VirtualServerSurfacesConfig {
    #[serde(default)]
    pub cli: bool,
    #[serde(default)]
    pub api: bool,
    #[serde(default)]
    pub mcp: bool,
    #[serde(default)]
    pub webui: bool,
}
```

Then add the field to `LabConfig`:

```rust
#[serde(default)]
pub virtual_servers: Vec<VirtualServerConfig>,
```

- [ ] **Step 4: Add focused gateway-side types**

In `crates/lab/src/dispatch/gateway/virtual_servers.rs`, add runtime helpers:

```rust
pub enum VirtualServerSource {
    LabService { service: String },
}

pub struct VirtualServerRecord {
    pub id: String,
    pub source: VirtualServerSource,
    pub enabled: bool,
    pub surfaces: VirtualServerSurfacesConfig,
    pub mcp_policy: Option<VirtualServerMcpPolicyConfig>,
}
```

Keep this file focused on virtual-server state only. Do not put canonical service credentials here.

- [ ] **Step 5: Add a unified server view model**

In `crates/lab/src/dispatch/gateway/view_models.rs`, add the normalized payloads the UI should consume:

```rust
pub struct ServerView {
    pub id: String,
    pub name: String,
    pub source: String,
    pub enabled: bool,
    pub surfaces: SurfaceStatesView,
    pub warnings: Vec<ServerWarningView>,
    pub config_summary: ServerConfigSummaryView,
}
```

The important rule: the browser should not need to reconstruct virtual-server state from raw config.

- [ ] **Step 6: Teach `GatewayManager` to list configured and enabled virtual servers with distinct state**

Update `manager.rs`:
- keep custom gateways in the existing list path
- load virtual-server records from `LabConfig.virtual_servers`
- surface configured Lab services in the main list even before enablement
- mark disabled/configured entries explicitly in the returned server view
- keep enabled vs configured vs connected status distinct for later filtering

- [ ] **Step 7: Re-run the targeted tests**

Run:

```bash
cargo test -p lab configured_service_appears_in_list_before_virtual_server_enablement -- --exact
cargo test -p lab disabling_virtual_server_preserves_canonical_service_config -- --exact
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/config.rs crates/lab/src/dispatch/gateway
git commit -m "feat: add virtual server persistence model"
```

### Task 2: Add a metadata-driven Lab Gateway catalog

**Files:**
- Create: `crates/lab/src/dispatch/gateway/service_catalog.rs`
- Modify: `crates/lab/src/registry.rs`
- Modify: `crates/lab/src/dispatch/gateway/{catalog.rs,dispatch.rs,manager.rs,types.rs}`
- Test: `crates/lab/src/dispatch/gateway/dispatch.rs`

- [ ] **Step 1: Write the failing tests**

Add these tests to `crates/lab/src/dispatch/gateway/dispatch.rs`:

```rust
#[test]
fn supported_services_lists_metadata_backed_lab_gateways() {
    let names: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
    assert!(names.contains(&"gateway.supported_services"));
}

#[tokio::test]
async fn supported_services_payload_includes_plex_when_feature_enabled() {
    let manager = test_manager();
    let value = dispatch_with_manager(&manager, "gateway.supported_services", json!({}))
        .await
        .expect("supported services");

    assert!(value.is_array());
}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab supported_services_lists_metadata_backed_lab_gateways -- --exact
cargo test -p lab supported_services_payload_includes_plex_when_feature_enabled -- --exact
```

Expected: FAIL because the action and payload do not exist.

- [ ] **Step 3: Implement `service_catalog.rs`**

Add a small Rust-owned view model derived from `PluginMeta`/registry:

```rust
pub struct SupportedServiceView {
    pub key: String,
    pub display_name: String,
    pub category: String,
    pub required_env: Vec<ServiceFieldView>,
    pub optional_env: Vec<ServiceFieldView>,
    pub supports_surfaces: Vec<String>,
}
```

Use existing metadata:
- `PluginMeta.name`
- `PluginMeta.display_name`
- `required_env`
- `optional_env`

Do not hand-maintain a second hardcoded list in the frontend.

- [ ] **Step 4: Add the dispatch action**

In `crates/lab/src/dispatch/gateway/catalog.rs`, add:

```rust
ActionSpec {
    name: "gateway.supported_services",
    description: "List Lab-backed services available for virtual server setup",
    destructive: false,
    returns: "SupportedServiceView[]",
    params: &[],
},
```

In `dispatch.rs`, route it to a new manager/helper method.

- [ ] **Step 5: Re-run the targeted tests**

Run:

```bash
cargo test -p lab supported_services_lists_metadata_backed_lab_gateways -- --exact
cargo test -p lab supported_services_payload_includes_plex_when_feature_enabled -- --exact
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/gateway crates/lab/src/registry.rs
git commit -m "feat: add metadata-backed lab gateway catalog"
```

### Task 3: Add canonical service-config read/write actions

**Files:**
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab/src/dispatch/gateway/{params.rs,catalog.rs,dispatch.rs,manager.rs}`
- Test: `crates/lab/src/dispatch/gateway/dispatch.rs`
- Test: `crates/lab/src/dispatch/gateway/manager.rs`

- [ ] **Step 1: Write the failing tests**

Add tests for canonical config write/read behavior:

```rust
#[tokio::test]
async fn setting_service_config_writes_canonical_env_backed_fields() {
    let manager = test_manager();
    dispatch_with_manager(
        &manager,
        "gateway.service_config.set",
        json!({
            "service": "plex",
            "values": {
                "PLEX_URL": "http://127.0.0.1:32400",
                "PLEX_TOKEN": "token"
            }
        }),
    ).await.expect("set service config");

    // assert canonical config now returns those values in redacted/detail-safe shape
}

#[tokio::test]
async fn configured_but_disabled_service_can_be_read_back_for_editing() {}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab setting_service_config_writes_canonical_env_backed_fields -- --exact
cargo test -p lab configured_but_disabled_service_can_be_read_back_for_editing -- --exact
```

Expected: FAIL because these actions do not exist.

- [ ] **Step 3: Add params for service-config operations**

In `crates/lab/src/dispatch/gateway/params.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfigGetParams {
    pub service: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfigSetParams {
    pub service: String,
    pub values: std::collections::BTreeMap<String, String>,
}
```

- [ ] **Step 4: Add dispatch actions**

In `catalog.rs`, add:

```rust
ActionSpec {
    name: "gateway.service_config.get",
    description: "Read canonical config for one Lab-backed service",
    destructive: false,
    returns: "ServiceConfigView",
    params: &[ParamSpec { name: "service", ty: "string", required: true, description: "Service key" }],
},
ActionSpec {
    name: "gateway.service_config.set",
    description: "Write canonical config for one Lab-backed service",
    destructive: true,
    returns: "ServiceConfigView",
    params: &[ParamSpec { name: "service", ty: "string", required: true, description: "Service key" }],
},
```

- [ ] **Step 5: Implement canonical writes using existing helpers**

In `manager.rs`, use `config.rs` helpers rather than inventing a UI-only store.

The implementation must:
- validate field names against the service metadata
- write canonical config/`.env`
- return a redacted/detail-safe view model

Do not return raw secrets in list or detail payloads.

- [ ] **Step 6: Add a redacted service-config view**

Use a response shape similar to:

```rust
pub struct ServiceConfigView {
    pub service: String,
    pub configured: bool,
    pub fields: Vec<ServiceConfigFieldView>,
}

pub struct ServiceConfigFieldView {
    pub name: String,
    pub present: bool,
    pub secret: bool,
    pub value_preview: Option<String>,
}
```

- [ ] **Step 7: Re-run the targeted tests**

Run:

```bash
cargo test -p lab setting_service_config_writes_canonical_env_backed_fields -- --exact
cargo test -p lab configured_but_disabled_service_can_be_read_back_for_editing -- --exact
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/config.rs crates/lab/src/dispatch/gateway
git commit -m "feat: add canonical service config actions"
```

### Task 4: Refresh live service clients after config updates

**Files:**
- Modify: `crates/lab/src/dispatch/clients.rs`
- Modify: `crates/lab/src/dispatch/gateway/manager.rs`
- Test: `crates/lab/src/dispatch/gateway/manager.rs`

- [ ] **Step 1: Write the failing test**

Add a test that would fail if clients stayed stale after `.env` writes:

```rust
#[tokio::test]
async fn service_clients_refresh_after_service_config_update() {
    let manager = test_manager();

    // Write initial invalid/unconfigured values, then valid ones.
    // Assert the manager now reports the updated state rather than stale startup state.
}
```

- [ ] **Step 2: Run the targeted test and verify it fails**

Run:

```bash
cargo test -p lab service_clients_refresh_after_service_config_update -- --exact
```

Expected: FAIL because `ServiceClients::from_env()` only runs at startup.

- [ ] **Step 3: Add a refreshable client container**

In `crates/lab/src/dispatch/clients.rs`, add a wrapper with refresh:

```rust
#[derive(Clone, Default)]
pub struct SharedServiceClients {
    inner: Arc<RwLock<ServiceClients>>,
}

impl SharedServiceClients {
    pub async fn refresh_from_env(&self) {
        *self.inner.write().await = ServiceClients::from_env();
    }
}
```

Keep the minimal change set:
- preserve current startup behavior
- add explicit refresh support used by gateway config writes

- [ ] **Step 4: Thread the refreshable clients into the gateway manager**

Update `GatewayManager` to optionally hold `SharedServiceClients` so `gateway.service_config.set` can refresh runtime state immediately after writing config.

- [ ] **Step 5: Re-run the targeted test**

Run:

```bash
cargo test -p lab service_clients_refresh_after_service_config_update -- --exact
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/clients.rs crates/lab/src/dispatch/gateway/manager.rs
git commit -m "feat: refresh service clients after config writes"
```

### Task 5: Add explicit virtual-server enable/disable actions

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/{params.rs,catalog.rs,dispatch.rs,manager.rs,virtual_servers.rs}`
- Test: `crates/lab/src/dispatch/gateway/dispatch.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn enabling_virtual_server_marks_existing_server_row_enabled() {}

#[tokio::test]
async fn disabling_virtual_server_keeps_server_row_visible_but_disabled() {}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab enabling_virtual_server_marks_existing_server_row_enabled -- --exact
cargo test -p lab disabling_virtual_server_keeps_server_row_visible_but_disabled -- --exact
```

Expected: FAIL because enable/disable actions do not exist.

- [ ] **Step 3: Add params**

In `params.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualServerNameParams {
    pub id: String,
}
```

- [ ] **Step 4: Add actions**

In `catalog.rs`:

```rust
ActionSpec {
    name: "gateway.virtual_server.enable",
    description: "Enable a configured Lab-backed service as a virtual server",
    destructive: true,
    returns: "ServerView",
    params: &[ParamSpec { name: "id", ty: "string", required: true, description: "Virtual server id" }],
},
ActionSpec {
    name: "gateway.virtual_server.disable",
    description: "Disable a Lab-backed virtual server without deleting canonical credentials",
    destructive: true,
    returns: "ServerView",
    params: &[ParamSpec { name: "id", ty: "string", required: true, description: "Virtual server id" }],
},
```

- [ ] **Step 5: Implement manager methods**

Add:

```rust
pub async fn enable_virtual_server(&self, id: &str) -> Result<ServerView, ToolError> { ... }
pub async fn disable_virtual_server(&self, id: &str) -> Result<ServerView, ToolError> { ... }
```

Rules:
- enabling creates or updates a virtual-server record and marks the existing visible server row enabled
- disabling keeps the server row visible but marks it disabled
- disabling must keep canonical service config
- the returned `ServerView` must reflect the new state immediately

- [ ] **Step 6: Re-run the targeted tests**

Run:

```bash
cargo test -p lab enabling_virtual_server_marks_existing_server_row_enabled -- --exact
cargo test -p lab disabling_virtual_server_keeps_server_row_visible_but_disabled -- --exact
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/gateway
git commit -m "feat: add virtual server enable and disable actions"
```

### Task 6: Add per-surface state and runtime enforcement

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/{types.rs,params.rs,catalog.rs,dispatch.rs,manager.rs}`
- Modify: `crates/lab/src/api/services/gateway.rs`
- Modify: `crates/lab/src/registry.rs`
- Test: `crates/lab/src/dispatch/gateway/manager.rs`
- Test: `crates/lab/src/api/services/gateway.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn disabled_surface_reports_disabled_not_broken() {}

#[tokio::test]
async fn disabled_mcp_surface_is_not_normally_discoverable() {}

#[tokio::test]
async fn disabled_api_surface_returns_surface_disabled_error() {}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab disabled_surface_reports_disabled_not_broken -- --exact
cargo test -p lab disabled_mcp_surface_is_not_normally_discoverable -- --exact
cargo test -p lab disabled_api_surface_returns_surface_disabled_error -- --exact
```

Expected: FAIL because surfaces are not modeled or enforced.

- [ ] **Step 3: Add the surface-state types**

In `types.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceStateView {
    pub enabled: bool,
    pub connected: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceStatesView {
    pub cli: SurfaceStateView,
    pub api: SurfaceStateView,
    pub mcp: SurfaceStateView,
    pub webui: SurfaceStateView,
}
```

- [ ] **Step 4: Add a surface update action**

Add params:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualServerSurfacePatchParams {
    pub id: String,
    pub surface: String,
    pub enabled: bool,
}
```

Add an action:

```rust
ActionSpec {
    name: "gateway.virtual_server.set_surface",
    description: "Enable or disable one runtime surface for a virtual server",
    destructive: true,
    returns: "ServerView",
    params: &[
        ParamSpec { name: "id", ty: "string", required: true, description: "Virtual server id" },
        ParamSpec { name: "surface", ty: "string", required: true, description: "One of cli, api, mcp, webui" },
        ParamSpec { name: "enabled", ty: "boolean", required: true, description: "Desired enabled state" },
    ],
},
```

- [ ] **Step 5: Enforce surface state in runtime paths**

Implement the minimum consistent enforcement:
- `MCP`: hidden or marked unavailable in discovery, and rejected at call time
- `API`: gateway/service-facing web paths reject with `surface_disabled`
- `CLI`: command/dispatch path rejects with `surface_disabled`

If a full route-level implementation is too invasive in one pass, centralize the enforcement at shared dispatch entry points so the same checks apply consistently.

- [ ] **Step 6: Re-run the targeted tests**

Run:

```bash
cargo test -p lab disabled_surface_reports_disabled_not_broken -- --exact
cargo test -p lab disabled_mcp_surface_is_not_normally_discoverable -- --exact
cargo test -p lab disabled_api_surface_returns_surface_disabled_error -- --exact
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/gateway crates/lab/src/api/services/gateway.rs crates/lab/src/registry.rs
git commit -m "feat: add per-surface virtual server controls"
```

### Task 7: Add action-level MCP exposure policy for Lab-backed services

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/{virtual_servers.rs,types.rs,params.rs,catalog.rs,dispatch.rs,manager.rs}`
- Modify: `crates/lab/src/registry.rs`
- Test: `crates/lab/src/dispatch/gateway/dispatch.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn denied_lab_service_action_returns_policy_denied() {}

#[tokio::test]
async fn allowed_lab_service_action_remains_callable() {}

#[tokio::test]
async fn action_policy_is_driven_by_real_service_actions_not_frontend_constants() {}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab denied_lab_service_action_returns_policy_denied -- --exact
cargo test -p lab allowed_lab_service_action_remains_callable -- --exact
cargo test -p lab action_policy_is_driven_by_real_service_actions_not_frontend_constants -- --exact
```

Expected: FAIL because only tool-level exposure exists today.

- [ ] **Step 3: Add the MCP action-policy types**

In `virtual_servers.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualServerMcpPolicyConfig {
    #[serde(default)]
    pub expose_service: bool,
    #[serde(default)]
    pub allowed_actions: Vec<String>,
}
```

The first implementation should be allowlist-only. Do not build a more complex DSL now.

- [ ] **Step 4: Add policy update params and action**

In `params.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualServerMcpPolicyParams {
    pub id: String,
    pub allowed_actions: Vec<String>,
}
```

In `catalog.rs`:

```rust
ActionSpec {
    name: "gateway.virtual_server.set_mcp_policy",
    description: "Set the allowed action list for a Lab-backed virtual server",
    destructive: true,
    returns: "ServerView",
    params: &[
        ParamSpec { name: "id", ty: "string", required: true, description: "Virtual server id" },
        ParamSpec { name: "allowed_actions", ty: "string[]", required: true, description: "Allowed action names" },
    ],
},
```

- [ ] **Step 5: Enforce the policy before service dispatch**

Add a shared guard that:
- resolves the backing service for the virtual server
- reads the requested action name
- compares it to `allowed_actions`
- returns a stable policy-denied error when blocked

Use a stable envelope kind such as:

```rust
ToolError::Sdk {
    sdk_kind: "policy_denied".to_string(),
    message: format!("action `{}` is not exposed for virtual server `{}`", action, id),
}
```

- [ ] **Step 6: Re-run the targeted tests**

Run:

```bash
cargo test -p lab denied_lab_service_action_returns_policy_denied -- --exact
cargo test -p lab allowed_lab_service_action_remains_callable -- --exact
cargo test -p lab action_policy_is_driven_by_real_service_actions_not_frontend_constants -- --exact
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/dispatch/gateway crates/lab/src/registry.rs
git commit -m "feat: add action-level mcp policy for virtual servers"
```

### Task 8: Expose unified server APIs from Rust

**Files:**
- Modify: `crates/lab/src/api/services/gateway.rs`
- Modify: `crates/lab/src/dispatch/gateway/{catalog.rs,dispatch.rs,view_models.rs}`
- Test: `crates/lab/src/api/services/gateway.rs`

- [ ] **Step 1: Write the failing API tests**

Add tests for the new actions over `/v1/gateway`:

```rust
#[tokio::test]
async fn gateway_supported_services_route_exists() {}

#[tokio::test]
async fn gateway_service_config_routes_exist() {}

#[tokio::test]
async fn gateway_virtual_server_enable_route_exists() {}
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cargo test -p lab gateway_supported_services_route_exists -- --exact
cargo test -p lab gateway_service_config_routes_exist -- --exact
cargo test -p lab gateway_virtual_server_enable_route_exists -- --exact
```

Expected: FAIL because these actions are not mounted or normalized yet.

- [ ] **Step 3: Extend the HTTP gateway service**

Update `crates/lab/src/api/services/gateway.rs` so the existing action endpoint can serve:
- `gateway.supported_services`
- `gateway.service_config.get`
- `gateway.service_config.set`
- `gateway.virtual_server.enable`
- `gateway.virtual_server.disable`
- `gateway.virtual_server.set_surface`
- `gateway.virtual_server.set_mcp_policy`

No separate bespoke HTTP surface is needed yet if the action endpoint remains clean.

- [ ] **Step 4: Re-run the targeted API tests**

Run:

```bash
cargo test -p lab gateway_supported_services_route_exists -- --exact
cargo test -p lab gateway_service_config_routes_exist -- --exact
cargo test -p lab gateway_virtual_server_enable_route_exists -- --exact
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api/services/gateway.rs crates/lab/src/dispatch/gateway
git commit -m "feat: expose virtual server gateway actions over http"
```

### Task 9: Replace the frontend data model with a unified server model

**Files:**
- Modify: `apps/gateway-admin/lib/types/gateway.ts`
- Modify: `apps/gateway-admin/lib/api/gateway-client.ts`
- Modify: `apps/gateway-admin/lib/hooks/use-gateways.ts`
- Modify or replace: `apps/gateway-admin/lib/server/gateway-adapter.test.ts`

- [ ] **Step 1: Write the failing frontend-facing normalization tests**

Add or replace tests with cases covering:

```ts
it('maps a lab-backed virtual server detail payload into the shared UI model', () => {})
it('preserves per-surface state in the shared UI model', () => {})
it('preserves action-level mcp policy in the shared UI model', () => {})
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run the narrowest available command. Prefer the project test harness if present; otherwise run lint/typecheck and note the gap.

Example:

```bash
cd apps/gateway-admin && pnpm test
```

Expected: FAIL because the current model only understands custom gateways.

- [ ] **Step 3: Replace the UI model**

In `lib/types/gateway.ts`, replace the current model with something like:

```ts
export interface ServerSurfaceState {
  enabled: boolean
  connected: boolean
  status: string
}

export interface GatewayServer {
  id: string
  name: string
  source: 'custom' | 'lab_service'
  enabled: boolean
  surfaces: {
    cli: ServerSurfaceState
    api: ServerSurfaceState
    mcp: ServerSurfaceState
    webui: ServerSurfaceState
  }
  mcp: {
    allowed_actions?: string[]
  }
}
```

- [ ] **Step 4: Update the API client and hooks**

Update `gateway-client.ts` and `use-gateways.ts` to call the new Rust-owned actions and return the new shared model.

Important rule: stop reconstructing the canonical state from multiple low-level gateway calls if Rust now owns the view model.

- [ ] **Step 5: Re-run the targeted tests**

Run:

```bash
cd apps/gateway-admin && pnpm test
```

Expected: PASS, or documented missing harness plus passing lint/typecheck.

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/lib
git commit -m "refactor: adopt unified virtual server ui model"
```

### Task 10: Replace the add-gateway dialog with the two-tab server picker

**Files:**
- Create: `apps/gateway-admin/components/gateway/{lab-service-picker.tsx,service-config-form.tsx}`
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-list-content.tsx`

- [ ] **Step 1: Write the failing UI tests**

```ts
it('shows Lab Gateways and Custom Gateways tabs', () => {})
it('shows a grid of supported Lab services in the Lab Gateways tab', () => {})
it('opens a metadata-driven service config form when a service tile is selected', () => {})
it('shows configured but disabled Lab services as visible inactive rows in the server list', () => {})
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cd apps/gateway-admin && pnpm test
```

Expected: FAIL because the current dialog only supports HTTP/stdio.

- [ ] **Step 3: Create the service picker**

In `lab-service-picker.tsx`, render the supported services from the Rust-backed `gateway.supported_services` payload.

Keep it simple:

```tsx
export function LabServicePicker({ services, onSelect }: Props) {
  return (
    <div className="grid grid-cols-3 gap-3">
      {services.map((service) => (
        <button key={service.key} onClick={() => onSelect(service)}>
          {service.display_name}
        </button>
      ))}
    </div>
  )
}
```

- [ ] **Step 4: Create the service config form**

In `service-config-form.tsx`, render fields from metadata rather than one-off handcoded Plex-only or Unraid-only logic.

- [ ] **Step 5: Replace `gateway-form-dialog.tsx`**

Add:
- `Lab Gateways` tab
- `Custom Gateways` tab
- service config save path
- explicit “Enable server” action after config save

Do not remove the existing custom gateway flow.

- [ ] **Step 6: Re-run the targeted tests**

Run:

```bash
cd apps/gateway-admin && pnpm test
```

Expected: PASS, or documented test-harness gap plus passing lint/typecheck.

- [ ] **Step 7: Commit**

```bash
git add apps/gateway-admin/components/gateway
git commit -m "feat: add lab gateways tab to add-server flow"
```

### Task 11: Add per-surface controls and action-policy editing to the server detail page

**Files:**
- Create: `apps/gateway-admin/components/gateway/{surface-status-panel.tsx,action-policy-editor.tsx}`
- Modify: `apps/gateway-admin/components/gateway/gateway-detail-content.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-table.tsx`

- [ ] **Step 1: Write the failing UI tests**

```ts
it('renders CLI, API, MCP, and WebUI state on the server detail page', () => {})
it('allows toggling a surface on the server detail page', () => {})
it('renders the action policy editor for lab-backed single-tool services', () => {})
```

- [ ] **Step 2: Run the targeted tests and verify they fail**

Run:

```bash
cd apps/gateway-admin && pnpm test
```

Expected: FAIL because the current detail page has no surface or action-policy controls.

- [ ] **Step 3: Add `surface-status-panel.tsx`**

Render one row per surface with enabled/connected state and a toggle.

- [ ] **Step 4: Add `action-policy-editor.tsx`**

Render the allowed action list for a Lab-backed service and call `gateway.virtual_server.set_mcp_policy`.

Keep v1 simple:
- checkbox list or add/remove tags
- no advanced search UI

- [ ] **Step 5: Integrate both into `gateway-detail-content.tsx`**

Add the new panels without turning the page into a service dashboard. Keep the page purely control-plane.

- [ ] **Step 6: Re-run the targeted tests**

Run:

```bash
cd apps/gateway-admin && pnpm test
```

Expected: PASS, or documented gap plus passing lint/typecheck.

- [ ] **Step 7: Commit**

```bash
git add apps/gateway-admin/components/gateway
git commit -m "feat: add surface controls and mcp policy editor"
```

### Task 12: Final verification and docs

**Files:**
- Modify: `docs/README.md` if needed
- Modify: owning docs if runtime semantics changed (`CONFIG.md`, `MCP.md`, `CLI.md`, `UPSTREAM.md`)
- Test: workspace-level verification

- [ ] **Step 1: Update the owning docs**

Document:
- canonical ownership of service config vs virtual-server state
- surface-disabled semantics
- action-policy semantics for Lab-backed virtual servers

- [ ] **Step 2: Run the backend crate tests**

Run:

```bash
cargo test -p lab
```

Expected: PASS.

- [ ] **Step 3: Run the all-features workspace tests**

Run:

```bash
cargo test --workspace --all-features --tests --no-fail-fast
```

Expected: PASS.

- [ ] **Step 4: Run the all-features workspace build**

Run:

```bash
cargo build --workspace --all-features
```

Expected: PASS.

- [ ] **Step 5: Run the frontend verification**

Run the strongest available command:

```bash
cd apps/gateway-admin && pnpm test
```

If there is no test harness, run:

```bash
cd apps/gateway-admin && pnpm lint
```

Expected: PASS.

- [ ] **Step 6: Manual smoke test**

Verify this exact flow:

1. Open Add Server.
2. Select `Lab Gateways`.
3. Choose Plex.
4. Save canonical config.
5. Confirm Plex does not appear in the active server list yet.
6. Confirm Plex appears in the server list as configured/disabled.
7. Enable Plex as a server.
8. Confirm the same Plex row now reads enabled/active.
9. Toggle `MCP` off and confirm it is not normally discoverable.
10. Toggle `MCP` on, restrict actions, and confirm denied actions return `policy_denied`.
11. Confirm list filters can separate configured, enabled, disabled, connected, and disconnected rows.
12. Confirm custom HTTP/stdio gateways still work.

- [ ] **Step 7: Commit**

```bash
git add docs
git commit -m "docs: record virtual server gateway behavior"
```

## Plan Review Notes

- The hardest failure mode in this project is stale runtime state after `.env` writes. Do not skip Task 4.
- The second hardest failure mode is fake enforcement where the UI toggles change but discovery and dispatch still leak disabled surfaces or denied actions. Do not ship surface or action-policy UI without runtime enforcement.
- Configured-but-disabled services stay visible in the main server list, so the UI and backend must model `configured`, `enabled`, `disabled`, `connected`, and `disconnected` as separate states rather than collapsing them.
