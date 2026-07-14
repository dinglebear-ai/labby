# Public OAuth Callback Relay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the public Codex MCP OAuth callback relay from the standalone Python service on `squirts` into Labby while preserving `https://callback.tootie.tv/callback/{machine_id}[/{suffix}]`.

**Architecture:** Add a Labby product-domain module under `crates/labby/src/oauth/public_relay/` that owns target validation, registry persistence, live snapshots, forwarding policy, and public error mapping. API routes, CLI commands, and doctor checks are thin adapters over that domain layer. Public callback routes are unauthenticated but registry mutation is handler-level `lab:admin` and fails closed.

**Tech Stack:** Rust 2024, Axum 0.8.9, Tokio 1.52.2, reqwest 0.13.3, tower-http 0.6.8, serde/serde_json, tempfile, tracing, existing Labby API/router/config patterns.

## Global Constraints

- Keep `https://callback.tootie.tv/callback/{machine_id}[/{suffix}]` compatible.
- Public callback routes are unauthenticated; registry mutation requires `lab:admin`.
- Relay is transport-only: no token exchange, no PKCE ownership, no Codex credential edits.
- Valid public targets are `http://100.64.0.0/10:38935/callback/<machine_id>` with no userinfo, query, or fragment.
- Public route must never accept a target URL from query, body, or headers.
- Do not reuse the generic SSRF helper directly; it intentionally rejects this Tailscale HTTP use case.
- Public forwarding defaults: query cap 16 KiB, request body cap 64 KiB, upstream response cap 128 KiB, connect timeout 1s, read timeout 2s, total timeout 5s, global concurrency 32, per-machine concurrency 2.
- Public forwarding uses no redirects and strips `Location` by default.
- Do not log callback query strings, request bodies, `code`, `state`, auth headers, cookies, full target URLs, or admin tokens.
- `/healthz` is shallow: process alive, public relay enabled, registry loaded. Target reachability belongs in authenticated doctor checks.
- Keep `[oauth.machines]` as local relay config; public relay registry is separate.
- Leave unrelated dirty files alone.

---

## File Structure

- Create `crates/labby/src/oauth/public_relay.rs`: module declaration and exports.
- Create `crates/labby/src/oauth/public_relay/types.rs`: `MachineId`, `RelayTarget`, `PublicRelayEntry`, import/report DTOs, public/admin views.
- Create `crates/labby/src/oauth/public_relay/policy.rs`: target policy, route suffix validation, limit constants, redaction helpers.
- Create `crates/labby/src/oauth/public_relay/store.rs`: sidecar registry load/save/import with backup-first atomic writes.
- Create `crates/labby/src/oauth/public_relay/manager.rs`: live validated snapshot manager, semaphores, mutation API.
- Create `crates/labby/src/oauth/public_relay/forward.rs`: bounded no-redirect public forwarder.
- Create `crates/labby/src/api/services/oauth_relay.rs`: public callback handlers, public `/healthz`, protected admin API routes.
- Modify `crates/labby/src/oauth.rs`: export `public_relay`.
- Modify `crates/labby/src/api/state.rs`: add optional `PublicRelayRegistryManager`.
- Modify `crates/labby/src/api/router.rs`: mount public callback/healthz routes outside `/v1`, mount protected admin routes under `/v1/oauth/relay`, and bypass/reserve `/callback/*` and `/healthz` from protected MCP interception.
- Modify `crates/labby/src/cli/oauth.rs`: add offline registry import/list/register/remove commands and preserve `relay-local`.
- Modify `crates/labby/src/dispatch/doctor/*` and `crates/labby/src/api/services/doctor.rs`: expose authenticated relay readiness checks.
- Modify docs: `docs/runtime/OAUTH.md`, `docs/OPERATIONS.md`, optional `docs/deploy/CALLBACK_RELAY.md`.

---

### Task 1: Contract Constants And Public Relay Module Skeleton

**Files:**
- Create: `crates/labby/src/oauth/public_relay.rs`
- Create: `crates/labby/src/oauth/public_relay/policy.rs`
- Create: `crates/labby/src/oauth/public_relay/types.rs`
- Modify: `crates/labby/src/oauth.rs`
- Test: `crates/labby/src/oauth/public_relay/policy.rs`

**Interfaces:**
- Produces: `pub const PUBLIC_QUERY_LIMIT_BYTES: usize`, `PUBLIC_REQUEST_BODY_LIMIT_BYTES`, `PUBLIC_RESPONSE_LIMIT_BYTES`, `PUBLIC_CONNECT_TIMEOUT`, `PUBLIC_READ_TIMEOUT`, `PUBLIC_TOTAL_TIMEOUT`, `PUBLIC_GLOBAL_CONCURRENCY`, `PUBLIC_PER_MACHINE_CONCURRENCY`.
- Produces: `MachineId::parse(&str) -> Result<MachineId, PublicRelayError>`.
- Produces: `validate_suffix_path(path: &str) -> Result<String, PublicRelayError>`.

- [ ] **Step 1: Add failing tests for machine IDs and suffix validation**

Add tests in `crates/labby/src/oauth/public_relay/policy.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_id_accepts_live_names() {
        for value in [
            "dookie",
            "shart",
            "squirts",
            "steamy",
            "steamy-wsl",
            "tootie",
            "vivobook-wsl",
        ] {
            assert_eq!(MachineId::parse(value).unwrap().as_str(), value);
        }
    }

    #[test]
    fn machine_id_rejects_confusing_values() {
        for value in ["", ".", "..", "node/a", "node\\a", " node", "node ", "node?a", "node#a"] {
            assert!(MachineId::parse(value).is_err(), "{value:?} should reject");
        }
    }

    #[test]
    fn suffix_path_rejects_traversal_and_encoded_slash() {
        for value in ["/callback2/x", "/callback/dookie/../x", "/callback/dookie/%2fsecret", "/callback/dookie/%5csecret"] {
            assert!(validate_suffix_path(value).is_err(), "{value:?} should reject");
        }
    }

    #[test]
    fn public_limits_are_small_and_explicit() {
        assert_eq!(PUBLIC_QUERY_LIMIT_BYTES, 16 * 1024);
        assert_eq!(PUBLIC_REQUEST_BODY_LIMIT_BYTES, 64 * 1024);
        assert_eq!(PUBLIC_RESPONSE_LIMIT_BYTES, 128 * 1024);
        assert_eq!(PUBLIC_GLOBAL_CONCURRENCY, 32);
        assert_eq!(PUBLIC_PER_MACHINE_CONCURRENCY, 2);
    }
}
```

- [ ] **Step 2: Run the focused test to verify it fails**

Run:

```bash
cargo test -p labby public_relay::policy --all-features
```

Expected: compile failure because `public_relay`, `MachineId`, limits, and suffix helpers do not exist.

- [ ] **Step 3: Add the module skeleton and constants**

Create `crates/labby/src/oauth/public_relay.rs`:

```rust
//! Public OAuth callback relay domain.

pub mod forward;
pub mod manager;
pub mod policy;
pub mod store;
pub mod types;

pub use manager::PublicRelayRegistryManager;
pub use policy::*;
pub use types::*;
```

Create `crates/labby/src/oauth/public_relay/policy.rs`:

```rust
use std::time::Duration;

use super::types::{MachineId, PublicRelayError};

pub const PUBLIC_QUERY_LIMIT_BYTES: usize = 16 * 1024;
pub const PUBLIC_REQUEST_BODY_LIMIT_BYTES: usize = 64 * 1024;
pub const PUBLIC_RESPONSE_LIMIT_BYTES: usize = 128 * 1024;
pub const PUBLIC_GLOBAL_CONCURRENCY: usize = 32;
pub const PUBLIC_PER_MACHINE_CONCURRENCY: usize = 2;
pub const PUBLIC_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
pub const PUBLIC_READ_TIMEOUT: Duration = Duration::from_secs(2);
pub const PUBLIC_TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

pub fn validate_suffix_path(path: &str) -> Result<String, PublicRelayError> {
    if path.len() > 2048 {
        return Err(PublicRelayError::InvalidSuffix("suffix path too long".into()));
    }
    if !path.starts_with("/callback/") && path != "/callback" {
        return Err(PublicRelayError::InvalidSuffix("path is not a callback route".into()));
    }
    let lower = path.to_ascii_lowercase();
    if lower.contains("%2f") || lower.contains("%5c") || path.contains('\\') {
        return Err(PublicRelayError::InvalidSuffix("encoded slash or backslash is not allowed".into()));
    }
    if path.split('/').any(|segment| segment == "." || segment == "..") {
        return Err(PublicRelayError::InvalidSuffix("dot segments are not allowed".into()));
    }
    Ok(path.to_string())
}
```

Create `crates/labby/src/oauth/public_relay/types.rs`:

```rust
use std::fmt;

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MachineId(String);

impl MachineId {
    pub fn parse(value: &str) -> Result<Self, PublicRelayError> {
        let trimmed = value.trim();
        if trimmed != value || trimmed.is_empty() || trimmed.len() > 64 {
            return Err(PublicRelayError::InvalidMachineId(value.to_string()));
        }
        if !trimmed
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(PublicRelayError::InvalidMachineId(value.to_string()));
        }
        if trimmed == "." || trimmed == ".." {
            return Err(PublicRelayError::InvalidMachineId(value.to_string()));
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MachineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PublicRelayError {
    #[error("invalid machine id")]
    InvalidMachineId(String),
    #[error("invalid callback suffix: {0}")]
    InvalidSuffix(String),
    #[error("invalid target: {0}")]
    InvalidTarget(String),
    #[error("registry unavailable: {0}")]
    RegistryUnavailable(String),
    #[error("machine is not registered")]
    UnknownMachine,
    #[error("machine is disabled")]
    DisabledMachine,
    #[error("relay overloaded")]
    Overloaded,
    #[error("request body too large")]
    BodyTooLarge,
    #[error("upstream response too large")]
    ResponseTooLarge,
    #[error("upstream timeout")]
    UpstreamTimeout,
    #[error("upstream error")]
    UpstreamError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicRelayEntry {
    pub machine_id: MachineId,
    pub target_url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Clone)]
pub struct RelayTarget {
    pub machine_id: MachineId,
    pub url: Url,
}
```

Modify `crates/labby/src/oauth.rs`:

```rust
pub mod error;
pub mod local_relay;
pub mod public_relay;
pub mod target;
pub mod upstream;
```

- [ ] **Step 4: Run the focused test**

Run:

```bash
cargo test -p labby public_relay::policy --all-features
```

Expected: tests pass or fail only on module wiring that should be fixed in this task.

- [ ] **Step 5: Commit**

```bash
git add crates/labby/src/oauth.rs crates/labby/src/oauth/public_relay.rs crates/labby/src/oauth/public_relay
git commit -m "feat(oauth): add public relay policy skeleton"
```

---

### Task 2: Typed Relay Target Validation

**Files:**
- Modify: `crates/labby/src/oauth/public_relay/types.rs`
- Modify: `crates/labby/src/oauth/public_relay/policy.rs`
- Test: `crates/labby/src/oauth/public_relay/types.rs`

**Interfaces:**
- Consumes: `MachineId`.
- Produces: `RelayTarget::parse(machine_id: MachineId, target_url: &str) -> Result<RelayTarget, PublicRelayError>`.
- Produces: `RelayTarget::redacted_label(&self) -> String`.

- [ ] **Step 1: Write failing target validation tests**

Add to `types.rs` test module:

```rust
#[test]
fn relay_target_accepts_live_tailscale_shape() {
    let machine = MachineId::parse("dookie").unwrap();
    let target = RelayTarget::parse(machine, "http://100.88.16.79:38935/callback/dookie").unwrap();
    assert_eq!(target.url.as_str(), "http://100.88.16.79:38935/callback/dookie");
}

#[test]
fn relay_target_rejects_unsafe_shapes() {
    let cases = [
        "https://100.88.16.79:38935/callback/dookie",
        "http://100.88.16.79:80/callback/dookie",
        "http://127.0.0.1:38935/callback/dookie",
        "http://169.254.169.254:38935/callback/dookie",
        "http://100.88.16.79:38935/callback/other",
        "http://user@100.88.16.79:38935/callback/dookie",
        "http://100.88.16.79:38935/callback/dookie?code=abc",
        "http://100.88.16.79:38935/callback/dookie#frag",
    ];
    for value in cases {
        let machine = MachineId::parse("dookie").unwrap();
        assert!(RelayTarget::parse(machine, value).is_err(), "{value} should reject");
    }
}
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p labby relay_target --all-features
```

Expected: compile failure or failing tests because `RelayTarget::parse` is missing.

- [ ] **Step 3: Implement the validator**

Add to `RelayTarget` in `types.rs`:

```rust
impl RelayTarget {
    pub fn parse(machine_id: MachineId, target_url: &str) -> Result<Self, PublicRelayError> {
        let url = Url::parse(target_url)
            .map_err(|error| PublicRelayError::InvalidTarget(error.to_string()))?;
        if url.scheme() != "http" {
            return Err(PublicRelayError::InvalidTarget("scheme must be http".into()));
        }
        if url.username() != "" || url.password().is_some() {
            return Err(PublicRelayError::InvalidTarget("userinfo is not allowed".into()));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(PublicRelayError::InvalidTarget("query and fragment are not allowed".into()));
        }
        if url.port_or_known_default() != Some(38935) {
            return Err(PublicRelayError::InvalidTarget("port must be 38935".into()));
        }
        let expected_path = format!("/callback/{}", machine_id.as_str());
        if url.path() != expected_path {
            return Err(PublicRelayError::InvalidTarget("path must match /callback/<machine_id>".into()));
        }
        let host = url
            .host_str()
            .ok_or_else(|| PublicRelayError::InvalidTarget("host is required".into()))?;
        let ip: std::net::IpAddr = host
            .parse()
            .map_err(|_| PublicRelayError::InvalidTarget("host must be a Tailscale IP".into()))?;
        if !is_tailscale_cgnat(ip) {
            return Err(PublicRelayError::InvalidTarget("host must be in 100.64.0.0/10".into()));
        }
        Ok(Self { machine_id, url })
    }

    pub fn redacted_label(&self) -> String {
        format!("{}@{}", self.machine_id, self.url.host_str().unwrap_or("unknown"))
    }
}

fn is_tailscale_cgnat(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 100 && (64..=127).contains(&octets[1])
        }
        std::net::IpAddr::V6(_) => false,
    }
}
```

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p labby relay_target --all-features
```

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add crates/labby/src/oauth/public_relay/types.rs
git commit -m "feat(oauth): validate public relay targets"
```

---

### Task 3: Registry Store And Live Snapshot Manager

**Files:**
- Create: `crates/labby/src/oauth/public_relay/store.rs`
- Create: `crates/labby/src/oauth/public_relay/manager.rs`
- Modify: `crates/labby/src/oauth/public_relay/types.rs`
- Modify: `crates/labby/src/api/state.rs`
- Test: `crates/labby/src/oauth/public_relay/store.rs`
- Test: `crates/labby/src/oauth/public_relay/manager.rs`

**Interfaces:**
- Produces: `PublicRelayRegistryStore::new(path: PathBuf)`.
- Produces: `load_snapshot(&self) -> Result<PublicRelaySnapshot, PublicRelayError>`.
- Produces: `save_entries(&self, entries: Vec<PublicRelayEntry>) -> Result<RegistryWriteOutcome, PublicRelayError>`.
- Produces: `PublicRelayRegistryManager::load(store: PublicRelayRegistryStore) -> Result<Self, PublicRelayError>`.
- Produces: `manager.resolve(&MachineId) -> Result<RelayTarget, PublicRelayError>`.
- Produces: `manager.replace_entries(entries) -> Result<ImportReport, PublicRelayError>`.

- [ ] **Step 1: Write failing registry tests**

In `store.rs`, add tests that create a temp registry file, import seven live entries, reject invalid entries into an import report, and prove corrupt existing content is backed up before replacement.

Use this JSON fixture:

```rust
const LIVE_REGISTRY_JSON: &str = r#"{
  "dookie": "http://100.88.16.79:38935/callback/dookie",
  "shart": "http://100.118.209.1:38935/callback/shart",
  "squirts": "http://100.75.111.118:38935/callback/squirts",
  "steamy": "http://100.119.83.39:38935/callback/steamy",
  "steamy-wsl": "http://100.74.16.82:38935/callback/steamy-wsl",
  "tootie": "http://100.120.242.29:38935/callback/tootie",
  "vivobook-wsl": "http://100.104.50.17:38935/callback/vivobook-wsl"
}"#;
```

Expected assertions:

```rust
let report = PublicRelayRegistryStore::parse_standalone_registry(LIVE_REGISTRY_JSON).unwrap();
assert_eq!(report.accepted.len(), 7);
assert!(report.quarantined.is_empty());
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p labby public_relay::store public_relay::manager --all-features
```

Expected: compile failure because store/manager are not implemented.

- [ ] **Step 3: Implement store and manager**

Implement sidecar persistence in `store.rs`:

```rust
#[derive(Debug, Clone)]
pub struct PublicRelayRegistryStore {
    path: std::path::PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PublicRelaySnapshot {
    pub entries: BTreeMap<MachineId, PublicRelayEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub accepted: Vec<String>,
    pub quarantined: Vec<QuarantinedEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct QuarantinedEntry {
    pub machine_id: String,
    pub reason: String,
}
```

Implement writes using `tokio::task::spawn_blocking` around a synchronous helper:

```rust
pub async fn save_entries_blocking_safe(&self, entries: Vec<PublicRelayEntry>) -> Result<(), PublicRelayError> {
    let path = self.path.clone();
    tokio::task::spawn_blocking(move || save_entries_sync(path, entries))
        .await
        .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?
}
```

In `manager.rs`, store an `Arc<RwLock<PublicRelaySnapshot>>` and semaphores:

```rust
#[derive(Clone)]
pub struct PublicRelayRegistryManager {
    store: PublicRelayRegistryStore,
    snapshot: Arc<tokio::sync::RwLock<PublicRelaySnapshot>>,
    global_limit: Arc<tokio::sync::Semaphore>,
    per_machine_limits: Arc<tokio::sync::RwLock<BTreeMap<MachineId, Arc<tokio::sync::Semaphore>>>>,
}
```

- [ ] **Step 4: Wire AppState**

Modify `crates/labby/src/api/state.rs` to add:

```rust
pub public_relay: Option<Arc<crate::oauth::public_relay::PublicRelayRegistryManager>>,
```

Add a `with_public_relay_manager` builder matching the existing `with_gateway_manager` style.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p labby public_relay::store public_relay::manager --all-features
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/labby/src/oauth/public_relay/store.rs crates/labby/src/oauth/public_relay/manager.rs crates/labby/src/oauth/public_relay/types.rs crates/labby/src/api/state.rs
git commit -m "feat(oauth): add public relay registry manager"
```

---

### Task 4: Bounded Public Forwarder And Local Relay Hardening

**Files:**
- Create: `crates/labby/src/oauth/public_relay/forward.rs`
- Modify: `crates/labby/src/oauth/local_relay.rs`
- Modify: `crates/labby/src/oauth/target.rs`
- Test: `crates/labby/src/oauth/public_relay/forward.rs`
- Test: `crates/labby/src/oauth/local_relay.rs`

**Interfaces:**
- Produces: `PublicRelayForwarder::new() -> Result<Self, PublicRelayError>`.
- Produces: `forward(&self, request: ForwardRequest) -> Result<ForwardResponse, PublicRelayError>`.
- Consumes: `RelayTarget`, query items, request headers, bounded body bytes.

- [ ] **Step 1: Write failing forwarder tests**

Add tests for:

```rust
#[tokio::test]
async fn public_forwarder_does_not_follow_redirects() { /* upstream returns 302 */ }

#[tokio::test]
async fn public_forwarder_strips_location_and_set_cookie() { /* upstream returns Location + Set-Cookie */ }

#[tokio::test]
async fn public_forwarder_rejects_large_response() { /* upstream returns PUBLIC_RESPONSE_LIMIT_BYTES + 1 */ }

#[tokio::test]
async fn public_forwarder_drops_auth_and_cookie_request_headers() { /* capture upstream headers */ }
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p labby public_forwarder --all-features
```

Expected: compile failure or failing tests because forwarder is missing.

- [ ] **Step 3: Implement no-redirect client and bounded response**

In `forward.rs`:

```rust
pub struct PublicRelayForwarder {
    client: reqwest::Client,
}

impl PublicRelayForwarder {
    pub fn new() -> Result<Self, PublicRelayError> {
        drop(rustls::crypto::ring::default_provider().install_default());
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(PUBLIC_CONNECT_TIMEOUT)
            .read_timeout(PUBLIC_READ_TIMEOUT)
            .timeout(PUBLIC_TOTAL_TIMEOUT)
            .no_gzip()
            .build()
            .map_err(|error| PublicRelayError::RegistryUnavailable(error.to_string()))?;
        Ok(Self { client })
    }
}
```

Read response chunks and abort over cap:

```rust
let mut out = bytes::BytesMut::new();
while let Some(chunk) = response.chunk().await.map_err(|_| PublicRelayError::UpstreamError)? {
    if out.len() + chunk.len() > PUBLIC_RESPONSE_LIMIT_BYTES {
        return Err(PublicRelayError::ResponseTooLarge);
    }
    out.extend_from_slice(&chunk);
}
```

- [ ] **Step 4: Harden local relay client**

In `local_relay.rs`, replace `reqwest::Client::new()` with a builder using `redirect(Policy::none())`, timeout, and no raw query/body logging. Keep local relay behavior otherwise intact.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p labby oauth::local_relay oauth::public_relay::forward --all-features
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/labby/src/oauth/public_relay/forward.rs crates/labby/src/oauth/local_relay.rs crates/labby/src/oauth/target.rs
git commit -m "feat(oauth): add bounded public relay forwarder"
```

---

### Task 5: Public Callback Routes And Protected Intercept Bypass

**Files:**
- Create: `crates/labby/src/api/services/oauth_relay.rs`
- Modify: `crates/labby/src/api/router.rs`
- Modify: `crates/labby/src/api/state.rs`
- Test: `crates/labby/src/api/router.rs` or `crates/labby/tests/oauth_public_relay.rs`

**Interfaces:**
- Consumes: `PublicRelayRegistryManager`, `PublicRelayForwarder`.
- Produces public routes: `GET|POST /callback/{machine_id}` and `GET|POST /callback/{machine_id}/{*suffix}`.
- Produces public route: `GET /healthz`.

- [ ] **Step 1: Write failing route tests**

Add integration tests that build a router with auth enabled and protected routes configured, then assert:

```rust
#[tokio::test]
async fn public_callback_bypasses_bearer_auth_and_protected_intercept() {
    let response = app
        .oneshot(Request::builder()
            .uri("/callback/dookie?code=abc&state=secret-state")
            .header("x-forwarded-host", "callback.tootie.tv")
            .body(Body::empty())
            .unwrap())
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(response.status(), StatusCode::OK); // until target fixture is wired
}
```

Also test `/callback2/dookie` does not match and `/healthz` returns JSON.

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p labby public_callback --all-features
```

Expected: fail because routes do not exist.

- [ ] **Step 3: Implement public route handlers**

In `api/services/oauth_relay.rs`, implement:

```rust
pub fn public_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/callback/{machine_id}", get(callback).post(callback))
        .route("/callback/{machine_id}/{*suffix}", get(callback).post(callback))
        .with_state(state)
}
```

In `callback`, enforce query length before parsing, acquire semaphores before body collection, use `axum::body::to_bytes(body, PUBLIC_REQUEST_BODY_LIMIT_BYTES)`, and map errors to non-enumerating responses.

- [ ] **Step 4: Bypass protected route interception**

In `api/router.rs`, add a helper near protected intercept logic:

```rust
fn is_public_relay_reserved_path(path: &str) -> bool {
    path == "/healthz" || path == "/callback" || path.starts_with("/callback/")
}
```

Use it before protected MCP matching so `/callback/*` and `/healthz` cannot be stolen by host/path interception.

- [ ] **Step 5: Run route tests**

Run:

```bash
cargo test -p labby public_callback --all-features
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/labby/src/api/services/oauth_relay.rs crates/labby/src/api/router.rs crates/labby/src/api/state.rs crates/labby/tests
git commit -m "feat(oauth): serve public callback relay routes"
```

---

### Task 6: Protected Admin API And CLI

**Files:**
- Modify: `crates/labby/src/api/services/oauth_relay.rs`
- Modify: `crates/labby/src/api/router.rs`
- Modify: `crates/labby/src/cli/oauth.rs`
- Test: `crates/labby/src/api/services/oauth_relay.rs`
- Test: `crates/labby/src/cli/oauth.rs`

**Interfaces:**
- Produces admin API under `/v1/oauth/relay`.
- Produces CLI commands: `labby oauth relay-registry list`, `import`, `register`, `remove`, `disable`, `enable`.

- [ ] **Step 1: Write failing admin auth tests**

Tests must cover unauthenticated, read-only auth, admin auth, no-auth HTTP mode, invalid target, and valid import.

Expected assertions:

```rust
assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
assert_eq!(read_only.status(), StatusCode::FORBIDDEN);
assert_eq!(admin.status(), StatusCode::OK);
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p labby oauth_relay_admin --all-features
```

Expected: fail because admin routes do not exist.

- [ ] **Step 3: Implement admin routes**

Add:

```rust
pub fn admin_routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/machines", get(list_machines).post(upsert_machine))
        .route("/machines/{machine_id}", get(get_machine).put(upsert_machine).delete(remove_machine))
        .route("/import", post(import_registry))
        .with_state(state)
}
```

Add a local `require_lab_admin(auth.as_ref())` patterned after `api/services/gateway.rs`.

- [ ] **Step 4: Mount admin routes only when auth is configured**

In `build_v1_router`, mount `/oauth/relay` only when `api_auth_configured` is true. Also keep handler-level `lab:admin` checks so route behavior remains fail-closed if router wiring changes.

- [ ] **Step 5: Add CLI commands**

Extend `OauthCommand` with:

```rust
RelayRegistry(RelayRegistryArgs),
```

For offline commands, print `restart_required: true` in JSON/human output after mutation. Preserve `relay-local` parse tests.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p labby oauth_relay_admin oauth_relay_local_cli --all-features
```

Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add crates/labby/src/api/services/oauth_relay.rs crates/labby/src/api/router.rs crates/labby/src/cli/oauth.rs
git commit -m "feat(oauth): add public relay admin surfaces"
```

---

### Task 7: Health, Doctor, Observability, And Error Mapping

**Files:**
- Modify: `crates/labby/src/api/services/oauth_relay.rs`
- Modify: `crates/labby/src/dispatch/doctor.rs`
- Modify: `crates/labby/src/dispatch/doctor/*`
- Modify: `crates/labby/src/api/services/doctor.rs`
- Modify: `crates/labby/src/api/error.rs` if new public relay error kinds are exposed through `ToolError`.
- Test: focused API/doctor/observability tests.

**Interfaces:**
- Produces shallow public `/healthz`.
- Produces authenticated doctor relay check.
- Produces log events with `surface`, `service`, `action`, `machine_id`, `status`, `kind`, `elapsed_ms`; no secrets.

- [ ] **Step 1: Write failing health and log tests**

Add tests:

```rust
#[tokio::test]
async fn healthz_is_shallow_and_does_not_probe_targets() { /* registry loaded but targets offline */ }

#[tokio::test]
async fn callback_logs_do_not_include_code_state_or_target_url() { /* capture tracing output */ }
```

- [ ] **Step 2: Run failing tests**

Run:

```bash
cargo test -p labby relay_health relay_observability --all-features
```

Expected: fail until health/log behavior exists.

- [ ] **Step 3: Implement shallow health and doctor check**

`/healthz` response shape:

```json
{"status":"ok","relay":"enabled","registry":"loaded"}
```

Doctor output should distinguish: disabled, missing registry, corrupt registry, empty registry, loaded count, and optional bounded target probe.

- [ ] **Step 4: Add error status mapping**

If public relay admin uses `ToolError::Sdk { sdk_kind }`, add explicit match arms in `api/error.rs` for new kinds such as `relay_invalid_target`, `relay_registry_unavailable`, `relay_auth_required`, and `relay_conflict`.

- [ ] **Step 5: Run tests**

Run:

```bash
cargo test -p labby relay_health relay_observability --all-features
```

Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add crates/labby/src/api/services/oauth_relay.rs crates/labby/src/dispatch/doctor.rs crates/labby/src/dispatch/doctor crates/labby/src/api/services/doctor.rs crates/labby/src/api/error.rs
git commit -m "feat(oauth): add relay health and observability"
```

---

### Task 8: Cutover Documentation And Full Verification

**Files:**
- Modify: `docs/runtime/OAUTH.md`
- Modify: `docs/OPERATIONS.md`
- Create: `docs/deploy/CALLBACK_RELAY.md`
- Modify: `docs/README.md` if docs index needs the new page.

**Interfaces:**
- Produces copy-paste cutover procedure.
- Produces rollback procedure to `callback-relay:39001`.

- [ ] **Step 1: Write docs with exact cutover commands**

Include these commands:

```bash
ssh squirts 'docker exec swag curl -fsS --max-time 5 http://100.88.16.79:40100/health'
ssh squirts 'docker exec callback-relay cat /app/.cache/callback-relay/registry.json' > /tmp/callback-relay-registry.json
labby oauth relay-registry import --file /tmp/callback-relay-registry.json --json
curl -fsS --max-time 5 https://callback.tootie.tv/healthz
```

Include rollback:

```bash
ssh squirts 'docker exec swag nginx -t'
ssh squirts 'docker exec swag nginx -s reload'
```

Describe restoring SWAG upstream to `callback-relay:39001`.

- [ ] **Step 2: Document client behavior**

State plainly:

- Regular non-headless desktop clients should keep local loopback callbacks.
- Remote/headless/cross-namespace clients may use `mcp_oauth_callback_url = "https://callback.tootie.tv/callback/<machine>"`.
- The relay forwards only; Codex or the MCP client still owns PKCE and token exchange.

- [ ] **Step 3: Run verification**

Run:

```bash
cargo fmt --all --check
cargo test -p labby oauth --all-features
cargo test -p labby public_relay --all-features
cargo test -p labby oauth_relay --all-features
cargo clippy --workspace --all-features -- -D warnings
```

Expected: all pass. If package ambiguity occurs, use the repo's known qualified package form for this workspace.

- [ ] **Step 4: Commit**

```bash
git add docs/runtime/OAUTH.md docs/OPERATIONS.md docs/deploy/CALLBACK_RELAY.md docs/README.md
git commit -m "docs(oauth): document public relay cutover"
```

---

## Self-Review Checklist

- Spec coverage: route compatibility, target policy, no redirects, caps, auth boundary, registry import, healthz, docs, and cutover are covered.
- Placeholder scan: this plan intentionally contains no deferred-work marker words or vague deferred-detail instructions.
- Type consistency: `MachineId`, `RelayTarget`, `PublicRelayRegistryStore`, `PublicRelayRegistryManager`, and `PublicRelayForwarder` are introduced before later tasks consume them.
- Deferrals are explicit: browser UI, legacy Python admin route, durable quarantine history, distributed rate limiting, non-Tailscale targets, and old-container cleanup after rollback window.

## Execution Notes For Work-It

- Create the implementation branch from a clean worktree, not the dirty coordinator checkout.
- Include this plan file in the implementation branch so reviewers can audit the plan used.
- Do not merge unrelated dirty docs from the coordinator checkout unless they are intentionally reconciled with Task 8 docs.
- After implementation, run `lavra-review`, three `code_simplifier` passes, all available `pr-review-toolkit` agents, fetch/resolve PR comments, save a session note, commit, push, and leave the PR green.
