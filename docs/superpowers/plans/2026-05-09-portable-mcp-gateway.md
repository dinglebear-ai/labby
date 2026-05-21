# Portable MCP Gateway Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Lab deployable behind generic reverse proxies while Lab owns protected MCP route OAuth, metadata, route matching, and upstream proxying.

**Architecture:** `LAB_PUBLIC_URL` remains the app/auth issuer. Protected route `public_host + public_path` remains the resource identity. A reverse proxy only forwards public app/gateway hosts to the same Lab listener; Lab validates route-audience JWTs, matches protected routes, applies upstream auth, and proxies to backend MCP servers.

**Tech Stack:** Rust 2024, axum, tower-http, reqwest, rmcp-adjacent MCP HTTP, lab-auth OAuth/JWT, serde, tokio, Next.js/React/TypeScript, Vitest, cargo-nextest.

---

## File Structure

Backend route safety and proxying:

- Modify `crates/lab/src/dispatch/gateway/protected_routes.rs`
  - Own normalized protected route indexing and route lookup.
  - Implement segment-boundary longest-prefix matching.
- Modify `crates/lab/src/dispatch/gateway/config.rs`
  - Own protected route validation and mutation-time normalization.
  - Add overlap, reserved-path, and unsafe path tests.
- Modify `crates/lab/src/config.rs`
  - Keep `ProtectedMcpRouteConfig::public_resource()` as the resource identity helper.
  - Add optional default MCP public URL/host only if needed by UI/templates.
- Modify `crates/lab/src/api/router.rs`
  - Own app auth routes, protected route metadata/challenge/auth/proxy behavior, Host/XFH policy, `/register` gating, public error redaction, and protected proxy client behavior.
- Modify `crates/lab/src/dispatch/gateway/manager.rs`
  - Remove raw backend URL logging from protected route mutation/test paths.

Doctor:

- Create `crates/lab/src/dispatch/doctor/proxy.rs`
  - Own black-box proxy checks and redacted `DoctorReport` findings.
- Modify `crates/lab/src/dispatch/doctor/params.rs`
  - Add typed parser for `proxy.check` params.
- Modify `crates/lab/src/dispatch/doctor/catalog.rs`
  - Register `proxy.check`.
- Modify `crates/lab/src/dispatch/doctor/dispatch.rs`
  - Route `proxy.check` to the new proxy checker.
- Modify `crates/lab/src/dispatch/doctor.rs` or module declarations if present
  - Export the new `proxy` module.
- Modify `crates/lab/src/cli/doctor.rs`
  - Add `labby doctor proxy --app-url ... --mcp-url ... --route ...`.
- Modify `apps/gateway-admin/lib/api/doctor-client.ts`
  - Add `doctorApi.proxyCheck()` using existing `performServiceAction`.

Docs and UI:

- Modify `docs/services/GATEWAY.md`
  - Replace canonical per-route reverse proxy examples with host-level examples and move per-route examples to migration/legacy text.
- Modify `docs/runtime/OAUTH.md`
  - Keep issuer/resource identity rules aligned with the implementation.
- Modify `docs/runtime/CONFIG.md`
  - Document default MCP public URL/host if added.
- Modify `docs/surfaces/TRANSPORT.md`
  - Link to reverse proxy requirements and stream semantics if stale.
- Create `docs/deploy/REVERSE_PROXY.md`
  - Host-level examples for nginx/SWAG, Caddy, Traefik, Cloudflare Tunnel, and Tailscale Funnel.
- Create `docs/deploy/SWAG_MIGRATION.md`
  - Classify old `oauth.conf` setups into migrate-to-Lab, keep-native-upstream, or keep-legacy.
- Modify `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
  - Remove `mcp.tootie.tv` fallback and require configured/entered MCP host for inline protected route creation.
- Modify `apps/gateway-admin/components/gateway/protected-mcp-routes-panel.tsx`
  - Clarify config validation vs external proxy verification.
- Add or modify gateway UI deployment panel files after backend safety and doctor land.

Verification commands:

- Backend focused:
  - `cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features protected_mcp_route`
  - `cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features doctor`
  - `cargo build --workspace --all-features`
- Frontend focused:
  - `pnpm --dir apps/gateway-admin test -- gateway-form-dialog protected-mcp-routes-panel doctor-client gateway-client`
- Full final:
  - `just test`
  - `just build`

---

## Task 1: Route Matching and Scope Safety

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/protected_routes.rs`
- Modify: `crates/lab/src/dispatch/gateway/config.rs`

- [ ] **Step 1: Write failing tests for overlapping and multi-segment routes**

Add tests in `crates/lab/src/dispatch/gateway/protected_routes.rs`:

```rust
#[test]
fn resolves_longest_prefix_by_segment_boundary() {
    let index = ProtectedRouteIndex::from_routes(&[
        route("tools", "mcp.example.com", "/tools"),
        route("tools-admin", "mcp.example.com", "/tools/admin"),
    ]);

    assert_eq!(
        index
            .resolve("mcp.example.com", "/tools/admin/list")
            .expect("admin route")
            .name,
        "tools-admin"
    );
    assert_eq!(
        index
            .resolve("mcp.example.com", "/tools/list")
            .expect("tools route")
            .name,
        "tools"
    );
    assert!(
        index.resolve("mcp.example.com", "/toolsmith").is_none(),
        "prefix matching must respect segment boundaries"
    );
}

#[test]
fn disabled_more_specific_route_does_not_shadow_parent() {
    let mut disabled = route("tools-admin", "mcp.example.com", "/tools/admin");
    disabled.enabled = false;
    let index = ProtectedRouteIndex::from_routes(&[
        route("tools", "mcp.example.com", "/tools"),
        disabled,
    ]);

    assert_eq!(
        index
            .resolve("mcp.example.com", "/tools/admin/list")
            .expect("parent route")
            .name,
        "tools"
    );
}
```

Add validation tests in `crates/lab/src/dispatch/gateway/config.rs`:

```rust
#[test]
fn insert_protected_route_rejects_exact_duplicate_but_allows_ordered_prefixes() {
    let mut cfg = LabConfig::default();
    insert_protected_mcp_route(&mut cfg, sample_protected_route("tools")).expect("tools");

    let mut admin = sample_protected_route("tools-admin");
    admin.public_path = "/tools/admin".to_string();
    admin.scopes = vec!["mcp:admin".to_string()];
    insert_protected_mcp_route(&mut cfg, admin).expect("more specific route");

    let mut dup = sample_protected_route("dup");
    dup.public_path = "/tools".to_string();
    let err = insert_protected_mcp_route(&mut cfg, dup).expect_err("duplicate exact route");
    assert_eq!(err.kind(), "conflict");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features resolves_longest_prefix_by_segment_boundary disabled_more_specific_route_does_not_shadow_parent insert_protected_route_rejects_exact_duplicate_but_allows_ordered_prefixes
```

Expected: at least `resolves_longest_prefix_by_segment_boundary` fails because current lookup only uses the first segment.

- [ ] **Step 3: Implement longest-prefix route index**

Replace the first-segment lookup in `crates/lab/src/dispatch/gateway/protected_routes.rs` with a per-host sorted vector:

```rust
#[derive(Debug, Clone, Default)]
pub struct ProtectedRouteIndex {
    routes: HashMap<String, Vec<ProtectedMcpRouteConfig>>,
    metadata: HashMap<(String, String), ProtectedMcpRouteConfig>,
}

impl ProtectedRouteIndex {
    #[must_use]
    pub fn from_routes(routes: &[ProtectedMcpRouteConfig]) -> Self {
        let mut index = Self::default();
        for route in routes.iter().filter(|route| route.enabled) {
            let host = normalize_host(&route.public_host)
                .unwrap_or_else(|| route.public_host.to_ascii_lowercase());
            index
                .metadata
                .insert((host.clone(), route.public_path.clone()), route.clone());
            index.routes.entry(host).or_default().push(route.clone());
        }
        for routes in index.routes.values_mut() {
            routes.sort_by(|a, b| b.public_path.len().cmp(&a.public_path.len()));
        }
        index
    }

    #[must_use]
    pub fn resolve(&self, host: &str, path: &str) -> Option<ProtectedMcpRouteConfig> {
        let host = normalize_host(host)?;
        let path = normalize_request_path(path)?;
        self.routes.get(&host)?.iter().find_map(|route| {
            if path_matches_prefix(&path, &route.public_path) {
                Some(route.clone())
            } else {
                None
            }
        })
    }

    #[must_use]
    pub fn resolve_exact_metadata_path(
        &self,
        host: &str,
        metadata_path: &str,
    ) -> Option<ProtectedMcpRouteConfig> {
        const PREFIX: &str = "/.well-known/oauth-protected-resource";
        let suffix = metadata_path.strip_prefix(PREFIX)?;
        let public_path = if suffix.is_empty() { "/mcp" } else { suffix };
        let host = normalize_host(host)?;
        self.metadata.get(&(host, public_path.to_string())).cloned()
    }
}

fn normalize_request_path(path: &str) -> Option<String> {
    let normalized = if path.is_empty() { "/" } else { path };
    Some(normalized.trim_end_matches('/').to_string()).filter(|path| !path.is_empty())
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/'))
}
```

- [ ] **Step 4: Run focused route tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features protected_route
```

Expected: protected route tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/dispatch/gateway/protected_routes.rs crates/lab/src/dispatch/gateway/config.rs
git commit -m "fix(gateway): use safe protected route prefix matching"
```

---

## Task 2: Reserved Paths, `/register` Gating, and Host Trust

**Files:**
- Modify: `crates/lab/src/dispatch/gateway/config.rs`
- Modify: `crates/lab/src/api/router.rs`
- Test: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Add failing tests for reserved paths and XFH spoofing**

In `crates/lab/src/dispatch/gateway/config.rs`, expand the reserved path test:

```rust
#[test]
fn insert_protected_route_rejects_lab_owned_paths_on_app_host() {
    for path in [
        "/auth",
        "/authorize",
        "/callback",
        "/token",
        "/revoke",
        "/jwks",
        "/register",
        "/success",
        "/health",
        "/ready",
        "/dev",
        "/setup",
        "/settings",
        "/_next",
        "/v1/proxy",
        "/.well-known/x",
        "/mcp",
    ] {
        let mut cfg = LabConfig::default();
        let mut route = sample_protected_route("bad");
        route.public_host = "lab.example.com".to_string();
        route.public_path = path.to_string();
        let err =
            insert_protected_mcp_route(&mut cfg, route).expect_err("path should be rejected");
        assert_eq!(err.kind(), "invalid_param", "{path}");
    }
}
```

In `crates/lab/src/api/router.rs`, add an XFH spoof negative test next to protected route tests:

```rust
#[tokio::test]
async fn protected_route_ignores_spoofed_x_forwarded_host_by_default() {
    let backend = wiremock::MockServer::start().await;
    let config = protected_route_config("syslog", "mcp.example.com", "/syslog", &backend.uri());
    let app = build_test_router_with_config(config).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/oauth-protected-resource/syslog")
                .header(header::HOST, "lab.example.com")
                .header("x-forwarded-host", "mcp.example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Add failing tests for `/register` disabled behavior**

In `crates/lab/src/api/router.rs`, add:

```rust
#[tokio::test]
async fn top_level_register_is_not_mounted_when_dynamic_registration_disabled() {
    let mut auth = test_auth_config();
    auth.enable_dynamic_registration = false;
    let app = build_test_router_with_auth_config(auth).await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"redirect_uris":["http://127.0.0.1/callback"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        matches!(response.status(), StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED),
        "unexpected status: {}",
        response.status()
    );
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features insert_protected_route_rejects_lab_owned_paths_on_app_host protected_route_ignores_spoofed_x_forwarded_host_by_default top_level_register_is_not_mounted_when_dynamic_registration_disabled
```

Expected: at least XFH and `/register` tests fail on current behavior.

- [ ] **Step 4: Implement Host-first behavior**

Change `request_host()` in `crates/lab/src/api/router.rs` to prefer `Host`:

```rust
fn request_host(request: &Request<Body>) -> Option<String> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(ToOwned::to_owned)
}
```

If trusted XFH support is added in a follow-up change, introduce it as a separate function that receives trusted proxy state. Do not keep implicit XFH preference.

- [ ] **Step 5: Gate `/register` route**

In `build_router()`, replace unconditional registration:

```rust
router = router
    .route("/jwks", get(auth_jwks))
    .route("/register", post(auth_register))
    .route("/authorize", get(auth_authorize));
```

with:

```rust
router = router
    .route("/jwks", get(auth_jwks))
    .route("/authorize", get(auth_authorize));

if auth_state
    .as_ref()
    .is_some_and(|state| state.config.enable_dynamic_registration)
{
    router = router.route("/register", post(auth_register));
}
```

Update authorization-server metadata generation in `lab-auth` or the Lab wrapper so `registration_endpoint` is omitted when dynamic registration is disabled. If the metadata type currently requires a string, make it `Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]`.

- [ ] **Step 6: Add reserved path validation**

In `crates/lab/src/dispatch/gateway/config.rs`, add a central constant and validate it when the route is on the app host. If `LabConfig` is not currently available in the route validator, add a higher-level validation pass in the config mutation path that has the full config and route.

```rust
const LAB_RESERVED_PUBLIC_PATH_PREFIXES: &[&str] = &[
    "/.well-known",
    "/_next",
    "/auth",
    "/authorize",
    "/callback",
    "/dev",
    "/health",
    "/jwks",
    "/mcp",
    "/ready",
    "/register",
    "/revoke",
    "/settings",
    "/setup",
    "/success",
    "/token",
    "/v1",
];

fn conflicts_with_reserved_prefix(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    LAB_RESERVED_PUBLIC_PATH_PREFIXES
        .iter()
        .any(|prefix| lower == *prefix || lower.starts_with(&format!("{prefix}/")))
}
```

- [ ] **Step 7: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features protected_route register oauth
```

Expected: tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/api/router.rs crates/lab/src/dispatch/gateway/config.rs crates/lab-auth/src
git commit -m "fix(oauth): harden protected route host and registration handling"
```

---

## Task 3: Protected Proxy Redaction, Client Reuse, and Timeout Policy

**Files:**
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/dispatch/gateway/manager.rs`

- [ ] **Step 1: Write failing tests for backend leak and proxy client behavior**

In `crates/lab/src/api/router.rs`, add:

```rust
#[tokio::test]
async fn protected_route_backend_failure_does_not_leak_backend_url() {
    let config = protected_route_config(
        "private",
        "mcp.example.com",
        "/private",
        "http://10.0.0.2:3100/mcp",
    );
    let app = build_test_router_with_config(config).await;
    let token = route_audience_token("https://mcp.example.com/private").await;

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/private")
                .header(header::HOST, "mcp.example.com")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("10.0.0.2"), "{text}");
    assert!(!text.contains("3100"), "{text}");
    assert!(!text.contains("/mcp"), "{text}");
}
```

Add or update manager log tests so mutation paths do not include raw backend URLs in captured structured logs.

- [ ] **Step 2: Run tests to verify failure**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features protected_route_backend_failure_does_not_leak_backend_url
```

Expected: fails because current response includes reqwest error details.

- [ ] **Step 3: Add shared protected proxy HTTP client**

In `crates/lab/src/api/state.rs`, add a shared client field:

```rust
#[derive(Clone)]
pub struct AppState {
    // existing fields
    pub protected_mcp_http: reqwest::Client,
}

fn protected_mcp_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(30))
        .build()
        .expect("protected MCP reqwest client configuration is valid")
}
```

Initialize the field in every `AppState` constructor. Use `std::time::Duration`.

- [ ] **Step 4: Use shared client and redacted public errors**

In `proxy_protected_mcp_route()`, replace:

```rust
let mut builder = reqwest::Client::new().request(method.clone(), upstream);
```

with:

```rust
let mut builder = state.protected_mcp_http.request(method.clone(), upstream);
```

Wrap the send in an upstream timeout that applies to request establishment, not the whole streaming response:

```rust
let upstream_response = match tokio::time::timeout(Duration::from_secs(30), builder.body(body).send()).await {
    Ok(Ok(response)) => response,
    Ok(Err(error)) => {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            method = %method,
            path = %original_path,
            upstream_target = %redacted_upstream_target(&upstream_target),
            elapsed_ms = started.elapsed().as_millis(),
            error = %error,
            "protected MCP route proxy failed: backend request failed"
        );
        return ToolError::Sdk {
            sdk_kind: "bad_gateway".into(),
            message: "protected MCP backend request failed".into(),
        }
        .into_response();
    }
    Err(_) => {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            method = %method,
            path = %original_path,
            upstream_target = %redacted_upstream_target(&upstream_target),
            elapsed_ms = started.elapsed().as_millis(),
            "protected MCP route proxy failed: backend request timed out"
        );
        return ToolError::Sdk {
            sdk_kind: "gateway_timeout".into(),
            message: "protected MCP backend request timed out".into(),
        }
        .into_response();
    }
};
```

Add helper:

```rust
fn redacted_upstream_target(target: &str) -> &'static str {
    if target.starts_with("upstream:") {
        "named_upstream"
    } else {
        "backend_url"
    }
}
```

- [ ] **Step 5: Decide and implement request body cap**

If streaming request bodies is not done now, lower the cap and return a stable error. Replace `50 * 1024 * 1024` with a named constant:

```rust
const PROTECTED_MCP_MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
```

Use the constant in `to_bytes()`. Add a test asserting an oversized request returns `413 Payload Too Large` or a stable `bad_request` envelope without backend details.

- [ ] **Step 6: Redact mutation logs**

In `crates/lab/src/dispatch/gateway/manager.rs`, replace log fields like `backend_url = %route.backend_url` with:

```rust
target_kind = if route.upstream.is_some() { "upstream" } else { "backend_url" },
public_resource = %route.public_resource(),
```

Do not log private backend URL, backend path, bearer token env, or OAuth token state.

- [ ] **Step 7: Run focused tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features protected_route gateway.protected_route
```

Expected: all focused tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/api/router.rs crates/lab/src/api/state.rs crates/lab/src/dispatch/gateway/manager.rs
git commit -m "fix(gateway): redact and bound protected route proxying"
```

---

## Task 4: Minimal `doctor.proxy.check`

**Files:**
- Create: `crates/lab/src/dispatch/doctor/proxy.rs`
- Modify: `crates/lab/src/dispatch/doctor/params.rs`
- Modify: `crates/lab/src/dispatch/doctor/catalog.rs`
- Modify: `crates/lab/src/dispatch/doctor/dispatch.rs`
- Modify: `crates/lab/src/dispatch/doctor.rs`
- Modify: `crates/lab/src/cli/doctor.rs`
- Modify: `apps/gateway-admin/lib/api/doctor-client.ts`

- [ ] **Step 1: Write failing dispatch/catalog tests**

In `crates/lab/src/dispatch/doctor/dispatch.rs` tests, add:

```rust
#[tokio::test]
async fn proxy_check_is_registered_in_doctor_catalog() {
    assert!(
        ACTIONS.iter().any(|action| action.name == "proxy.check"),
        "doctor proxy.check action must be registered"
    );
}
```

In `apps/gateway-admin/lib/api/doctor-client.ts` tests, add a test that `doctorApi.proxyCheck()` posts action `proxy.check` through `performServiceAction`.

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features proxy_check_is_registered_in_doctor_catalog
pnpm --dir apps/gateway-admin test -- doctor-client
```

Expected: action and TS method are missing.

- [ ] **Step 3: Add params and action spec**

In `crates/lab/src/dispatch/doctor/params.rs`:

```rust
#[derive(Debug, Clone)]
pub struct ProxyCheckParams {
    pub app_url: String,
    pub mcp_url: String,
    pub route: String,
    pub expected_issuer: Option<String>,
    pub timeout_ms: u64,
}

pub fn parse_proxy_check(params: &serde_json::Value) -> Result<ProxyCheckParams, ToolError> {
    let app_url = crate::dispatch::helpers::require_str(params, "app_url")?.to_string();
    let mcp_url = crate::dispatch::helpers::require_str(params, "mcp_url")?.to_string();
    let route = crate::dispatch::helpers::require_str(params, "route")?.to_string();
    let expected_issuer = params
        .get("expected_issuer")
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned);
    let timeout_ms = params
        .get("timeout_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or(5_000);
    Ok(ProxyCheckParams {
        app_url,
        mcp_url,
        route,
        expected_issuer,
        timeout_ms,
    })
}
```

In `catalog.rs`, add `proxy.check` with required params.

- [ ] **Step 4: Implement proxy checker**

Create `crates/lab/src/dispatch/doctor/proxy.rs`:

```rust
use std::time::{Duration, Instant};

use reqwest::header;

use crate::dispatch::doctor::{Finding, Report, Severity};
use crate::dispatch::error::ToolError;

use super::params::ProxyCheckParams;

pub async fn run_proxy_check(params: ProxyCheckParams) -> Result<Report, ToolError> {
    let timeout = Duration::from_millis(params.timeout_ms.clamp(500, 30_000));
    let client = reqwest::Client::builder()
        .connect_timeout(timeout)
        .timeout(timeout)
        .build()
        .map_err(|error| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: format!("failed to build doctor HTTP client: {error}"),
        })?;

    let mut findings = Vec::new();
    findings.push(check_authorization_metadata(&client, &params, timeout).await);
    findings.push(check_protected_resource_metadata(&client, &params, timeout).await);
    findings.push(check_unauthenticated_challenge(&client, &params, timeout).await);
    Ok(Report { findings })
}

async fn check_authorization_metadata(
    client: &reqwest::Client,
    params: &ProxyCheckParams,
    timeout: Duration,
) -> Finding {
    let started = Instant::now();
    let url = format!(
        "{}/.well-known/oauth-authorization-server",
        params.app_url.trim_end_matches('/')
    );
    match tokio::time::timeout(timeout, client.get(url).send()).await {
        Ok(Ok(response)) if response.status().is_success() => Finding {
            service: Some("doctor".into()),
            category: Some("proxy.app_metadata".into()),
            severity: Severity::Ok,
            message: "Lab authorization metadata is reachable.".into(),
            hint: None,
            elapsed_ms: Some(started.elapsed().as_millis() as u64),
        },
        Ok(Ok(response)) => Finding {
            service: Some("doctor".into()),
            category: Some("proxy.app_metadata".into()),
            severity: Severity::Error,
            message: format!("Lab authorization metadata returned HTTP {}.", response.status()),
            hint: Some("Check app URL, TLS, and reverse proxy host forwarding.".into()),
            elapsed_ms: Some(started.elapsed().as_millis() as u64),
        },
        Ok(Err(_)) | Err(_) => Finding {
            service: Some("doctor".into()),
            category: Some("proxy.app_metadata".into()),
            severity: Severity::Error,
            message: "Lab authorization metadata is not reachable.".into(),
            hint: Some("Check DNS, TLS, proxy target, and firewall rules.".into()),
            elapsed_ms: Some(started.elapsed().as_millis() as u64),
        },
    }
}
```

Add `check_protected_resource_metadata()` and `check_unauthenticated_challenge()` in the same style. They must not include bearer values, backend URLs, raw response bodies, or private IPs in `message` or `hint`.

- [ ] **Step 5: Wire dispatch and CLI**

In `dispatch.rs`, route the action:

```rust
"proxy.check" => {
    let p = super::params::parse_proxy_check(&params)?;
    let report = super::proxy::run_proxy_check(p).await?;
    to_json(report)
}
```

In `cli/doctor.rs`, add:

```rust
Proxy {
    #[arg(long)]
    app_url: String,
    #[arg(long)]
    mcp_url: String,
    #[arg(long)]
    route: String,
}
```

and call the dispatch action with those params.

- [ ] **Step 6: Add frontend API method**

In `apps/gateway-admin/lib/api/doctor-client.ts`:

```ts
export interface ProxyCheckParams {
  app_url: string
  mcp_url: string
  route: string
  expected_issuer?: string
  timeout_ms?: number
}

proxyCheck(params: ProxyCheckParams, signal?: AbortSignal): Promise<DoctorReport> {
  if (USE_MOCK_DATA) {
    signal?.throwIfAborted?.()
    return Promise.resolve({
      findings: [
        {
          category: 'proxy.protected_resource',
          severity: 'ok',
          message: 'Mock protected resource metadata is reachable.',
          elapsed_ms: 4,
        },
      ],
    })
  }
  return doctorAction<DoctorReport>('proxy.check', params, signal)
},
```

- [ ] **Step 7: Run tests**

Run:

```bash
cargo nextest run --manifest-path crates/lab/Cargo.toml --all-features proxy.check doctor
pnpm --dir apps/gateway-admin test -- doctor-client
```

Expected: pass.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/dispatch/doctor* crates/lab/src/cli/doctor.rs apps/gateway-admin/lib/api/doctor-client.ts
git commit -m "feat(doctor): add protected MCP proxy check"
```

---

## Task 5: UI Hardcoded Host Removal and Minimal Deployment View

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx`
- Modify: `apps/gateway-admin/components/gateway/protected-mcp-routes-panel.tsx`
- Modify: `apps/gateway-admin/lib/hooks/use-gateways.ts`
- Modify: `apps/gateway-admin/lib/api/gateway-client.ts`
- Modify: `apps/gateway-admin/lib/types/gateway.ts`

- [ ] **Step 1: Add failing UI tests for no personal host defaults**

Add or extend `apps/gateway-admin/components/gateway/protected-mcp-routes-panel.test.tsx`:

```tsx
import { render, screen } from '@testing-library/react'
import { test, expect } from 'vitest'
import { GatewayFormDialog } from './gateway-form-dialog'

test('custom gateway form does not render personal protected route host fallback', () => {
  render(
    <GatewayFormDialog
      open
      onOpenChange={() => {}}
      gateway={null}
      onSave={async () => {}}
    />,
  )

  expect(screen.queryByText(/mcp\.tootie\.tv/i)).toBeNull()
  expect(document.body.textContent).not.toContain('syslog')
  expect(document.body.textContent).not.toContain('axon')
})
```

- [ ] **Step 2: Run test to verify failure**

Run:

```bash
pnpm --dir apps/gateway-admin test -- gateway-form-dialog protected-mcp-routes-panel
```

Expected: fails while the form still renders `mcp.tootie.tv`.

- [ ] **Step 3: Remove constant fallback**

In `gateway-form-dialog.tsx`, replace:

```ts
const PROTECTED_MCP_PUBLIC_HOST = process.env.NEXT_PUBLIC_PROTECTED_MCP_HOST || 'mcp.tootie.tv'
```

with state derived from a backend/config hook:

```ts
const protectedMcpPublicHost = deployment?.mcp_public_host ?? ''
const protectedHostLabel = protectedMcpPublicHost || 'mcp.example.com'
```

If the backend deployment view is not available yet, make the route host an explicit input next to protected path and require it before saving:

```ts
const [protectedPublicHost, setProtectedPublicHost] = useState('')

if (protectedPublicPath && !protectedPublicHost.trim()) {
  newErrors.protectedPublicPath = 'Set the MCP public host before publishing this protected route'
}
```

In `buildProtectedRouteInput()`, use the entered/configured host:

```ts
public_host: protectedPublicHost.trim().toLowerCase(),
```

- [ ] **Step 4: Clarify route test vs proxy doctor copy**

In `protected-mcp-routes-panel.tsx`, change labels so existing test says route config validation:

```tsx
<Button type="button" onClick={handleValidateRoute}>
  Validate route config
</Button>
```

Add text near doctor integration:

```tsx
<FieldDescription>
  External proxy verification runs through Doctor proxy checks after the public host is reachable.
</FieldDescription>
```

- [ ] **Step 5: Add doctor API call hook only after Task 4**

In `use-gateways.ts`, add a hook wrapper that calls `doctorApi.proxyCheck()` rather than raw fetch. Use `AbortSignal` and `performServiceAction` through `doctor-client.ts`.

- [ ] **Step 6: Run UI tests**

Run:

```bash
pnpm --dir apps/gateway-admin test -- gateway-form-dialog protected-mcp-routes-panel doctor-client gateway-client
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add apps/gateway-admin/components/gateway apps/gateway-admin/lib/api apps/gateway-admin/lib/hooks apps/gateway-admin/lib/types
git commit -m "fix(gateway-ui): remove protected route host fallback"
```

---

## Task 6: Portable Reverse Proxy Documentation

**Files:**
- Create: `docs/deploy/REVERSE_PROXY.md`
- Modify: `docs/services/GATEWAY.md`
- Modify: `docs/runtime/OAUTH.md`
- Modify: `docs/runtime/CONFIG.md`
- Modify: `docs/surfaces/TRANSPORT.md`

- [ ] **Step 1: Add doc checks for forbidden canonical examples**

Create a shell or Rust doc test if one exists locally. Minimal shell check:

```bash
rg -n "oauth\\.conf|auth_request|mcp\\.tootie\\.tv|syslog\\.tootie\\.tv|100\\.|/syslog" docs/deploy/REVERSE_PROXY.md docs/services/GATEWAY.md
```

Expected after docs are updated: no matches in canonical sections. If legacy migration sections need those terms, mark them with headings containing `Legacy` and exclude them in the check script.

- [ ] **Step 2: Create `docs/deploy/REVERSE_PROXY.md`**

Add these sections with exact host-level examples:

```markdown
# Reverse Proxy Deployment

Lab expects the public reverse proxy to terminate TLS and forward requests to the same Lab listener.

Recommended:
- `https://lab.example.com` -> `http://lab:8765`
- `https://mcp.example.com` -> `http://lab:8765`

The reverse proxy does not know about individual protected MCP routes. Add `/tools`, `/logs`, or `/mcp` routes in Lab, not in nginx/Caddy/Traefik.
```

Include nginx/SWAG example:

```nginx
server {
    server_name mcp.example.com;

    location / {
        proxy_pass http://lab:8765;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header Authorization $http_authorization;
        proxy_set_header Accept $http_accept;
        proxy_set_header Content-Type $content_type;
        proxy_set_header MCP-Protocol-Version $http_mcp_protocol_version;
        proxy_set_header MCP-Session-Id $http_mcp_session_id;
        proxy_set_header Last-Event-ID $http_last_event_id;
        proxy_buffering off;
        proxy_request_buffering off;
        gzip off;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }
}
```

Include Caddy:

```caddyfile
mcp.example.com {
    reverse_proxy lab:8765
}
```

Include Traefik labels or dynamic config with `passHostHeader: true`, no `StripPrefix`, and long forwarding timeouts.

- [ ] **Step 3: Update existing docs**

In `docs/services/GATEWAY.md`, replace route-specific examples as canonical. Add a link:

```markdown
For deployable reverse proxy templates, see [Reverse Proxy Deployment](../deploy/REVERSE_PROXY.md).
```

Move any per-route `/syslog` examples under a heading:

```markdown
### Legacy SWAG Per-Route Examples

These are not the recommended Lab gateway deployment model.
```

- [ ] **Step 4: Run doc checks**

Run:

```bash
rg -n "mcp\\.tootie\\.tv|syslog\\.tootie\\.tv|100\\.88|auth_request|oauth\\.conf" docs/deploy/REVERSE_PROXY.md
rg -n "MCP-Protocol-Version|MCP-Session-Id|Last-Event-ID|proxy_buffering off|proxy_request_buffering off" docs/deploy/REVERSE_PROXY.md
```

Expected: first command has no output; second command finds the required nginx header/streaming lines.

- [ ] **Step 5: Commit**

```bash
git add docs/deploy/REVERSE_PROXY.md docs/services/GATEWAY.md docs/runtime/OAUTH.md docs/runtime/CONFIG.md docs/surfaces/TRANSPORT.md
git commit -m "docs: add portable Lab reverse proxy templates"
```

---

## Task 7: SWAG Migration Guide

**Files:**
- Create: `docs/deploy/SWAG_MIGRATION.md`
- Modify: `docs/services/GATEWAY.md`

- [ ] **Step 1: Create migration guide**

Create `docs/deploy/SWAG_MIGRATION.md`:

```markdown
# Migrating SWAG MCP OAuth Configs to Lab Protected Routes

Lab protected routes replace reverse-proxy `oauth.conf` for Lab-owned MCP gateway routes.

## Classifications

### Migrate to Lab Gateway

Use this when SWAG protects an MCP backend only to require OAuth before reaching it. Remove edge `auth_request`, proxy the public host to Lab, and create a Lab protected route.

### Keep Native Upstream

Use this when the upstream MCP server owns OAuth discovery, callback, and token endpoints. Keep that upstream public origin if Lab or other clients need to authenticate to it.

### Keep Legacy

Use this for non-Lab deployments or endpoints intentionally still protected by an external OAuth proxy.
```

Add checklist and smoke commands:

```bash
curl -i https://mcp.example.com/.well-known/oauth-protected-resource/tools
curl -i -X POST https://mcp.example.com/tools
```

Expected unauthenticated route response:

```text
HTTP/2 401
WWW-Authenticate: Bearer resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource/tools"
```

- [ ] **Step 2: Link from gateway docs**

In `docs/services/GATEWAY.md`, add:

```markdown
For migrating older SWAG `oauth.conf` or `auth_request` MCP routes, see [SWAG Migration](../deploy/SWAG_MIGRATION.md).
```

- [ ] **Step 3: Run doc checks**

Run:

```bash
rg -n "migrate-to-Lab|keep-native-upstream|keep-legacy|auth_request|oauth\\.conf" docs/deploy/SWAG_MIGRATION.md docs/services/GATEWAY.md
```

Expected: migration guide contains all three classifications and explicitly names the legacy edge auth terms.

- [ ] **Step 4: Commit**

```bash
git add docs/deploy/SWAG_MIGRATION.md docs/services/GATEWAY.md
git commit -m "docs: document SWAG OAuth migration path"
```

---

## Task 8: Final Verification and Bead Closure Prep

**Files:**
- Verify all files touched by previous tasks.
- Update beads only after code and docs are verified.

- [ ] **Step 1: Run backend full verification**

Run:

```bash
cargo nextest run --workspace --all-features
cargo build --workspace --all-features
```

Expected: all tests/build pass.

- [ ] **Step 2: Run frontend verification**

Run:

```bash
pnpm --dir apps/gateway-admin test
```

Expected: all frontend tests pass.

- [ ] **Step 3: Run final docs scans**

Run:

```bash
rg -n "mcp\\.tootie\\.tv|syslog\\.tootie\\.tv|axon\\.tootie\\.tv|100\\.88|oauth\\.conf|auth_request" apps/gateway-admin docs/deploy/REVERSE_PROXY.md docs/services/GATEWAY.md docs/runtime/OAUTH.md docs/runtime/CONFIG.md
```

Expected: no personal host/private IP matches in UI or canonical docs. `oauth.conf`/`auth_request` may appear only in SWAG migration or legacy sections.

- [ ] **Step 4: Smoke protected route behavior locally**

Start Lab with OAuth config in a test environment, then run:

```bash
labby doctor proxy \
  --app-url https://lab.example.com \
  --mcp-url https://mcp.example.com \
  --route /tools \
  --format json
```

Expected: metadata/challenge checks pass in a properly proxied environment; failures are classified and redacted.

- [ ] **Step 5: Update beads**

After verification, add closure comments with evidence:

```bash
bd comments add lab-mvtg.1 "VERIFICATION: route matching, DCR gating, Host/XFH, redaction, and proxy timeout/body tests passed with <commands>."
bd comments add lab-mvtg.2 "VERIFICATION: reverse proxy docs scanned clean and include required MCP headers."
bd comments add lab-mvtg.3 "VERIFICATION: doctor.proxy.check tests and CLI parse tests passed."
bd comments add lab-mvtg.4 "VERIFICATION: UI tests passed and no personal host defaults remain."
bd comments add lab-mvtg.5 "VERIFICATION: SWAG migration guide includes classifications and smoke expectations."
```

- [ ] **Step 6: Commit final verification notes if docs changed**

```bash
git status --short
git add docs/superpowers/plans/2026-05-09-portable-mcp-gateway.md
git commit -m "docs: plan portable MCP gateway work"
```

---

## Self-Review

Spec coverage:

- `lab-mvtg.1` safety/model fixes are covered by Tasks 1-3.
- `lab-mvtg.2` host-level reverse proxy examples are covered by Task 6.
- `lab-mvtg.3` doctor proxy check is covered by Task 4.
- `lab-mvtg.4` UI fallback removal and deployment guidance are covered by Task 5.
- `lab-mvtg.5` SWAG migration docs are covered by Task 7.
- Epic verification and closure prep are covered by Task 8.

Placeholder scan:

- No placeholder language from the writing-plans checklist is used.
- Deferrable work is explicitly marked in the plan header and not hidden inside implementation steps.
- Every code-changing task includes target files, test steps, implementation shape, and verification commands.

Type consistency:

- `doctor.proxy.check` is used consistently across Rust catalog/dispatch, CLI, and TypeScript client.
- Protected route terminology stays consistent: `LAB_PUBLIC_URL` for issuer, `public_host + public_path` for resource identity.
- UI consistently distinguishes `gateway.protected_route.test` from `doctor.proxy.check`.
