# OAuth Local Callback Relay Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a local OAuth callback forwarder to `lab` so a browser machine can accept localhost redirects and forward them to a configured remote callback target.

**Architecture:** Add a small top-level `oauth` module in the `lab` crate with three focused responsibilities: config-backed machine target resolution, a loopback-only HTTP forwarder that preserves callback path/query/body, and a thin CLI shim that starts the forwarder. Reuse the proven semantics from the existing Python relay: suffix-path query preservation, hop-by-hop header stripping, precise operator-facing errors, and no token/code leakage in logs.

**Tech Stack:** Rust 2024, `tokio`, `axum`, `reqwest`, `clap`, `serde`, `tracing`, existing `LabConfig` loading

---

## File Structure

- Create: `crates/lab/src/oauth.rs`
  - Public module declarations and shared re-exports for the local relay feature.
- Create: `crates/lab/src/oauth/error.rs`
  - Focused error enum for target resolution, bind failures, and forwarding failures.
- Create: `crates/lab/src/oauth/target.rs`
  - Machine config types, target lookup, URL construction, and hop-by-hop header filtering helpers.
- Create: `crates/lab/src/oauth/local_relay.rs`
  - Loopback listener, axum router, request forwarding handler, and runtime startup/shutdown path.
- Create: `crates/lab/src/cli/oauth.rs`
  - Thin `clap` shim for `lab oauth relay-local`.
- Modify: `crates/lab/src/cli.rs`
  - Register `oauth` module and top-level `Command::Oauth`.
- Modify: `crates/lab/src/main.rs`
  - Declare the new top-level `oauth` sibling module for the binary crate.
- Modify: `crates/lab/src/config.rs`
  - Add durable `[oauth.machines.<id>]` config parsing and tests.
- Modify: `docs/CLI.md`
  - Document the new `lab oauth relay-local` command and flags.
- Modify: `docs/OPERATIONS.md`
  - Document the browser-machine workflow and the fact that the helper is normally run on demand.
- Modify: `docs/CONFIG.md`
  - Document the new `[oauth.machines.*]` config shape.
- Modify: `config.example.toml`
  - Add a copyable config example for named callback targets.

---

### Task 1: Add config-backed machine target definitions

**Files:**
- Create: `crates/lab/src/oauth.rs`
- Create: `crates/lab/src/oauth/target.rs`
- Modify: `crates/lab/src/config.rs`
- Test: `crates/lab/src/config.rs`
- Test: `crates/lab/src/oauth/target.rs`

- [ ] **Step 1: Write the failing config tests**

Add tests in `crates/lab/src/config.rs` that deserialize a config snippet with:

```toml
[oauth.machines.dookie]
target_url = "http://100.88.16.79:38935/callback/dookie"
description = "Dookie Claude callback target"
default_port = 38935
```

and assert:

```rust
assert_eq!(cfg.oauth.machines["dookie"].target_url, "http://100.88.16.79:38935/callback/dookie");
assert_eq!(cfg.oauth.machines["dookie"].default_port, Some(38935));
```

- [ ] **Step 2: Run the config tests to verify they fail**

Run:

```bash
cargo test -p lab oauth_machine -- --nocapture
```

Expected: FAIL because `LabConfig` does not yet expose an `oauth` machine registry.

- [ ] **Step 3: Add the config types**

Implement the minimal config structures in `crates/lab/src/config.rs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OauthPreferences {
    #[serde(default)]
    pub machines: std::collections::BTreeMap<String, OauthMachineConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthMachineConfig {
    pub target_url: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_port: Option<u16>,
}
```

and add `pub oauth: OauthPreferences` to `LabConfig`.

Use:

```rust
#[serde(default)]
pub oauth: OauthPreferences,
```

so existing configs without an `[oauth]` section continue to parse unchanged.

- [ ] **Step 4: Add target resolution tests in the new module**

Create `crates/lab/src/oauth/target.rs` tests that assert:

```rust
let machines = BTreeMap::from([(
    "dookie".to_string(),
    OauthMachineConfig {
        target_url: "http://100.88.16.79:38935/callback/dookie".to_string(),
        description: None,
        default_port: Some(38935),
    },
)]);

let resolved = resolve_machine_target(&machines, "dookie")?;
assert_eq!(resolved.machine_id.as_deref(), Some("dookie"));
assert_eq!(resolved.target_url.as_str(), "http://100.88.16.79:38935/callback/dookie");
```

and a missing-machine test that expects the error to include the requested ID plus available IDs.

- [ ] **Step 5: Implement minimal target lookup**

In `crates/lab/src/oauth/target.rs`, add a small resolver API:

```rust
pub struct ResolvedTarget {
    pub machine_id: Option<String>,
    pub target_url: url::Url,
    pub default_port: Option<u16>,
}

pub fn resolve_machine_target(
    machines: &BTreeMap<String, OauthMachineConfig>,
    machine_id: &str,
) -> Result<ResolvedTarget, OauthRelayError>
```

Do not add forwarding logic yet. Only validate machine existence and URL parsing.

- [ ] **Step 6: Run the focused tests to verify they pass**

Run:

```bash
cargo test -p lab@0.3.3 oauth_machine -- --nocapture
cargo test -p lab@0.3.3 resolve_machine_target -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/config.rs crates/lab/src/oauth.rs crates/lab/src/oauth/target.rs
git commit -m "feat: add oauth relay machine config"
```

### Task 2: Implement forwarding URL construction and header filtering

**Files:**
- Modify: `crates/lab/src/oauth/target.rs`
- Create: `crates/lab/src/oauth/error.rs`
- Test: `crates/lab/src/oauth/target.rs`

- [ ] **Step 1: Write the failing URL construction tests**

Add tests for the Python-relay-compatible behavior:

```rust
assert_eq!(
    build_forward_url(
        &Url::parse("http://100.88.16.79:38935/callback/dookie").unwrap(),
        "foo/bar",
        &[("code", "abc"), ("state", "xyz")],
    )?.as_str(),
    "http://100.88.16.79:38935/callback/dookie/foo/bar?code=abc&state=xyz"
);
```

and:

```rust
assert_eq!(
    build_forward_url(
        &Url::parse("http://target/callback?existing=1").unwrap(),
        "",
        &[("code", "abc")],
    )?.as_str(),
    "http://target/callback?existing=1&code=abc"
);
```

- [ ] **Step 2: Write the failing header-filter test**

Add a test that confirms a helper removes the standard hop-by-hop headers:

```rust
assert!(!filtered.contains_key("connection"));
assert!(!filtered.contains_key("transfer-encoding"));
assert!(filtered.contains_key("content-type"));
```

- [ ] **Step 3: Write the failing response-header-filter test**

Add a second helper test that verifies the response-side filtering removes:

```rust
assert!(!filtered.contains_key("connection"));
assert!(!filtered.contains_key("content-length"));
assert!(filtered.contains_key("content-type"));
```

- [ ] **Step 4: Run the focused tests to verify they fail**

Run:

```bash
cargo test -p lab@0.3.3 build_forward_url -- --nocapture
cargo test -p lab@0.3.3 hop_by_hop -- --nocapture
```

Expected: FAIL because the helpers do not yet exist.

- [ ] **Step 5: Add the minimal error type**

Create `crates/lab/src/oauth/error.rs`:

```rust
#[derive(Debug, thiserror::Error)]
pub enum OauthRelayError {
    #[error("unknown oauth relay machine `{machine_id}`; available machines: {available}")]
    UnknownMachine { machine_id: String, available: String },
    #[error("invalid oauth relay target URL `{value}`: {source}")]
    InvalidTargetUrl { value: String, source: url::ParseError },
    #[error("failed to bind local oauth relay on {bind_addr}: {source}")]
    Bind { bind_addr: String, source: std::io::Error },
    #[error("failed to reach oauth relay target `{target}`: {source}")]
    Upstream { target: String, source: reqwest::Error },
    #[error("oauth relay target `{target}` timed out after {timeout_ms}ms")]
    UpstreamTimeout { target: String, timeout_ms: u64 },
}
```

- [ ] **Step 6: Implement the URL and header helpers**

In `crates/lab/src/oauth/target.rs`, add:

```rust
pub fn build_forward_url(
    target_base: &url::Url,
    suffix_path: &str,
    query_items: &[(&str, &str)],
) -> Result<url::Url, OauthRelayError>
```

and a helper that strips the same hop-by-hop headers as the Python relay.

- [ ] **Step 7: Run the focused tests to verify they pass**

Run:

```bash
cargo test -p lab@0.3.3 build_forward_url -- --nocapture
cargo test -p lab@0.3.3 hop_by_hop -- --nocapture
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/lab/src/oauth/error.rs crates/lab/src/oauth/target.rs
git commit -m "feat: add oauth relay forwarding helpers"
```

### Task 3: Build the loopback local relay runtime

**Files:**
- Create: `crates/lab/src/oauth/local_relay.rs`
- Modify: `crates/lab/src/oauth.rs`
- Test: `crates/lab/src/oauth/local_relay.rs`

- [ ] **Step 1: Write the failing end-to-end forwarding test**

Create an integration-style unit test in `crates/lab/src/oauth/local_relay.rs` that:

1. starts a mock upstream axum server on a random port
2. starts the local relay on another random loopback port
3. sends:

```http
POST /callback/dookie/extra?code=abc&state=xyz
Content-Type: application/x-www-form-urlencoded
```

4. asserts the mock upstream receives `/callback/dookie/extra?code=abc&state=xyz`
5. asserts the mock upstream receives the exact request body once and only once
6. asserts the relay returns the upstream status, body, and content type unchanged

Use an assertion shape like:

```rust
assert_eq!(seen_requests.len(), 1);
assert_eq!(seen_requests[0].path_and_query, "/callback/dookie/extra?code=abc&state=xyz");
assert_eq!(seen_requests[0].body, b"grant_type=authorization_code");
assert_eq!(response.status(), StatusCode::CREATED);
assert_eq!(response_body, "ok-from-upstream");
assert_eq!(response.headers()["content-type"], "text/plain; charset=utf-8");
```

- [ ] **Step 2: Write the failing unreachable-upstream test**

Add a test that starts the relay with a target port that is not listening and asserts:

```rust
assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
assert!(body.contains("failed to reach oauth relay target"));
```

- [ ] **Step 3: Write the failing timeout and bind-collision tests**

Add:

- a timeout test where the upstream intentionally sleeps longer than the relay timeout and the relay responds with:

```rust
assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
assert!(body.contains("timed out"));
```

- a bind-collision test that pre-binds `127.0.0.1:<port>` and asserts `run_local_relay(...)` returns `OauthRelayError::Bind`.

- [ ] **Step 4: Write the failing response-filter and redaction tests**

Add tests that verify:

- hop-by-hop response headers are removed from the proxied response
- error bodies are JSON with a single `detail` string
- request logs do not contain raw `code=...` or `state=...` values

Use the existing tracing test support pattern already used elsewhere in this repo rather than inventing a custom logger harness.

- [ ] **Step 5: Run the focused tests to verify they fail**

Run:

```bash
cargo test -p lab@0.3.3 oauth_local_relay -- --nocapture
```

Expected: FAIL because there is no relay runtime yet.

- [ ] **Step 6: Implement the minimal relay runtime**

In `crates/lab/src/oauth/local_relay.rs`, add:

```rust
pub struct LocalRelayConfig {
    pub bind_addr: std::net::SocketAddr,
    pub resolved_target: ResolvedTarget,
    pub request_timeout: std::time::Duration,
}

pub async fn run_local_relay(config: LocalRelayConfig) -> Result<(), OauthRelayError>
```

Use `axum` for the local listener and `reqwest::Client` for forwarding. Keep the handler focused:

```rust
async fn relay_callback(
    State(state): State<RelayState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response
```

Bind only to loopback and reject non-`GET`/`POST` methods with `405`.

- [ ] **Step 7: Add startup logging and operator-visible startup output**

On successful startup, emit both:

```rust
tracing::info!(
    surface = "oauth_relay",
    bind_addr = %config.bind_addr,
    machine_id = ?config.resolved_target.machine_id,
    target = %config.resolved_target.target_url,
    "oauth relay local listener ready"
);
```

and a concise stdout line such as:

```text
OAuth relay listening on http://127.0.0.1:38935 -> http://100.88.16.79:38935/callback/dookie
```

so operators can confirm the forwarding target before the first callback arrives.

- [ ] **Step 8: Add safe request logging**

Log one event per forwarded callback with:

```rust
tracing::info!(
    surface = "oauth_relay",
    method = %method,
    path = %uri.path(),
    machine_id = ?state.resolved_target.machine_id,
    target_host = %target_host,
    status = %status,
    elapsed_ms = elapsed.as_millis(),
    "oauth relay forward complete"
);
```

Do not log raw query strings or request bodies.

- [ ] **Step 9: Add explicit response filtering and timeout mapping**

Make sure the handler:

- filters hop-by-hop headers on both the outbound request and returned response
- maps transport errors to `502`
- maps timeout errors to `504`
- preserves non-2xx upstream status/body/content-type when the upstream was reached
- returns small JSON error envelopes like:

```json
{"detail":"failed to reach oauth relay target: connection refused"}
```

- [ ] **Step 10: Run the focused tests to verify they pass**

Run:

```bash
cargo test -p lab@0.3.3 oauth_local_relay -- --nocapture
```

Expected: PASS.

- [ ] **Step 11: Commit**

```bash
git add crates/lab/src/oauth.rs crates/lab/src/oauth/local_relay.rs
git commit -m "feat: add local oauth callback relay runtime"
```

### Task 4: Add the CLI shim and wire it into `lab`

**Files:**
- Create: `crates/lab/src/cli/oauth.rs`
- Modify: `crates/lab/src/cli.rs`
- Modify: `crates/lab/src/main.rs`
- Modify: `crates/lab/src/oauth/target.rs`
- Test: `crates/lab/src/cli/oauth.rs`

- [ ] **Step 1: Write the failing CLI parsing test**

Add a test in `crates/lab/src/cli/oauth.rs` that parses:

```rust
let cli = Cli::try_parse_from([
    "lab",
    "oauth",
    "relay-local",
    "--machine",
    "dookie",
    "--port",
    "38935",
])?;
```

and asserts the parsed command shape matches the new subcommand.

- [ ] **Step 2: Write the failing explicit-target parsing test**

Add a second test for:

```rust
let cli = Cli::try_parse_from([
    "lab",
    "oauth",
    "relay-local",
    "--forward-base",
    "http://100.88.16.79:38935/callback/dookie",
    "--port",
    "38935",
])?;
```

and assert `--machine` and `--forward-base` are mutually exclusive.

- [ ] **Step 3: Write the failing explicit-target runtime test**

Add a test for the target resolver path that proves:

```rust
let resolved = resolve_explicit_target(
    "http://100.88.16.79:38935/callback/dookie",
    Some(38935),
)?;
assert_eq!(resolved.machine_id, None);
assert_eq!(resolved.target_url.as_str(), "http://100.88.16.79:38935/callback/dookie");
```

- [ ] **Step 4: Run the parsing tests to verify they fail**

Run:

```bash
cargo test -p lab@0.3.3 oauth_relay_local_cli -- --nocapture
```

Expected: FAIL because the `oauth` command does not exist yet.

- [ ] **Step 5: Implement the thin CLI shim**

Create `crates/lab/src/cli/oauth.rs` with the thin command shape:

```rust
#[derive(Debug, Args)]
pub struct OauthArgs {
    #[command(subcommand)]
    pub command: OauthCommand,
}

#[derive(Debug, Subcommand)]
pub enum OauthCommand {
    RelayLocal(RelayLocalArgs),
}

#[derive(Debug, Args)]
pub struct RelayLocalArgs {
    #[arg(long, conflicts_with = "forward_base")]
    pub machine: Option<String>,
    #[arg(long, conflicts_with = "machine")]
    pub forward_base: Option<String>,
    #[arg(long)]
    pub port: u16,
}
```

Add `Command::Oauth(oauth::OauthArgs)` to `crates/lab/src/cli.rs` and dispatch to `oauth::run(args, &config).await`.
Add `mod oauth;` to `crates/lab/src/main.rs`.

- [ ] **Step 6: Keep business logic out of the CLI**

In the CLI `run` function, only:

- validate that exactly one of `--machine` or `--forward-base` is present
- resolve the target using `oauth::target`
- call `oauth::local_relay::run_local_relay`

Do not build forwarding URLs or manipulate headers in the CLI module.

- [ ] **Step 7: Implement the explicit-target resolution path**

In `crates/lab/src/oauth/target.rs`, add:

```rust
pub fn resolve_explicit_target(
    target_url: &str,
    default_port: Option<u16>,
) -> Result<ResolvedTarget, OauthRelayError>
```

and make the CLI use it when `--forward-base` is provided.

- [ ] **Step 8: Run the parsing tests and one manual smoke command**

Run:

```bash
cargo test -p lab@0.3.3 oauth_relay_local_cli -- --nocapture
cargo run -- oauth relay-local --help
```

Expected:

- tests PASS
- help output includes `relay-local`, `--machine`, `--forward-base`, and `--port`

- [ ] **Step 9: Commit**

```bash
git add crates/lab/src/cli.rs crates/lab/src/cli/oauth.rs
git commit -m "feat: add oauth relay-local cli command"
```

### Task 5: Document the operator workflow and verify the full slice

**Files:**
- Modify: `README.md`
- Modify: `docs/CLI.md`
- Modify: `docs/OPERATIONS.md`
- Modify: `docs/CONFIG.md`
- Modify: `config.example.toml`
- Test: workspace verification commands

- [ ] **Step 1: Update `docs/CONFIG.md` with the new config shape**

Document:

```toml
[oauth.machines.dookie]
target_url = "http://100.88.16.79:38935/callback/dookie"
description = "Dookie Claude callback target"
default_port = 38935
```

and explain that `target_url` is the callback base URL, not just a host.

- [ ] **Step 2: Update `config.example.toml` with a copyable example**

Add a small commented example:

```toml
[oauth.machines.dookie]
target_url = "http://100.88.16.79:38935/callback/dookie"
description = "Dookie Claude callback target"
default_port = 38935
```

- [ ] **Step 3: Update `docs/CLI.md` with the new command contract**

Document:

- `lab oauth relay-local --machine <id> --port <port>`
- `lab oauth relay-local --forward-base <url> --port <port>`
- mutual exclusivity of `--machine` and `--forward-base`
- loopback-only bind behavior

- [ ] **Step 4: Update `docs/OPERATIONS.md` with the browser-machine workflow**

Add an explicit section covering:

- why localhost-only callback clients need the relay-local helper
- `lab oauth relay-local --machine dookie --port 38935`
- ad hoc explicit-target usage
- the requirement that the remote callback listener is already running
- the fact that `lab` does not mint tokens or complete PKCE itself
- the expectation that the helper is usually run on demand rather than as a daemon in the first cut

- [ ] **Step 5: Update `README.md` with a short operator example**

Add one concise example showing a browser machine forwarding a localhost callback to a remote Tailscale target.

- [ ] **Step 6: Run the focused test suite**

Run:

```bash
cargo test -p lab@0.3.3 oauth_machine -- --nocapture
cargo test -p lab@0.3.3 build_forward_url -- --nocapture
cargo test -p lab@0.3.3 oauth_local_relay -- --nocapture
cargo test -p lab@0.3.3 oauth_relay_local_cli -- --nocapture
```

Expected: PASS.

- [ ] **Step 7: Run the all-features verification slice**

Run:

```bash
cargo test --workspace --all-features --tests --no-fail-fast
cargo build --workspace --all-features
```

Expected:

- tests PASS
- build PASS

- [ ] **Step 8: Run a manual smoke test**

In one terminal, run a tiny target server that records requests. In another, start:

```bash
cargo run -- oauth relay-local --forward-base http://127.0.0.1:48081/callback/dookie --port 38935
```

Then send:

```bash
curl -i 'http://127.0.0.1:38935/callback/dookie/extra?code=abc&state=xyz'
```

Expected:

- local relay returns the upstream response
- target server sees `/callback/dookie/extra?code=abc&state=xyz`

- [ ] **Step 9: Commit**

```bash
git add README.md docs/CLI.md docs/OPERATIONS.md docs/CONFIG.md config.example.toml
git commit -m "docs: document oauth relay-local workflow"
```
