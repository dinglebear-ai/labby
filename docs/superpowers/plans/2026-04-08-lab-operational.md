# Lab Binary Operational Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the `lab` binary from a scaffold with stub returns to a first working state: clean compile on every feature combination, a real `HttpClient`, Retired upstream wired end-to-end (CLI + MCP + HTTP + health), and `lab serve --transport stdio` actually speaking MCP.

**Architecture:** Retired upstream is the reference service — we make it real, then every other service becomes a copy-paste follow-up. Non-Retired upstream services stay as `//! not yet implemented` stub modules that compile under all feature combinations but return an explicit `ApiError::Internal("not yet implemented")` at runtime. `HttpClient` becomes a thin `reqwest` wrapper with auth-header injection and JSON helpers. `lab serve` mounts the real `rmcp` stdio server backed by a `ToolRegistry` that actually contains entries. `lab health`, `lab doctor`, `lab help`, and the axum `/v1/retired-upstream` route all read from that same registry.

**Tech Stack:** Rust 2024, `reqwest` (rustls), `rmcp` 1.3, `axum` 0.8, `clap` derive, `tokio`, `tracing`, `wiremock` (tests).

---

## Scope Check

This plan is one subsystem: "the `lab` binary reaches first working state." The work below is **tightly coupled** — you cannot fix the compile errors without also touching the empty scaffold files, and you cannot demo `lab serve` without at least one real service in the registry. Splitting it further produces plans that don't individually ship working software.

Services other than Retired upstream get **stub-level** treatment only (empty files → `//! not yet implemented` placeholders that satisfy the compiler). Per-service fill-in is **out of scope** — each gets its own future plan once this one lands.

Explicitly out of scope:
- Implementing any non-Retired upstream service's types or endpoints.
- TUI plugin manager logic (`crates/lab/src/tui/app.rs` stays a stub).
- `lab install`/`uninstall`/`init` (stay stubs — they mutate `.mcp.json` which deserves its own plan).
- `extract.apply` / `extract.diff` (already tracked separately).
- HTTP API surface beyond `/health`, `/ready`, and `/v1/retired-upstream/system/status` (one demo route).

---

## File Structure

**Created:**
- `crates/lab-apis/src/retired-upstream/types/common.rs` — shared ids (`Retired upstreamVersion`) for `system/status`.
- `tests/retired-upstream_health.rs` at `crates/lab-apis/tests/` — wiremock integration test for `Retired upstreamClient::health`.
- `tests/http_client.rs` at `crates/lab-apis/tests/` — wiremock integration test for `HttpClient::get_json` auth-header injection.

**Modified (substantive logic):**
- `crates/lab-apis/src/core/http.rs` — real reqwest impl with auth injection, retries TBD later.
- `crates/lab-apis/src/retired-upstream/client.rs` — real `health()` calling `HttpClient::get_json`.
- `crates/lab-apis/src/retired-upstream/client/system.rs` — add `system_status()` method returning `SystemStatus`.
- `crates/lab-apis/src/retired-upstream/types/system.rs` — add minimal `SystemStatus { version, app_name, instance_name }` struct.
- `crates/lab-apis/src/retired-upstream.rs` — impl `ServiceClient` for `Retired upstreamClient`.
- `crates/lab/Cargo.toml` — narrow `default` features to `["retired-upstream"]` only; stop referencing unscaffolded services.
- `crates/lab/src/main.rs` — load config → build registry → pass into dispatch.
- `crates/lab/src/cli.rs` — extend dispatch signature to accept the registry.
- `crates/lab/src/mcp/registry.rs` — `build_default_registry()` actually adds Retired upstream under `#[cfg(feature = "retired-upstream")]`.
- `crates/lab/src/mcp/services.rs` — declare `pub mod retired-upstream;` under the feature.
- `crates/lab/src/mcp/services/retired-upstream.rs` — real dispatch for `system.status` and `help` actions.
- `crates/lab/src/cli/serve.rs` — real stdio MCP server (rmcp) that walks the registry.
- `crates/lab/src/cli/health.rs` — iterate registry, call `ServiceClient::health`, print table/JSON.
- `crates/lab/src/cli/doctor.rs` — env-var presence check per `PluginMeta::required_env` for every registered service.
- `crates/lab/src/cli/retired-upstream.rs` — new file; thin CLI shim for `lab retired-upstream system status`.
- `crates/lab/src/api/router.rs` — mount `/v1/retired-upstream/system/status`.
- `crates/lab/src/api/state.rs` — hold an `Option<Retired upstreamClient>` built from env at startup.
- `crates/lab/src/catalog.rs` — wire `ActionEntry` population from per-service `ACTIONS` slices.
- `crates/lab/src/tui/metadata.rs` — populate with `lab_apis::retired-upstream::META` under the feature gate.

**Stubbed (empty → minimal placeholder content so the compiler stops complaining, no behavior):**
- `crates/lab-apis/src/{retired-upstream,retired-upstream,retired-upstream,retired-upstream,retired-upstream,retired-upstream,retired-upstream,tailscale,linkding,memos,bytestash,paperless,arcane,unraid,unifi,gotify,qdrant,tei,apprise}.rs` — each must exist with just `//! not yet implemented` + a stub `PluginMeta` (see Task 2) so feature gates compile.
- All empty files in `crates/lab-apis/src/*/client.rs,types.rs,error.rs` for the same services — replace `0B` with `//! not yet implemented`.
- `crates/lab/src/cli/{retired-upstream,retired-upstream,retired-upstream,openai,arcane}.rs` — `//! not yet implemented`.
- `crates/lab/src/mcp/services/{retired-upstream,retired-upstream,retired-upstream,openai,arcane}.rs` — `//! not yet implemented`.
- `crates/lab-apis/src/arcane.rs`, `crates/lab-apis/src/retired-upstream.rs`, `crates/lab-apis/src/retired-upstream.rs`, `crates/lab-apis/src/retired-upstream.rs` — already exist empty, fill with the placeholder pattern from Task 2.

---

## Verification Commands

Every task finishes with **one** of these, per its scope. You should never proceed to the next task if these fail.

- `cargo check -p lab-apis --features "retired-upstream retired-upstream"` — retired-upstream sanity.
- `cargo check -p lab --no-default-features --features retired-upstream` — lab binary with only retired-upstream.
- `cargo check -p lab --features all` — everything on, everything compiling (stubs must not break `all`).
- `cargo nextest run -p lab-apis --features "retired-upstream retired-upstream"` — Retired upstream + HttpClient tests green.
- `cargo nextest run -p lab` — lab crate tests green.
- `cargo run -p lab --no-default-features --features retired-upstream -- help` — real help output.
- `cargo run -p lab --no-default-features --features retired-upstream -- health` — table with one retired-upstream row (likely `reachable=false` unless env is set; that's fine — it must not panic).

---

### Task 1: Narrow default features and unblock the compile

**Why this first:** Right now `cargo check -p lab` fails immediately because default features turn on `retired-upstream` and `retired-upstream`, but `lab-apis/src/lib.rs` declares `pub mod retired-upstream;` and `pub mod retired-upstream;` under those features with no file on disk. Every follow-up task would start from a broken baseline without this.

**Files:**
- Modify: `crates/lab/Cargo.toml:60` (the `default = [...]` line)

- [ ] **Step 1: Change the default feature set to Retired upstream-only.**

Edit `crates/lab/Cargo.toml`. Locate:

```toml
default = ["retired-upstream", "retired-upstream", "retired-upstream", "retired-upstream", "retired-upstream", "retired-upstream"]
```

Replace with:

```toml
default = ["retired-upstream"]
```

Retired upstream is the only service that will be fully wired in this plan. `all` still references every service — Task 2 makes that set actually compile.

- [ ] **Step 2: Verify a minimal feature check passes.**

Run: `cargo check -p lab`
Expected: no errors (warnings about unused code are fine).

- [ ] **Step 3: Commit.**

```bash
git add crates/lab/Cargo.toml
git commit -m "chore(lab): narrow default features to retired-upstream while scaffolds are stubs"
```

---

### Task 2: Fill every empty service scaffold with a compilable placeholder

**Why:** `cargo check -p lab --features all` must succeed so we don't rot the `all` feature. That means every service module declared in `lab-apis/src/lib.rs` needs to exist as at least a non-empty file with a valid `PluginMeta`. Most of these files are currently `0B`.

**Files (all one-liner rewrites — same template):**
- `crates/lab-apis/src/arcane.rs`
- `crates/lab-apis/src/retired-upstream.rs`
- `crates/lab-apis/src/retired-upstream.rs`
- `crates/lab-apis/src/retired-upstream.rs`
- `crates/lab-apis/src/retired-upstream.rs` **(create)**
- `crates/lab-apis/src/retired-upstream.rs` **(create)**
- `crates/lab-apis/src/tailscale.rs` **(create)**
- `crates/lab-apis/src/linkding.rs` **(create)**
- `crates/lab-apis/src/memos.rs` **(create)**
- `crates/lab-apis/src/bytestash.rs` **(create)**
- `crates/lab-apis/src/paperless.rs` **(create)**
- `crates/lab-apis/src/unraid.rs` **(create)**
- `crates/lab-apis/src/unifi.rs` **(create)**
- `crates/lab-apis/src/retired-upstream.rs` **(create)**

- [ ] **Step 1: Write the universal placeholder template.**

Every file above must have this exact shape, substituting `{name}`, `{display}`, `{category}`, and `{desc}`. Example for `retired-upstream.rs`:

```rust
//! Retired upstream client — not yet implemented.
//!
//! This module exists so the `retired-upstream` feature compiles. The real client,
//! types, and MCP dispatch are deferred to a per-service plan.

use crate::core::plugin::{Category, PluginMeta};

/// Compile-time metadata for the retired-upstream module.
pub const META: PluginMeta = PluginMeta {
    name: "retired-upstream",
    display_name: "Retired upstream",
    description: "Usenet download client (placeholder — not yet implemented)",
    category: Category::Download,
    docs_url: "https://retired-upstream.org/wiki/",
    required_env: &[],
    optional_env: &[],
    default_port: Some(8080),
};
```

- [ ] **Step 2: Write the same shape for every file listed above.**

Substitutions:

| file | name | display | category | default_port |
|---|---|---|---|---|
| arcane.rs | `arcane` | `Arcane` | `Network` | `3000` |
| retired-upstream.rs | `retired-upstream` | `Retired upstream` | `Media` | `32400` |
| retired-upstream.rs | `retired-upstream` | `Retired upstream` | `Indexer` | `9696` |
| retired-upstream.rs | `retired-upstream` | `Retired upstream` | `Retired upstream` | `8989` |
| retired-upstream.rs | `retired-upstream` | `Retired upstream` | `Download` | `8080` |
| retired-upstream.rs | `retired-upstream` | `retired-upstream` | `Download` | `8080` |
| tailscale.rs | `tailscale` | `Tailscale` | `Network` | `None` |
| linkding.rs | `linkding` | `Linkding` | `Notes` | `9090` |
| memos.rs | `memos` | `Memos` | `Notes` | `5230` |
| bytestash.rs | `bytestash` | `ByteStash` | `Notes` | `5000` |
| paperless.rs | `paperless` | `Paperless-ngx` | `Documents` | `8000` |
| unraid.rs | `unraid` | `Unraid` | `Network` | `None` |
| unifi.rs | `unifi` | `UniFi` | `Network` | `443` |
| retired-upstream.rs | `retired-upstream` | `Retired upstream` | `Media` | `8181` |

For `default_port: None`, write `default_port: None,`. For numeric ports, `default_port: Some(8080),`.

- [ ] **Step 3: Empty sub-module files must also compile.**

Find every `0B` file under `crates/lab-apis/src/*/client.rs`, `*/types.rs`, `*/error.rs` for the stub services, and replace each with:

```rust
//! Not yet implemented.
```

Use this shell loop from the repo root to list them first:

```bash
find crates/lab-apis/src -name "*.rs" -size 0 -print
```

For every path printed, write the one-line doc comment above.

- [ ] **Step 4: Verify the `all` feature compiles.**

Run: `cargo check -p lab-apis --features all`
Expected: no errors.

Run: `cargo check -p lab --features all`
Expected: no errors.

- [ ] **Step 5: Commit.**

```bash
git add crates/lab-apis/src
git commit -m "chore(lab-apis): placeholder META for every stub service so \`all\` compiles"
```

---

### Task 3: Real `HttpClient::get_json` with auth header injection (TDD)

**Files:**
- Modify: `crates/lab-apis/src/core/http.rs`
- Create: `crates/lab-apis/tests/http_client.rs`

- [ ] **Step 1: Write the failing wiremock test.**

Create `crates/lab-apis/tests/http_client.rs`:

```rust
//! Integration test — `HttpClient::get_json` must inject the Auth header
//! and decode a JSON body into a user-provided type.

use serde::Deserialize;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

use lab_apis::core::{Auth, HttpClient};

#[derive(Debug, Deserialize, PartialEq)]
struct Pong {
    message: String,
}

#[tokio::test]
async fn get_json_injects_api_key_header_and_decodes_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ping"))
        .and(header("X-Api-Key", "secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(Pong {
            message: "pong".into(),
        }))
        .mount(&server)
        .await;

    let client = HttpClient::new(
        server.uri(),
        Auth::ApiKey {
            header: "X-Api-Key".into(),
            key: "secret".into(),
        },
    );

    let pong: Pong = client.get_json("/ping").await.expect("get_json");
    assert_eq!(
        pong,
        Pong {
            message: "pong".into()
        }
    );
}
```

- [ ] **Step 2: Run the test to verify it fails.**

Run: `cargo nextest run -p lab-apis --features "retired-upstream retired-upstream" --test http_client`
Expected: FAIL with `ApiError::Internal("HttpClient::get_json not yet implemented")`.

- [ ] **Step 3: Implement the minimal real `HttpClient`.**

Replace the body of `crates/lab-apis/src/core/http.rs` with:

```rust
//! Shared HTTP client — thin reqwest wrapper with auth injection and JSON helpers.

use reqwest::{Client, RequestBuilder};

use crate::core::auth::Auth;
use crate::core::error::ApiError;

/// Shared HTTP client. Cheap to clone — wraps `reqwest::Client` which is `Arc`-based internally.
#[derive(Debug, Clone)]
pub struct HttpClient {
    base_url: String,
    auth: Auth,
    inner: Client,
}

impl HttpClient {
    /// Construct a new client with a base URL and auth strategy.
    #[must_use]
    pub fn new(base_url: impl Into<String>, auth: Auth) -> Self {
        let inner = Client::builder()
            .user_agent(concat!("lab-apis/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest::Client::build");
        Self {
            base_url: base_url.into(),
            auth,
            inner,
        }
    }

    /// Base URL this client targets.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Auth strategy.
    #[must_use]
    pub const fn auth(&self) -> &Auth {
        &self.auth
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_string()
        } else if path.starts_with('/') {
            format!("{}{path}", self.base_url.trim_end_matches('/'))
        } else {
            format!("{}/{path}", self.base_url.trim_end_matches('/'))
        }
    }

    fn apply_auth(&self, req: RequestBuilder) -> RequestBuilder {
        match &self.auth {
            Auth::None => req,
            Auth::ApiKey { header, key } => req.header(header, key),
            Auth::Token { token } => req.header("Authorization", format!("Token {token}")),
            Auth::Bearer { token } => req.bearer_auth(token),
            Auth::Basic { username, password } => req.basic_auth(username, Some(password)),
            Auth::Session { cookie } => req.header("Cookie", cookie),
        }
    }

    /// GET a path and decode JSON.
    ///
    /// # Errors
    /// Returns [`ApiError`] on transport, status, or decode failure.
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, ApiError> {
        let url = self.url(path);
        let resp = self
            .apply_auth(self.inner.get(&url))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        Self::decode(resp).await
    }

    /// POST a JSON body and decode the JSON response.
    ///
    /// # Errors
    /// Returns [`ApiError`] on transport, status, or decode failure.
    pub async fn post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        let url = self.url(path);
        let resp = self
            .apply_auth(self.inner.post(&url).json(body))
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        Self::decode(resp).await
    }

    async fn decode<T: serde::de::DeserializeOwned>(
        resp: reqwest::Response,
    ) -> Result<T, ApiError> {
        let status = resp.status();
        if status.is_success() {
            return resp
                .json::<T>()
                .await
                .map_err(|e| ApiError::Decode(e.to_string()));
        }

        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        Err(match code {
            401 | 403 => ApiError::Auth,
            404 => ApiError::NotFound,
            429 => ApiError::RateLimited { retry_after: None },
            500..=599 => ApiError::Server { status: code, body },
            _ => ApiError::Server { status: code, body },
        })
    }
}
```

- [ ] **Step 4: Run the test to verify it passes.**

Run: `cargo nextest run -p lab-apis --features "retired-upstream retired-upstream" --test http_client`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/lab-apis/src/core/http.rs crates/lab-apis/tests/http_client.rs
git commit -m "feat(lab-apis): real HttpClient with auth injection + JSON helpers"
```

---

### Task 4: Retired upstream `system.status` — real type, real client method (TDD)

**Files:**
- Modify: `crates/lab-apis/src/retired-upstream/types/system.rs`
- Modify: `crates/lab-apis/src/retired-upstream/client/system.rs`
- Create: `crates/lab-apis/tests/retired-upstream_health.rs`

- [ ] **Step 1: Write the failing wiremock test.**

Create `crates/lab-apis/tests/retired-upstream_health.rs`:

```rust
//! Integration test — `Retired upstreamClient::system_status` must hit
//! `GET /api/v3/system/status` with the `X-Api-Key` header and decode the
//! minimal `SystemStatus` shape Retired upstream returns.

use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

use lab_apis::core::Auth;
use lab_apis::retired-upstream::Retired upstreamClient;

#[tokio::test]
async fn system_status_ok() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v3/system/status"))
        .and(header("X-Api-Key", "abc123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "version": "5.0.0.1234",
            "appName": "Retired upstream",
            "instanceName": "Retired upstream"
        })))
        .mount(&server)
        .await;

    let client = Retired upstreamClient::new(
        &server.uri(),
        Auth::ApiKey {
            header: "X-Api-Key".into(),
            key: "abc123".into(),
        },
    );

    let status = client.system_status().await.expect("system_status");
    assert_eq!(status.version, "5.0.0.1234");
    assert_eq!(status.app_name, "Retired upstream");
}
```

- [ ] **Step 2: Run it to verify it fails.**

Run: `cargo nextest run -p lab-apis --features "retired-upstream retired-upstream" --test retired-upstream_health`
Expected: FAIL with either "no method `system_status`" or "SystemStatus not found".

- [ ] **Step 3: Define the `SystemStatus` type.**

Replace the contents of `crates/lab-apis/src/retired-upstream/types/system.rs` with:

```rust
//! Retired upstream `system/status` response shape.
//!
//! Only the fields `lab` actually reads are modeled — the full upstream
//! response has ~30 fields that are uninteresting for a liveness probe.

use serde::{Deserialize, Serialize};

/// Subset of Retired upstream's `/api/v3/system/status` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatus {
    /// Application version string.
    pub version: String,
    /// `"Retired upstream"` always — present for symmetry with Retired upstream/Retired upstream.
    pub app_name: String,
    /// User-configurable instance name (defaults to `"Retired upstream"`).
    pub instance_name: String,
}
```

- [ ] **Step 4: Implement `system_status` on the client.**

Replace the contents of `crates/lab-apis/src/retired-upstream/client/system.rs` with:

```rust
//! `impl Retired upstreamClient` block for `/api/v3/system/*` endpoints.

use super::Retired upstreamClient;
use crate::retired-upstream::error::Retired upstreamError;
use crate::retired-upstream::types::system::SystemStatus;

impl Retired upstreamClient {
    /// `GET /api/v3/system/status`.
    ///
    /// # Errors
    /// Returns [`Retired upstreamError::Api`] on transport, auth, or decode failure.
    pub async fn system_status(&self) -> Result<SystemStatus, Retired upstreamError> {
        self.http
            .get_json("/api/v3/system/status")
            .await
            .map_err(Retired upstreamError::from)
    }
}
```

- [ ] **Step 5: Ensure `Retired upstreamError::Api` accepts `ApiError`.**

Read `crates/lab-apis/src/retired-upstream/error.rs` and confirm it already has `#[from] ApiError` (the scaffold should). If it doesn't, add the variant:

```rust
use crate::core::ApiError;

#[derive(Debug, thiserror::Error)]
pub enum Retired upstreamError {
    #[error(transparent)]
    Api(#[from] ApiError),
}
```

- [ ] **Step 6: Run the test to verify it passes.**

Run: `cargo nextest run -p lab-apis --features "retired-upstream retired-upstream" --test retired-upstream_health`
Expected: PASS.

- [ ] **Step 7: Commit.**

```bash
git add crates/lab-apis/src/retired-upstream crates/lab-apis/tests/retired-upstream_health.rs
git commit -m "feat(lab-apis): retired-upstream system_status endpoint + wiremock test"
```

---

### Task 5: Retired upstream `health()` uses `system_status`, and `impl ServiceClient`

**Files:**
- Modify: `crates/lab-apis/src/retired-upstream/client.rs` (the `health` method only)
- Modify: `crates/lab-apis/src/retired-upstream.rs` (add `ServiceClient` impl below `META`)

- [ ] **Step 1: Rewrite `Retired upstreamClient::health`.**

In `crates/lab-apis/src/retired-upstream/client.rs`, replace the existing `health` method body:

```rust
    /// Probe `GET /api/v3/system/status` as a liveness check.
    ///
    /// # Errors
    /// Returns `Retired upstreamError::Api` if the request fails or the server
    /// returns a non-2xx status.
    pub async fn health(&self) -> Result<(), Retired upstreamError> {
        self.system_status().await.map(|_| ())
    }
```

- [ ] **Step 2: Implement `ServiceClient` for `Retired upstreamClient`.**

Append to `crates/lab-apis/src/retired-upstream.rs` (after the existing `META` constant):

```rust
use std::time::Instant;

use crate::core::{ApiError, ServiceClient, ServiceStatus};

impl ServiceClient for Retired upstreamClient {
    fn name(&self) -> &str {
        "retired-upstream"
    }

    fn service_type(&self) -> &str {
        "retired-upstream"
    }

    async fn health(&self) -> Result<ServiceStatus, ApiError> {
        let start = Instant::now();
        match self.system_status().await {
            Ok(status) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: true,
                version: Some(status.version),
                latency_ms: start.elapsed().as_millis() as u64,
                message: None,
            }),
            Err(crate::retired-upstream::Retired upstreamError::Api(ApiError::Auth)) => Ok(ServiceStatus {
                reachable: true,
                auth_ok: false,
                version: None,
                latency_ms: start.elapsed().as_millis() as u64,
                message: Some("auth failed".into()),
            }),
            Err(crate::retired-upstream::Retired upstreamError::Api(e)) => {
                Ok(ServiceStatus::unreachable(e.to_string()))
            }
        }
    }
}
```

Note: `Retired upstreamClient::health()` and `ServiceClient::health()` are two different methods with different return types — they don't collide (one is an inherent method, the other is a trait method).

- [ ] **Step 3: Verify it compiles.**

Run: `cargo check -p lab-apis --features "retired-upstream retired-upstream"`
Expected: no errors.

- [ ] **Step 4: Commit.**

```bash
git add crates/lab-apis/src/retired-upstream
git commit -m "feat(lab-apis): impl ServiceClient for Retired upstreamClient"
```

---

### Task 6: `build_default_registry` actually registers Retired upstream

**Files:**
- Modify: `crates/lab/src/mcp/registry.rs`

- [ ] **Step 1: Register Retired upstream behind its feature flag.**

Replace `crates/lab/src/mcp/registry.rs` with:

```rust
//! Runtime tool registry. Services register themselves here during
//! startup; the MCP server walks the registry to expose tools and the
//! catalog module walks it to produce discovery docs.

/// Metadata the registry keeps about each registered service.
#[derive(Debug, Clone)]
pub struct RegisteredService {
    /// Service / tool name.
    pub name: &'static str,
    /// Short description from `PluginMeta::description`.
    pub description: &'static str,
    /// Category slug.
    pub category: &'static str,
}

/// Collection of registered services, built at startup.
#[derive(Debug, Default)]
pub struct ToolRegistry {
    services: Vec<RegisteredService>,
}

impl ToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            services: Vec::new(),
        }
    }

    /// Register a service. Duplicates are ignored (first registration wins).
    pub fn register(&mut self, service: RegisteredService) {
        if !self.services.iter().any(|s| s.name == service.name) {
            self.services.push(service);
        }
    }

    /// Borrow the current service list.
    #[must_use]
    pub fn services(&self) -> &[RegisteredService] {
        &self.services
    }
}

/// Build a registry with every feature-enabled service registered.
#[must_use]
pub fn build_default_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        reg.register(RegisteredService {
            name: meta.name,
            description: meta.description,
            category: category_slug(meta.category),
        });
    }

    reg
}

#[cfg(any(feature = "retired-upstream"))]
const fn category_slug(cat: lab_apis::core::Category) -> &'static str {
    use lab_apis::core::Category;
    match cat {
        Category::Media => "media",
        Category::Retired upstream => "retired-upstream",
        Category::Indexer => "indexer",
        Category::Download => "download",
        Category::Notes => "notes",
        Category::Documents => "documents",
        Category::Network => "network",
        Category::Notifications => "notifications",
        Category::Ai => "ai",
        Category::Bootstrap => "bootstrap",
    }
}
```

- [ ] **Step 2: Verify it compiles.**

Run: `cargo check -p lab`
Expected: no errors. (`lab` default features = `["retired-upstream"]` after Task 1.)

- [ ] **Step 3: Commit.**

```bash
git add crates/lab/src/mcp/registry.rs
git commit -m "feat(lab): build_default_registry registers Retired upstream under its feature flag"
```

---

### Task 7: MCP dispatch module for Retired upstream

**Files:**
- Modify: `crates/lab/src/mcp/services.rs`
- Modify (from `0B` placeholder): `crates/lab/src/mcp/services/retired-upstream.rs`

- [ ] **Step 1: Declare the module under its feature gate.**

Replace `crates/lab/src/mcp/services.rs` with:

```rust
//! Per-service dispatch modules.
//!
//! Each enabled service declares one submodule here, feature-gated. Each
//! submodule exposes a `dispatch` async function that takes the action
//! name and a free-form `serde_json::Value` params object.

#[cfg(feature = "retired-upstream")]
pub mod retired-upstream;
```

- [ ] **Step 2: Write the Retired upstream dispatcher.**

Replace `crates/lab/src/mcp/services/retired-upstream.rs` with:

```rust
//! MCP dispatch for the Retired upstream tool.
//!
//! Exposes `system.status` and the built-in `help` action. Additional actions
//! (`movie.search`, `queue.list`, ...) land in follow-up plans.

use anyhow::Result;
use serde_json::{Value, json};

use lab_apis::core::Auth;
use lab_apis::retired-upstream::Retired upstreamClient;

/// Build a Retired upstream client from the default-instance env vars. Returns `None`
/// if either `RETIRED_UPSTREAM_URL` or `RETIRED_UPSTREAM_API_KEY` is missing.
#[must_use]
pub fn client_from_env() -> Option<Retired upstreamClient> {
    let url = std::env::var("RETIRED_UPSTREAM_URL").ok()?;
    let key = std::env::var("RETIRED_UPSTREAM_API_KEY").ok()?;
    Some(Retired upstreamClient::new(
        &url,
        Auth::ApiKey {
            header: "X-Api-Key".into(),
            key,
        },
    ))
}

/// Dispatch one MCP call against the Retired upstream tool.
///
/// # Errors
/// Returns an error if the action is unknown, required env is missing,
/// or the client call fails.
pub async fn dispatch(action: &str, _params: Value) -> Result<Value> {
    match action {
        "help" => Ok(json!({
            "service": "retired-upstream",
            "actions": [
                { "name": "system.status", "description": "Return Retired upstream system status", "destructive": false },
                { "name": "help", "description": "Show this catalog", "destructive": false },
            ]
        })),
        "system.status" => {
            let client = client_from_env()
                .ok_or_else(|| anyhow::anyhow!("missing RETIRED_UPSTREAM_URL or RETIRED_UPSTREAM_API_KEY"))?;
            let status = client.system_status().await?;
            Ok(serde_json::to_value(status)?)
        }
        unknown => anyhow::bail!(
            "unknown action `retired-upstream.{unknown}` — call `retired-upstream.help` for the catalog"
        ),
    }
}
```

- [ ] **Step 3: Verify compile.**

Run: `cargo check -p lab`
Expected: no errors.

- [ ] **Step 4: Commit.**

```bash
git add crates/lab/src/mcp/services.rs crates/lab/src/mcp/services/retired-upstream.rs
git commit -m "feat(lab): MCP dispatch for retired-upstream system.status + help"
```

---

### Task 8: Real `lab serve --transport stdio`

**Files:**
- Modify: `crates/lab/src/cli/serve.rs`

**Note on `rmcp`:** This plan treats `rmcp` 1.3 as a black box — the exact server-builder surface depends on which re-exports `rmcp` exposes. If the API differs from what's written below, adapt the call sites but keep the dispatch shape (one tool per registered service, each routing to `crate::mcp::services::<name>::dispatch`). Do not rewrite the registry to work around an rmcp signature mismatch.

- [ ] **Step 1: Wire the stdio server.**

Replace `crates/lab/src/cli/serve.rs` with:

```rust
//! `lab serve` — start the MCP server.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Args, ValueEnum};

use crate::mcp::registry::{ToolRegistry, build_default_registry};

/// Transport choices for `lab serve`.
#[derive(Debug, Clone, Copy, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Transport {
    /// stdin/stdout framing (default, used by Claude Desktop etc.).
    Stdio,
    /// HTTP transport — requires `LAB_MCP_HTTP_TOKEN` in the environment.
    Http,
}

/// `lab serve` arguments.
#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Comma- or space-separated list of services to enable. Empty = all
    /// registered services.
    #[arg(long, value_delimiter = ',')]
    pub services: Vec<String>,
    /// Transport to use.
    #[arg(long, value_enum, default_value_t = Transport::Stdio)]
    pub transport: Transport,
    /// Bind host for the HTTP transport.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// Bind port for the HTTP transport.
    #[arg(long, default_value_t = 8765)]
    pub port: u16,
}

/// Run the serve subcommand.
pub async fn run(args: ServeArgs) -> Result<ExitCode> {
    let registry = build_default_registry();
    let registry = filter_registry(registry, &args.services);

    match args.transport {
        Transport::Stdio => run_stdio(registry).await,
        Transport::Http => {
            tracing::warn!(host = %args.host, port = args.port, "http transport not yet wired");
            Ok(ExitCode::from(64))
        }
    }
}

fn filter_registry(registry: ToolRegistry, services: &[String]) -> ToolRegistry {
    if services.is_empty() {
        return registry;
    }
    let mut out = ToolRegistry::new();
    for entry in registry.services() {
        if services.iter().any(|s| s == entry.name) {
            out.register(entry.clone());
        }
    }
    out
}

/// Stdio MCP loop. This is a minimal line-delimited JSON-RPC pump so the
/// binary is operational before the full `rmcp` integration lands. Each
/// incoming request is dispatched to the per-service module; responses are
/// emitted as JSON on stdout.
async fn run_stdio(registry: ToolRegistry) -> Result<ExitCode> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    tracing::info!(
        services = registry.services().len(),
        "lab serve (stdio) ready",
    );

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let err = serde_json::json!({
                    "kind": "decode_error",
                    "message": e.to_string(),
                });
                stdout.write_all(err.to_string().as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                continue;
            }
        };

        let service = req.get("service").and_then(|v| v.as_str()).unwrap_or("");
        let action = req.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(serde_json::Value::Null);

        let result = dispatch(&registry, service, action, params).await;
        let body = match result {
            Ok(v) => serde_json::json!({ "data": v }),
            Err(e) => serde_json::json!({ "kind": "internal_error", "message": e.to_string() }),
        };
        stdout.write_all(body.to_string().as_bytes()).await?;
        stdout.write_all(b"\n").await?;
    }

    Ok(ExitCode::SUCCESS)
}

async fn dispatch(
    registry: &ToolRegistry,
    service: &str,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    if !registry.services().iter().any(|s| s.name == service) {
        anyhow::bail!("unknown service `{service}`");
    }
    match service {
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => crate::mcp::services::retired-upstream::dispatch(action, params).await,
        other => anyhow::bail!("service `{other}` has no dispatcher wired"),
    }
}
```

This is **intentionally a placeholder JSON-RPC-lite pump** — not spec-compliant MCP. It exists so `lab serve --transport stdio` does something demonstrable end-to-end against the real `Retired upstreamClient`. Full `rmcp` integration is its own plan.

- [ ] **Step 2: Verify it compiles.**

Run: `cargo check -p lab`
Expected: no errors.

- [ ] **Step 3: Smoke test.**

Run: `echo '{"service":"retired-upstream","action":"help"}' | cargo run -p lab --no-default-features --features retired-upstream -- serve --transport stdio`
Expected: single-line JSON output on stdout containing `"data"` and the `system.status` action entry. Press Ctrl-D or close stdin to exit.

- [ ] **Step 4: Commit.**

```bash
git add crates/lab/src/cli/serve.rs
git commit -m "feat(lab): lab serve --transport stdio with per-service dispatch"
```

---

### Task 9: Real `lab health`

**Files:**
- Modify: `crates/lab/src/cli/health.rs`

- [ ] **Step 1: Implement the health table.**

Replace `crates/lab/src/cli/health.rs` with:

```rust
//! `lab health` — quick reachability ping for every configured service.

use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;

use crate::output::{OutputFormat, print};

/// One row of the health report.
#[derive(Debug, Clone, Serialize)]
pub struct HealthRow {
    /// Service identifier.
    pub service: String,
    /// Whether the base URL responded successfully.
    pub reachable: bool,
    /// Whether credentials were accepted.
    pub auth_ok: bool,
    /// Reported version, if any.
    pub version: Option<String>,
    /// Round-trip latency in milliseconds.
    pub latency_ms: u64,
    /// Error / info message, if any.
    pub message: Option<String>,
}

/// Run the health subcommand.
pub async fn run(format: OutputFormat) -> Result<ExitCode> {
    let mut rows: Vec<HealthRow> = Vec::new();

    #[cfg(feature = "retired-upstream")]
    rows.push(retired-upstream_row().await);

    print(&rows, format)?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(feature = "retired-upstream")]
async fn retired-upstream_row() -> HealthRow {
    use lab_apis::core::ServiceClient;

    let Some(client) = crate::mcp::services::retired-upstream::client_from_env() else {
        return HealthRow {
            service: "retired-upstream".into(),
            reachable: false,
            auth_ok: false,
            version: None,
            latency_ms: 0,
            message: Some("RETIRED_UPSTREAM_URL / RETIRED_UPSTREAM_API_KEY not set".into()),
        };
    };

    match client.health().await {
        Ok(s) => HealthRow {
            service: "retired-upstream".into(),
            reachable: s.reachable,
            auth_ok: s.auth_ok,
            version: s.version,
            latency_ms: s.latency_ms,
            message: s.message,
        },
        Err(e) => HealthRow {
            service: "retired-upstream".into(),
            reachable: false,
            auth_ok: false,
            version: None,
            latency_ms: 0,
            message: Some(e.to_string()),
        },
    }
}
```

- [ ] **Step 2: Verify it compiles.**

Run: `cargo check -p lab`
Expected: no errors.

- [ ] **Step 3: Smoke test.**

Run: `cargo run -p lab --no-default-features --features retired-upstream -- health`
Expected: a JSON array with one entry for retired-upstream. Without env vars set the message should say `"RETIRED_UPSTREAM_URL / RETIRED_UPSTREAM_API_KEY not set"` — no panic, no error exit.

- [ ] **Step 4: Commit.**

```bash
git add crates/lab/src/cli/health.rs
git commit -m "feat(lab): lab health iterates registry and pings Retired upstream"
```

---

### Task 10: Real `lab doctor` — env-var presence checks

**Files:**
- Modify: `crates/lab/src/cli/doctor.rs`

- [ ] **Step 1: Walk `PluginMeta::required_env` per registered service.**

Replace the body of `run` in `crates/lab/src/cli/doctor.rs` with:

```rust
/// Run the doctor subcommand.
pub async fn run(format: OutputFormat) -> Result<ExitCode> {
    let mut findings: Vec<Finding> = Vec::new();

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        for env in meta.required_env {
            let present = std::env::var(env.name).is_ok();
            findings.push(Finding {
                service: meta.name.into(),
                check: format!("env:{}", env.name),
                severity: if present {
                    Severity::Ok
                } else {
                    Severity::Fail
                },
                message: if present {
                    format!("{} is set", env.name)
                } else {
                    format!("{} is missing ({})", env.name, env.description)
                },
            });
        }
    }

    let report = Report { findings };
    print(&report, format)?;

    let worst = report.findings.iter().map(|f| f.severity).fold(
        Severity::Ok,
        |acc, s| match (acc, s) {
            (Severity::Fail, _) | (_, Severity::Fail) => Severity::Fail,
            (Severity::Warn, _) | (_, Severity::Warn) => Severity::Warn,
            _ => Severity::Ok,
        },
    );

    Ok(match worst {
        Severity::Ok => ExitCode::SUCCESS,
        Severity::Warn => ExitCode::from(1),
        Severity::Fail => ExitCode::from(2),
    })
}
```

Keep the existing `Severity`, `Finding`, `Report` definitions above — do not touch them.

- [ ] **Step 2: Add `Clone, Copy` to `Severity` if not already.**

Read the top of the file. The existing `Severity` derives `Debug, Clone, Copy, Serialize` — it is — leave it.

- [ ] **Step 3: Verify it compiles.**

Run: `cargo check -p lab`
Expected: no errors.

- [ ] **Step 4: Smoke test.**

Run: `cargo run -p lab --no-default-features --features retired-upstream -- doctor`
Expected: JSON report with two retired-upstream findings (one per required env var), both likely `"severity":"fail"` when nothing is set. Exit code 2.

- [ ] **Step 5: Commit.**

```bash
git add crates/lab/src/cli/doctor.rs
git commit -m "feat(lab): lab doctor checks PluginMeta::required_env per service"
```

---

### Task 11: `lab help` reflects the real registry

**Files:**
- Modify: `crates/lab/src/catalog.rs`

The existing `build_catalog` already walks the registry — it just sets `actions: Vec::new()`. Populate it.

- [ ] **Step 1: Add a per-service action list.**

Replace `build_catalog` in `crates/lab/src/catalog.rs`:

```rust
/// Build a [`Catalog`] from the current tool registry.
#[must_use]
pub fn build_catalog(registry: &ToolRegistry) -> Catalog {
    let services = registry
        .services()
        .iter()
        .map(|svc| ServiceCatalog {
            name: svc.name.to_string(),
            description: svc.description.to_string(),
            category: svc.category.to_string(),
            actions: actions_for(svc.name),
        })
        .collect();

    Catalog { services }
}

fn actions_for(service: &str) -> Vec<ActionEntry> {
    match service {
        #[cfg(feature = "retired-upstream")]
        "retired-upstream" => vec![
            ActionEntry {
                name: "system.status".into(),
                description: "Return Retired upstream system status".into(),
                destructive: false,
            },
            ActionEntry {
                name: "help".into(),
                description: "Show this catalog".into(),
                destructive: false,
            },
        ],
        _ => Vec::new(),
    }
}
```

- [ ] **Step 2: Verify it compiles and smoke test.**

Run: `cargo run -p lab --no-default-features --features retired-upstream -- help`
Expected: JSON catalog with one service entry containing two actions.

- [ ] **Step 3: Commit.**

```bash
git add crates/lab/src/catalog.rs
git commit -m "feat(lab): lab help populates per-service action entries"
```

---

### Task 12: `tui::metadata::all_plugins` lists Retired upstream

**Files:**
- Modify: `crates/lab/src/tui/metadata.rs`

- [ ] **Step 1: Return one row for Retired upstream.**

Replace `all_plugins` in `crates/lab/src/tui/metadata.rs`:

```rust
/// Return every compiled-in plugin.
#[must_use]
pub fn all_plugins() -> Vec<PluginRow> {
    let mut rows = Vec::new();

    #[cfg(feature = "retired-upstream")]
    {
        let meta = lab_apis::retired-upstream::META;
        rows.push(PluginRow {
            name: meta.name,
            description: meta.description,
            category: match meta.category {
                lab_apis::core::Category::Retired upstream => "retired-upstream",
                _ => "other",
            },
        });
    }

    rows
}
```

- [ ] **Step 2: Verify it compiles.**

Run: `cargo check -p lab`
Expected: no errors.

- [ ] **Step 3: Commit.**

```bash
git add crates/lab/src/tui/metadata.rs
git commit -m "feat(lab): tui metadata lists Retired upstream under its feature gate"
```

---

### Task 13: HTTP API `/v1/retired-upstream/system/status` demo route

**Files:**
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Hold an optional `Retired upstreamClient` in `AppState`.**

Replace `crates/lab/src/api/state.rs`:

```rust
//! Shared application state for axum handlers.

use std::sync::Arc;

#[cfg(feature = "retired-upstream")]
use lab_apis::retired-upstream::Retired upstreamClient;

/// Application state passed to every axum handler via `State<AppState>`.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

struct AppStateInner {
    #[cfg(feature = "retired-upstream")]
    retired-upstream: Option<Retired upstreamClient>,
}

impl AppState {
    /// Construct a new `AppState` by reading env vars for each enabled service.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                #[cfg(feature = "retired-upstream")]
                retired-upstream: crate::mcp::services::retired-upstream::client_from_env(),
            }),
        }
    }

    /// Borrow the optional Retired upstream client.
    #[cfg(feature = "retired-upstream")]
    #[must_use]
    pub fn retired-upstream(&self) -> Option<&Retired upstreamClient> {
        self.inner.retired-upstream.as_ref()
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 2: Mount the demo route.**

Replace `crates/lab/src/api/router.rs`:

```rust
//! Top-level axum router builder.

use std::time::Duration;

use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use tower_http::{
    compression::CompressionLayer, cors::CorsLayer, timeout::TimeoutLayer,
    trace::TraceLayer,
};

use super::{error::ApiResult, health, state::AppState};

/// Build the full `lab` HTTP router.
#[must_use]
pub fn build_router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/health", get(health::health))
        .route("/ready", get(health::ready));

    #[cfg(feature = "retired-upstream")]
    {
        router = router.route("/v1/retired-upstream/system/status", get(retired-upstream_system_status));
    }

    router
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(CompressionLayer::new())
        .layer(CorsLayer::permissive())
}

#[cfg(feature = "retired-upstream")]
async fn retired-upstream_system_status(
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    let Some(client) = state.retired-upstream() else {
        return Err(super::error::ApiError::UnknownInstance("retired-upstream".into()));
    };
    let status = client.system_status().await.map_err(|e| match e {
        lab_apis::retired-upstream::Retired upstreamError::Api(err) => super::error::ApiError::Sdk(err),
    })?;
    Ok(Json(serde_json::to_value(status).unwrap_or_default()))
}
```

- [ ] **Step 3: Verify it compiles.**

Run: `cargo check -p lab`
Expected: no errors.

- [ ] **Step 4: Commit.**

```bash
git add crates/lab/src/api
git commit -m "feat(lab): HTTP demo route /v1/retired-upstream/system/status"
```

---

### Task 14: Clean up empty CLI/MCP stub files

Empty 0B files exist at `crates/lab/src/cli/{retired-upstream,retired-upstream,retired-upstream,openai,arcane}.rs` and `crates/lab/src/mcp/services/{retired-upstream,retired-upstream,retired-upstream,openai,arcane}.rs`. They cause warnings or break once the parent modules declare them. Task 2 covered `lab-apis`; this task does the same for `lab`.

**Files:**
- Modify: all 10 listed above (find with the shell loop below).

- [ ] **Step 1: Find every empty `.rs` under `crates/lab/src`.**

Run: `find crates/lab/src -name "*.rs" -size 0 -print`

- [ ] **Step 2: Replace each with a one-line placeholder.**

For every file printed, overwrite with:

```rust
//! Not yet implemented.
```

These modules are **not** declared anywhere in `cli.rs` / `mcp/services.rs` yet (they're dangling), so the placeholder is purely housekeeping. Do **not** add `pub mod retired-upstream;` etc. to the parent — those come in per-service follow-up plans.

- [ ] **Step 3: Verify no regressions.**

Run: `cargo check -p lab --features all`
Expected: no errors.

- [ ] **Step 4: Commit.**

```bash
git add crates/lab/src/cli crates/lab/src/mcp/services
git commit -m "chore(lab): placeholder doc-comment in dangling CLI/MCP stub files"
```

---

### Task 15: Full-workspace green-check gate

- [ ] **Step 1: Full check, default features.**

Run: `cargo check -p lab`
Expected: clean.

- [ ] **Step 2: Full check, all features.**

Run: `cargo check -p lab --features all`
Expected: clean. Warnings about unused Retired upstream resource files are fine.

- [ ] **Step 3: Run every test.**

Run: `cargo nextest run -p lab-apis --features "retired-upstream retired-upstream"`
Expected: all tests pass (the two new wiremock tests + any pre-existing).

Run: `cargo nextest run -p lab`
Expected: all tests pass (lab has no tests yet — this just confirms the build).

- [ ] **Step 4: Smoke-test every operational command.**

```bash
cargo run -p lab -- help
cargo run -p lab -- health
cargo run -p lab -- doctor || true   # exit 2 is expected when env is unset
echo '{"service":"retired-upstream","action":"help"}' | cargo run -p lab -- serve --transport stdio
```

Each should emit JSON and exit without panicking.

- [ ] **Step 5: Final commit.**

If the steps above required any small fixups, commit them:

```bash
git add -A
git commit -m "chore(lab): final green-check gate" || true
```

---

## Self-Review

**Spec coverage against "get crates/lab operational":**
- Compile errors fixed — Task 1 (narrow default) + Task 2 (fill every stub). ✅
- Real HTTP — Task 3. ✅
- Reference service end-to-end — Tasks 4, 5, 6, 7 (types, client method, `ServiceClient`, registry, MCP dispatcher). ✅
- `lab serve` operational — Task 8 (acknowledged placeholder JSON-RPC-lite, not full rmcp). ✅
- `lab health` / `lab doctor` / `lab help` wired — Tasks 9, 10, 11. ✅
- TUI metadata populated — Task 12. ✅
- HTTP API demo — Task 13. ✅
- Stub cleanup — Task 14. ✅
- Full-workspace gate — Task 15. ✅

**Gaps I'm aware of and explicitly deferring:**
- `rmcp` full protocol compliance: Task 8 uses a line-delimited JSON shim. Replacing it with real `rmcp` server-builder wiring is a follow-up — it requires reading rmcp 1.3's actual surface, which is outside the scope of a single plan.
- HTTP transport for `lab serve`: still prints a warning and exits. Separate plan.
- `lab install`/`uninstall`/`init`: still stubs. Separate plan.
- All non-Retired upstream services: still stubs. Per-service follow-up plans, one each.
- `extract.apply` / `extract.diff` real implementations: already tracked separately.
- Per-service CLI subcommands (`lab retired-upstream system status`, etc.): the plan deliberately doesn't add `lab retired-upstream` because it would double the scope. `lab serve` + `lab health` + `lab doctor` are enough to prove the binary is operational end-to-end.

**Placeholder scan:** Task 8 explicitly calls out the stdio placeholder; Task 4 documents the minimal SystemStatus subset. No "TBD" / "implement later" / "similar to Task N" patterns elsewhere.

**Type consistency:** `client_from_env` is declared in `mcp/services/retired-upstream.rs` (Task 7) and reused by `cli/health.rs` (Task 9) and `api/state.rs` (Task 13) — spelled consistently. `SystemStatus` field casing (`version`, `app_name`, `instance_name`) matches the serde `rename_all = "camelCase"` so the wiremock fixture in Task 4 (`appName`, `instanceName`) decodes correctly. `ServiceClient::health()` and `Retired upstreamClient::health()` coexist as one trait method + one inherent method — intentional.

---

Plan complete and saved to `docs/superpowers/plans/2026-04-08-lab-operational.md`. Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
