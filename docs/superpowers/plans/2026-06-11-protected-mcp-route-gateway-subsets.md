# Protected MCP Route Gateway Subsets Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Protected MCP routes can expose route-scoped gateway subsets, so one OAuth-protected route like `/media` can aggregate selected Lab built-ins and selected upstream MCP servers without exposing the full root `/mcp` catalog.

**Architecture:** Keep the existing protected-route OAuth resource/scopes and legacy reverse-proxy behavior, but add an explicit target model with a new `gateway_subset` mode. Route-scoped MCP sessions carry an `McpRouteScope` on `LabMcpServer`; all list and dispatch paths consult that scope before exposing or invoking built-ins, upstream tools, resources, prompts, and Code Mode.

**Tech Stack:** Rust 2024, Axum, rmcp streamable HTTP, Tokio, serde, existing `GatewayManager`, `UpstreamPool`, and `ToolRegistry` APIs.

---

## Issue Review Summary

GitHub issue: `jmagar/lab#110`, "Fix protected MCP routes to expose route-scoped gateway subsets".

Current behavior confirmed in code:

- `crates/lab/src/config.rs` defines protected routes with flat `upstream` and `backend_url` fields.
- `crates/lab/src/dispatch/gateway/config.rs` validates exactly one of `upstream` or `backend_url`.
- `crates/lab/src/dispatch/gateway/protected_routes.rs` resolves protected requests by first path segment only.
- `crates/lab/src/api/router.rs` authenticates route-specific OAuth correctly, then calls `proxy_protected_mcp_route`.
- `proxy_protected_mcp_route` resolves one target and forwards the HTTP body as a reverse proxy.
- Root `/mcp` already constructs `LabMcpServer` sessions with `GatewayManager` and `UpstreamPool` aggregation.
- MCP handlers currently use full gateway visibility in tools, resources, prompts, and Code Mode.

Core requirement:

- A protected route with a `gateway_subset` target must behave like the normal `/mcp` gateway, scoped to configured built-in services and upstream names.
- The scope must be enforced on both discovery and dispatch. Hiding a tool from `list_tools` is not enough.

Implementation decision:

- Preserve legacy `backend_url` and `upstream` route configs as explicit proxy modes.
- Add a new optional `[protected_mcp_routes.target]` table for gateway subsets.
- Add host-aware longest-prefix protected-route matching, because the issue identifies metadata/request mismatch for nested public paths.
- Build scoped streamable HTTP MCP services for `gateway_subset` routes and dispatch to them after protected-route authentication.

## File Structure

Modify:

- `crates/lab/src/config.rs`: add `ProtectedMcpRouteTarget`, `ProtectedGatewaySubsetTarget`, serde defaults, normalization, public helper methods, config tests.
- `crates/lab/src/dispatch/gateway/config.rs`: validate target mode, normalize gateway-subset names, keep legacy proxy validation.
- `crates/lab/src/dispatch/gateway/protected_routes.rs`: switch route lookup to host-aware longest-prefix matching.
- `crates/lab/src/mcp/route_scope.rs`: new scoped visibility/dispatch policy type.
- `crates/lab/src/mcp.rs`: declare `route_scope`.
- `crates/lab/src/mcp/server.rs`: add `route_scope: McpRouteScope` to `LabMcpServer`.
- `crates/lab/src/mcp/catalog.rs`: enforce route scope for built-ins, Code Mode visibility, catalog JSON, snapshots, and upstream access helpers.
- `crates/lab/src/mcp/handlers_tools.rs`: list only allowed built-ins/upstreams and only expose Code Mode when the route scope allows it.
- `crates/lab/src/mcp/call_tool.rs`: reject hidden built-in service calls before dispatch.
- `crates/lab/src/mcp/call_tool_upstream.rs`: resolve raw and subject-scoped upstream tools within the allowed upstream set.
- `crates/lab/src/mcp/call_tool_codemode.rs`: pass the route scope into Code Mode search/execute.
- `crates/lab/src/mcp/handlers_resources.rs`: list and read only allowed built-in/upstream resources.
- `crates/lab/src/mcp/resource_proxy.rs`: reject reads for route-hidden upstream resource URIs.
- `crates/lab/src/mcp/handlers_prompts.rs`: list and get only allowed built-in/upstream prompts.
- `crates/lab/src/dispatch/upstream/pool/tools.rs`: add allowlist-aware tool list and lookup helpers.
- `crates/lab/src/dispatch/upstream/pool/resources_list.rs`: add allowlist-aware resource listing.
- `crates/lab/src/dispatch/upstream/pool/resources_read.rs`: add `read_upstream_resource_allowed`.
- `crates/lab/src/dispatch/upstream/pool/prompts_list.rs`: add allowlist-aware prompt listing and owner lookup.
- `crates/lab/src/dispatch/gateway/manager/code_mode_resolve.rs`: add scoped raw-tool resolution.
- `crates/lab/src/cli/serve.rs`: build scoped MCP services and include protected-route public hosts in allowed-host validation.
- `crates/lab/src/api/state.rs`: store the scoped protected MCP service router, if mounted.
- `crates/lab/src/api/router.rs`: route authenticated `gateway_subset` traffic to scoped MCP service, keep proxy mode for legacy targets.
- `crates/lab/src/cli/gateway.rs`: accept `gateway_subset` target flags in add/update commands.
- `docs/surfaces/MCP.md`: document route-scoped protected MCP routes.
- `docs/dev/ERRORS.md`: document the stable `route_scope_denied` error kind if it is new.

Test files:

- Existing inline tests in the files above.
- Add `crates/lab/src/mcp/route_scope.rs` unit tests.
- Add route-service integration tests in `crates/lab/src/api/router.rs` tests, reusing existing protected-route test helpers.

## Task 1: Add the Protected Route Target Model

**Files:**
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab/src/dispatch/gateway/config.rs`
- Test: `crates/lab/src/config.rs`
- Test: `crates/lab/src/dispatch/gateway/config.rs`

- [ ] **Step 1: Write failing config parse tests**

Add these tests near the existing protected-route config tests in `crates/lab/src/config.rs`:

```rust
#[test]
fn protected_route_gateway_subset_target_parses() {
    let toml = r#"
[[protected_mcp_routes]]
name = "media"
public_host = "mcp.example.com"
public_path = "/media"
scopes = ["mcp:media"]

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["sonarr", "radarr", " prowlarr "]
services = ["gateway"]
expose_code_mode = true
"#;

    let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
    cfg.normalize_protected_mcp_routes().expect("normalize");
    let route = &cfg.protected_mcp_routes[0];

    assert_eq!(route.name, "media");
    assert_eq!(route.backend_url, "");
    assert_eq!(route.upstream, None);
    assert!(route.is_gateway_subset());
    let target = route.gateway_subset_target().expect("gateway subset");
    assert_eq!(target.upstreams, vec!["sonarr", "radarr", "prowlarr"]);
    assert_eq!(target.services, vec!["gateway"]);
    assert!(target.expose_code_mode);
}

#[test]
fn protected_route_legacy_backend_url_maps_to_proxy_target() {
    let toml = r#"
[[protected_mcp_routes]]
name = "syslog"
public_host = "mcp.example.com"
public_path = "/syslog"
backend_url = "http://10.0.0.2:3100/mcp"
"#;

    let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
    cfg.normalize_protected_mcp_routes().expect("normalize");
    let route = &cfg.protected_mcp_routes[0];

    assert!(matches!(
        route.effective_target(),
        ProtectedMcpRouteEffectiveTarget::BackendUrl { .. }
    ));
}

#[test]
fn protected_route_rejects_target_with_legacy_backend() {
    let toml = r#"
[[protected_mcp_routes]]
name = "bad"
public_host = "mcp.example.com"
public_path = "/bad"
backend_url = "http://10.0.0.2:3100/mcp"

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["sonarr"]
"#;

    let mut cfg: LabConfig = toml::from_str(toml).expect("parse");
    let err = cfg
        .normalize_protected_mcp_routes()
        .expect_err("target and backend_url must conflict");
    assert!(
        err.to_string()
            .contains("protected MCP route target cannot be combined with upstream or backend_url")
    );
}
```

- [ ] **Step 2: Run the config tests and verify they fail**

Run:

```bash
cargo test -p lab config::tests::protected_route_gateway_subset_target_parses config::tests::protected_route_legacy_backend_url_maps_to_proxy_target config::tests::protected_route_rejects_target_with_legacy_backend --all-features
```

Expected: compile failure for undefined `ProtectedMcpRouteEffectiveTarget`, missing `target` field, and missing helper methods.

- [ ] **Step 3: Implement target types in `crates/lab/src/config.rs`**

Add these types above `ProtectedMcpRouteConfig`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProtectedMcpRouteTarget {
    GatewaySubset(ProtectedGatewaySubsetTarget),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProtectedGatewaySubsetTarget {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstreams: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
    #[serde(default)]
    pub expose_code_mode: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectedMcpRouteEffectiveTarget {
    BackendUrl { url: String },
    Upstream { name: String },
    GatewaySubset(ProtectedGatewaySubsetTarget),
}
```

Add this field to `ProtectedMcpRouteConfig` after `health_path`:

```rust
    /// Explicit route target. Omitted for legacy proxy routes that use
    /// `backend_url` or `upstream`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ProtectedMcpRouteTarget>,
```

Add helper methods to `impl ProtectedMcpRouteConfig`:

```rust
    #[must_use]
    pub fn effective_target(&self) -> ProtectedMcpRouteEffectiveTarget {
        if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &self.target {
            return ProtectedMcpRouteEffectiveTarget::GatewaySubset(target.clone());
        }
        if let Some(name) = self.upstream.as_ref() {
            return ProtectedMcpRouteEffectiveTarget::Upstream { name: name.clone() };
        }
        ProtectedMcpRouteEffectiveTarget::BackendUrl {
            url: self.backend_url.clone(),
        }
    }

    #[must_use]
    pub fn is_gateway_subset(&self) -> bool {
        matches!(self.target, Some(ProtectedMcpRouteTarget::GatewaySubset(_)))
    }

    #[must_use]
    pub fn gateway_subset_target(&self) -> Option<&ProtectedGatewaySubsetTarget> {
        match &self.target {
            Some(ProtectedMcpRouteTarget::GatewaySubset(target)) => Some(target),
            None => None,
        }
    }
```

- [ ] **Step 4: Normalize and validate target fields**

In `crates/lab/src/config.rs`, update every `ProtectedMcpRouteConfig` test fixture to set:

```rust
target: None,
```

In `crates/lab/src/dispatch/gateway/config.rs`, update `normalize_protected_mcp_route`:

```rust
    if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &mut route.target {
        target.upstreams = normalize_name_list(std::mem::take(&mut target.upstreams), "target.upstreams")?;
        target.services = normalize_name_list(std::mem::take(&mut target.services), "target.services")?;
    }
```

Add this helper near the other normalization helpers:

```rust
fn normalize_name_list(values: Vec<String>, param: &str) -> Result<Vec<String>, ToolError> {
    let mut normalized = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ToolError::InvalidParam {
                message: format!("{param} entries must not be empty"),
                param: param.to_string(),
            });
        }
        let name = trimmed.to_string();
        if !normalized.contains(&name) {
            normalized.push(name);
        }
    }
    Ok(normalized)
}
```

Update `validate_protected_mcp_route`:

```rust
    if route.target.is_some() && (route.upstream.is_some() || !route.backend_url.is_empty()) {
        return Err(ToolError::InvalidParam {
            message: "protected MCP route target cannot be combined with upstream or backend_url"
                .to_string(),
            param: "target".to_string(),
        });
    }

    if let Some(ProtectedMcpRouteTarget::GatewaySubset(target)) = &route.target {
        if target.upstreams.is_empty() && target.services.is_empty() && !target.expose_code_mode {
            return Err(ToolError::InvalidParam {
                message: "gateway_subset target must expose at least one upstream, service, or Code Mode"
                    .to_string(),
                param: "target".to_string(),
            });
        }
        return Ok(());
    }
```

Keep the existing legacy `match (route.upstream.as_deref(), route.backend_url.is_empty())` after this block.

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cargo test -p lab protected_route_gateway_subset_target_parses protected_route_legacy_backend_url_maps_to_proxy_target protected_route_rejects_target_with_legacy_backend --all-features
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/config.rs crates/lab/src/dispatch/gateway/config.rs
git commit -m "feat: add protected MCP route target model"
```

## Task 2: Make Protected Route Matching Longest-Prefix

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/protected_routes.rs`
- Test: `crates/lab/src/dispatch/gateway/protected_routes.rs`

- [ ] **Step 1: Write failing route-matching tests**

Replace `resolves_by_host_and_first_path_segment` with:

```rust
#[test]
fn resolves_by_host_and_longest_path_prefix() {
    let index = ProtectedRouteIndex::from_routes(&[
        route("mcp", "mcp.tootie.tv", "/mcp"),
        route("openapi", "mcp.tootie.tv", "/mcp/openapi/foo"),
        route("other-host", "other.tootie.tv", "/mcp/openapi/foo"),
    ]);

    assert_eq!(
        index
            .resolve("mcp.tootie.tv", "/mcp/openapi/foo")
            .expect("exact nested")
            .name,
        "openapi"
    );
    assert_eq!(
        index
            .resolve("mcp.tootie.tv", "/mcp/openapi/foo/sse")
            .expect("nested prefix")
            .name,
        "openapi"
    );
    assert_eq!(
        index
            .resolve("mcp.tootie.tv", "/mcp/other")
            .expect("root prefix")
            .name,
        "mcp"
    );
    assert_eq!(
        index
            .resolve("other.tootie.tv", "/mcp/openapi/foo")
            .expect("host scoped")
            .name,
        "other-host"
    );
    assert!(index.resolve("mcp.tootie.tv", "/mcproxy").is_none());
}
```

- [ ] **Step 2: Run the route test and verify it fails**

Run:

```bash
cargo test -p lab dispatch::gateway::protected_routes::tests::resolves_by_host_and_longest_path_prefix --all-features
```

Expected: failure because first-segment matching returns `/mcp`.

- [ ] **Step 3: Implement longest-prefix matching**

Replace the `ProtectedRouteIndex` internals with:

```rust
#[derive(Debug, Clone, Default)]
pub struct ProtectedRouteIndex {
    routes: HashMap<String, Vec<ProtectedMcpRouteConfig>>,
}
```

Update `from_routes`:

```rust
        for route in routes.iter().filter(|route| route.enabled) {
            let host = normalize_host(&route.public_host)
                .unwrap_or_else(|| route.public_host.to_ascii_lowercase());
            index.routes.entry(host).or_default().push(route.clone());
        }
        for routes in index.routes.values_mut() {
            routes.sort_by(|left, right| right.public_path.len().cmp(&left.public_path.len()));
        }
```

Replace `resolve`:

```rust
    pub fn resolve(&self, host: &str, path: &str) -> Option<ProtectedMcpRouteConfig> {
        let host = normalize_host(host)?;
        let path = normalize_request_path(path);
        self.routes.get(&host).and_then(|routes| {
            routes
                .iter()
                .find(|route| path_matches_prefix(&path, &route.public_path))
                .cloned()
        })
    }
```

Replace metadata lookup:

```rust
        self.routes.get(&host).and_then(|routes| {
            routes
                .iter()
                .find(|route| route.public_path == public_path)
                .cloned()
        })
```

Add helpers:

```rust
fn normalize_request_path(path: &str) -> String {
    let path = path.split('?').next().unwrap_or(path).trim();
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    if path == prefix {
        return true;
    }
    let prefix = prefix.trim_end_matches('/');
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.starts_with('/'))
}
```

Remove `route_key` and `first_path_segment` if unused.

- [ ] **Step 4: Run targeted tests**

Run:

```bash
cargo test -p lab dispatch::gateway::protected_routes --all-features
```

Expected: all protected route index tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/gateway/protected_routes.rs
git commit -m "fix: match protected MCP routes by longest prefix"
```

## Task 3: Add MCP Route Scope

**Files:**
- Create: `crates/lab/src/mcp/route_scope.rs`
- Modify: `crates/lab/src/mcp.rs`
- Modify: `crates/lab/src/mcp/server.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Test: `crates/lab/src/mcp/route_scope.rs`

- [ ] **Step 1: Write route scope unit tests**

Create `crates/lab/src/mcp/route_scope.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_allows_everything() {
        let scope = McpRouteScope::Root;
        assert!(scope.allows_service("gateway"));
        assert!(scope.allows_upstream("sonarr"));
        assert!(scope.exposes_code_mode());
        assert_eq!(scope.label(), "root");
    }

    #[test]
    fn protected_subset_allows_only_configured_names() {
        let scope = McpRouteScope::protected_subset(
            "media",
            ["sonarr", "radarr"],
            ["gateway"],
            true,
        );
        assert!(scope.allows_service("gateway"));
        assert!(!scope.allows_service("logs"));
        assert!(scope.allows_upstream("sonarr"));
        assert!(!scope.allows_upstream("github"));
        assert!(scope.exposes_code_mode());
        assert_eq!(scope.label(), "protected:media");
    }

    #[test]
    fn protected_subset_can_hide_code_mode() {
        let scope = McpRouteScope::protected_subset("ops", ["unifi"], ["device"], false);
        assert!(!scope.exposes_code_mode());
    }
}
```

- [ ] **Step 2: Run the route scope tests and verify they fail**

Run:

```bash
cargo test -p lab mcp::route_scope --all-features
```

Expected: compile failure until the type is implemented and the module is declared.

- [ ] **Step 3: Implement `McpRouteScope`**

Replace the file body above the tests with:

```rust
use std::collections::BTreeSet;

use crate::config::{ProtectedGatewaySubsetTarget, ProtectedMcpRouteConfig};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpRouteScope {
    Root,
    ProtectedSubset {
        route_name: String,
        upstreams: BTreeSet<String>,
        services: BTreeSet<String>,
        expose_code_mode: bool,
    },
}

impl Default for McpRouteScope {
    fn default() -> Self {
        Self::Root
    }
}

impl McpRouteScope {
    pub(crate) fn protected_subset<I, J, S, T>(
        route_name: impl Into<String>,
        upstreams: I,
        services: J,
        expose_code_mode: bool,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        J: IntoIterator<Item = T>,
        S: AsRef<str>,
        T: AsRef<str>,
    {
        Self::ProtectedSubset {
            route_name: route_name.into(),
            upstreams: upstreams
                .into_iter()
                .map(|name| name.as_ref().to_string())
                .collect(),
            services: services
                .into_iter()
                .map(|name| name.as_ref().to_string())
                .collect(),
            expose_code_mode,
        }
    }

    pub(crate) fn from_protected_route(route: &ProtectedMcpRouteConfig) -> Option<Self> {
        let target: &ProtectedGatewaySubsetTarget = route.gateway_subset_target()?;
        Some(Self::protected_subset(
            route.name.clone(),
            target.upstreams.iter().map(String::as_str),
            target.services.iter().map(String::as_str),
            target.expose_code_mode,
        ))
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::Root => "root".to_string(),
            Self::ProtectedSubset { route_name, .. } => format!("protected:{route_name}"),
        }
    }

    pub(crate) fn is_root(&self) -> bool {
        matches!(self, Self::Root)
    }

    pub(crate) fn allows_service(&self, service: &str) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset { services, .. } => services.contains(service),
        }
    }

    pub(crate) fn allows_upstream(&self, upstream: &str) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset { upstreams, .. } => upstreams.contains(upstream),
        }
    }

    pub(crate) fn exposes_code_mode(&self) -> bool {
        match self {
            Self::Root => true,
            Self::ProtectedSubset {
                expose_code_mode, ..
            } => *expose_code_mode,
        }
    }

    pub(crate) fn allowed_upstreams(&self) -> Option<&BTreeSet<String>> {
        match self {
            Self::Root => None,
            Self::ProtectedSubset { upstreams, .. } => Some(upstreams),
        }
    }
}
```

Declare it in `crates/lab/src/mcp.rs`:

```rust
pub(crate) mod route_scope;
```

- [ ] **Step 4: Thread the scope into `LabMcpServer`**

Add this import to `crates/lab/src/mcp/server.rs`:

```rust
use crate::mcp::route_scope::McpRouteScope;
```

Add this field to `LabMcpServer`:

```rust
    /// Visibility and dispatch constraints for this MCP route/session.
    pub route_scope: McpRouteScope,
```

Update the root HTTP MCP service constructor in `crates/lab/src/cli/serve.rs`:

```rust
                route_scope: crate::mcp::route_scope::McpRouteScope::Root,
```

Update all test `LabMcpServer` literals to include:

```rust
route_scope: crate::mcp::route_scope::McpRouteScope::Root,
```

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cargo test -p lab mcp::route_scope --all-features
cargo check -p lab --all-features
```

Expected: route scope tests pass and `cargo check` identifies only call sites that still need the new field; update those literals and rerun until clean.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/mcp.rs crates/lab/src/mcp/route_scope.rs crates/lab/src/mcp/server.rs crates/lab/src/cli/serve.rs crates/lab/src
git commit -m "feat: add MCP route scope"
```

## Task 4: Enforce Scope for Built-ins, Catalog, and Code Mode Visibility

**Files:**
- Modify: `crates/lab/src/mcp/catalog.rs`
- Modify: `crates/lab/src/mcp/handlers_tools.rs`
- Modify: `crates/lab/src/mcp/handlers_resources.rs`
- Modify: `crates/lab/src/mcp/call_tool.rs`
- Test: existing inline MCP tests or add new tests beside existing helpers

- [ ] **Step 1: Add failing unit tests for built-in visibility**

Add this test module to `crates/lab/src/mcp/catalog.rs` tests:

```rust
#[tokio::test]
async fn protected_scope_hides_unlisted_builtin_services() {
    let registry = crate::registry::build_default_registry();
    let server = LabMcpServer {
        registry: std::sync::Arc::new(registry),
        gateway_manager: None,
        node_role: None,
        peers: std::sync::Arc::new(tokio::sync::RwLock::new(Vec::new())),
        logging_level: std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
            crate::mcp::logging::logging_level_rank(rmcp::model::LoggingLevel::Info),
        )),
        route_scope: crate::mcp::route_scope::McpRouteScope::protected_subset(
            "media",
            std::iter::empty::<&str>(),
            ["gateway"],
            false,
        ),
    };

    assert!(server.service_visible_on_mcp("gateway").await);
    assert!(!server.service_visible_on_mcp("logs").await);
    assert_eq!(
        server.code_mode_visibility().await,
        CodeModeVisibility::Raw,
        "protected route with expose_code_mode=false must not expose search/execute"
    );
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
cargo test -p lab mcp::catalog::tests::protected_scope_hides_unlisted_builtin_services --all-features
```

Expected: failure because `service_visible_on_mcp` ignores `route_scope`.

- [ ] **Step 3: Scope `service_visible_on_mcp` and catalog helpers**

At the top of `service_visible_on_mcp` in `crates/lab/src/mcp/catalog.rs`, add:

```rust
        if !self.route_scope.allows_service(service) {
            return false;
        }
```

At the top of `code_mode_visibility`, add:

```rust
        if !self.route_scope.exposes_code_mode() {
            return CodeModeVisibility::Raw;
        }
```

In `catalog_json`, keep the existing `service_visible_on_mcp` call; it will now enforce route scope.

In `snapshot_catalog`, keep the existing service loop and update upstream insertion later in Task 5.

- [ ] **Step 4: Reject hidden built-in tool calls**

In `crates/lab/src/mcp/call_tool.rs`, find the branch that resolves `svc` from the requested service/tool. Before dispatching to a built-in service, add:

```rust
        if !self.service_visible_on_mcp(service).await {
            let elapsed_ms = start.elapsed().as_millis();
            self.emit_dispatch_notification(
                &context,
                service,
                action,
                elapsed_ms,
                DispatchLogOutcome::Failure {
                    level: rmcp::model::LoggingLevel::Warning,
                    kind: "route_scope_denied",
                },
            )
            .await;
            return Ok(CallToolResult::error(vec![Content::text(
                crate::mcp::envelope::build_error(
                    service,
                    action,
                    "route_scope_denied",
                    &format!("service `{service}` is not exposed on this MCP route"),
                )
                .to_string(),
            )]));
        }
```

Use the imports already present in `call_tool.rs`; add `Content` or `DispatchLogOutcome` imports only if missing.

- [ ] **Step 5: Scope Code Mode app resources**

In `crates/lab/src/mcp/handlers_resources.rs`, no separate route-scope check is needed for Code Mode app resources if the call remains:

```rust
code_mode_app_resources_visible(
    self.code_mode_visibility().await.exposes_synthetic_tools(),
    auth,
)
```

Verify this exact expression is still present. If it was changed, restore it so hidden Code Mode also hides `ui://lab/code-mode/*`.

- [ ] **Step 6: Run targeted tests**

Run:

```bash
cargo test -p lab mcp::catalog::tests::protected_scope_hides_unlisted_builtin_services --all-features
cargo check -p lab --all-features
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/mcp/catalog.rs crates/lab/src/mcp/handlers_tools.rs crates/lab/src/mcp/handlers_resources.rs crates/lab/src/mcp/call_tool.rs
git commit -m "feat: enforce MCP route scope for built-ins"
```

## Task 5: Add Allowlist-Aware Upstream Pool Helpers

**Files:**
- Modify: `crates/lab/src/dispatch/upstream/pool/tools.rs`
- Modify: `crates/lab/src/dispatch/upstream/pool/resources_list.rs`
- Modify: `crates/lab/src/dispatch/upstream/pool/resources_read.rs`
- Modify: `crates/lab/src/dispatch/upstream/pool/prompts_list.rs`
- Test: existing upstream pool tests

- [ ] **Step 1: Add allowlist helper tests**

In the existing tests for pool tools/resources/prompts, add tests that use two upstreams `alpha` and `beta` and an allowlist containing only `alpha`. The expected assertions are:

```rust
assert!(
    pool.healthy_tools_allowed(Some(&["alpha".to_string()].into_iter().collect()))
        .await
        .iter()
        .all(|tool| tool.upstream == "alpha")
);
assert!(
    pool.find_tool_allowed("shared_tool", Some(&["alpha".to_string()].into_iter().collect()))
        .await
        .is_some_and(|(upstream, _)| upstream == "alpha")
);
assert!(
    pool.find_tool_allowed("beta_only", Some(&["alpha".to_string()].into_iter().collect()))
        .await
        .is_none()
);
```

Use the existing pool test-support constructors in each module. Do not create a new fake runtime if the module already has a helper.

- [ ] **Step 2: Run the helper tests and verify they fail**

Run:

```bash
cargo test -p lab dispatch::upstream::pool --all-features
```

Expected: compile failure for missing allowlist helper methods.

- [ ] **Step 3: Implement tool allowlist helpers**

In `crates/lab/src/dispatch/upstream/pool/tools.rs`, import:

```rust
use std::collections::BTreeSet;
```

Add:

```rust
fn upstream_allowed(allowed: Option<&BTreeSet<String>>, upstream: &str) -> bool {
    allowed.is_none_or(|names| names.contains(upstream))
}
```

Add methods:

```rust
    pub async fn healthy_tools_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        let mut tools: Vec<UpstreamTool> = catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .filter(|(_, entry)| entry.tool_health.is_routable())
            .flat_map(|(_, entry)| {
                entry.tools.values().filter_map(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool.tool.name.as_ref())
                        .then(|| tool.clone())
                })
            })
            .take(MAX_UPSTREAM_TOOLS + 1)
            .collect();
        if tools.len() > MAX_UPSTREAM_TOOLS {
            tools.truncate(MAX_UPSTREAM_TOOLS);
            tracing::warn!(
                limit = MAX_UPSTREAM_TOOLS,
                "upstream tool catalog exceeds limit - truncating to cap"
            );
        }
        tools
    }

    pub async fn find_tool_allowed(
        &self,
        tool_name: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Option<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .filter(|(_, entry)| entry.tool_health.is_routable())
            .find_map(|(name, entry)| {
                entry.tools.get(tool_name).and_then(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool_name)
                        .then(|| (name.to_string(), tool.clone()))
                })
            })
    }
```

- [ ] **Step 4: Implement resource and prompt allowlist helpers**

In `resources_list.rs`, add:

```rust
    pub async fn list_upstream_resources_allowed(
        &self,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Vec<Resource> {
        if allowed.is_none() {
            return self.list_upstream_resources().await;
        }
        let allowed = allowed.expect("checked some");
        let mut resources = self.list_upstream_resources().await;
        resources.retain(|resource| {
            resource
                .uri
                .strip_prefix("lab://upstream/")
                .and_then(|rest| rest.split('/').next())
                .is_some_and(|upstream| allowed.contains(upstream))
        });
        resources
    }
```

In `resources_read.rs`, add:

```rust
    pub async fn read_upstream_resource_allowed(
        &self,
        uri: &str,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Option<Result<ReadResourceResult, String>> {
        if let Some(allowed) = allowed {
            let upstream = uri
                .strip_prefix("lab://upstream/")
                .and_then(|rest| rest.split('/').next())?;
            if !allowed.contains(upstream) {
                return None;
            }
        }
        self.read_upstream_resource(uri).await
    }
```

In `prompts_list.rs`, add:

```rust
    pub async fn list_upstream_prompts_allowed(
        &self,
        builtin_name_refs: &[&str],
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Vec<rmcp::model::Prompt> {
        let mut prompts = self.list_upstream_prompts(builtin_name_refs).await;
        if let Some(allowed) = allowed {
            let owners = self.cached_prompt_ownership_map().await;
            prompts.retain(|prompt| {
                owners
                    .get(prompt.name.as_ref())
                    .is_some_and(|upstream| allowed.contains(upstream))
            });
        }
        prompts
    }

    pub async fn find_prompt_owner_allowed(
        &self,
        prompt_name: &str,
        allowed: Option<&std::collections::BTreeSet<String>>,
    ) -> Option<String> {
        let owner = self.find_prompt_owner(prompt_name).await?;
        if allowed.is_some_and(|names| !names.contains(&owner)) {
            return None;
        }
        Some(owner)
    }
```

- [ ] **Step 5: Run pool tests**

Run:

```bash
cargo test -p lab dispatch::upstream::pool --all-features
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/dispatch/upstream/pool
git commit -m "feat: add scoped upstream pool lookups"
```

## Task 6: Enforce Scope for Upstream Tools and Code Mode

**Files:**
- Modify: `crates/lab/src/mcp/catalog.rs`
- Modify: `crates/lab/src/mcp/handlers_tools.rs`
- Modify: `crates/lab/src/mcp/call_tool_upstream.rs`
- Modify: `crates/lab/src/mcp/call_tool_codemode.rs`
- Modify: `crates/lab/src/dispatch/gateway/manager/code_mode_resolve.rs`
- Test: `crates/lab/src/dispatch/gateway/manager/tests/code_mode.rs`

- [ ] **Step 1: Add failing scoped raw-tool resolution tests**

In `crates/lab/src/dispatch/gateway/manager/tests/code_mode.rs`, add:

```rust
#[tokio::test]
async fn resolve_raw_upstream_tool_scoped_rejects_hidden_qualified_upstream() {
    let manager = manager_with_two_cached_upstreams().await;
    let allowed = ["alpha".to_string()].into_iter().collect();

    let err = manager
        .resolve_raw_upstream_tool_scoped("beta::ping", Some(&allowed), None, None)
        .await
        .expect_err("beta hidden");

    assert_eq!(err.kind(), "unknown_tool");
}

#[tokio::test]
async fn resolve_raw_upstream_tool_scoped_ambiguity_is_within_allowed_subset() {
    let manager = manager_with_two_cached_upstreams().await;
    let allowed = ["alpha".to_string()].into_iter().collect();

    let (upstream, tool) = manager
        .resolve_raw_upstream_tool_scoped("ping", Some(&allowed), None, None)
        .await
        .expect("only alpha participates");

    assert_eq!(upstream, "alpha");
    assert_eq!(tool.tool.name.as_ref(), "ping");
}
```

If the exact helper is not named `manager_with_two_cached_upstreams`, use the existing code-mode test helper that creates `alpha` and `beta`; do not duplicate its setup.

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
cargo test -p lab dispatch::gateway::manager::tests::code_mode::resolve_raw_upstream_tool_scoped --all-features
```

Expected: compile failure for missing `resolve_raw_upstream_tool_scoped`.

- [ ] **Step 3: Add scoped gateway manager resolver**

In `crates/lab/src/dispatch/gateway/manager/code_mode_resolve.rs`, add a scoped variant:

```rust
    pub async fn resolve_raw_upstream_tool_scoped(
        &self,
        tool: &str,
        allowed_upstreams: Option<&std::collections::BTreeSet<String>>,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(String, UpstreamTool), ToolError> {
        if allowed_upstreams.is_none() {
            return self
                .resolve_raw_upstream_tool(tool, owner, oauth_subject)
                .await;
        }

        let selector = ToolExecuteSelector::parse(tool, None)?;
        let allowed = allowed_upstreams.expect("checked some");
        let cfg = self.config.read().await.clone();
        let priority_by_upstream: HashMap<String, f32> = cfg
            .upstream
            .iter()
            .map(|upstream| (upstream.name.clone(), upstream.priority))
            .collect();
        let Some(pool) = self.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown tool `{}`", selector.display_name()),
            });
        };

        if let Some(upstream_name) = selector.upstream.as_deref() {
            if !allowed.contains(upstream_name) {
                return Err(ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
            }
            self.ensure_upstream_tool_runtime_ready(upstream_name, owner, oauth_subject)
                .await?;
            return pool
                .healthy_tools_for_upstream(upstream_name)
                .await
                .into_iter()
                .find(|candidate| candidate.tool.name.as_ref() == selector.tool_name)
                .map(|tool| (upstream_name.to_string(), tool))
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
        }

        let mut matches = Vec::new();
        for upstream in cfg.upstream.iter().filter(|upstream| {
            upstream.enabled
                && allowed.contains(&upstream.name)
                && is_routable(upstream.priority)
        }) {
            self.ensure_upstream_tool_runtime_ready(&upstream.name, owner, oauth_subject)
                .await?;
            matches.extend(
                pool.healthy_tools_for_upstream(&upstream.name)
                    .await
                    .into_iter()
                    .filter(|candidate| candidate.tool.name.as_ref() == selector.tool_name)
                    .map(|tool| (upstream.name.clone(), tool)),
            );
        }

        if matches.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown tool `{}`", selector.display_name()),
            });
        }
        if matches.len() > 1 {
            let valid = matches
                .iter()
                .map(|(upstream, tool)| format!("{upstream}::{}", tool.tool.name))
                .collect::<Vec<_>>();
            return Err(ToolError::AmbiguousTool {
                message: format!(
                    "tool `{}` matched multiple upstream tools",
                    selector.tool_name
                ),
                valid,
            });
        }
        Ok(matches.into_iter().next().expect("checked len"))
    }
```

Remove `priority_by_upstream` from this new function if the compiler reports it unused.

- [ ] **Step 4: Use scope in MCP tool listing and calls**

In `handlers_tools.rs`, replace:

```rust
let upstream_tools = pool.healthy_tools().await;
```

with:

```rust
let upstream_tools = pool
    .healthy_tools_allowed(self.route_scope.allowed_upstreams())
    .await;
```

For subject-scoped tools, filter the returned upstream name:

```rust
                    if !self.route_scope.allows_upstream(&_upstream_name) {
                        continue;
                    }
```

In `call_tool_upstream.rs`, replace:

```rust
manager.resolve_raw_upstream_tool(service, Some(&raw_runtime_owner), raw_oauth_subject.as_deref())
```

with:

```rust
manager
    .resolve_raw_upstream_tool_scoped(
        service,
        self.route_scope.allowed_upstreams(),
        Some(&raw_runtime_owner),
        raw_oauth_subject.as_deref(),
    )
```

In the subject-scoped branch, skip upstreams not allowed by `self.route_scope.allows_upstream(&upstream_name)`.

In `catalog.rs` `snapshot_catalog`, replace `pool.healthy_tool_names()` insertion with names from:

```rust
for tool in pool
    .healthy_tools_allowed(self.route_scope.allowed_upstreams())
    .await
{
    tools.insert(tool.tool.name.to_string());
}
```

- [ ] **Step 5: Scope Code Mode search/execute**

In `call_tool_codemode.rs`, locate the Code Mode search catalog construction. Filter every catalog entry:

```rust
.filter(|entry| self.route_scope.allows_upstream(&entry.upstream))
```

In the execute sandbox callTool resolver, pass `self.route_scope.allowed_upstreams()` into `resolve_raw_upstream_tool_scoped`. Also intersect user-supplied `upstreams` with the route scope:

```rust
if let Some(route_allowed) = self.route_scope.allowed_upstreams() {
    if requested_upstreams
        .as_ref()
        .is_some_and(|requested| requested.iter().any(|name| !route_allowed.contains(name)))
    {
        return Err(ToolError::Sdk {
            sdk_kind: "route_scope_denied".to_string(),
            message: "Code Mode requested an upstream outside this protected route scope"
                .to_string(),
        });
    }
}
```

- [ ] **Step 6: Run targeted tests**

Run:

```bash
cargo test -p lab dispatch::gateway::manager::tests::code_mode --all-features
cargo check -p lab --all-features
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/mcp/catalog.rs crates/lab/src/mcp/handlers_tools.rs crates/lab/src/mcp/call_tool_upstream.rs crates/lab/src/mcp/call_tool_codemode.rs crates/lab/src/dispatch/gateway/manager/code_mode_resolve.rs crates/lab/src/dispatch/gateway/manager/tests/code_mode.rs
git commit -m "feat: enforce MCP route scope for upstream tools"
```

## Task 7: Enforce Scope for Resources and Prompts

**Files:**
- Modify: `crates/lab/src/mcp/handlers_resources.rs`
- Modify: `crates/lab/src/mcp/resource_proxy.rs`
- Modify: `crates/lab/src/mcp/handlers_prompts.rs`
- Test: existing handler or pool tests

- [ ] **Step 1: Add failing read/get enforcement tests**

Add tests that build a protected route scope with allowed upstream `alpha` and then attempt:

```rust
let denied_resource_uri = "lab://upstream/beta/file:///tmp/secret";
let denied_prompt = "beta/prompt";
```

Expected assertions:

```rust
assert!(server.route_scope.allows_upstream("alpha"));
assert!(!server.route_scope.allows_upstream("beta"));
```

Then call the handler-level read/get path and assert the result is `resource_not_found` for the denied resource and `invalid_params` for the denied prompt. Use existing MCP handler tests if present; otherwise add focused unit tests around the new helper methods added in Task 5.

- [ ] **Step 2: Run tests and verify they fail**

Run:

```bash
cargo test -p lab mcp::handlers_resources mcp::handlers_prompts --all-features
```

Expected: hidden upstream resources/prompts are still reachable.

- [ ] **Step 3: Scope resource listing and reading**

In `handlers_resources.rs`, replace:

```rust
resources.extend(pool.list_upstream_resources().await);
```

with:

```rust
resources.extend(
    pool.list_upstream_resources_allowed(self.route_scope.allowed_upstreams())
        .await,
);
```

For subject-scoped resources, filter by route scope:

```rust
resources.retain(|resource| {
    resource
        .uri
        .strip_prefix("lab://upstream/")
        .and_then(|rest| rest.split('/').next())
        .is_none_or(|upstream| self.route_scope.allows_upstream(upstream))
});
```

In `resource_proxy.rs`, replace `pool.read_upstream_resource(uri).await` with:

```rust
pool.read_upstream_resource_allowed(uri, self.route_scope.allowed_upstreams())
    .await
```

In `handlers_resources.rs`, before subject-scoped read, add:

```rust
            && self.route_scope.allows_upstream(upstream_name)
```

- [ ] **Step 4: Scope prompt listing and get**

In `handlers_prompts.rs`, replace:

```rust
let upstream_prompts = pool.list_upstream_prompts(&builtin_name_refs).await;
```

with:

```rust
let upstream_prompts = pool
    .list_upstream_prompts_allowed(&builtin_name_refs, self.route_scope.allowed_upstreams())
    .await;
```

Replace:

```rust
pool.find_prompt_owner(&request.name).await
```

with:

```rust
pool.find_prompt_owner_allowed(&request.name, self.route_scope.allowed_upstreams()).await
```

For subject-scoped prompt owner, require:

```rust
&& self.route_scope.allows_upstream(&upstream_name)
```

before calling `subject_scoped_get_prompt`.

- [ ] **Step 5: Run targeted tests**

Run:

```bash
cargo test -p lab mcp::handlers_resources mcp::handlers_prompts dispatch::upstream::pool --all-features
cargo check -p lab --all-features
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/mcp/handlers_resources.rs crates/lab/src/mcp/resource_proxy.rs crates/lab/src/mcp/handlers_prompts.rs crates/lab/src/dispatch/upstream/pool
git commit -m "feat: enforce MCP route scope for resources and prompts"
```

## Task 8: Serve Gateway Subsets Through Scoped MCP Services

**Files:**
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Test: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Add failing API behavior tests**

In `crates/lab/src/api/router.rs` tests, add a protected route fixture:

```rust
fn protected_gateway_subset_route() -> crate::config::ProtectedMcpRouteConfig {
    crate::config::ProtectedMcpRouteConfig {
        name: "media".to_string(),
        enabled: true,
        public_host: "mcp.tootie.tv".to_string(),
        public_path: "/media".to_string(),
        upstream: None,
        backend_url: String::new(),
        backend_mcp_path: "/mcp".to_string(),
        scopes: vec!["mcp:media".to_string()],
        health_path: None,
        target: Some(crate::config::ProtectedMcpRouteTarget::GatewaySubset(
            crate::config::ProtectedGatewaySubsetTarget {
                upstreams: vec!["sonarr".to_string(), "radarr".to_string()],
                services: vec!["gateway".to_string()],
                expose_code_mode: true,
            },
        )),
    }
}
```

Add a test that sends an unauthenticated `POST /media` with host `mcp.tootie.tv` and asserts it gets the existing OAuth challenge for `https://mcp.tootie.tv/media`. Add a second test that authenticates with the route scope and asserts the request is not sent to a backend proxy server. Use the existing bearer/OAuth protected-route test helpers in this file.

- [ ] **Step 2: Run the API tests and verify they fail**

Run:

```bash
cargo test -p lab api::router::tests::protected_gateway_subset --all-features
```

Expected: compile failure for `target` fixture until Task 1 is complete, or behavior failure because gateway subset still enters proxy mode.

- [ ] **Step 3: Add scoped service state**

In `crates/lab/src/api/state.rs`, add:

```rust
    /// Router containing protected route scoped MCP services, mounted by host/path
    /// after protected route auth.
    pub protected_mcp_router: Option<Arc<axum::Router>>,
```

Initialize it in `from_registry`:

```rust
            protected_mcp_router: None,
```

Add builder:

```rust
    #[must_use]
    pub fn with_protected_mcp_router(mut self, router: axum::Router) -> Self {
        self.protected_mcp_router = Some(Arc::new(router));
        self
    }
```

- [ ] **Step 4: Build scoped MCP services in `cli/serve.rs`**

Refactor `build_mcp_service` into:

```rust
fn build_mcp_service_with_scope(
    state: &AppState,
    mcp_config: &crate::config::McpPreferences,
    notifier: PeerNotifier,
    route_scope: crate::mcp::route_scope::McpRouteScope,
    extra_allowed_hosts: &[String],
) -> Result<StreamableHttpService<LabMcpServer, LocalSessionManager>>
```

Inside allowed host construction, call:

```rust
let mut allowed_hosts = allowed_hosts(
    mcp_config.allowed_hosts.as_deref().unwrap_or(&[]),
    state
        .auth_config
        .as_ref()
        .and_then(|cfg| cfg.public_url.as_ref().map(url::Url::as_str)),
);
for host in extra_allowed_hosts {
    if !allowed_hosts.contains(host) {
        allowed_hosts.push(host.clone());
    }
}
```

Set the server field:

```rust
route_scope: route_scope.clone(),
```

Keep `build_mcp_service` as a wrapper that passes `McpRouteScope::Root` and `&[]`.

Add:

```rust
fn build_protected_mcp_router(
    state: &AppState,
    mcp_config: &crate::config::McpPreferences,
    notifier: PeerNotifier,
) -> Result<Option<axum::Router>> {
    let routes: Vec<_> = state
        .config
        .protected_mcp_routes
        .iter()
        .filter(|route| route.enabled && route.is_gateway_subset())
        .cloned()
        .collect();
    if routes.is_empty() {
        return Ok(None);
    }

    let mut router = axum::Router::new();
    for route in routes {
        let Some(scope) = crate::mcp::route_scope::McpRouteScope::from_protected_route(&route) else {
            continue;
        };
        let service = build_mcp_service_with_scope(
            state,
            mcp_config,
            notifier.clone(),
            scope,
            std::slice::from_ref(&route.public_host),
        )?;
        router = router.nest_service(&route.public_path, service);
    }
    Ok(Some(router))
}
```

Derive or implement `Clone` for `PeerNotifier` if it is not already cloneable:

```rust
#[derive(Clone)]
struct PeerNotifier {
    peers: Arc<RwLock<Vec<Peer<RoleServer>>>>,
}
```

In `build_http_router`, before calling `build_router`, build the protected router and attach it to state:

```rust
    let protected_mcp_router = build_protected_mcp_router(&state, mcp_config, notifier.clone())?;
    let state = if let Some(router) = protected_mcp_router {
        state.with_protected_mcp_router(router)
    } else {
        state
    };
```

- [ ] **Step 5: Dispatch gateway-subset traffic to the scoped service**

In `crates/lab/src/api/router.rs`, import:

```rust
use crate::config::ProtectedMcpRouteEffectiveTarget;
```

In `protected_mcp_route_entry`, after successful authentication and before proxying, add:

```rust
    if matches!(
        route.effective_target(),
        ProtectedMcpRouteEffectiveTarget::GatewaySubset(_)
    ) {
        let Some(router) = state.protected_mcp_router.as_ref() else {
            tracing::error!(
                route = %route.name,
                resource = %route.public_resource(),
                "protected MCP gateway subset failed: scoped router missing"
            );
            return ToolError::Sdk {
                sdk_kind: "bad_gateway".into(),
                message: "protected MCP gateway subset service is not mounted".into(),
            }
            .into_response();
        };
        return router.clone().oneshot(request).await.unwrap_or_else(|error| {
            tracing::error!(
                route = %route.name,
                resource = %route.public_resource(),
                error = %error,
                "protected MCP gateway subset failed: scoped service error"
            );
            ToolError::Sdk {
                sdk_kind: "bad_gateway".into(),
                message: format!("protected MCP gateway subset service failed: {error}"),
            }
            .into_response()
        });
    }
```

Add required import:

```rust
use tower::ServiceExt;
```

If `tower` is not already a direct dependency for the `lab` crate, add it to `crates/lab/Cargo.toml` with the same version already present in `Cargo.lock`.

Update `proxy_protected_mcp_route` and `protected_route_upstream_target` to switch on `route.effective_target()` and only accept `BackendUrl` or `Upstream`. If called for `GatewaySubset`, return `bad_gateway` with message `gateway_subset routes must be served by the scoped MCP service`.

- [ ] **Step 6: Run targeted API checks**

Run:

```bash
cargo test -p lab api::router::tests::protected_gateway_subset --all-features
cargo check -p lab --all-features
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/api/state.rs crates/lab/src/api/router.rs crates/lab/src/cli/serve.rs crates/lab/Cargo.toml Cargo.lock
git commit -m "feat: serve protected MCP gateway subsets"
```

## Task 9: Update CLI Protected Route Add/Update

**Files:**
- Modify: `crates/lab/src/cli/gateway.rs`
- Test: existing CLI/gateway dispatch tests

- [ ] **Step 1: Add CLI args**

In the protected route add/update arg structs, add:

```rust
    /// Expose a scoped Lab gateway MCP surface instead of proxying one backend.
    #[arg(long)]
    pub gateway_subset: bool,
    /// Upstream names to expose for --gateway-subset. Repeat or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub target_upstream: Vec<String>,
    /// Built-in Lab service names to expose for --gateway-subset. Repeat or comma-separate.
    #[arg(long, value_delimiter = ',')]
    pub target_service: Vec<String>,
    /// Expose Code Mode search/execute on this gateway subset.
    #[arg(long)]
    pub expose_code_mode: bool,
```

- [ ] **Step 2: Build target from args**

In `protected_route_from_args`, set:

```rust
        target: args.gateway_subset.then(|| {
            crate::config::ProtectedMcpRouteTarget::GatewaySubset(
                crate::config::ProtectedGatewaySubsetTarget {
                    upstreams: args.target_upstream,
                    services: args.target_service,
                    expose_code_mode: args.expose_code_mode,
                },
            )
        }),
```

For update construction, set the same `target` field.

When `gateway_subset` is true, set `upstream: None` and `backend_url: String::new()`. When false, preserve current legacy behavior.

- [ ] **Step 3: Run CLI compilation and dispatch tests**

Run:

```bash
cargo test -p lab dispatch::gateway::dispatch::tests::protected_route_dispatch_add_list_and_test_share_gateway_actions --all-features
cargo check -p lab --all-features
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add crates/lab/src/cli/gateway.rs
git commit -m "feat: add gateway subset protected-route CLI flags"
```

## Task 10: Documentation and Error Kind Alignment

**Files:**
- Modify: `docs/surfaces/MCP.md`
- Modify: `docs/dev/ERRORS.md`
- Modify: `crates/lab/src/mcp/error.rs` if stable kind mapping requires it

- [ ] **Step 1: Document config shape**

Add this section to `docs/surfaces/MCP.md`:

```markdown
### Protected Route Gateway Subsets

Protected MCP routes can either proxy one legacy backend target or expose a route-scoped Lab gateway subset. A gateway subset reuses the same `LabMcpServer`, `GatewayManager`, and `UpstreamPool` model as `/mcp`, but the route's OAuth resource and scopes define a narrower authorization boundary.

```toml
[[protected_mcp_routes]]
name = "media"
public_host = "mcp.example.com"
public_path = "/media"
scopes = ["mcp:media"]

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["sonarr", "radarr", "prowlarr"]
services = ["gateway"]
expose_code_mode = true
```

The route above exposes only the listed upstreams, the listed built-in Lab services, and Code Mode if `expose_code_mode = true`. The allowlist is enforced for `tools/list`, `tools/call`, `resources/list`, `resources/read`, `prompts/list`, `prompts/get`, Code Mode `search`, and Code Mode `execute`.

The OAuth protected resource remains route-specific: `https://mcp.example.com/media`. A token for one protected route does not authorize another route with a different resource or scope set.
```
```

- [ ] **Step 2: Document `route_scope_denied`**

In `docs/dev/ERRORS.md`, add:

```markdown
| `route_scope_denied` | Caller requested a service, upstream, tool, resource, prompt, or Code Mode target that is not exposed by the current protected MCP route scope. | MCP tool result error envelope |
```

If `crates/lab/src/mcp/error.rs` maps kinds explicitly, add `route_scope_denied` to the canonical mapping.

- [ ] **Step 3: Run docs-adjacent checks**

Run:

```bash
cargo test -p lab mcp::error --all-features
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add docs/surfaces/MCP.md docs/dev/ERRORS.md crates/lab/src/mcp/error.rs
git commit -m "docs: describe protected MCP gateway subsets"
```

## Task 11: Final Verification

**Files:**
- All implementation files above

- [ ] **Step 1: Format**

Run:

```bash
cargo fmt --all
```

Expected: no output or formatted files only.

- [ ] **Step 2: Run all-features check**

Run:

```bash
cargo check --workspace --all-features
```

Expected: success.

- [ ] **Step 3: Run all-features nextest**

Run:

```bash
cargo nextest run --workspace --all-features
```

Expected: success.

- [ ] **Step 4: Run clippy**

Run:

```bash
cargo clippy --workspace --all-features -- -D warnings
```

Expected: success.

- [ ] **Step 5: Manual smoke config**

Create a temporary config snippet outside the repo:

```bash
cat > /tmp/lab-protected-subset.toml <<'EOF'
[[protected_mcp_routes]]
name = "media"
public_host = "mcp.example.com"
public_path = "/media"
scopes = ["mcp:media"]

[protected_mcp_routes.target]
kind = "gateway_subset"
upstreams = ["sonarr", "radarr"]
services = ["gateway"]
expose_code_mode = true
EOF
```

Run the config parser path used by existing tests or a local `labby gateway protected-route add --gateway-subset` command once the CLI flags exist. Expected: the route is accepted, listed, and serialized with `[protected_mcp_routes.target]`.

- [ ] **Step 6: Commit any final fixes**

```bash
git status --short
git add crates/lab/src docs/surfaces/MCP.md docs/dev/ERRORS.md crates/lab/Cargo.toml Cargo.lock
git commit -m "test: verify protected MCP gateway subsets"
```

Only make this commit if final verification required code or docs changes not already committed.

## Self-Review

Spec coverage:

- Multiple upstreams behind one protected route: covered by Tasks 1, 6, and 8.
- Route-specific OAuth resource/scopes: preserved by Task 8, which keeps `authenticate_protected_route_request`.
- Existing proxy behavior: preserved by Task 1 target helpers and Task 8 proxy fallback.
- Tool list/call enforcement: Tasks 4 and 6.
- Resource list/read enforcement: Task 7.
- Prompt list/get enforcement: Task 7.
- Code Mode search/execute enforcement: Task 6.
- Nested path semantics: Task 2 implements longest-prefix matching.
- Protected route public hosts in RMCP allowed hosts: Task 8.
- Docs: Task 10.
- All-features verification: Task 11.

Placeholder scan:

- The plan contains no deferred-work placeholder language.
- Steps that change code include concrete snippets or exact replacement instructions.

Type consistency:

- Config target types are `ProtectedMcpRouteTarget`, `ProtectedGatewaySubsetTarget`, and `ProtectedMcpRouteEffectiveTarget`.
- Runtime scope type is `McpRouteScope`.
- Gateway manager scoped resolver is `resolve_raw_upstream_tool_scoped`.
- Pool helpers consistently use `allowed: Option<&BTreeSet<String>>`.
