# Remote Gateway Target Resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make plugin-launched and explicitly configured Labby clients consistently manage the selected remote daemon, with no silent execution against a competing local config.

**Architecture:** `live_gateway.rs` will own a single `TargetSet` resolver, safe endpoint construction, redirect-disabled probing, bounded discovery, typed probe failures, and the selected target's authority. `LiveGateway` retains whether its target was explicit so every caller can enforce the same rule: explicit failures are terminal; only exhaustion of opportunistic discovery permits standalone local behavior. Gateway CLI, stdio, proxy OAuth, and doctor remain thin consumers of that shared result.

**Tech Stack:** Rust 2024, Tokio, Reqwest, URL, Labby `ToolError` recovery contract, tracing, cargo-nextest.

**Spec:** `docs/design/REMOTE_GATEWAY_TARGET.md`

## Global Constraints

- Keep `rmcp = "=3.1.0"`, Rust `1.97.1`, edition `2024`, and the existing 11-crate workspace unchanged.
- Use native async Rust; do not add `async-trait`, unsafe code, or new dependencies.
- Use existing stable error kinds from `docs/dev/ERRORS.md`: `invalid_param` for target validation, `auth_required`/`auth_failed` for authentication, `service_unavailable` for failed daemon discovery, and `bridge_transport_error` for MCP bridge initialization/transport.
- Preserve mutation timeout semantics; do not add client-side timeouts to operations whose ambiguous completion could duplicate side effects.
- Explicit remote targets fail closed before and after detection; opportunistic discovery alone retains standalone local fallback.
- Use `reqwest::redirect::Policy::none()`, `Url::join`, and centralized sanitized-origin formatting for every remote endpoint.
- Treat `CLAUDE_PLUGIN_OPTION_SERVER_URL`, `LABBY_SERVER_URL`, and `LABBY_MCP_HTTP_TOKEN` as trusted operator inputs belonging to one authority domain; never log their raw values.
- Preserve unrelated worktrees and regenerate code-owned documentation only when the authoritative generator reports drift.

---

## File Map

- `docs/design/REMOTE_GATEWAY_TARGET.md`: stable behavior, safety boundaries, and non-goals.
- `crates/labby/src/live_gateway.rs`: `TargetSet`, endpoint joining, redirect-disabled client, discovery deadlines, `ProbeFailure`, capability snapshot, and `LiveGateway` authority.
- `crates/labby/src/cli/gateway/dispatch.rs`: generic gateway actions; local manager is reached only after `Ok(None)`.
- `crates/labby/src/cli/gateway/list.rs`: typed list rendering; explicit decode/dispatch failures never fall back.
- `crates/labby/src/cli/gateway/code.rs`: remote Code Mode; explicit MCP failures never become `TrustedLocal` execution.
- `crates/labby/src/cli/serve.rs`: actual stdio thin-client startup and bounded remote MCP initialization.
- `crates/labby/src/proxy/oauth.rs`: explicit target error propagation and reuse of discovered action capabilities.
- `crates/labby/src/dispatch/doctor/preflight.rs`: detailed failed finding for explicit target errors.
- `docs/runtime/ENV.md`, `docs/services/GATEWAY.md`, `plugins/labby/README.md`: operator contract and examples.

### Task 1: Implement safe authoritative target resolution

**Files:**
- Modify: `crates/labby/src/live_gateway.rs`
- Test: `crates/labby/src/live_gateway.rs`

**Interfaces:**
- Consumes: `LabConfig`, explicit raw values from `CLAUDE_PLUGIN_OPTION_SERVER_URL` and `LABBY_SERVER_URL`, existing local-bind/public URL values, and `LABBY_MCP_HTTP_TOKEN`.
- Produces:

```rust
enum TargetSet {
    Explicit { base_url: Url, source: &'static str },
    Opportunistic(Vec<Url>),
}

impl TargetSet {
    fn first(&self) -> &Url {
        match self {
            Self::Explicit { base_url, .. } => base_url,
            Self::Opportunistic(urls) => &urls[0],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeStage { Health, Identity, Actions }

struct ProbeFailure {
    stage: ProbeStage,
    status: Option<StatusCode>,
    kind: &'static str,
    message: String,
}
```

`TargetSet` makes multiple explicit targets and mode/source mismatches unrepresentable. `LiveGateway` stores the selected target's explicit flag/source and exposes endpoint operations rather than a raw string base URL.

- [ ] **Step 1: Write failing pure resolver and URL-safety tests**

Add table-driven tests that do not mutate process-global environment:

```rust
#[test]
fn plugin_target_wins_and_terminal_mcp_is_normalized() {
    let targets = resolve_target_set_from(
        Some("https://plugin.example/prefix/mcp"),
        Some("https://operator.example"),
        None,
        None,
        &LabConfig::default(),
    ).expect("valid explicit target");

    assert!(matches!(targets, TargetSet::Explicit { source: "CLAUDE_PLUGIN_OPTION_SERVER_URL", .. }));
    assert_eq!(targets.first().as_str(), "https://plugin.example/prefix/");
    assert_eq!(targets.first().join("health").unwrap().as_str(), "https://plugin.example/prefix/health");
}

#[test]
fn explicit_target_validation_matrix() {
    for accepted in [
        "https://example.com",
        "https://example.com/base/mcp/",
        "http://localhost:8765/mcp",
        "http://127.0.0.1:8765",
        "http://[::1]:8765/mcp",
    ] {
        normalize_explicit_target(accepted).unwrap_or_else(|e| panic!("{accepted}: {e}"));
    }
    for rejected in [
        "http://remote.example:8765",
        "ftp://example.com/mcp",
        "https://user:secret@example.com/mcp",
        "https://example.com/mcp?token=secret",
        "https://example.com/mcp#fragment",
    ] {
        let error = normalize_explicit_target(rejected).expect_err(rejected);
        assert_eq!(error.kind(), "invalid_param");
        assert!(!error.to_string().contains("secret"));
    }
}
```

Also cover blank higher-priority values, nonterminal `/mcp/tools`, percent-encoded paths, explicit ports, IDNA hosts, and candidate deduplication.

- [ ] **Step 2: Run resolver tests and verify they fail**

Run: `cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features live_gateway::tests::plugin_target_wins live_gateway::tests::explicit_target_validation_matrix`

Expected: FAIL because `TargetSet`, `resolve_target_set_from`, and `normalize_explicit_target` do not exist.

- [ ] **Step 3: Implement `TargetSet` and safe endpoint joining**

Implement precedence as:

```text
non-empty CLAUDE_PLUGIN_OPTION_SERVER_URL
non-empty LABBY_SERVER_URL
otherwise: local bind, LABBY_MCP_GATEWAY_URL, LABBY_PUBLIC_URL
```

Normalize exactly a terminal `/mcp` or `/mcp/` while retaining any preceding reverse-proxy prefix. Reject unsupported/credential-bearing inputs before formatting errors. Give `LiveGateway` helpers that use `Url::join("health")`, `join("v1/gateway/actions")`, `join("v1/gateway")`, and `join("mcp")`; remove endpoint `format!` calls.

- [ ] **Step 4: Write and fail redirect-isolation tests**

Start two local test servers. The configured server returns `302 Location: <second-origin>` from `/health`, `/v1/gateway/actions`, `/v1/gateway`, and `/mcp`. Assert each explicit operation fails and the second server records zero requests and zero `Authorization` headers.

Run: `cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features live_gateway::tests::redirect`

Expected: FAIL because the current default client follows redirects.

- [ ] **Step 5: Disable redirects on the one shared client**

Construct it exactly once per detection run:

```rust
let client = reqwest::Client::builder()
    .redirect(reqwest::redirect::Policy::none())
    .build()
    .map_err(|error| ToolError::Sdk {
        sdk_kind: "service_unavailable".into(),
        message: format!("remote Labby client initialization failed: {error}"),
    })?;
```

Treat every 3xx as a typed probe/dispatch failure; opportunistic discovery may advance to its next already-resolved candidate but must never follow the response.

- [ ] **Step 6: Run the complete resolver/security slice**

Run: `cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features live_gateway::tests`

Expected: PASS with exact-path and redirect-target assertions.

- [ ] **Step 7: Commit**

```bash
git add crates/labby/src/live_gateway.rs
git commit -m "fix: resolve remote gateway targets safely"
```

### Task 2: Make detection typed, bounded, and observable across every caller

**Files:**
- Modify: `crates/labby/src/live_gateway.rs`
- Modify: `crates/labby/src/cli/gateway/dispatch.rs`
- Modify: `crates/labby/src/dispatch/doctor/preflight.rs`
- Modify: `crates/labby/src/proxy/oauth.rs`
- Test: adjacent unit/integration tests for all four modules

**Interfaces:**
- Consumes: `TargetSet` and safe client from Task 1.
- Produces: `pub async fn detect(config: &LabConfig) -> Result<Option<LiveGateway>, ToolError>`. `Ok(None)` means only that bounded opportunistic discovery found no daemon. Every explicit parse/probe/identity/auth failure is `Err`.
- `LiveGateway` carries a parsed action-name capability set obtained during identity probing so later code does not refetch the catalog per capability.

- [ ] **Step 1: Write failing probe classification tests**

Add server fixtures for connection refusal, aggregate timeout, health 503, actions 401, actions 403, malformed JSON, and a healthy non-Labby service. Assert:

```rust
assert_eq!(invalid_url.kind(), "invalid_param");
assert_eq!(unauthorized.kind(), "auth_required");
assert_eq!(forbidden.kind(), "auth_failed");
assert_eq!(unavailable.kind(), "service_unavailable");
assert_eq!(unavailable.recovery().action, "retry_later");
```

Assert error envelopes and tracing fields contain only the source name and sanitized origin, never raw userinfo/query/fragment/token data.

- [ ] **Step 2: Run probe tests and verify they fail**

Run: `cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features live_gateway::tests::explicit_probe live_gateway::tests::opportunistic_discovery_deadline`

Expected: FAIL because detection collapses stages to `Option`/`bool` and has no aggregate deadline.

- [ ] **Step 3: Implement staged failures and an overall deadline**

Keep the existing `800ms` per-request limit and wrap the complete opportunistic candidate loop in a named overall deadline constant (initial value `3s`, covered by a paused-time or bounded-elapsed test). Preserve candidate priority and sequential behavior; concurrent probing is outside scope. Map `ProbeFailure` to stable `ToolError` kinds and emit one terminal event:

```rust
tracing::warn!(
    surface = "cli",
    service = "gateway",
    action = "remote.detect",
    source,
    origin = %sanitized_origin,
    elapsed_ms,
    kind,
    fallback_suppressed = explicit,
    "remote gateway detection failed"
);
```

Do not emit one warning per opportunistic candidate.

- [ ] **Step 4: Prove generic gateway dispatch cannot construct local state after explicit failure**

Add a lazy-manager test seam/counter. Call `dispatch_gateway_action` with an explicit unreachable target and assert the counter remains zero. Then call with no explicit target and no daemon and assert the local manager is created once.

- [ ] **Step 5: Define doctor and proxy OAuth semantics**

Update `dispatch/doctor/preflight.rs` so explicit detection errors become a failed finding containing the stable kind, sanitized origin, and remediation; do not abort unrelated checks. `Ok(None)` retains the current generic no-daemon finding.

Update `proxy/oauth.rs` so explicit errors propagate with context and opportunistic `Ok(None)` retains the existing “live daemon required” error. Replace three serial `supports_action` calls with one in-memory check against the capability set already stored on `LiveGateway`; test that proxy startup makes one actions request total.

- [ ] **Step 6: Audit every caller and run focused tests**

Run:

```bash
rg -n 'live_gateway::detect|remote::detect' crates/labby/src
cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features live_gateway gateway doctor proxy
```

Expected: every caller distinguishes `Err`, `Ok(None)`, and `Ok(Some(_))`; no `.ok()`, `unwrap_or(None)`, or error-to-local conversion remains. Tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/labby/src/live_gateway.rs crates/labby/src/cli/gateway/dispatch.rs crates/labby/src/dispatch/doctor/preflight.rs crates/labby/src/proxy/oauth.rs
git commit -m "fix: fail closed on explicit gateway discovery"
```

### Task 3: Preserve authority after detection in list, Code Mode, and stdio

**Files:**
- Modify: `crates/labby/src/cli/gateway/list.rs`
- Modify: `crates/labby/src/cli/gateway/code.rs`
- Modify: `crates/labby/src/cli/serve.rs`
- Modify: `crates/labby/src/live_gateway.rs`
- Test: adjacent module tests and existing stdio proxy runtime tests

**Interfaces:**
- Consumes: `LiveGateway` with explicit authority and safe endpoint/capability helpers.
- Produces: `LiveGateway::allows_local_fallback() -> bool`, bounded MCP initialization/Code Mode execution helpers, and adapters that permit local behavior only after opportunistic `Ok(None)` or an opportunistic post-selection failure explicitly allowed by the existing compatibility contract.

- [ ] **Step 1: Write failing post-detection authority tests**

Cover a reachable explicit daemon followed by:

- malformed `gateway.list` JSON;
- `/v1/gateway` 401, 403, and 500;
- `/mcp` connection refusal after healthy HTTP probes;
- MCP initialize stall;
- Code Mode tool error/timeout.

For each explicit case assert a structured nonzero error and assert the local dispatch/`TrustedLocal` executor counter remains zero. Add opportunistic counterparts proving the existing compatibility fallback remains where intentionally supported.

- [ ] **Step 2: Run tests and verify misleading fallback/hang behavior**

Run: `cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features gateway::list gateway::code stdio explicit_post_detection`

Expected: FAIL because list and Code Mode currently fall back locally, and MCP initialization is not bounded.

- [ ] **Step 3: Remove explicit list and Code Mode fallback**

In `list.rs`, propagate remote dispatch/decode failure when `live.allows_local_fallback()` is false. In `code.rs`, never call the local `TrustedLocal` executor after an explicit remote MCP connection/tool failure. Preserve existing warning/fallback only for opportunistic targets and include the sanitized source in the warning.

- [ ] **Step 4: Bound MCP initialization and Code Mode without timing out mutations**

Add a startup/initialization timeout constant (initial value `10s`) around remote MCP service initialization and the non-mutating Code Mode connection/call setup. Do not wrap arbitrary gateway mutations in this timeout. Map initialization/transport expiry to `bridge_transport_error` and pin its serialized recovery metadata in tests.

- [ ] **Step 5: Update actual stdio startup in `cli/serve.rs`**

Use:

```rust
match crate::live_gateway::detect(config).await? {
    Some(live) => live.serve_stdio_bridge().await,
    None => run_standalone_stdio(config, registry).await,
}
```

The bridge helper enforces the bounded initialization phase, then permits the long-lived session to run without a fixed lifetime timeout. An explicit initialization error exits nonzero; it never starts standalone.

- [ ] **Step 6: Run caller, timeout, and fallback regressions**

Run:

```bash
rg -n 'live_gateway::detect|remote::detect' crates/labby/src
cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features live_gateway gateway code stdio proxy doctor
```

Expected: PASS; the stalled-MCP fixture completes within the test bound, explicit local counters remain zero, and opportunistic bootstrap behavior remains covered.

- [ ] **Step 7: Commit**

```bash
git add crates/labby/src/live_gateway.rs crates/labby/src/cli/gateway/list.rs crates/labby/src/cli/gateway/code.rs crates/labby/src/cli/serve.rs
git commit -m "fix: preserve remote gateway authority after detection"
```

### Task 4: Document and verify the complete contract

**Files:**
- Modify: `docs/runtime/ENV.md`
- Modify: `docs/services/GATEWAY.md`
- Modify: `plugins/labby/README.md`
- Modify if generator requires: `docs/generated/*`
- Test: docs, all-features workspace, and fresh-process runtime checks

**Interfaces:**
- Consumes: final behavior and constants from Tasks 1–3.
- Produces: an operator contract that distinguishes explicit client target, daemon bind, app URL, MCP resource URL, trusted target/token boundary, fallback behavior, and timeout/error behavior.

- [ ] **Step 1: Update the exact precedence/fallback table**

Document:

```text
CLAUDE_PLUGIN_OPTION_SERVER_URL  explicit invocation target; fail closed
LABBY_SERVER_URL                 explicit operator target; fail closed
local bind candidate             opportunistic
LABBY_MCP_GATEWAY_URL            opportunistic compatibility candidate
LABBY_PUBLIC_URL                 opportunistic compatibility candidate
standalone local config          only after bounded opportunistic exhaustion
```

Explain terminal `/mcp` normalization, reverse-proxy path preservation, redirect rejection, safe loopback HTTP, trusted target/token pairing, and the difference from `mcp.host`/`mcp.port`.

- [ ] **Step 2: Update gateway and plugin docs with post-detection semantics**

State that explicit authority covers probing, gateway dispatch, list decoding, Code Mode, and stdio MCP initialization. Include one plugin example and one ordinary `LABBY_SERVER_URL` example. Explain that opportunistic compatibility discovery may still fall back locally and that this is intentionally unsuitable when the operator requires one authoritative daemon.

- [ ] **Step 3: Check generated-doc ownership before regenerating**

Run: `just docs-check`

Expected: PASS if the new client target is hand-documented only. If it reports authoritative metadata drift, run `just docs-generate`, inspect the diff, and rerun `just docs-check`; never stage the entire generated directory blindly.

- [ ] **Step 4: Run focused and full verification**

Run:

```bash
cargo nextest run --manifest-path crates/labby/Cargo.toml --all-features live_gateway gateway code stdio proxy doctor
just check
just test
just lint
just docs-check
```

Expected: every command passes. Record unrelated pre-existing failures with the exact command and boundary rather than weakening acceptance.

- [ ] **Step 5: Verify the reported Tidewave scenario in fresh processes**

With a temporary client home whose XDG config omits Tidewave, provide only the supported server target and bearer token, then run:

```bash
LABBY_HOME="$temporary_client_home" \
LABBY_SERVER_URL="$production_labby_base_url" \
LABBY_MCP_HTTP_TOKEN="$production_token" \
labby --json gateway get tidewave
```

Expected: success from the connected daemon. Repeat with `LABBY_SERVER_URL=http://127.0.0.1:9`; expect a structured nonzero `service_unavailable` error, `fallback_suppressed=true` in logs, and no `config.toml` created under the temporary client home. Repeat through the plugin-provided `CLAUDE_PLUGIN_OPTION_SERVER_URL` path to prove the original integration boundary.

- [ ] **Step 6: Commit**

```bash
git add docs/design/REMOTE_GATEWAY_TARGET.md docs/runtime/ENV.md docs/services/GATEWAY.md plugins/labby/README.md
git add docs/generated  # only when Step 3 produced reviewed generated changes
git commit -m "docs: define authoritative remote gateway routing"
```

## Deferred Follow-ups

- CLI `--server-url`, response-source metadata, persisted target migration, target-scoped token storage, certificate pinning, private-network allowlists, concurrent public probing, global client caching, metrics, HashSet deduplication, and mutation operation IDs remain outside this repair. Each requires a separate validated requirement; none blocks authoritative Tidewave routing.

## Self-Review Record

- Spec coverage: every requirement and acceptance criterion maps to Tasks 1–4, including redirect isolation, post-detection authority, all callers, bounded discovery/MCP initialization, action-capability reuse, redaction, and fresh-process proof.
- Placeholder scan: every code/test step names exact interfaces, commands, assertions, and expected outcomes.
- Type consistency: all tasks use `TargetSet`, `ProbeFailure`, `detect(&LabConfig) -> Result<Option<LiveGateway>, ToolError>`, stored capability names, and `LiveGateway::allows_local_fallback()` consistently.
- Review coverage: all 12 actionable architecture, simplicity, security, and performance recommendations are incorporated; deferred items are explicitly listed with scope rationale.
