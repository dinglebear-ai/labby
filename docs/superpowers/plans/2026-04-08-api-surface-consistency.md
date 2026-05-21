# API Surface Consistency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three improvements to the HTTP API surface: migrate stub handlers to the canonical `ToolError` type, add `GET /v1/{service}/actions` discovery endpoint, and wire `lab serve --transport http` to actually start the axum server.

**Architecture:** All three tasks share zero overlap — they can be implemented independently, committed separately. `ToolError` (in `crates/lab/src/mcp/envelope.rs`) is already the canonical cross-surface error type; handlers just need to use it directly instead of wrapping in `ApiError`. The actions endpoint is a single shared route in `router.rs` backed by a pre-built `Arc<Catalog>` added to `AppState`. HTTP transport reuses `build_router` from `api/router.rs`.

**Tech Stack:** Rust, axum 0.8, tower-http, tokio, `crate::mcp::envelope::ToolError`, `crate::catalog::{Catalog, build_catalog}`, `crate::mcp::registry::build_default_registry`

---

## File Map

| File | Change |
|------|--------|
| `crates/lab/src/api/state.rs` | Add `Arc<Catalog>` field, update `new()` |
| `crates/lab/src/api/router.rs` | Add `GET /v1/{service}/actions` route + handler |
| `crates/lab/src/catalog.rs` | Make `convert_actions` pub |
| `crates/lab/src/api/services/sonarr.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/prowlarr.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/plex.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/tautulli.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/sabnzbd.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/qbittorrent.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/tailscale.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/linkding.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/memos.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/bytestash.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/paperless.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/arcane.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/unraid.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/unifi.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/overseerr.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/gotify.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/openai.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/qdrant.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/tei.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/apprise.rs` | Migrate to `ToolError` |
| `crates/lab/src/api/services/extract.rs` | Migrate to `ToolError` |
| `crates/lab/src/cli/serve.rs` | Wire `Transport::Http` arm |

---

## Task 1: Migrate stub API handlers to ToolError

**Context:** Every stub handler currently returns `ApiResult<Json<Value>>` and wraps the dispatch error with `ApiError::Internal(e.to_string())`. That produces `{"kind":"internal_error"}` for *every* error from those services, discarding the real kind. The radarr handler (already done) is the reference. We need to do the same for all 20 remaining services.

**Current pattern (all 20 stubs look like this):**
```rust
// crates/lab/src/api/services/sonarr.rs
use crate::api::{error::{ApiError, ApiResult}, state::AppState};

async fn handle(...) -> ApiResult<Json<Value>> {
    crate::mcp::services::sonarr::dispatch(&req.action, req.params)
        .await
        .map(Json)
        .map_err(|e| ApiError::Internal(e.to_string()))
}
```

**Target pattern (identical to radarr):**
```rust
use crate::api::state::AppState;

async fn handle(...) -> Result<Json<Value>, crate::mcp::envelope::ToolError> {
    let start = std::time::Instant::now();
    let action = req.action.clone();
    let result = crate::mcp::services::sonarr::dispatch(&req.action, req.params).await;
    let elapsed_ms = start.elapsed().as_millis();
    match &result {
        Ok(_) => tracing::info!(service = "sonarr", action, elapsed_ms, "dispatch ok"),
        Err(e) => tracing::warn!(service = "sonarr", action, elapsed_ms, kind = e.kind(), "dispatch error"),
    }
    result.map(Json)
}
```

**Files:**
- Modify: `crates/lab/src/api/services/sonarr.rs`
- Modify: `crates/lab/src/api/services/prowlarr.rs`
- Modify: `crates/lab/src/api/services/plex.rs`
- Modify: `crates/lab/src/api/services/tautulli.rs`
- Modify: `crates/lab/src/api/services/sabnzbd.rs`
- Modify: `crates/lab/src/api/services/qbittorrent.rs`
- Modify: `crates/lab/src/api/services/tailscale.rs`
- Modify: `crates/lab/src/api/services/linkding.rs`
- Modify: `crates/lab/src/api/services/memos.rs`
- Modify: `crates/lab/src/api/services/bytestash.rs`
- Modify: `crates/lab/src/api/services/paperless.rs`
- Modify: `crates/lab/src/api/services/arcane.rs`
- Modify: `crates/lab/src/api/services/unraid.rs`
- Modify: `crates/lab/src/api/services/unifi.rs`
- Modify: `crates/lab/src/api/services/overseerr.rs`
- Modify: `crates/lab/src/api/services/gotify.rs`
- Modify: `crates/lab/src/api/services/openai.rs`
- Modify: `crates/lab/src/api/services/qdrant.rs`
- Modify: `crates/lab/src/api/services/tei.rs`
- Modify: `crates/lab/src/api/services/apprise.rs`
- Modify: `crates/lab/src/api/services/extract.rs`

- [ ] **Step 1: Apply the migration to all 21 service files**

  Replace every occurrence of the old pattern. The exact new content for each file follows the same template — only the service name changes. Apply to all 21 services: `sonarr`, `prowlarr`, `plex`, `tautulli`, `sabnzbd`, `qbittorrent`, `tailscale`, `linkding`, `memos`, `bytestash`, `paperless`, `arcane`, `unraid`, `unifi`, `overseerr`, `gotify`, `openai`, `qdrant`, `tei`, `apprise`, `extract`.

  Template (substitute `SERVICE_NAME` → actual service name, e.g. `sonarr`):
  ```rust
  //! HTTP route group for the `SERVICE_NAME` service.

  use axum::{Json, Router, extract::State, routing::post};
  use serde::Deserialize;
  use serde_json::Value;

  use crate::api::state::AppState;

  #[derive(Debug, Deserialize)]
  pub struct ActionRequest {
      pub action: String,
      #[serde(default)]
      pub params: Value,
  }

  pub fn routes(_state: AppState) -> Router<AppState> {
      Router::new()
          .route("/", post(handle))
  }

  async fn handle(
      State(_state): State<AppState>,
      Json(req): Json<ActionRequest>,
  ) -> Result<Json<Value>, crate::mcp::envelope::ToolError> {
      let start = std::time::Instant::now();
      let action = req.action.clone();
      let result = crate::mcp::services::SERVICE_NAME::dispatch(&req.action, req.params).await;
      let elapsed_ms = start.elapsed().as_millis();
      match &result {
          Ok(_) => tracing::info!(service = "SERVICE_NAME", action, elapsed_ms, "dispatch ok"),
          Err(e) => tracing::warn!(service = "SERVICE_NAME", action, elapsed_ms, kind = e.kind(), "dispatch error"),
      }
      result.map(Json)
  }
  ```

  Note: the `extract` service dispatch returns `anyhow::Result<Value>`, not `Result<Value, ToolError>`. Check `crates/lab/src/mcp/services/extract.rs` — if it still returns `anyhow::Result`, its stub handler needs a different error mapping. If so, convert the anyhow error to `ToolError::Sdk`:
  ```rust
  // for extract only, until its dispatch is migrated:
  let result = crate::mcp::services::extract::dispatch(&req.action, req.params)
      .await
      .map_err(|e| crate::mcp::envelope::ToolError::Sdk {
          sdk_kind: "internal_error".into(),
          message: e.to_string(),
      });
  ```

- [ ] **Step 2: Verify it compiles**

  Run: `rtk cargo check --all-features 2>&1`
  
  Expected: zero errors. Fix any type mismatch — most likely `extract` dispatch if it still returns `anyhow::Result`.

- [ ] **Step 3: Run tests**

  Run: `rtk cargo test --workspace --all-features 2>&1`

  Expected: 24 passed (same as before — this task adds no new tests since the behavior is unchanged; real tests land in Task 2).

- [ ] **Step 4: Commit**

  ```bash
  rtk git add crates/lab/src/api/services/
  rtk git commit -m "fix(api): migrate stub handlers from ApiError::Internal to ToolError"
  ```

---

## Task 2: Add AppState catalog + GET /v1/{service}/actions

**Context:** `api/CLAUDE.md` specifies `GET /v1/<service>/actions` mirroring the `lab://<service>/actions` MCP resource. The implementation needs one place to get action metadata without duplicating `actions_for()` per handler. Solution: put a pre-built `Arc<Catalog>` in `AppState`, then add a single `GET /v1/{service}/actions` route in `router.rs` with a path parameter.

`Catalog` is already defined in `crates/lab/src/catalog.rs`. It serializes to:
```json
{
  "services": [
    {
      "name": "radarr",
      "description": "...",
      "category": "Servarr",
      "actions": [
        { "name": "movie.search", "description": "...", "destructive": false }
      ]
    }
  ]
}
```

The `GET /v1/{service}/actions` endpoint returns only the `actions` array for the named service.

**Files:**
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/api/router.rs`

- [ ] **Step 1: Update AppState to hold a pre-built catalog**

  Replace the entire content of `crates/lab/src/api/state.rs`:
  ```rust
  //! Shared application state for axum handlers.

  use std::sync::Arc;

  use crate::catalog::{Catalog, build_catalog};
  use crate::mcp::registry::build_default_registry;

  /// Application state passed to every axum handler via `State<AppState>`.
  #[derive(Clone)]
  pub struct AppState {
      /// Pre-built service+action catalog for discovery endpoints.
      pub catalog: Arc<Catalog>,
  }

  impl AppState {
      /// Build state from the default (all enabled features) registry.
      #[must_use]
      pub fn new() -> Self {
          let registry = build_default_registry();
          Self {
              catalog: Arc::new(build_catalog(&registry)),
          }
      }
  }

  impl Default for AppState {
      fn default() -> Self {
          Self::new()
      }
  }
  ```

- [ ] **Step 2: Add the actions route to router.rs**

  In `crates/lab/src/api/router.rs`, add the import for `Path` extractor at the top:
  ```rust
  use axum::{Router, extract::{Path, State}, http::{HeaderName, StatusCode}, routing::get};
  ```

  Then add the route after the `/ready` route (before the per-service nests), and add the handler function at the bottom of the file:

  After `router = router.nest("/v1/extract", ...)`, add before the `#[cfg(feature = "radarr")]` block:
  ```rust
  router = router.route("/v1/{service}/actions", get(service_actions));
  ```

  And at the bottom of the file (before the closing `}`), add:
  ```rust
  async fn service_actions(
      State(state): State<AppState>,
      Path(service): Path<String>,
  ) -> Result<axum::Json<serde_json::Value>, crate::mcp::envelope::ToolError> {
      let entry = state
          .catalog
          .services
          .iter()
          .find(|s| s.name == service)
          .ok_or_else(|| crate::mcp::envelope::ToolError::UnknownInstance {
              message: format!("unknown service `{service}`"),
              valid: state.catalog.services.iter().map(|s| s.name.clone()).collect(),
          })?;
      Ok(axum::Json(
          serde_json::to_value(&entry.actions)
              .unwrap_or(serde_json::Value::Array(vec![])),
      ))
  }
  ```

- [ ] **Step 3: Verify it compiles**

  Run: `rtk cargo check --all-features 2>&1`

  Expected: zero errors. Common issues:
  - `Path` import conflict with `std::path::Path` — use the full `axum::extract::Path` if so.
  - `build_default_registry` not in scope — it's in `crate::mcp::registry`.

- [ ] **Step 4: Write a test for the actions endpoint**

  Add a test module at the bottom of `crates/lab/src/api/router.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use axum::body::Body;
      use axum::http::{Request, StatusCode};
      use tower::ServiceExt; // for .oneshot()

      use super::*;

      #[tokio::test]
      async fn actions_known_service_returns_200() {
          let state = AppState::new();
          let app = build_router(state);
          let response = app
              .oneshot(
                  Request::builder()
                      .method("GET")
                      .uri("/v1/extract/actions")
                      .body(Body::empty())
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(response.status(), StatusCode::OK);
          let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
          let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
          assert!(json.is_array(), "body should be a JSON array");
      }

      #[tokio::test]
      async fn actions_unknown_service_returns_400() {
          let state = AppState::new();
          let app = build_router(state);
          let response = app
              .oneshot(
                  Request::builder()
                      .method("GET")
                      .uri("/v1/doesnotexist/actions")
                      .body(Body::empty())
                      .unwrap(),
              )
              .await
              .unwrap();
          assert_eq!(response.status(), StatusCode::BAD_REQUEST);
          let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
          let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
          assert_eq!(json["kind"], "unknown_instance");
      }
  }
  ```

  Check what tower dep exposes `ServiceExt` — it may need `use tower::ServiceExt` or the axum test helpers. If `tower` isn't a direct dep, add to `Cargo.toml` under `[dev-dependencies]`:
  ```toml
  tower = { workspace = true, features = ["util"] }
  ```
  Check if `tower` is in the workspace `Cargo.toml` first:
  ```bash
  rtk grep "^tower" /home/jmagar/workspace/lab/Cargo.toml
  ```

- [ ] **Step 5: Run the tests**

  Run: `rtk cargo test --workspace --all-features 2>&1`

  Expected: 26 passed (24 existing + 2 new). If `tower::ServiceExt` isn't available, check the axum docs for `Router::into_service()` + `tower::Service::call()` as an alternative.

- [ ] **Step 6: Commit**

  ```bash
  rtk git add crates/lab/src/api/state.rs crates/lab/src/api/router.rs crates/lab/Cargo.toml
  rtk git commit -m "feat(api): add GET /v1/{service}/actions discovery endpoint"
  ```

---

## Task 3: Wire lab serve --transport http

**Context:** `crates/lab/src/cli/serve.rs` has `Transport::Http` arm that currently logs a warning and exits with code 64. The axum HTTP server is fully built in `crates/lab/src/api/` — `build_router(AppState)` produces a ready router. This task just starts it.

The `ServeArgs` already has `host: String` and `port: u16` with defaults `127.0.0.1` and `8765`. The existing docstring says "requires `LAB_MCP_HTTP_TOKEN` in the environment" — log a warning if it's absent but still start (homelab default: unauthenticated, per `api/CLAUDE.md`).

**Files:**
- Modify: `crates/lab/src/cli/serve.rs`

- [ ] **Step 1: Add the run_http function**

  In `crates/lab/src/cli/serve.rs`, add this import at the top:
  ```rust
  use crate::api::{router::build_router, state::AppState};
  ```

  Replace the `Transport::Http` arm:
  ```rust
  // Before:
  Transport::Http => {
      tracing::warn!(host = %args.host, port = args.port, "http transport not yet wired");
      Ok(ExitCode::from(64))
  }

  // After:
  Transport::Http => run_http(args.host, args.port).await,
  ```

  Add the `run_http` function at the bottom of the file:
  ```rust
  async fn run_http(host: String, port: u16) -> Result<ExitCode> {
      if std::env::var("LAB_MCP_HTTP_TOKEN").is_err() {
          tracing::warn!("LAB_MCP_HTTP_TOKEN not set — HTTP API is unauthenticated");
      }

      let state = AppState::new();
      let router = build_router(state);
      let addr = format!("{host}:{port}");
      let listener = tokio::net::TcpListener::bind(&addr).await?;

      tracing::info!(%addr, "lab serve (http) ready");
      axum::serve(listener, router).await?;
      Ok(ExitCode::SUCCESS)
  }
  ```

- [ ] **Step 2: Verify it compiles**

  Run: `rtk cargo check --all-features 2>&1`

  Expected: zero errors. Common issue: `axum::serve` signature — in axum 0.8 it's `axum::serve(listener, router.into_make_service())`. Check the axum version:
  ```bash
  rtk grep "^axum" /home/jmagar/workspace/lab/crates/lab/Cargo.toml
  ```
  If axum 0.8: `axum::serve(listener, router.into_make_service()).await?`

- [ ] **Step 3: Run tests**

  Run: `rtk cargo test --workspace --all-features 2>&1`

  Expected: still 26 passed — this task adds no new tests (the HTTP server is an integration concern; unit-testing it would require a full TCP bind which is fragile in CI).

- [ ] **Step 4: Smoke test manually (optional but recommended)**

  ```bash
  cargo run --all-features -- serve --transport http --port 9999 &
  sleep 1
  curl -s http://localhost:9999/health | jq .
  # Expected: {"status":"ok"}
  curl -s http://localhost:9999/v1/extract/actions | jq .
  # Expected: JSON array of extract actions
  curl -s http://localhost:9999/v1/doesnotexist/actions | jq .
  # Expected: {"kind":"unknown_instance","message":"...","valid":[...]}
  kill %1
  ```

- [ ] **Step 5: Commit**

  ```bash
  rtk git add crates/lab/src/cli/serve.rs
  rtk git commit -m "feat(serve): wire --transport http to start axum server"
  ```

---

## Self-Review

**Spec coverage:**
- ✅ 21 stub handlers migrated to `ToolError`
- ✅ `GET /v1/{service}/actions` endpoint added (single shared handler)
- ✅ `lab serve --transport http` wired

**Placeholder scan:**
- None. All code blocks are complete.

**Type consistency:**
- `AppState::catalog` is `Arc<Catalog>` — `Catalog` is from `crate::catalog`, `build_catalog` returns `Catalog`, `Arc::new(build_catalog(...))` produces `Arc<Catalog>`. ✓
- `service_actions` handler returns `Result<axum::Json<serde_json::Value>, ToolError>` — `ToolError` implements `IntoResponse`, `serde_json::Value` serializes fine. ✓
- `run_http` imports `build_router` and `AppState` — both are `pub` in their respective modules. ✓

**Potential gotcha — axum::serve API:**
In axum 0.8 the signature is:
```rust
axum::serve(listener, router.into_make_service()).await?
```
If the project is on axum 0.7 it may differ. Check version before implementing Task 3 Step 2.
