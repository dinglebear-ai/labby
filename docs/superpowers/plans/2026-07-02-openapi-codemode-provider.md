# OpenAPI-to-Tool Code Mode Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a third Code Mode local provider — `openapi` — that turns a configured OpenAPI spec into locally-dispatched, LLM-callable Code Mode tools (`openapi::<label>.<operationId>(params)`) using the `rmcp-openapi` library crate **for spec parsing / tool-descriptor generation only**, while `labby-openapi` performs the actual outbound HTTP through its own hardened client. No sidecar MCP server.

**Architecture:** A new isolated crate `crates/labby-openapi` owns all `rmcp-openapi`/`reqwest`-touching code (keeping `labby-codemode` HTTP-free, per its charter). `labby-codemode` gains an `Openapi` local-provider variant and consumes `labby-openapi` through a required `CodeModeHost` accessor. Specs load once at process start (concurrently, with per-spec timeouts, a body-size cap, and degraded-boot fallback) into an in-memory per-label registry. **`rmcp-openapi` is used ONLY to parse a spec into operation descriptors (method, path template, input schema, security scheme); the outbound HTTP call is executed by `labby-openapi`'s own `reqwest::Client` built with `redirect::Policy::none()` and a custom connector that re-validates the connecting peer IP** — because the security review confirmed `rmcp-openapi` v0.31.2 exposes no redirect control and no client-injection seam, making its built-in executor unsafe for an SSRF-sensitive surface. Dispatch is gated by the existing admin+unscoped gate PLUS a mandatory deny-by-default per-operation allowlist PLUS SSRF containment reusing the workspace-canonical `labby_primitives::ssrf` guard. Credentials are injected server-side, after the sandbox boundary — the JS snippet never sees a raw key.

**Tech Stack:** Rust 2024, `rmcp-openapi` v0.31.2 (crates.io, `rmcp = ^1.0.0` compatible with the pinned `1.7.0`; transitive `reqwest ^0.13` matches the workspace `0.13` pin) — **used for spec parsing only**; `reqwest` (workspace pin, hardened client owned by `labby-openapi`); `tokio`, `serde`/`serde_json`, `thiserror`, `tracing`, `labby-runtime` (`ToolError`, `lab_home`), `labby-primitives::ssrf`.

## Global Constraints

- **Crate placement is LOCKED:** all `rmcp-openapi`/`reqwest` code lives in the NEW crate `crates/labby-openapi`. Dependency direction is strictly `labby-codemode -> labby-openapi -> labby-runtime`/`labby-primitives`. `labby-openapi` MUST NOT depend on `labby-codemode` or `labby-gateway` (cycle-free). `labby-apis` is NOT touched.
- **`rmcp-openapi-server` (the standalone binary crate) is OUT OF SCOPE.** Use only the `rmcp-openapi` library crate, and only its spec-parsing / tool-descriptor surface — **NOT** its built-in HTTP executor (`Tool::call()`/`execute()`/`HttpClient::execute_tool_call()`), which the security review confirmed follows redirects with no override.
- **The outbound HTTP call is executed by `labby-openapi`'s OWN hardened `reqwest::Client`:** `redirect::Policy::none()`, `https_only(true)`, explicit `connect_timeout` + per-call `timeout`, and a custom resolver/connector (or single-connect-then-verify pattern) that re-checks the connecting peer IP against `labby_primitives::ssrf::check_ip_not_private` — this is the ONLY defense that closes both the redirect-bypass and DNS-rebinding gaps. Reference implementation: the `acp_registry` installer's "pin a validated address, re-check the peer IP post-connect" pattern noted in `crates/labby-primitives/src/ssrf.rs`'s module doc.
- **`base_url_override` is MANDATORY in config** (not optional). `rmcp-openapi` never consults the spec's `servers[]`, so the base URL must always be operator-configured and SSRF-validated. Config validation rejects a spec with no `base_url_override`. There is NO `servers[]` parsing/selection code.
- **Discovery-catalog integration is OUT OF SCOPE for v1.** Ship a flat, non-discoverable JS shim `globalThis.openapi = { call: function(label, operationId, params) {...} }`, matching the exact bar `state`/`git` already clear. Do NOT add `openapi` operations to `CodeModeDiscoveryEntry`/`generate_discovery_js()`. Because there is no per-operation JS proxy in v1, there is NO operationId→JS-identifier sanitization and no per-op `input_schema` surfaced — both are deferred with discovery.
- **Refresh policy is LOCKED to load-once-at-process-start for v1.** No background refresh, no `ArcSwap`, no TTL. A spec that fails to load is omitted with a structured WARN; `labby serve` still reaches ready. Any future refresh MUST rebuild the whole `SpecEntry` (allowlist is baked in at load), never patch it in place.
- **Config loading lives ONLY in `crates/labby/src/config.rs`.** `labby-codemode`/`labby-openapi` never read env vars or files. Non-secret config (spec URL, label, **mandatory** base_url, allowlist) in `config.toml` `[[openapi.specs]]`; credentials (`OPENAPI_<LABEL>_API_KEY` / `OPENAPI_<LABEL>_TOKEN`) in `.env`.
- **Gate is admin+unscoped (`local_providers_allowed()`, reused — no second gating fn) PLUS a mandatory deny-by-default per-operation allowlist PLUS the hardened-client SSRF containment above.** All three required.
- **SSRF containment MUST reuse `labby_primitives::ssrf`** (`parse_validated_https_url`, `check_host_not_private`, `check_ip_not_private`, `is_cgnat`, `redact_url`) — verified present with those signatures (`crates/labby-primitives/src/ssrf.rs:59,78,118,136,161`). Do NOT hand-roll RFC1918/loopback/CGNAT checks.
- **Credentials injected server-side after the sandbox boundary.** The JS snippet only supplies `{operationId, params}`. Consistent with the runner's `env_clear()` invariant.
- **Mandatory redaction** (`docs/dev/OBSERVABILITY.md:367-375`) on happy AND error paths. Default logging captures method / resolved host / path template / status / `elapsed_ms` only — never third-party response bodies. NEVER `.to_string()` or `{:?}`/`{}` a raw `rmcp_openapi::*` or upstream error into any `tracing`/`ToolError` message — always map to a fixed, scrubbed `OpenApiError` variant first. A committed canary test (Task 8) enforces this.
- **Standard dispatch fields:** `surface`, `service`, `action`, `elapsed_ms` always; `kind` on errors. Level: INFO success, WARN caller errors, ERROR fatal.
- **No panics / no `unwrap`** on spec parse or dispatch paths — return typed `ToolError`-compatible errors. The `From<OpenApiError> for ToolError` impl MUST target the REAL `ToolError` variants (`crates/labby-runtime/src/error.rs`): `Sdk { sdk_kind, message }`, `UnknownInstance { message, valid }`, `Forbidden { message, required_scopes }`, `InvalidParam { message, param }`, `UnknownAction`, `MissingParam`, `Conflict`, `AmbiguousTool`, `ConfirmationRequired`. There is NO `ToolError::Timeout` or `ToolError::Internal` — timeout/internal go through `Sdk { sdk_kind: "timeout" / "internal_error" }`.
- **operationId is the RAW allowlist + dispatch key.** The allowlist check and the HTTP path-template substitution key off the SAME raw operationId. (No sanitized JS identifier exists in v1.)
- **`openapi` dispatch MUST NOT share `LOCAL_PROVIDER_LOCK`** (`runner_drive.rs:47`) with `state`/`git`, and MUST NOT be routed through `dispatch_local_provider_stub` (which runs *inside* that lock at `runner_drive.rs:645`). It branches in `enqueue_local_provider_call` **before** the lock is taken.
- **Build/verify truth is all-features:** `cargo nextest run --all-features`, `cargo clippy --all-features -- -D warnings`, `RUSTFLAGS="-D warnings" cargo check --all-targets`, `cargo deny check`. No `mod.rs`. No `#[async_trait]`. Prefer `impl Trait`/concrete types.
- **Commit frequently**, path-limited, one clear message per task.

---

## File Structure

New crate `crates/labby-openapi/`:

| File | Responsibility |
|------|----------------|
| `Cargo.toml` | deps `rmcp-openapi = "0.31.2"`, `labby-runtime`, `labby-primitives`, `tokio`, `serde`, `serde_json`, `thiserror`, `tracing`, `url`, `reqwest`. |
| `src/lib.rs` | Public surface: config types, `OpenApiRegistry`, `dispatch::dispatch_openapi_call`. |
| `src/config.rs` | `OpenApiSpecConfig` (label, spec source, **mandatory** base_url, allowed_operations, credential), `OpenApiProviderConfig`, secret-redacting `Debug`, `RESERVED_NAMESPACES`. Pure data. |
| `src/error.rs` | `OpenApiError` (thiserror, scrubbed `Display`), `impl From<OpenApiError> for ToolError` (real variants), `kind()`. |
| `src/ssrf.rs` | `validate_base_url(cfg) -> url::Url` through `labby_primitives::ssrf`. (No `servers[]` parsing.) |
| `src/http.rs` | The hardened `reqwest::Client` builder (`redirect::none`, `https_only`, connect/read timeouts, peer-IP-revalidating connector) + `execute_operation(client, op, base_url, params, credential)` performing the outbound call and mapping errors to scrubbed `OpenApiError`. **This is where the actual HTTP happens — not rmcp-openapi.** |
| `src/convert.rs` | Spec → `OperationDescriptor { operation_id, method, path_template, security }` via `rmcp-openapi`'s parsing surface; allowlist filtering on the raw operationId. No JS-identifier sanitization, no stored `input_schema` in v1. |
| `src/registry.rs` | `OpenApiRegistry` (`Arc<HashMap<label, SpecEntry>>`), concurrent load-at-startup with per-spec timeout + body-size cap + truncation WARN. |
| `src/dispatch.rs` | `dispatch_openapi_call(registry, http_client, label, op, params)` — lookup, allowlist-implied unknown-op, credential injection, calls `http::execute_operation`. |
| `src/tests_config.rs` / `tests_ssrf.rs` / `tests_dispatch.rs` | Config redaction / label-collision; base-URL validation + RFC1918/CGNAT rejection; dispatch happy / unknown-op / unknown-label / **secret-canary-leak** tests. |

Modified files:

| File | Change |
|------|--------|
| `Cargo.toml` (root) | Add `crates/labby-openapi` to `members`; add `rmcp-openapi` to `[workspace.dependencies]`. |
| `crates/labby-codemode/Cargo.toml` | Add `labby-openapi = { path = "../labby-openapi" }`. |
| `crates/labby-codemode/src/local_provider.rs` | Add `LocalProviderName::Openapi`; extend `as_str()`, `is_reserved_provider_namespace()`, `try_parse_local_provider_call()`. |
| `crates/labby-codemode/src/runner_drive.rs` | Add `openapi_registry` + shared `openapi_http_client` to `RunnerConfig`; branch `Openapi` in `enqueue_local_provider_call` **before** the lock (no `LOCAL_PROVIDER_LOCK`, not via the stub). |
| `crates/labby-codemode/src/execute.rs` | Assemble the registry/client into `RunnerConfig` from `self.host`; emit the openapi shim on the host path only. |
| `crates/labby-codemode/src/preamble.rs` | Add `generate_openapi_provider_js() -> &'static str` (const shim). |
| `crates/labby-codemode/src/host.rs` | Add a REQUIRED (no-default) `CodeModeHost::openapi_registry()` accessor (+ shared client). |
| `crates/labby-codemode/src/tests_ids_schema.rs` | `openapi::<label>.<op>` parse coverage incl. dotted-operationId. |
| `crates/labby/src/config.rs` | Load `[[openapi.specs]]` + `OPENAPI_<LABEL>_*`; reject reserved/duplicate labels and **missing base_url**; build the `OpenApiRegistry` + hardened client at startup; inject into the host. |
| `crates/labby-codemode/CLAUDE.md`, `docs/dev/CODE_MODE.md` | Document the `openapi` provider. |
| `deny.toml` | Confirm `rmcp-openapi` + transitive deps pass; adjust allow-list if required. |

---

## Task 1: Confirm `rmcp-openapi` v0.31.2 facts + LOCK the architecture pivot (bead `.2` remainder — HARD GATE)

The security review already verified the three blockers against v0.31.2 source. This task **confirms** them in-repo and records the go/no-go decision. No production code.

**Pre-recorded findings (verified by the engineering review; re-confirm, don't re-discover):**
1. **Redirects: NOT configurable.** `rmcp-openapi`'s `build_reqwest_client()` = `Client::builder().user_agent(..).timeout(..)` — no `redirect::Policy`, no client-injection hook. ⇒ **DO NOT use `rmcp-openapi`'s HTTP executor.**
2. **`servers[]`: never consulted.** `Spec::to_openapi_tools()` takes an explicit `Option<url::Url>` base_url; the library ignores spec `servers[]`. ⇒ **`base_url_override` mandatory; no `extract_servers`.**
3. **Error leakage:** `HttpError.details: Option<Value>` and `ResponseParsingError.raw_response: Option<String>` can carry the upstream body; `#[serde(skip_serializing_if)]` guards only serialization, not `Debug`/`Display`. ⇒ **never format a raw rmcp-openapi error; canary-test it.**

**Files:**
- Create: `docs/superpowers/plans/2026-07-02-openapi-codemode-provider.research.md`

- [ ] **Step 1: Vendor and confirm**

```bash
cargo new /tmp/rmcp-openapi-probe --lib && cd /tmp/rmcp-openapi-probe
cargo add rmcp-openapi@0.31.2 && cargo fetch
SRC=$(find ~/.cargo/registry/src -maxdepth 1 -type d -name 'rmcp-openapi-0.31.2')
rg -n "redirect|ClientBuilder|reqwest::Client|Policy::" "$SRC/src"      # expect: no redirect policy
rg -n "servers|to_openapi_tools|base_url" "$SRC/src"                     # expect: explicit base_url param, no servers[]
rg -n "#\[error|details|raw_response|Authorization" "$SRC/src/error.rs"  # confirm leaky fields
rg -n "pub fn generate|ToolGenerator|operation_id|method|path" "$SRC/src" # confirm the parse-only surface we WILL use
```

- [ ] **Step 2: Confirm the parse-only surface exists**

Identify the exact API that yields, per operation, WITHOUT executing HTTP: `operation_id`, HTTP `method`, `path` template, and (if available) the security scheme. Prefer `ToolGenerator::generate_tool_metadata()` / `generate_openapi_tools()` (the lower-level defs API) over `Server::builder()`. Record the exact function names + `input`/`output` types Tasks 6/8 will call.

- [ ] **Step 3: Confirm the credential/security-scheme surface**

Record how the parsed operation exposes its `securitySchemes` (apiKey header/query/cookie vs http bearer/basic) so Task 8's `labby-openapi`-owned injection maps our `OpenApiCredential` correctly. v1 supports **header-style** injection only (`Authorization: Bearer <token>` for Token; a spec-declared apiKey **header** name for ApiKey); apiKey-in-query and apiKey-in-cookie are explicitly deferred (record which the first target spec needs).

- [ ] **Step 4: Write findings + the LOCKED pivot, and commit**

Write the research note: the three confirmed facts (each with a `$SRC` file:line), the parse-only API names, the security-scheme mapping, and the LOCKED decision: *"rmcp-openapi = spec parsing only; labby-openapi owns the hardened HTTP client."* Then:
```bash
cd /home/jmagar/workspace/lab && git add docs/superpowers/plans/2026-07-02-openapi-codemode-provider.research.md
git commit -m "docs(openapi): confirm rmcp-openapi v0.31.2 facts; lock parse-only + own-HTTP-client pivot"
```

---

## Task 2: Scaffold the `labby-openapi` crate (bead `.1`)

**Files:** Create `crates/labby-openapi/{Cargo.toml,src/lib.rs,CLAUDE.md}`; modify root `Cargo.toml`, `deny.toml`.

**Interfaces:** Produces crate `labby-openapi` compiling empty; workspace builds all-features.

- [ ] **Step 1: Add to workspace + deps**

Root `Cargo.toml`: append `"crates/labby-openapi"` to `members`; add `rmcp-openapi = "0.31.2"` to `[workspace.dependencies]`.

- [ ] **Step 2: Write `crates/labby-openapi/Cargo.toml`**

```toml
[package]
name = "labby-openapi"
description = "OpenAPI-spec-to-Code-Mode-tool derivation. Parses specs via rmcp-openapi; executes outbound HTTP via its OWN hardened reqwest client. Isolates rmcp-openapi/reqwest out of labby-codemode."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
authors.workspace = true
readme = "../../README.md"
publish = false

[dependencies]
labby-runtime = { path = "../labby-runtime" }
labby-primitives = { path = "../labby-primitives" }
rmcp-openapi = { workspace = true }
tokio = { workspace = true, features = ["sync", "time", "rt"] }
reqwest = { workspace = true }
futures = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tracing = { workspace = true }
url = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
wiremock = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: Minimal `src/lib.rs` + stub modules**

```rust
//! OpenAPI-spec-to-Code-Mode-tool derivation. Parses specs via `rmcp-openapi`;
//! executes outbound HTTP via its OWN hardened `reqwest` client (redirects off,
//! peer-IP re-validated). Isolates `rmcp-openapi`/`reqwest` out of
//! `labby-codemode`. MUST NOT depend on `labby-codemode`/`labby-gateway`.
pub mod config;
pub mod error;
```
Create `//! placeholder` stubs for `config.rs`, `error.rs` so it compiles.

- [ ] **Step 4: `CLAUDE.md` + symlinks**

Charter: isolate `rmcp-openapi`; parse-only (never its HTTP executor); own hardened client; all SSRF via `labby_primitives::ssrf`; credentials server-side; no `mod.rs`; never format raw upstream errors. Then:
```bash
cd /home/jmagar/workspace/lab/crates/labby-openapi && ln -sf CLAUDE.md AGENTS.md && ln -sf CLAUDE.md GEMINI.md
```

- [ ] **Step 5: Build + deny-check + commit**

```bash
cd /home/jmagar/workspace/lab && cargo check -p labby-openapi --all-features && cargo deny check
git add Cargo.toml deny.toml crates/labby-openapi/
git commit -m "feat(openapi): scaffold isolated labby-openapi crate (parse-only rmcp-openapi + own HTTP)"
```
(If `cargo deny` flags a new transitive license, add it to `[licenses].allow` and note it in the commit.)

---

## Task 3: Config types + loading + validation (beads `.3` — merged types+loader)

Config types (secret-redacting) live in `labby-openapi`; the ONLY env/file reads live in `crates/labby/src/config.rs`. **`base_url` is mandatory**; reject reserved/duplicate labels and missing base_url at load time.

**Files:**
- Modify: `crates/labby-openapi/src/config.rs`, `src/lib.rs`; create `src/tests_config.rs`.
- Modify: `crates/labby/src/config.rs` (loader + inline `#[cfg(test)]`).

**Interfaces:**
```rust
// labby-openapi
pub const RESERVED_NAMESPACES: [&str; 3] = ["state", "git", "openapi"];
pub enum SpecSource { Url(url::Url), Path(std::path::PathBuf) }
pub enum OpenApiCredential { ApiKey { header: String, value: String }, BearerToken(String) } // Debug-redacted
pub struct OpenApiSpecConfig { pub label: String, pub spec_source: SpecSource,
    pub base_url: url::Url,                    // MANDATORY
    pub allowed_operations: Vec<String>, pub credential: Option<OpenApiCredential> }
pub struct OpenApiProviderConfig { pub specs: Vec<OpenApiSpecConfig> }
// labby binary
pub fn load_openapi_provider_config(toml: &ConfigToml, env: &Env) -> Result<OpenApiProviderConfig, ConfigError>;
```

- [ ] **Step 1: Failing tests (redaction + mandatory base_url + reserved label)**

`src/tests_config.rs`:
```rust
use crate::config::{OpenApiCredential, OpenApiSpecConfig, SpecSource};

#[test]
fn debug_never_prints_credential_value() {
    let cfg = OpenApiSpecConfig {
        label: "vendor".into(),
        spec_source: SpecSource::Url("https://api.example.com/openapi.json".parse().unwrap()),
        base_url: "https://api.example.com".parse().unwrap(),
        allowed_operations: vec!["getUser".into()],
        credential: Some(OpenApiCredential::BearerToken("super-secret-token".into())),
    };
    let dbg = format!("{cfg:?}");
    assert!(!dbg.contains("super-secret-token"), "credential leaked: {dbg}");
    assert!(dbg.contains("vendor"));
}
```
In `crates/labby/src/config.rs` `#[cfg(test)]`:
```rust
#[test]
fn reserved_label_rejected() {
    let toml = r#"[[openapi.specs]]
        label = "git"
        base_url = "https://api.example.com"
        spec_url = "https://api.example.com/openapi.json"
        allowed_operations = ["getUser"]"#;
    let err = load_openapi_provider_config(&parse_config_toml(toml).unwrap(), &Env::empty()).unwrap_err();
    assert!(matches!(err, ConfigError::ReservedLabel { ref label } if label == "git"));
}
#[test]
fn missing_base_url_rejected() {
    let toml = r#"[[openapi.specs]]
        label = "vendor"
        spec_url = "https://api.example.com/openapi.json"
        allowed_operations = ["getUser"]"#;
    let err = load_openapi_provider_config(&parse_config_toml(toml).unwrap(), &Env::empty()).unwrap_err();
    assert!(matches!(err, ConfigError::MissingBaseUrl { .. }));
}
#[test]
fn credential_read_from_env_not_toml() {
    let toml = r#"[[openapi.specs]]
        label = "vendor"
        base_url = "https://api.example.com"
        spec_url = "https://api.example.com/openapi.json"
        allowed_operations = ["getUser"]"#;
    let env = Env::from_pairs([("OPENAPI_VENDOR_TOKEN", "tok-123")]);
    let cfg = load_openapi_provider_config(&parse_config_toml(toml).unwrap(), &env).unwrap();
    assert!(cfg.specs[0].credential.is_some());
}
```

- [ ] **Step 2: Run to verify fail**

`cargo test -p labby-openapi debug_never_prints` and `cargo test -p labby --all-features reserved_label missing_base_url credential_read` → FAIL.

- [ ] **Step 3: Implement types with manual `Debug`**

`crates/labby-openapi/src/config.rs`: define the types above. Manual `Debug` for `SpecSource` (redact via `labby_primitives::ssrf::redact_url`) and `OpenApiCredential` (`ApiKey { header, value: _ }` → `ApiKey { header, value: <redacted> }`; `BearerToken(_)` → `BearerToken(<redacted>)`). `OpenApiSpecConfig`/`OpenApiProviderConfig` derive `Debug` (their fields are now redaction-safe). Add `RESERVED_NAMESPACES`. Wire `pub mod config;` + `#[cfg(test)] mod tests_config;`.

- [ ] **Step 4: Implement the loader**

In `crates/labby/src/config.rs`:
```rust
use labby_openapi::config::{OpenApiCredential, OpenApiProviderConfig, OpenApiSpecConfig, SpecSource, RESERVED_NAMESPACES};

pub fn load_openapi_provider_config(toml: &ConfigToml, env: &Env) -> Result<OpenApiProviderConfig, ConfigError> {
    let mut specs = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw in toml.openapi_specs() {
        let label = raw.label.trim().to_string();
        if RESERVED_NAMESPACES.contains(&label.as_str()) { return Err(ConfigError::ReservedLabel { label }); }
        if !seen.insert(label.clone()) { return Err(ConfigError::DuplicateLabel { label }); }
        let base_url = raw.base_url.as_deref()
            .ok_or_else(|| ConfigError::MissingBaseUrl { label: label.clone() })?
            .parse().map_err(|_| ConfigError::InvalidBaseUrl { label: label.clone() })?;
        let upper = label.to_uppercase();
        let credential = env.get(&format!("OPENAPI_{upper}_TOKEN"))
            .map(|t| OpenApiCredential::BearerToken(t.to_string()))
            .or_else(|| env.get(&format!("OPENAPI_{upper}_API_KEY"))
                .map(|k| OpenApiCredential::ApiKey { header: raw.api_key_header.clone().unwrap_or_else(|| "X-API-Key".into()), value: k.to_string() }));
        let spec_source = match (&raw.spec_url, &raw.spec_path) {
            (Some(u), None) => SpecSource::Url(u.parse().map_err(|_| ConfigError::InvalidSpecUrl { label: label.clone() })?),
            (None, Some(p)) => SpecSource::Path(p.into()),
            _ => return Err(ConfigError::SpecSourceAmbiguous { label: label.clone() }),
        };
        specs.push(OpenApiSpecConfig { label, spec_source, base_url, allowed_operations: raw.allowed_operations.clone(), credential });
    }
    Ok(OpenApiProviderConfig { specs })
}
```
Add `ConfigError` variants: `ReservedLabel`, `DuplicateLabel`, `MissingBaseUrl`, `InvalidBaseUrl`, `InvalidSpecUrl`, `SpecSourceAmbiguous` (each `{ label }`). Add the `[[openapi.specs]]` deserialize struct: `label`, `base_url` (Option, validated non-None), `spec_url`, `spec_path`, `api_key_header` (Option), `allowed_operations` (default empty).

- [ ] **Step 5: Run to verify pass + commit**

```bash
cargo test -p labby-openapi tests_config && cargo test -p labby --all-features reserved_label missing_base_url credential_read
git add crates/labby-openapi/src/ crates/labby/src/config.rs crates/labby/Cargo.toml
git commit -m "feat(openapi): config types (secret-redacted) + loader with mandatory base_url and label validation"
```

---

## Task 4: SSRF base-URL validation (bead `.4` — security core, simplified)

`rmcp-openapi` never reads `servers[]`, so this reduces to validating the mandatory operator-configured `base_url` through the canonical guard at load time. (Request-time peer-IP re-validation lives in the hardened client, Task 8 — that, not a hostname string check, is the DNS-rebinding defense.)

**Files:** Create `crates/labby-openapi/src/ssrf.rs`, `src/tests_ssrf.rs`; modify `src/lib.rs`.

**Interfaces:** `pub fn validate_base_url(cfg: &OpenApiSpecConfig) -> Result<url::Url, OpenApiError>` — https-only, rejects loopback/link-local/RFC1918/CGNAT/private-TLD via `parse_validated_https_url`.

- [ ] **Step 1: Failing tests**

`src/tests_ssrf.rs`:
```rust
use crate::config::{OpenApiCredential, OpenApiSpecConfig, SpecSource};
use crate::ssrf::validate_base_url;

fn spec(base: &str) -> OpenApiSpecConfig {
    OpenApiSpecConfig { label: "vendor".into(),
        spec_source: SpecSource::Url("https://api.example.com/openapi.json".parse().unwrap()),
        base_url: base.parse().unwrap(), allowed_operations: vec![], credential: None }
}
#[test] fn public_https_ok() { assert!(validate_base_url(&spec("https://api.example.com")).is_ok()); }
#[test] fn rfc1918_rejected() { assert!(validate_base_url(&spec("https://192.168.1.10")).is_err()); }
#[test] fn cgnat_rejected() { assert!(validate_base_url(&spec("https://100.64.0.1")).is_err()); }
#[test] fn loopback_rejected() { assert!(validate_base_url(&spec("https://127.0.0.1")).is_err()); }
#[test] fn plain_http_rejected() { assert!(validate_base_url(&spec("http://api.example.com")).is_err()); }
```

- [ ] **Step 2: Run to verify fail** — `cargo test -p labby-openapi ssrf` → FAIL.

- [ ] **Step 3: Implement**

```rust
use crate::config::OpenApiSpecConfig;
use crate::error::OpenApiError;

pub fn validate_base_url(cfg: &OpenApiSpecConfig) -> Result<url::Url, OpenApiError> {
    labby_primitives::ssrf::parse_validated_https_url(cfg.base_url.as_str())
        .map_err(|e| OpenApiError::SsrfRejected { label: cfg.label.clone(), reason: e.kind().to_string() })
}
```
Add `OpenApiError::SsrfRejected { label, reason }` (Task 5 finalizes `error.rs`; add this variant now).

- [ ] **Step 4: Run to verify pass + commit**

```bash
cargo test -p labby-openapi ssrf
git add crates/labby-openapi/src/ssrf.rs crates/labby-openapi/src/tests_ssrf.rs crates/labby-openapi/src/lib.rs crates/labby-openapi/src/error.rs
git commit -m "feat(openapi): load-time base-URL SSRF validation via canonical labby_primitives::ssrf"
```

---

## Task 5: Error type (real `ToolError` mapping) + spec-to-descriptor conversion (bead `.4`)

**Files:** Modify `crates/labby-openapi/src/error.rs`; create `src/convert.rs`; modify `src/lib.rs`.

**Interfaces:**
```rust
pub struct OperationDescriptor { pub operation_id: String, pub method: reqwest::Method,
    pub path_template: String, pub security: Option<SecurityScheme> } // NO js_identifier, NO stored input_schema in v1
pub fn convert_spec(spec_json: &str, allowed: &[String]) -> Result<Vec<OperationDescriptor>, OpenApiError>;
```

- [ ] **Step 1: Failing allowlist test**

`src/convert.rs` inline `#[cfg(test)]`, using a small fixture spec string with operations `getUser` and `deleteUser`:
```rust
#[tokio::test]
async fn allowlist_filters_on_raw_operation_id() {
    let ops = super::convert_spec(FIXTURE_SPEC, &["getUser".to_string()]).unwrap();
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].operation_id, "getUser");
    assert!(!ops.iter().any(|o| o.operation_id == "deleteUser"), "deny-by-default");
}
```

- [ ] **Step 2: Run to verify fail** — `cargo test -p labby-openapi convert` → FAIL.

- [ ] **Step 3: Finalize `error.rs` with the REAL `ToolError` variants**

```rust
use labby_runtime::ToolError;

#[derive(Debug, thiserror::Error)]
pub enum OpenApiError {
    #[error("spec `{label}` base URL rejected by SSRF guard: {reason}")]
    SsrfRejected { label: String, reason: String },
    #[error("failed to parse OpenAPI spec `{label}`")]
    SpecParse { label: String },
    #[error("spec document `{label}` exceeds the size cap")]
    SpecTooLarge { label: String },
    #[error("unknown spec label `{label}`")]
    UnknownInstance { label: String, valid: Vec<String> },
    #[error("unknown operation `{operation_id}` in spec `{label}`")]
    UnknownOperation { label: String, operation_id: String },
    #[error("request for spec `{label}` blocked: resolved to a private address")]
    RequestBlockedPrivateAddr { label: String },
    #[error("upstream request for spec `{label}` failed")]  // NO body/url/auth
    UpstreamRequest { label: String },
    #[error("upstream request for spec `{label}` timed out")]
    UpstreamTimeout { label: String },
}

impl OpenApiError {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SsrfRejected { .. } | Self::SpecParse { .. } | Self::SpecTooLarge { .. } => "config_error",
            Self::RequestBlockedPrivateAddr { .. } => "forbidden",
            Self::UnknownInstance { .. } => "unknown_instance",
            Self::UnknownOperation { .. } => "unknown_action",
            Self::UpstreamRequest { .. } => "internal_error",
            Self::UpstreamTimeout { .. } => "timeout",
        }
    }
}

impl From<OpenApiError> for ToolError {
    fn from(e: OpenApiError) -> Self {
        // Message is our OWN scrubbed Display — never a raw upstream error string.
        let msg = e.to_string();
        match &e {
            OpenApiError::UnknownInstance { valid, .. } =>
                ToolError::UnknownInstance { message: msg, valid: valid.clone() },
            OpenApiError::UnknownOperation { .. } =>
                ToolError::UnknownAction { message: msg, valid: vec![], hint: None },
            OpenApiError::RequestBlockedPrivateAddr { .. } =>
                ToolError::Forbidden { message: msg, required_scopes: vec![] },
            OpenApiError::SsrfRejected { .. } | OpenApiError::SpecParse { .. } | OpenApiError::SpecTooLarge { .. } =>
                ToolError::InvalidParam { message: msg, param: "spec".into() },
            OpenApiError::UpstreamTimeout { .. } =>
                ToolError::Sdk { sdk_kind: "timeout".into(), message: msg },
            OpenApiError::UpstreamRequest { .. } =>
                ToolError::Sdk { sdk_kind: "internal_error".into(), message: msg },
        }
    }
}
```
(Verify each variant's exact field set against `crates/labby-runtime/src/error.rs:19-86` — `UnknownAction`/`UnknownInstance`/`Sdk`/`Forbidden`/`InvalidParam` — and adjust field names if they differ. The load-bearing invariants: the message comes from `OpenApiError::Display`, timeout/internal use `Sdk { sdk_kind }`, and `unknown_instance` maps to `ToolError::UnknownInstance` so the kind survives end-to-end.)

- [ ] **Step 4: Implement `convert_spec` (parse-only)**

```rust
use crate::error::OpenApiError;

pub struct OperationDescriptor {
    pub operation_id: String,
    pub method: reqwest::Method,
    pub path_template: String,
    pub security: Option<SecurityScheme>,
}

/// Parse a spec into allowlisted operation descriptors using rmcp-openapi's
/// PARSE-ONLY surface (Task 1). Does NOT execute HTTP and does NOT use
/// rmcp-openapi's Tool::call()/execute(). Deny-by-default on the RAW operationId.
pub fn convert_spec(spec_json: &str, allowed: &[String]) -> Result<Vec<OperationDescriptor>, OpenApiError> {
    let tools = rmcp_openapi::ToolGenerator::generate_openapi_tools(spec_json)  // exact fn per Task 1
        .map_err(|_| OpenApiError::SpecParse { label: String::new() })?;
    let mut out = Vec::new();
    for t in tools {
        let raw = t.operation_id().to_string();
        if !allowed.iter().any(|a| a == &raw) { continue; }
        out.push(OperationDescriptor {
            operation_id: raw,
            method: parse_method(t.method()),
            path_template: t.path().to_string(),
            security: extract_security(&t),
        });
    }
    Ok(out)
}
```
(Adjust `generate_openapi_tools`/`operation_id`/`method`/`path` to the real API names from Task 1. `SecurityScheme` is a small local enum: `Bearer` | `ApiKeyHeader(String)`.)

- [ ] **Step 5: Run to verify pass + commit**

```bash
cargo test -p labby-openapi convert
git add crates/labby-openapi/src/error.rs crates/labby-openapi/src/convert.rs crates/labby-openapi/src/lib.rs
git commit -m "feat(openapi): real ToolError mapping + parse-only spec-to-descriptor conversion with raw-id allowlist"
```

---

## Task 6: Registry — concurrent, timeout-bounded, body-capped, degraded-boot load (bead `.4`, perf)

Load specs concurrently with a per-spec timeout, a shared `reqwest::Client` for spec fetch, and a spec-document size cap **before parse**. WARN on any truncation or omission. No per-spec semaphore (deferred — the 30s wall-clock + per-call timeout + call budget bound concurrency).

**Files:** Create `crates/labby-openapi/src/registry.rs`; modify `src/lib.rs`.

**Interfaces:**
```rust
pub struct OperationHandle { pub operation_id: String, pub method: reqwest::Method,
    pub path_template: String, pub security: Option<SecurityScheme>,
    pub base_url: url::Url, pub credential: Option<OpenApiCredential> }
pub struct SpecEntry { pub operations: std::collections::HashMap<String, OperationHandle> } // label is the outer key; not stored here
#[derive(Clone, Default)] pub struct OpenApiRegistry { inner: std::sync::Arc<std::collections::HashMap<String, SpecEntry>> }
impl OpenApiRegistry {
    pub async fn load(cfg: OpenApiProviderConfig, spec_fetch_client: reqwest::Client, per_spec_timeout: std::time::Duration) -> Self;
    pub fn labels(&self) -> Vec<String>;
    pub fn operation(&self, label: &str, op: &str) -> Result<&OperationHandle, OpenApiError>;
    pub fn is_empty(&self) -> bool;
}
```

- [ ] **Step 1: Failing degraded-boot test**

`src/registry.rs` inline `#[cfg(test)]`, good spec from a local fixture-file `SpecSource::Path`, bad spec pointing at an unroutable base_url:
```rust
#[tokio::test]
async fn one_bad_spec_omitted_without_blocking_good_one() {
    let cfg = OpenApiProviderConfig { specs: vec![
        good_fixture_spec("goodlabel"),
        bad_spec("badlabel", "https://10.255.255.1"),  // SSRF-rejected at validate_base_url
    ]};
    let client = crate::http::build_spec_fetch_client();
    let started = std::time::Instant::now();
    let reg = OpenApiRegistry::load(cfg, client, std::time::Duration::from_secs(2)).await;
    assert!(reg.labels().contains(&"goodlabel".to_string()));
    assert!(!reg.labels().contains(&"badlabel".to_string()));
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "concurrent + bounded");
}
```

- [ ] **Step 2: Run to verify fail** — FAIL.

- [ ] **Step 3: Implement**

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const MAX_SPECS: usize = 10;
const MAX_OPERATIONS_PER_SPEC: usize = 200;
const MAX_SPEC_BYTES: usize = 4 * 1024 * 1024; // reject spec documents larger than 4 MiB before parse

impl OpenApiRegistry {
    pub async fn load(cfg: OpenApiProviderConfig, client: reqwest::Client, per_spec_timeout: Duration) -> Self {
        let total = cfg.specs.len();
        let specs: Vec<_> = cfg.specs.into_iter().take(MAX_SPECS).collect();
        if total > MAX_SPECS {
            tracing::warn!(service = "openapi", kept = MAX_SPECS, configured = total,
                "openapi: MAX_SPECS exceeded — extra specs dropped");
        }
        let loads = specs.into_iter().map(|spec| {
            let client = client.clone();
            async move {
                let label = spec.label.clone();
                match tokio::time::timeout(per_spec_timeout, load_one_spec(spec, &client)).await {
                    Ok(Ok(entry)) => Some((label, entry)),
                    Ok(Err(e)) => { tracing::warn!(service="openapi", label=%label, kind=e.kind(), "openapi spec omitted: load failed"); None }
                    Err(_)      => { tracing::warn!(service="openapi", label=%label, kind="timeout", "openapi spec omitted: load timed out"); None }
                }
            }
        });
        let map: HashMap<_, _> = futures::future::join_all(loads).await.into_iter().flatten().collect();
        Self { inner: Arc::new(map) }
    }
    pub fn labels(&self) -> Vec<String> { let mut v: Vec<_> = self.inner.keys().cloned().collect(); v.sort(); v }
    pub fn is_empty(&self) -> bool { self.inner.is_empty() }
    pub fn operation(&self, label: &str, op: &str) -> Result<&OperationHandle, OpenApiError> {
        let entry = self.inner.get(label)
            .ok_or_else(|| OpenApiError::UnknownInstance { label: label.into(), valid: self.labels() })?;
        entry.operations.get(op)
            .ok_or_else(|| OpenApiError::UnknownOperation { label: label.into(), operation_id: op.into() })
    }
}

async fn load_one_spec(spec: OpenApiSpecConfig, client: &reqwest::Client) -> Result<SpecEntry, OpenApiError> {
    let base_url = crate::ssrf::validate_base_url(&spec)?;                 // mandatory + SSRF-checked
    let spec_json = fetch_spec_json(&spec.spec_source, client, &spec.label).await?; // body-size-capped
    let descriptors = crate::convert::convert_spec(&spec_json, &spec.allowed_operations)?;
    let mut operations = HashMap::new();
    let converted = descriptors.len();
    for d in descriptors.into_iter().take(MAX_OPERATIONS_PER_SPEC) {
        operations.insert(d.operation_id.clone(), OperationHandle {
            operation_id: d.operation_id, method: d.method, path_template: d.path_template,
            security: d.security, base_url: base_url.clone(), credential: spec.credential.clone(),
        });
    }
    if converted > MAX_OPERATIONS_PER_SPEC {
        tracing::warn!(service="openapi", label=%spec.label, kept=MAX_OPERATIONS_PER_SPEC, converted,
            "openapi: MAX_OPERATIONS_PER_SPEC exceeded — extra operations dropped");
    }
    Ok(SpecEntry { operations })
}
```
Implement `fetch_spec_json`: for `SpecSource::Url`, use the shared `client` (Task 8's `build_spec_fetch_client`), stream the body but **abort past `MAX_SPEC_BYTES`** → `OpenApiError::SpecTooLarge`; for `SpecSource::Path`, read the file with the same cap.

- [ ] **Step 4: Run to verify pass + commit**

```bash
cargo test -p labby-openapi registry
git add crates/labby-openapi/src/registry.rs crates/labby-openapi/src/lib.rs
git commit -m "feat(openapi): registry with concurrent timeout-bounded degraded-boot load + spec body-size cap + truncation warnings"
```

---

## Task 7: Hardened HTTP client + dispatch with server-side credential injection (bead `.4`, security C2–C5)

The security-critical core: `labby-openapi` performs the outbound call itself with `redirect::none()` and a peer-IP-revalidating connector, injects the credential server-side, bounds it with a per-call timeout, and maps errors to scrubbed `OpenApiError` — with a committed **canary leak test**.

**Files:** Create `crates/labby-openapi/src/http.rs`, `src/dispatch.rs`, `src/tests_dispatch.rs`; modify `src/lib.rs`.

**Interfaces:**
```rust
pub fn build_spec_fetch_client() -> reqwest::Client;          // redirect::none, https_only, connect+read timeouts
pub fn build_dispatch_client() -> reqwest::Client;            // same + peer-IP-revalidating connector
pub async fn execute_operation(client: &reqwest::Client, op: &OperationHandle, params: serde_json::Value) -> Result<serde_json::Value, OpenApiError>;
pub async fn dispatch_openapi_call(registry: &OpenApiRegistry, client: &reqwest::Client, label: &str, operation_id: &str, params: serde_json::Value) -> Result<serde_json::Value, OpenApiError>;
```

- [ ] **Step 1: Failing dispatch + canary tests**

`src/tests_dispatch.rs` with `wiremock` (helper builds a registry whose `OperationHandle.base_url` is the mock URI — the SSRF guard is unit-tested in Task 4, so this helper constructs the `SpecEntry` directly to test dispatch logic in isolation; document that clearly):
```rust
#[tokio::test]
async fn happy_path_calls_allowed_operation() { /* mount GET /users/{id} -> 200 {"id":"7"}; assert out["id"]=="7" */ }

#[tokio::test]
async fn unknown_operation_returns_unknown_action() {
    let reg = registry_from_fixture(&mock.uri(), &["getUser"]).await; // deleteUser filtered out at load
    let err = dispatch_openapi_call(&reg, &client, "vendor", "deleteUser", json!({"id":"7"})).await.unwrap_err();
    assert_eq!(err.kind(), "unknown_action");
}
#[tokio::test]
async fn unknown_label_returns_unknown_instance() {
    let err = dispatch_openapi_call(&OpenApiRegistry::default(), &client, "nope", "getUser", json!({})).await.unwrap_err();
    assert_eq!(err.kind(), "unknown_instance");
}
#[tokio::test]
async fn upstream_error_body_never_leaks_into_error() {
    // Mount a 500 whose body contains a canary. Assert the canary appears in
    // NEITHER Display, Debug, NOR the mapped ToolError's serialized form.
    let reg = registry_from_fixture(&mock_500_with_body("CANARY-9f3b-SECRET").await, &["getUser"]).await;
    let err = dispatch_openapi_call(&reg, &client, "vendor", "getUser", json!({"id":"7"})).await.unwrap_err();
    let tool_err: labby_runtime::ToolError = err.clone().into();
    for s in [format!("{err}"), format!("{err:?}"), format!("{tool_err:?}"), serde_json::to_string(&tool_err).unwrap()] {
        assert!(!s.contains("CANARY-9f3b-SECRET"), "response body leaked: {s}");
    }
}
```

- [ ] **Step 2: Run to verify fail** — FAIL.

- [ ] **Step 3: Implement the hardened client + dispatch**

`src/http.rs`:
```rust
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
const PER_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

pub fn build_spec_fetch_client() -> reqwest::Client { base_builder().build().expect("spec fetch client") }
pub fn build_dispatch_client() -> reqwest::Client {
    // Same hardening PLUS a resolver/connector that re-checks the connecting
    // peer IP against check_ip_not_private on every connect (DNS-rebinding
    // defense — the ONLY real one; a hostname string check is not sufficient).
    base_builder().dns_resolver(std::sync::Arc::new(SsrfResolver)).build().expect("dispatch client")
}
fn base_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())   // redirects OFF — closes the 302→metadata bypass
        .https_only(true)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(PER_CALL_TIMEOUT)
}

/// Custom resolver: resolve, then reject any address in a private/loopback/CGNAT
/// range before the socket connects. Mirrors the acp_registry installer pattern.
struct SsrfResolver;
impl reqwest::dns::Resolve for SsrfResolver { /* resolve via hickory/std, filter each IP through
    labby_primitives::ssrf::check_ip_not_private; drop private addrs; error if none remain */ }

pub async fn execute_operation(client: &reqwest::Client, op: &OperationHandle, params: serde_json::Value)
    -> Result<serde_json::Value, OpenApiError>
{
    let url = build_url(&op.base_url, &op.path_template, &params)?; // PATH_SEGMENT-encoded substitution
    let mut req = client.request(op.method.clone(), url);
    req = inject_credential(req, op);                                 // server-side; JS never sees the key
    req = apply_query_and_body(req, op, &params)?;
    let resp = req.send().await.map_err(|e| map_reqwest_err(e, &op /* label */))?; // NEVER format e's Display into the message
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.map_err(|_| OpenApiError::UpstreamRequest { label: String::new() })?;
    if !status.is_success() {
        // Do NOT include `body` in the error — it may carry the upstream response body.
        return Err(OpenApiError::UpstreamRequest { label: String::new() });
    }
    Ok(body)
}
```
`src/dispatch.rs`:
```rust
pub async fn dispatch_openapi_call(registry: &OpenApiRegistry, client: &reqwest::Client,
    label: &str, operation_id: &str, params: serde_json::Value) -> Result<serde_json::Value, OpenApiError>
{
    let handle = registry.operation(label, operation_id)?; // unknown label/op → structured error
    let out = crate::http::execute_operation(client, handle, params).await?;
    tracing::info!(service="openapi", action=operation_id, label=%label,
        host=%handle.base_url.host_str().unwrap_or_default(), status="ok",
        "openapi dispatch complete"); // NO body, NO url-with-query, NO auth
    Ok(out)
}
```
`map_reqwest_err` classifies timeouts → `UpstreamTimeout`, connect/private-addr rejections → `RequestBlockedPrivateAddr`, everything else → `UpstreamRequest` — **never embedding the reqwest error's `Display`**. `inject_credential` maps `OpenApiCredential::BearerToken` → `Authorization: Bearer`, `ApiKey { header, value }` → that header. The per-call timeout is the `reqwest::Client` `.timeout()` (bounds the whole call, safely inside Code Mode's 30s wall clock).

- [ ] **Step 4: Run to verify pass + commit**

```bash
cargo test -p labby-openapi dispatch
git add crates/labby-openapi/src/http.rs crates/labby-openapi/src/dispatch.rs crates/labby-openapi/src/tests_dispatch.rs crates/labby-openapi/src/lib.rs
git commit -m "feat(openapi): hardened own HTTP client (redirect-off, peer-IP recheck) + dispatch with credential injection + canary leak test"
```

---

## Task 8: `LocalProviderName::Openapi` + ID parsing incl. dotted-operationId (bead `.5`, finding #11)

**Files:** Modify `crates/labby-codemode/src/local_provider.rs`, `src/tests_ids_schema.rs`.

**Interfaces:** `LocalProviderName::Openapi` (`as_str() == "openapi"`); `is_reserved_provider_namespace("openapi") == true`; `try_parse_local_provider_call("openapi::vendor.getUser")` → `LocalProviderCall { provider: Openapi, method: "vendor.getUser", params: Null }`. The `method` carries `"<label>.<operationId>"`; the split happens in Task 9's dispatch, so a dotted operationId is preserved intact.

- [ ] **Step 1: Failing parse tests**

`tests_ids_schema.rs`:
```rust
#[test] fn parses_openapi_provider_call() {
    let c = try_parse_local_provider_call("openapi::vendor.getUser").unwrap().unwrap();
    assert_eq!(c.provider, LocalProviderName::Openapi); assert_eq!(c.method, "vendor.getUser");
}
#[test] fn openapi_preserves_dotted_operation_id() {
    let c = try_parse_local_provider_call("openapi::vendor.pets.list").unwrap().unwrap();
    assert_eq!(c.provider, LocalProviderName::Openapi); assert_eq!(c.method, "vendor.pets.list");
}
#[test] fn openapi_is_reserved() { assert!(is_reserved_provider_namespace("openapi")); }
```

- [ ] **Step 2: Run to verify fail** — `cargo test -p labby-codemode --all-features openapi` → FAIL.

- [ ] **Step 3: Add the variant + extend the three functions**

Add `Openapi` to `LocalProviderName`; extend `as_str()` (`Self::Openapi => "openapi"`), `is_reserved_provider_namespace` (`"state" | "git" | "openapi"`), and the `try_parse_local_provider_call` match (`"openapi" => LocalProviderName::Openapi`). `split_namespaced_id` splits only on the first `::` and rejects a third segment, so `openapi::vendor.pets.list` → `namespace="openapi"`, `method="vendor.pets.list"` unchanged. No other parsing change.

- [ ] **Step 4: Run to verify pass + commit**

```bash
cargo test -p labby-codemode --all-features openapi
git add crates/labby-codemode/src/local_provider.rs crates/labby-codemode/src/tests_ids_schema.rs
git commit -m "feat(openapi): LocalProviderName::Openapi + dotted-operationId-safe ID parsing"
```

---

## Task 9: Wire dispatch in runner_drive — SEPARATE lock, pre-lock branch, required host accessor (bead `.5`, concurrency fix; findings I4/I5/I6)

**Files:** Modify `crates/labby-codemode/Cargo.toml`, `src/runner_drive.rs`, `src/host.rs`, `src/execute.rs`.

**Interfaces:**
- `RunnerConfig` gains `pub openapi_registry: labby_openapi::OpenApiRegistry` and `pub openapi_http_client: reqwest::Client` (both cheap `Arc`-backed clones).
- `CodeModeHost` gains a **REQUIRED** (no default) `fn openapi_registry(&self) -> labby_openapi::OpenApiRegistry;` and `fn openapi_http_client(&self) -> reqwest::Client;` (or one accessor returning both). Every host — prod and tests — implements them (tests return `Default::default()` / `build_dispatch_client()` explicitly).
- `enqueue_local_provider_call` branches on `local.provider`: `State`/`Git` keep the `LOCAL_PROVIDER_LOCK` path via `dispatch_local_provider_stub`; `Openapi` is dispatched by a NEW `dispatch_openapi_provider(...)` **before** any lock and **not** through the stub. The `local_providers_allowed()` gate wraps both branches.

- [ ] **Step 1: Failing "openapi does not block on LOCAL_PROVIDER_LOCK" test**

In `runner_drive.rs` `#[cfg(test)]`:
```rust
#[tokio::test]
async fn openapi_dispatch_does_not_block_on_local_provider_lock() {
    let lock = LOCAL_PROVIDER_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
    let _held = lock.lock().await; // a slow state/git op holds the lock
    let reg = labby_openapi::OpenApiRegistry::default();
    let client = labby_openapi::http::build_dispatch_client();
    let call = crate::local_provider::LocalProviderCall {
        provider: crate::local_provider::LocalProviderName::Openapi,
        method: "vendor.getUser".into(), params: serde_json::Value::Null,
    };
    let res = tokio::time::timeout(std::time::Duration::from_secs(2),
        dispatch_openapi_provider(&reg, &client, call, serde_json::json!({}))).await;
    assert!(res.is_ok(), "openapi must not block on LOCAL_PROVIDER_LOCK");
}
```

- [ ] **Step 2: Run to verify fail** — FAIL.

- [ ] **Step 3: Add dep, config fields, required accessor, pre-lock branch**

Add `labby-openapi = { path = "../labby-openapi" }` to `crates/labby-codemode/Cargo.toml`.

In `host.rs`, add the two REQUIRED accessors to `CodeModeHost` (no default impl — matching the trait's all-required-methods discipline; a missed override becomes a compile error, not a silent no-op). Update every existing impl (the gateway host in Task 10; test hosts here) to implement them.

In `runner_drive.rs`:
- Add `openapi_registry` + `openapi_http_client` to `RunnerConfig` and both `test_config` builders.
- In `enqueue_local_provider_call`, branch:
```rust
let result = if !super::execute::local_providers_allowed(&caller, &capability_filter) {
    Err(ToolError::Forbidden { message: "local Code Mode providers require unscoped lab:admin".into(),
        required_scopes: vec!["lab:admin".into()] })
} else if matches!(local.provider, LocalProviderName::Openapi) {
    // NO LOCAL_PROVIDER_LOCK — openapi has no shared mutable local state.
    dispatch_openapi_provider(&openapi_registry, &openapi_http_client, local, params).await
} else {
    let _guard = LOCAL_PROVIDER_LOCK.get_or_init(|| Mutex::new(())).lock().await;
    dispatch_local_provider_stub(local, params).await
};
```
  (Clone `cfg.openapi_registry` / `cfg.openapi_http_client` into the async block alongside `caller`/`capability_filter`.)
- Add:
```rust
async fn dispatch_openapi_provider(
    registry: &labby_openapi::OpenApiRegistry, client: &reqwest::Client,
    local: LocalProviderCall, params: Value,
) -> Result<Value, ToolError> {
    let (label, op) = local.method.split_once('.').ok_or_else(|| ToolError::InvalidParam {
        message: "openapi call must be openapi::<label>.<operationId>".into(), param: "id".into() })?;
    labby_openapi::dispatch::dispatch_openapi_call(registry, client, label, op, params).await.map_err(Into::into)
}
```
- **Do NOT add an `Openapi` arm to `dispatch_local_provider_stub`** — that function runs inside the lock; the `Openapi` path must never reach it.

In `execute.rs`, when assembling `RunnerConfig` (the `run_in_runner`/`execute_sandboxed` site that already has `self.host`), populate `openapi_registry: host.openapi_registry()` and `openapi_http_client: host.openapi_http_client()`. Do not thread these down the positional `run_in_runner` arg list from callers — read them from `self.host` at the config-build site.

- [ ] **Step 4: Run to verify pass + commit**

```bash
cargo test -p labby-codemode --all-features openapi_dispatch_does_not_block
git add crates/labby-codemode/Cargo.toml crates/labby-codemode/src/runner_drive.rs crates/labby-codemode/src/host.rs crates/labby-codemode/src/execute.rs
git commit -m "feat(openapi): wire dispatch into runner_drive via pre-lock branch (no LOCAL_PROVIDER_LOCK) + required host accessors"
```

---

## Task 10: JS preamble shim `globalThis.openapi.call` (bead `.6`; findings perf/M11)

**Files:** Modify `crates/labby-codemode/src/preamble.rs`, `src/execute.rs`.

**Interfaces:** `pub(crate) fn generate_openapi_provider_js() -> &'static str` — returns a `const` shim (no per-call `String` allocation). Emitted **only on the host path** (`execute.rs:203`), never the host-less path (which has no registry). `operationId` is passed as a JS string VALUE, so no sanitization is needed.

- [ ] **Step 1: Failing "emitted JS is valid" test**

`preamble.rs` `#[cfg(test)]` (reuse the crate's `boa_parser`):
```rust
#[test] fn openapi_shim_is_valid_js() {
    let js = generate_openapi_provider_js();
    assert!(js.contains("globalThis.openapi") && js.contains("call"));
    let mut interner = boa_interner::Interner::default();
    let parsed = boa_parser::Parser::new(boa_parser::Source::from_bytes(js.as_bytes())).parse_script(&mut interner);
    assert!(parsed.is_ok(), "shim must be valid JS: {parsed:?}");
}
```

- [ ] **Step 2: Run to verify fail** — FAIL.

- [ ] **Step 3: Implement the const shim + host-path-only emission**

```rust
pub(crate) fn generate_openapi_provider_js() -> &'static str {
    // Flat, non-discoverable (Decision 3). operationId passed as a VALUE, so no
    // JS-identifier sanitization is needed. Routes through the callTool bridge;
    // the parent intercepts `openapi::<label>.<operationId>` (local_provider.rs).
    r#"
globalThis.openapi = {
  call: function (label, operationId, params) {
    if (typeof label !== "string" || typeof operationId !== "string") {
      throw new Error(JSON.stringify({ kind: "missing_param", message: "openapi.call(label, operationId, params) requires string label and operationId" }));
    }
    return callTool("openapi::" + label + "." + operationId, params == null ? {} : params);
  }
};
"#
}
```
In `execute.rs`, append `generate_openapi_provider_js()` **only** at the host-path proxy build (execute.rs:~203) when `local_providers_allowed(caller, scope)` AND `host.openapi_registry()` is non-empty (`!is_empty()`). Do NOT emit on the host-less path (execute.rs:~158) — there is no registry there, so the shim would only ever error.

- [ ] **Step 4: Run to verify pass + commit**

```bash
cargo test -p labby-codemode --all-features openapi_shim_is_valid_js
git add crates/labby-codemode/src/preamble.rs crates/labby-codemode/src/execute.rs
git commit -m "feat(openapi): const globalThis.openapi.call shim emitted on the host path only"
```

---

## Task 11: Build the registry + client at startup and inject into the host (binary wiring; finding I5)

**Files:** Modify `crates/labby/src/config.rs` (or the Code Mode host construction site) and the gateway `CodeModeHost` impl.

**Interfaces:** At `labby serve` startup, build the hardened dispatch client + the `OpenApiRegistry` (concurrently, 8s per-spec timeout), store both on the gateway host, and implement the REQUIRED `CodeModeHost::openapi_registry()` / `openapi_http_client()` accessors to return the stored clones.

- [ ] **Step 1: Wire startup construction**

```rust
let openapi_cfg = load_openapi_provider_config(&config_toml, &env)?;   // config errors DO fail boot (bad TOML)
let openapi_http_client = labby_openapi::http::build_dispatch_client();
let openapi_registry = labby_openapi::OpenApiRegistry::load(
    openapi_cfg, labby_openapi::http::build_spec_fetch_client(), std::time::Duration::from_secs(8)).await;
// spec-LOAD failures never fail boot — load() already degrades + WARNs per spec.
if !openapi_registry.is_empty() {
    tracing::info!(service="openapi", specs=?openapi_registry.labels(), "openapi code-mode provider ready");
} else {
    tracing::info!(service="openapi", "openapi code-mode provider: no specs configured/loaded");
}
// store openapi_registry + openapi_http_client on the gateway host
```
Implement the two `CodeModeHost` accessors on the gateway host to return the stored clones. Because the accessors are REQUIRED (Task 9), a missing implementation is a compile error — the feature can't silently disable.

- [ ] **Step 2: Verify boot doesn't hang on a bad spec**

`cargo check -p labby --all-features` — confirm the `load` result is `await`ed but never `?`-propagated into a boot error (only `load_openapi_provider_config`'s config-parse errors fail boot). Reuse Task 6's degraded-boot guarantee.

- [ ] **Step 3: Commit**

```bash
git add crates/labby/src/
git commit -m "feat(openapi): build registry + hardened client at startup and inject via required host accessors"
```

---

## Task 12: Docs, observability audit, end-to-end validation (bead `.7`)

**Files:** Modify `crates/labby-codemode/CLAUDE.md`, `docs/dev/CODE_MODE.md`.

- [ ] **Step 1: Update `crates/labby-codemode/CLAUDE.md`**

Name `openapi` as the third local provider in the "Exception" paragraph; note it does outbound HTTP through the isolated `labby-openapi` crate's OWN hardened client (redirects off, peer-IP re-validated), does NOT share `LOCAL_PROVIDER_LOCK`, and is wired via required `CodeModeHost` accessors — the first provider requiring cross-crate dispatch wiring (name this cost explicitly). No new files were added to `labby-codemode`.

- [ ] **Step 2: Document the surface in `docs/dev/CODE_MODE.md`**

`openapi` section: config (`[[openapi.specs]]` with **mandatory** `base_url` + `OPENAPI_<LABEL>_*`), the `openapi.call(label, operationId, params)` JS API, the admin+unscoped+allowlist gate, SSRF containment (redirect-off + peer-IP recheck), load-once refresh, a worked example, and the note that discovery (`codemode.search`) does NOT list openapi ops in v1. File the explicitly-deferred follow-ups: discovery-catalog integration (which would re-introduce `input_schema` + per-op JS proxies + operationId sanitization), background `ArcSwap` refresh, per-spec rate/concurrency caps, and apiKey-in-query/cookie injection.

- [ ] **Step 3: Full workspace verification**

```bash
cd /home/jmagar/workspace/lab
cargo fmt --all
cargo clippy --all-features -- -D warnings
RUSTFLAGS="-D warnings" cargo check --all-targets --all-features
cargo nextest run --all-features
cargo deny check
```
All must pass.

- [ ] **Step 4: Manual happy-path + secret-leak audit (HARD GATE)**

Configure a fixture/real spec with a credential in `.env`; under `LAB_LOG=debug`, run a snippet calling `openapi.call("vendor","getUser",{id:"7"})`:
- Confirm the dispatch log has `service=openapi action=getUser status=ok elapsed_ms=...` and NO token / api-key / response-body / query-string-auth anywhere.
- `grep` the captured logs for the credential value AND for a response-body canary — expect zero matches. This is the hard gate (in addition to Task 7's committed canary test, which is the durable regression guard). Do not close the epic if any secret appears.

- [ ] **Step 5: Manual failing-path runs**

(a) Unreachable/invalid spec URL → spec omitted at boot with a WARN, `labby serve` still ready. (b) Unknown operation → `unknown_action`; unknown label → `unknown_instance`; a spec whose base_url would redirect → the request fails closed (redirect disabled), NOT a panic/hang. Confirm each is WARN with a `kind` field.

- [ ] **Step 6: Commit**

```bash
git add crates/labby-codemode/CLAUDE.md docs/dev/CODE_MODE.md
git commit -m "docs(openapi): document openapi Code Mode provider + record e2e/secret-leak validation"
```

---

## Self-Review

**Spec coverage (7 beads + locked decisions + BOTH eng-review rounds):**
- `.1` crate placement → Task 2. `.2` research → Task 1 (findings pre-confirmed). `.3` config+validation → Task 3 (merged). `.4` conversion/registry/SSRF/HTTP → Tasks 4,5,6,7. `.5` interception+dispatch+separate-lock+dotted-op → Tasks 8,9. `.6` shim → Task 10. `.7` docs+e2e → Tasks 11,12. ✅

**Round-2 eng-review findings applied:**
- **C1** wrong `ToolError` variants → Task 5 uses real `Sdk{sdk_kind}` / `UnknownInstance` / `Forbidden` / `InvalidParam`, with a Global Constraint spelling out the real enum. ✅
- **C2/C3** redirect-follow SSRF bypass → architecture pivot: `rmcp-openapi` parse-only; `labby-openapi` owns a `redirect::none()` client (Tasks 1, 5, 7). ✅
- **C4** DNS-rebinding theater → Task 7 peer-IP-revalidating connector (`SsrfResolver`), not a hostname string check; `revalidate_resolved_host` removed. ✅
- **C5** error-body leak → Task 5 scrubbed `Display` + `From`; Task 7 committed canary test; Global Constraint bans formatting raw upstream errors. ✅
- **servers[] dead code / base_url ambiguity** → `base_url` mandatory (Tasks 1,3); `extract_servers`/`resolve_and_pin_base_url` removed; Task 4 is a simple validate. ✅
- **I5** defaulted trait method → Task 9 REQUIRED accessors (compile error if missed). ✅
- **I6** stub-match option → Task 9 pre-lock branch only; explicit "do NOT add an Openapi arm to the stub." ✅
- **I4** positional threading → Task 9 reads from `self.host` at the config-build site. ✅
- **M10** unknown_instance kind loss → Task 5 maps to `ToolError::UnknownInstance`. ✅
- **M8** silent truncation → Task 6 WARNs on `MAX_SPECS`/`MAX_OPERATIONS` truncation. ✅
- **M11** shim on host-less path → Task 10 host-path-only emission. ✅
- **Perf:** shared spec-fetch client + connect timeout (Task 6/7); spec body-size cap before parse (Task 6); const shim string (Task 10); `labels()` off the happy path (Task 6). ✅
- **Simplicity:** dropped `js_identifier`/`sanitize_operation_id` (dead in v1), dropped stored `input_schema`, dropped the per-spec `Arc<Semaphore>` (also resolves the "permit wait outside the per-call timeout" issue since the client `.timeout()` now bounds the whole call), removed `PinnedBaseUrl` newtype, removed `SpecEntry.label` denormalization, merged config types+loader into Task 3. ✅

**Round-1 eng-review findings (still applied):** SSRF reuse (Task 4), boot-time concurrency + degraded boot (Task 6), per-call timeout (Task 7), dotted-operationId test (Task 8), label collision (Task 3), allowlist/dispatch same raw key (Task 5), first-boot fallback (Tasks 6/11). ✅

**Placeholder scan:** Each step carries real code. Remaining external-library unknowns (exact `rmcp-openapi` parse-fn names, security-scheme accessor) are resolved in Task 1 before dependent tasks and each dependent step names the fallback. ✅

**Type consistency:** `LocalProviderCall{provider,method,params}`, `OpenApiRegistry`, `OperationHandle`, `dispatch_openapi_call(registry, client, label, op, params)`, `execute_operation`, `OpenApiError::kind()`, real `ToolError` variants, `method="<label>.<operationId>"` (split on first `.` in Task 9, preserved in Task 8) — consistent across Tasks 3–11. ✅

---

## Execution Handoff

**Plan complete and saved to `docs/superpowers/plans/2026-07-02-openapi-codemode-provider.md`. Two execution options:**

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints.
