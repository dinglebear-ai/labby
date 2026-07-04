# rmcp-openapi v0.31.2 Research — Task 1 (HARD GATE)

**Status: GO.** All three security blockers re-confirmed in-repo against the vendored
crate source. The architecture pivot is **LOCKED**: *rmcp-openapi is used for spec
parsing / tool-descriptor generation ONLY; `labby-openapi` owns the hardened outbound
HTTP client.*

Vendored source root (`$SRC`):
`~/.cargo/registry/src/index.crates.io-*/rmcp-openapi-0.31.2/`

Probe crate: `/tmp/rmcp-openapi-probe` (`cargo add rmcp-openapi@0.31.2`, 304 deps locked,
`rmcp-macros v1.8.0` / `rmcp` in the `1.x` line — compatible with the workspace `rmcp = "1.7"`).

---

## Confirmed security findings

### Finding 1 — Redirects are NOT configurable ⇒ do NOT use the executor
`$SRC/src/http_client.rs:135-148` `HttpClient::build_reqwest_client`:
```rust
let mut builder = Client::builder()
    .user_agent(&user_agent)
    .timeout(Duration::from_secs(timeout_seconds));
if insecure { builder = builder.danger_accept_invalid_certs(true)
                                .danger_accept_invalid_hostnames(true); }
builder.build().expect("Failed to create HTTP client")
```
No `redirect::Policy`, no client-injection hook. `Tool::call()` (`$SRC/src/tool/mod.rs:42`)
and `Tool::execute()` (`:180`) both route through `client.execute_tool_call(&metadata, args)`
(`:89`, `:203`) — reqwest's default redirect policy (follow up to 10). **Unsafe for an
SSRF-sensitive surface. We never call `Tool::call`/`execute`/`generate_openapi_tools`.**

### Finding 2 — `servers[]` is never consulted ⇒ base_url mandatory
`$SRC/src/spec.rs:145` `Spec::to_openapi_tools(&self, filters, base_url: Option<url::Url>, ...)`
and `$SRC/src/http_client.rs:211` `with_base_url` — the base URL is an explicit caller-supplied
parameter. Nothing reads the spec document's `servers[]`. **`base_url_override` in our config
is mandatory; there is NO `servers[]` parsing/selection code.**

### Finding 3 — Error types can carry the upstream body ⇒ never format them
`$SRC/src/error.rs`:
- `ToolCallExecutionError::HttpError { status, message, details: Option<Value> }` (`:590,597-599`)
- `ToolCallExecutionError::ResponseParsingError { reason, raw_response: Option<String> }` (`:613,620`)

`#[serde(skip_serializing_if)]` guards only *serialization*, not `Debug`/`Display`. **We never
`{}`/`{:?}`/`.to_string()` a raw `rmcp_openapi::*` error into a `tracing`/`ToolError` message —
always map to a fixed, scrubbed `OpenApiError` variant first. Task 7 has a committed canary test.**

Because we do NOT use the executor at all, these error types never even reach our code on the
happy/error paths — our own `reqwest` errors are what we map. The canary test still guards
against accidental body inclusion.

---

## Parse-only API we WILL use (Tasks 5/6)

The clean parse-only path constructs **no** `HttpClient`:

```rust
let spec = rmcp_openapi::Spec::from_value(serde_json::from_str::<serde_json::Value>(spec_json)?)?;
let metadata: Vec<rmcp_openapi::ToolMetadata> =
    spec.to_tool_metadata(None /*filters*/, false, false, false)?;
```

- `Spec::from_value(Value) -> Result<Spec, Error>` — `$SRC/src/spec.rs:25`. Pure parse.
- `Spec::to_tool_metadata(filters, skip_tool_desc, skip_param_desc, param_examples) -> Result<Vec<ToolMetadata>, Error>`
  — `$SRC/src/spec.rs:35`. Iterates paths×methods, calls `ToolGenerator::generate_tool_metadata`
  per operation. **Does NOT build an HttpClient.** (`to_openapi_tools`, by contrast, DOES —
  `$SRC/src/spec.rs:164` → `generate_openapi_tools` at `$SRC/src/tool_generator.rs:802-826`
  constructs `HttpClient::new()`. We avoid it.)

`ToolMetadata` public fields (`$SRC/src/tool/metadata.rs:32-55`), all `pub`:
| field | type | meaning |
|-------|------|---------|
| `name` | `String` | **operationId**, or fallback `"{METHOD}_{path-with-slashes-underscored}"` when the spec omits `operationId` (`$SRC/src/tool_generator.rs:759-765`) |
| `method` | `String` | HTTP verb, upper-case (`"GET"`, `"POST"`, …) |
| `path` | `String` | path template, e.g. `/users/{id}` |
| `parameters` | `serde_json::Value` | input schema (not stored in v1) |
| `security` | `Option<Vec<String>>` | **always `None`** — see caveat |

**Mapping to the plan's guessed API:** the plan sketch used `ToolGenerator::generate_openapi_tools`
+ `tool.operation_id()` / `tool.method()` / `tool.path()`. The REAL surface is
`Spec::to_tool_metadata` returning `ToolMetadata` with the plain fields `name` / `method` / `path`.
`OperationDescriptor.operation_id` ← `ToolMetadata.name`. `convert_spec` takes the same
`spec_json: &str` + `allowed: &[String]` signature; the allowlist keys off the RAW `name`.

## Security-scheme caveat (Task 3/5 deviation — no constraint impact)

`ToolMetadata.security` is **hardcoded `None`** in v0.31.2:
`$SRC/src/tool_generator.rs:792` — `security: None, // TODO: Extract security requirements`.
The library does NOT extract per-operation `securitySchemes`. Therefore:
- `OperationDescriptor.security`/`extract_security` from the plan sketch is **dropped** —
  there is nothing to extract. `OperationDescriptor` carries only `operation_id`, `method`,
  `path_template`.
- Credential injection is driven **entirely by our own `OpenApiCredential`** config
  (`BearerToken` → `Authorization: Bearer`; `ApiKey { header, value }` → that header),
  applied unconditionally server-side in `http::execute_operation`. This is **header-style
  injection only** (v1), matching the plan's stated v1 scope. apiKey-in-query / cookie remain
  deferred. No LOCKED constraint depends on rmcp-openapi surfacing the scheme.

## SSRF containment approach (Task 7) — reuse the installer pattern

Rather than a custom `reqwest::dns::Resolve` impl, reuse the workspace-canonical
"resolve → validate every IP → pin ONE validated address → re-check the connected peer"
pattern from `crates/labby-apis/src/acp_registry/installer.rs:254-378`:
`tokio::net::lookup_host((host,port))` → `ssrf::check_ip_not_private` on each →
`Client::builder().redirect(Policy::none()).no_proxy().resolve_to_addrs(host,&[pinned])
.https_only(true).connect_timeout(..).timeout(..)` → after `send()`, re-check
`resp.remote_addr()` peer IP against the pin AND `check_ip_not_private`. The plan explicitly
sanctions this ("resolve-then-validate-then-connect to a pinned IP is acceptable"). This means
the dispatch client is built **per-call** (pinned to the resolved base_url host) rather than a
single shared client; `build_spec_fetch_client` (spec fetch) is likewise per-fetch pinned.
The `openapi_http_client` threaded through `RunnerConfig`/host stays a plain hardened
`reqwest::Client` used as a fallback / for tests, but the actual per-operation call re-resolves
and pins the base_url host.

## LOCKED DECISION

> **rmcp-openapi = spec parsing only (`Spec::from_value` + `Spec::to_tool_metadata`).
> `labby-openapi` owns the hardened HTTP client (`redirect::none()`, `https_only`, pinned
> validated IP, peer-IP re-check). Never `Tool::call`/`execute`/`generate_openapi_tools`.
> `base_url` mandatory. Never format raw rmcp-openapi errors. Credential injection from our
> own config (rmcp-openapi does not expose the scheme).**
