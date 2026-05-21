# Lab Auth Google MCP OAuth Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Google-backed internal MCP OAuth authorization server for the HTTP transport without breaking existing static bearer-token deployments, and remove the old external-issuer/JWKS validation path.

**Architecture:** Create a new `crates/lab-auth` crate that owns OAuth metadata, guarded client registration, authorize/callback/token routes, Google integration, JWT/JWKS, and SQLite-backed auth state. Keep `crates/lab` as the only MCP/`rmcp` crate: it mounts `lab-auth` routes into the axum app and uses one combined auth middleware that supports `LAB_AUTH_MODE=bearer|oauth`.

**Tech Stack:** Rust 2024, `axum`, `tokio`, `rusqlite`, `serde`, `jsonwebtoken`, `rsa`, `sha2`, `tempfile`

---

## File Structure

### New crate: `crates/lab-auth`

- Create: `crates/lab-auth/Cargo.toml`
  Purpose: Isolated auth-server crate with no `rmcp` dependency.
- Create: `crates/lab-auth/src/lib.rs`
  Purpose: Public exports for config, state, router, JWT verification, and shared auth types consumed by `lab`.
- Create: `crates/lab-auth/src/config.rs`
  Purpose: `LAB_AUTH_MODE`, `LAB_PUBLIC_URL`, Google credentials, default paths/TTLs, and startup validation.
- Create: `crates/lab-auth/src/error.rs`
  Purpose: Typed auth-server errors plus mapping to stable `kind` values and HTTP status codes.
- Create: `crates/lab-auth/src/types.rs`
  Purpose: DTOs for metadata, registration, authorize state, token exchange, JWT claims, and persisted rows.
- Create: `crates/lab-auth/src/state.rs`
  Purpose: Shared `AuthState` holding config, SQLite store, JWT signer/verifier, concrete Google provider, and rate-limiter state.
- Create: `crates/lab-auth/src/routes.rs`
  Purpose: Axum sub-router builder and route composition for the auth surface.
- Create: `crates/lab-auth/src/metadata.rs`
  Purpose: `/.well-known/oauth-authorization-server`, `/.well-known/oauth-protected-resource`, and `/jwks`.
- Create: `crates/lab-auth/src/google.rs`
  Purpose: Concrete Google authorize URL generation, code exchange, refresh exchange, and subject extraction.
- Create: `crates/lab-auth/src/authorize.rs`
  Purpose: Guarded registration plus authorize/callback flow in one cohesive module.
- Create: `crates/lab-auth/src/token.rs`
  Purpose: Authorization-code exchange, non-rotating refresh token exchange, and `lab` JWT issuance.
- Create: `crates/lab-auth/src/jwt.rs`
  Purpose: Key generation/loading, file-permission enforcement, JWT mint/validate helpers, and JWKS generation.
- Create: `crates/lab-auth/src/sqlite.rs`
  Purpose: SQLite schema, WAL/busy_timeout initialization, atomic auth-code redemption, and all blocking DB work via `spawn_blocking`.

### Existing files to modify

- Modify: `Cargo.toml`
  Purpose: Add `crates/lab-auth` to the workspace and add new shared dependencies.
- Modify: `crates/lab/Cargo.toml`
  Purpose: Depend on `lab-auth`.
- Modify: `crates/lab/src/api/router.rs`
  Purpose: Mount the new auth routes and replace the bearer-only middleware with a mode-aware combined middleware.
- Modify: `crates/lab/src/api/state.rs`
  Purpose: Carry `lab-auth` state/verifier alongside the existing registry/catalog.
- Modify: `crates/lab/src/cli/serve.rs`
  Purpose: Preserve bearer mode, require `LAB_PUBLIC_URL` in OAuth mode, initialize `lab-auth`, and pass auth state into the router.
- Modify: `crates/lab/src/config.rs`
  Purpose: Delete old external issuer config, add `LAB_AUTH_MODE` / `LAB_PUBLIC_URL` plumbing, and preserve existing bearer-token config.
- Modify: `crates/lab/src/main.rs`
  Purpose: Load auth config early enough for HTTP server startup.
- Modify: `crates/lab/src/api/error.rs`
  Purpose: Fix the `confirmation_required` 400/422 mismatch called out in the review.
- Modify: `docs/README.md`
  Purpose: Replace the old resource-server description with the new bearer-or-OAuth model.
- Modify: `docs/MCP.md`
  Purpose: Document auth discovery, registration, authorize, token, JWKS, and mode behavior.
- Modify: `docs/CONFIG.md`
  Purpose: Document reduced config surface and mode selection.
- Modify: `docs/ENV.md`
  Purpose: Replace `LAB_OAUTH_*` env docs with `LAB_AUTH_MODE`, `LAB_PUBLIC_URL`, and Google OAuth env docs.
- Modify: `docs/OPERATIONS.md`
  Purpose: Document key recovery, file permissions, DB location, and WAL expectations.
- Modify: `docs/plans/mcp-streamable-http-oauth-proxy.md`
  Purpose: Mark the old external-issuer Phase 1 plan as superseded.

### Tests to add or extend

- Test: `crates/lab-auth/src/config.rs`
- Test: `crates/lab-auth/src/sqlite.rs`
- Test: `crates/lab-auth/src/jwt.rs`
- Test: `crates/lab-auth/src/google.rs`
- Test: `crates/lab-auth/src/authorize.rs`
- Test: `crates/lab-auth/src/token.rs`
- Test: `crates/lab/src/api/router.rs`
- Test: `crates/lab/src/cli/serve.rs`
- Test: `crates/lab/src/api/error.rs`

## Task 1: Scaffold `lab-auth` as a pure axum/auth/storage crate

**Files:**
- Create: `crates/lab-auth/Cargo.toml`
- Create: `crates/lab-auth/src/lib.rs`
- Create: `crates/lab-auth/src/error.rs`
- Modify: `Cargo.toml`
- Modify: `crates/lab/Cargo.toml`

- [ ] **Step 1: Write the failing workspace test**

```rust
#[test]
fn lab_auth_crate_exports_router_and_verifier() {
    let _router_fn = lab_auth::routes::router;
    let _verify_fn = lab_auth::jwt::validate_access_token;
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p lab --lib lab_auth_crate_exports_router_and_verifier -- --exact`
Expected: FAIL because `lab-auth` is not a workspace member and the symbols do not exist.

- [ ] **Step 3: Add the new crate and minimal public surface**

```toml
[workspace]
members = ["crates/lab-apis", "crates/lab", "crates/lab-auth"]

[workspace.dependencies]
rusqlite = { version = "0.37", features = ["bundled"] }
jsonwebtoken = "10"
rsa = { version = "0.10", features = ["pem"] }
rand = "0.9"
```

```rust
// crates/lab-auth/src/lib.rs
pub mod config;
pub mod error;
pub mod routes;
pub mod jwt;
```

- [ ] **Step 4: Run the new crate compile target**

Run: `cargo test -p lab-auth --lib`
Expected: PASS with the new crate compiling, even though most modules are stubs.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/lab/Cargo.toml crates/lab-auth
git commit -m "feat: scaffold lab-auth crate"
```

## Task 2: Add mode-aware auth configuration and startup validation

**Files:**
- Create: `crates/lab-auth/src/config.rs`
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Test: `crates/lab-auth/src/config.rs`
- Test: `crates/lab/src/cli/serve.rs`

- [ ] **Step 1: Write the failing config tests**

```rust
#[test]
fn bearer_mode_preserves_existing_http_token_behavior() {
    let cfg = AuthModeConfig::from_sources(fake_env_with("LAB_AUTH_MODE", "bearer")).unwrap();
    assert!(matches!(cfg.mode, AuthMode::Bearer));
}

#[test]
fn oauth_mode_requires_public_url_and_google_credentials() {
    let err = AuthConfig::from_sources(
        fake_env_with_many([
            ("LAB_AUTH_MODE", "oauth"),
            ("LAB_GOOGLE_CLIENT_ID", "id"),
        ])
    ).unwrap_err();
    assert!(err.to_string().contains("LAB_PUBLIC_URL"));
}

#[test]
fn oauth_mode_defaults_paths_and_callback() {
    let cfg = AuthConfig::from_sources(fake_env_with_many([
        ("LAB_AUTH_MODE", "oauth"),
        ("LAB_PUBLIC_URL", "https://lab.example.com"),
        ("LAB_GOOGLE_CLIENT_ID", "id"),
        ("LAB_GOOGLE_CLIENT_SECRET", "secret"),
    ])).unwrap();
    assert_eq!(cfg.sqlite_path.file_name().unwrap(), "auth.db");
    assert_eq!(cfg.key_path.file_name().unwrap(), "auth-jwt.pem");
    assert_eq!(cfg.google.callback_path, "/auth/google/callback");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p lab-auth bearer_mode_preserves_existing_http_token_behavior oauth_mode_requires_public_url_and_google_credentials oauth_mode_defaults_paths_and_callback`
Expected: FAIL because the auth config types and mode selection do not exist.

- [ ] **Step 3: Write minimal configuration code**

```rust
pub enum AuthMode {
    Bearer,
    OAuth,
}

pub struct AuthConfig {
    pub mode: AuthMode,
    pub public_url: Url,
    pub sqlite_path: PathBuf,
    pub key_path: PathBuf,
    pub google: GoogleConfig,
}
```

- [ ] **Step 4: Run startup-resolution tests**

Run: `cargo test -p lab transport_resolution_prefers_cli_then_env_then_config config_defaults_are_available_for_serve_resolution -- --exact`
Expected: PASS with bearer mode preserved and OAuth-mode gating added.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-auth/src/config.rs crates/lab/src/config.rs crates/lab/src/cli/serve.rs
git commit -m "feat: add mode-aware auth config"
```

## Task 3: Build SQLite storage with WAL, busy timeout, permissions, and atomic code redemption

**Files:**
- Create: `crates/lab-auth/src/sqlite.rs`
- Create: `crates/lab-auth/src/types.rs`
- Test: `crates/lab-auth/src/sqlite.rs`

- [ ] **Step 1: Write the failing SQLite tests**

```rust
#[tokio::test]
async fn sqlite_store_enables_wal_and_busy_timeout() {
    let store = temp_store().await;
    assert_eq!(pragma(&store, "journal_mode").await, "wal");
    assert!(pragma_ms(&store, "busy_timeout").await >= 5_000);
}

#[tokio::test]
async fn sqlite_store_redeems_auth_code_only_once_under_race() {
    let store = temp_store().await;
    store.insert_auth_code(sample_code()).await.unwrap();
    let (a, b) = tokio::join!(
        store.redeem_auth_code("code-123"),
        store.redeem_auth_code("code-123"),
    );
    assert!(a.is_ok() ^ b.is_ok());
}

#[cfg(unix)]
#[tokio::test]
async fn sqlite_store_refuses_world_readable_database_file() {
    let path = temp_db_path();
    std::fs::write(&path, []).unwrap();
    std::fs::set_permissions(&path, PermissionsExt::from_mode(0o644)).unwrap();
    let err = SqliteStore::open(path).await.unwrap_err();
    assert!(err.to_string().contains("permissions"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p lab-auth sqlite_store_enables_wal_and_busy_timeout sqlite_store_redeems_auth_code_only_once_under_race`
Expected: FAIL because the SQLite store does not exist.

- [ ] **Step 3: Implement the SQLite store**

```rust
pub struct SqliteStore {
    conn: Arc<tokio::sync::Mutex<rusqlite::Connection>>,
}

// open(): enforce permissions, set WAL, busy_timeout, foreign_keys
// all calls: wrap rusqlite work in tokio::task::spawn_blocking
// redeem_auth_code(): DELETE ... RETURNING ...
```

- [ ] **Step 4: Run the SQLite tests**

Run: `cargo test -p lab-auth sqlite:: -- --nocapture`
Expected: PASS with WAL enabled, busy timeout set, permission checks enforced, and single-use auth codes.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-auth/src/sqlite.rs crates/lab-auth/src/types.rs
git commit -m "feat: add sqlite auth store"
```

## Task 4: Add persisted signing keys, JWT issuance, and JWKS

**Files:**
- Create: `crates/lab-auth/src/jwt.rs`
- Test: `crates/lab-auth/src/jwt.rs`

- [ ] **Step 1: Write the failing JWT tests**

```rust
#[test]
fn generated_key_is_reused_on_second_load() {
    let dir = tempfile::tempdir().unwrap();
    let first = SigningKeys::load_or_create(dir.path().join("auth-jwt.pem")).unwrap();
    let second = SigningKeys::load_or_create(dir.path().join("auth-jwt.pem")).unwrap();
    assert_eq!(first.key_id, second.key_id);
}

#[cfg(unix)]
#[test]
fn signing_key_refuses_world_readable_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("auth-jwt.pem");
    std::fs::write(&path, "bad").unwrap();
    std::fs::set_permissions(&path, PermissionsExt::from_mode(0o644)).unwrap();
    let err = SigningKeys::load_or_create(path).unwrap_err();
    assert!(err.to_string().contains("permissions"));
}

#[test]
fn minted_access_token_round_trips_and_contains_kid() {
    let signer = test_signer();
    let token = signer.issue_access_token(sample_claims()).unwrap();
    let claims = signer.validate_access_token(&token).unwrap();
    assert_eq!(claims.aud, "https://lab.example.com");
    assert!(!claims.jti.is_empty());
    assert!(token_header(&token).get("kid").is_some());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p lab-auth generated_key_is_reused_on_second_load minted_access_token_round_trips_and_contains_kid`
Expected: FAIL because signing helpers do not exist.

- [ ] **Step 3: Implement signing and JWKS**

```rust
pub struct AccessClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub exp: usize,
    pub iat: usize,
    pub jti: String,
    pub scope: String,
    pub azp: String,
}
```

- [ ] **Step 4: Run JWT tests**

Run: `cargo test -p lab-auth jwt:: -- --nocapture`
Expected: PASS with persisted keys, restrictive file permissions, `kid` headers, and JWKS generation.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-auth/src/jwt.rs
git commit -m "feat: add lab-auth jwt signing"
```

## Task 5: Implement the concrete Google provider

**Files:**
- Create: `crates/lab-auth/src/google.rs`
- Create: `crates/lab-auth/src/state.rs`
- Test: `crates/lab-auth/src/google.rs`

- [ ] **Step 1: Write the failing Google tests**

```rust
#[test]
fn google_authorize_url_includes_offline_access_and_pkce() {
    let provider = test_google_provider();
    let url = provider.authorize_url(sample_request()).unwrap();
    assert!(url.as_str().contains("access_type=offline"));
    assert!(url.as_str().contains("code_challenge="));
}

#[tokio::test]
async fn google_exchange_parses_subject_and_refresh_token() {
    let provider = mocked_google_provider();
    let token = provider.exchange_code("code", "verifier").await.unwrap();
    assert_eq!(token.subject, "google-subject-123");
    assert_eq!(token.refresh_token.as_deref(), Some("refresh-token"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p lab-auth google_authorize_url_includes_offline_access_and_pkce google_exchange_parses_subject_and_refresh_token`
Expected: FAIL because the Google provider does not exist.

- [ ] **Step 3: Implement the concrete provider**

```rust
pub struct GoogleProvider {
    pub client_id: String,
    pub client_secret: SecretString,
    pub redirect_uri: Url,
    pub scopes: Vec<String>,
    pub http: reqwest::Client,
}
```

- [ ] **Step 4: Run provider tests**

Run: `cargo test -p lab-auth google:: -- --nocapture`
Expected: PASS with offline-access params and token exchange parsing covered by mocks.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-auth/src/google.rs crates/lab-auth/src/state.rs
git commit -m "feat: add google oauth provider"
```

## Task 6: Implement guarded registration plus authorize/callback flow

**Files:**
- Create: `crates/lab-auth/src/routes.rs`
- Create: `crates/lab-auth/src/metadata.rs`
- Create: `crates/lab-auth/src/authorize.rs`
- Test: `crates/lab-auth/src/metadata.rs`
- Test: `crates/lab-auth/src/authorize.rs`

- [ ] **Step 1: Write the failing route tests**

```rust
#[tokio::test]
async fn authorization_server_metadata_exposes_lab_endpoints() {
    let app = test_auth_router();
    let response = get(&app, "/.well-known/oauth-authorization-server").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_contains(&response, "token_endpoint", "https://lab.example.com/token");
}

#[tokio::test]
async fn register_requires_bootstrap_secret_and_loopback_redirect() {
    let app = test_auth_router();
    let response = post_json(&app, "/register", json!({
        "redirect_uris": ["http://127.0.0.1:7777/callback"]
    })).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn authorize_persists_full_state_and_redirects_to_google() {
    let app = test_auth_router_with_registered_client();
    let response = get(&app, "/authorize?client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256").await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_header_contains(&response, "location", "accounts.google.com");
}

#[tokio::test]
async fn callback_rejects_expired_or_mismatched_state() {
    let app = test_auth_router_with_mock_google();
    let response = get(&app, "/auth/google/callback?state=bad-state&code=upstream-code").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p lab-auth authorization_server_metadata_exposes_lab_endpoints register_requires_bootstrap_secret_and_loopback_redirect authorize_persists_full_state_and_redirects_to_google callback_rejects_expired_or_mismatched_state`
Expected: FAIL because the routes do not exist.

- [ ] **Step 3: Implement the route surface**

```rust
Router::new()
    .route("/.well-known/oauth-authorization-server", get(authorization_server_metadata))
    .route("/.well-known/oauth-protected-resource", get(protected_resource_metadata))
    .route("/jwks", get(jwks))
    .route("/register", post(register_client))
    .route("/authorize", get(authorize))
    .route("/auth/google/callback", get(callback));
```

- [ ] **Step 4: Run route tests**

Run: `cargo test -p lab-auth metadata:: authorize:: -- --nocapture`
Expected: PASS with bootstrap-secret enforcement, loopback-only redirect policy, rate limiting, and full state binding.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-auth/src/routes.rs crates/lab-auth/src/metadata.rs crates/lab-auth/src/authorize.rs
git commit -m "feat: add guarded oauth authorize flow"
```

## Task 7: Implement token exchange with non-rotating refresh tokens

**Files:**
- Create: `crates/lab-auth/src/token.rs`
- Test: `crates/lab-auth/src/token.rs`

- [ ] **Step 1: Write the failing token tests**

```rust
#[tokio::test]
async fn token_endpoint_mints_lab_jwt_and_refresh_token() {
    let app = test_auth_router();
    seed_authorization_code(&app).await;
    let response = post_form(&app, "/token", "grant_type=authorization_code&code=lab-code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_has_string(&response, "access_token");
    assert_json_has_string(&response, "refresh_token");
}

#[tokio::test]
async fn token_endpoint_redeems_authorization_code_once() {
    let app = test_auth_router();
    seed_authorization_code(&app).await;
    let (a, b) = tokio::join!(
        post_form(&app, "/token", "grant_type=authorization_code&code=lab-code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier"),
        post_form(&app, "/token", "grant_type=authorization_code&code=lab-code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&code_verifier=verifier")
    );
    assert!(a.status() == StatusCode::OK || b.status() == StatusCode::OK);
    assert!(a.status() == StatusCode::BAD_REQUEST || b.status() == StatusCode::BAD_REQUEST);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p lab-auth token_endpoint_mints_lab_jwt_and_refresh_token token_endpoint_redeems_authorization_code_once`
Expected: FAIL because `/token` is not implemented.

- [ ] **Step 3: Implement token exchange**

```rust
pub async fn token(
    State(state): State<AuthState>,
    Form(request): Form<TokenRequest>,
) -> Result<Json<TokenResponse>, AuthError> { /* authorization_code or refresh_token */ }
```

- [ ] **Step 4: Run token tests**

Run: `cargo test -p lab-auth token:: -- --nocapture`
Expected: PASS with JWT issuance, refresh-token persistence, and atomic code redemption covered.

- [ ] **Step 5: Commit**

```bash
git add crates/lab-auth/src/token.rs
git commit -m "feat: add token exchange flow"
```

## Task 8: Cut `lab` over to combined bearer-or-OAuth middleware and remove the old external issuer path

**Files:**
- Modify: `crates/lab/src/api/router.rs`
- Modify: `crates/lab/src/api/state.rs`
- Modify: `crates/lab/src/cli/serve.rs`
- Modify: `crates/lab/src/config.rs`
- Modify: `crates/lab/src/main.rs`
- Modify: `crates/lab/src/api/error.rs`
- Test: `crates/lab/src/api/router.rs`
- Test: `crates/lab/src/cli/serve.rs`
- Test: `crates/lab/src/api/error.rs`

- [ ] **Step 1: Write the failing integration tests**

```rust
#[tokio::test]
async fn bearer_mode_still_accepts_lab_mcp_http_token() {
    let state = test_app_state_in_bearer_mode();
    let app = build_router(state, None);
    let response = get_with_bearer(&app, "/v1/extract/actions", "secret-token").await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn oauth_mode_accepts_lab_auth_jwt() {
    let state = test_app_state_with_lab_auth();
    let app = build_router(state, Some(test_auth_state()));
    let token = issue_test_lab_token();
    let response = get_with_bearer(&app, "/v1/extract/actions", &token).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn oauth_mode_missing_token_returns_www_authenticate_metadata_hint() {
    let state = test_app_state_with_lab_auth();
    let app = build_router(state, Some(test_auth_state()));
    let response = get(&app, "/mcp").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_header_contains(&response, "www-authenticate", "resource_metadata=");
}

#[test]
fn confirmation_required_maps_to_422() {
    assert_eq!(ApiError::status_for_kind("confirmation_required"), StatusCode::UNPROCESSABLE_ENTITY);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p lab bearer_mode_still_accepts_lab_mcp_http_token oauth_mode_accepts_lab_auth_jwt oauth_mode_missing_token_returns_www_authenticate_metadata_hint confirmation_required_maps_to_422`
Expected: FAIL because `lab` still uses the old bearer-only middleware and the old 400 mapping.

- [ ] **Step 3: Implement the mode-aware integration**

```rust
let auth_router = if matches!(auth_mode, AuthMode::OAuth) {
    Some(lab_auth::routes::router(auth_state.clone()))
} else {
    None
};

let protected = Router::new()
    .nest("/v1", v1)
    .nest("/mcp", mcp_service)
    .route_layer(axum::middleware::from_fn_with_state(
        combined_auth_state.clone(),
        require_protected_route_auth,
    ));
```

- [ ] **Step 4: Run router/serve/error tests**

Run: `cargo test -p lab api::router::tests -- --nocapture`
Expected: PASS with bearer mode preserved, OAuth mode accepting `lab-auth` JWTs, RFC-compliant `WWW-Authenticate` metadata hints, and `confirmation_required` returning 422.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api/router.rs crates/lab/src/api/state.rs crates/lab/src/cli/serve.rs crates/lab/src/config.rs crates/lab/src/main.rs crates/lab/src/api/error.rs
git commit -m "feat: add combined bearer and oauth auth middleware"
```

## Task 9: Update docs, mark the old plan superseded, and run full verification

**Files:**
- Modify: `docs/README.md`
- Modify: `docs/MCP.md`
- Modify: `docs/CONFIG.md`
- Modify: `docs/ENV.md`
- Modify: `docs/OPERATIONS.md`
- Modify: `docs/plans/mcp-streamable-http-oauth-proxy.md`

- [ ] **Step 1: Write the failing doc checklist**

```text
- README still says lab is only a resource server
- ENV still documents LAB_OAUTH_ISSUER / LAB_OAUTH_AUDIENCE
- MCP docs do not mention /register, /authorize, /token, /jwks, LAB_AUTH_MODE, or LAB_PUBLIC_URL
- operations docs do not mention 0600 permission enforcement or key recovery
- old streamable-http oauth plan is not marked superseded
```

- [ ] **Step 2: Run a targeted grep to verify the old docs still exist**

Run: `rtk rg -n "LAB_OAUTH_ISSUER|resource server|oauth-protected-resource|/token|/register|LAB_AUTH_MODE|LAB_PUBLIC_URL" docs`
Expected: Matches still point at the removed external-issuer design and missing new auth-mode docs.

- [ ] **Step 3: Update docs to match the final design**

```md
`LAB_AUTH_MODE=bearer` preserves the existing static token behavior.
`LAB_AUTH_MODE=oauth` enables the Google-backed MCP OAuth flow and requires `LAB_PUBLIC_URL`.
Google tokens remain server-side only; clients receive `lab` access tokens.
```

- [ ] **Step 4: Run the full verification suite**

Run: `cargo test --workspace --all-features --tests --no-fail-fast`
Expected: PASS

Run: `cargo build --workspace --all-features`
Expected: PASS

Run: `cargo clippy --workspace --all-features --tests -- -D warnings`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add docs/README.md docs/MCP.md docs/CONFIG.md docs/ENV.md docs/OPERATIONS.md docs/plans/mcp-streamable-http-oauth-proxy.md
git commit -m "docs: document bearer and oauth auth modes"
```

## Notes for the implementing worker

- `lab-auth` does not need `rmcp` because it is not speaking MCP transport. It only serves ordinary HTTP auth endpoints (`/register`, `/authorize`, `/token`, `/.well-known/...`, `/jwks`) and issues/verifies JWTs. `crates/lab` remains the only crate that knows about MCP framing, `rmcp::ServerHandler`, and `/mcp` transport wiring.
- Keep Google tokens server-side only. MCP clients must only see `lab` tokens.
- Initial launch uses loopback-only redirect URIs plus a bootstrap secret for registration. Do not allow arbitrary HTTPS redirect URIs yet.
- All `rusqlite` work must run off the async executor, and SQLite must be opened in WAL mode with a non-zero busy timeout.
- Enforce restrictive file permissions for `auth.db` and `auth-jwt.pem` and add failure-path tests for those checks.
- Delete the old `LAB_OAUTH_ISSUER` / external JWKS path completely.
