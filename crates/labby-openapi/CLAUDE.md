# labby-openapi — Charter

Isolated crate that turns an operator-configured OpenAPI spec into locally-dispatched
Code Mode operations for the `openapi` provider. It exists to keep `rmcp-openapi` and
`reqwest` **out of** `labby-codemode` (whose charter is HTTP-free).

## Hard rules

- **Parse-only use of `rmcp-openapi`.** Use ONLY `Spec::from_value` + `Spec::to_tool_metadata`
  to derive `{operation_id (= ToolMetadata.name), method, path_template}`. NEVER call
  `Tool::call()`, `Tool::execute()`, `generate_openapi_tools`, or any path that constructs
  `rmcp_openapi::HttpClient` — its client follows redirects with no override (unsafe for SSRF).
- **This crate owns the outbound HTTP.** The hardened `reqwest::Client` in `http.rs` is built
  with `redirect::Policy::none()`, `https_only(true)`, explicit connect/read timeouts, and the
  workspace-canonical "resolve → validate every IP → pin one → re-check the connected peer IP"
  SSRF pattern (mirrors `labby-apis::acp_registry::installer`). The peer-IP re-check is the
  load-bearing DNS-rebinding defense — a hostname string check is NOT sufficient.
- **All SSRF checks go through `labby_primitives::ssrf`** (`parse_validated_https_url`,
  `check_ip_not_private`, …). Do NOT hand-roll RFC1918/loopback/CGNAT checks.
- **`base_url` is mandatory in config.** `rmcp-openapi` never reads the spec's `servers[]`;
  the base URL is always operator-configured and SSRF-validated at load time. No `servers[]`
  parsing.
- **Credentials injected server-side**, from our own `OpenApiCredential` config (rmcp-openapi
  does not expose per-operation security schemes — `ToolMetadata.security` is always `None`).
  The JS snippet never sees a raw key. Header-style injection only in v1.
- **Never format a raw `rmcp_openapi::*` or `reqwest` error** into any `tracing` field or
  `ToolError`/`OpenApiError` message — always map to a fixed, scrubbed `OpenApiError` variant
  first. A committed canary test enforces this.
- **No `mod.rs`.** A module `foo` is `foo.rs` sibling to `foo/`. No `#[async_trait]`.
- **No env/file reads.** Config loading lives ONLY in `crates/labby/src/config.rs`.
- **Dependency direction:** `labby-codemode -> labby-openapi -> labby-runtime`/`labby-primitives`.
  MUST NOT depend on `labby-codemode` or `labby-gateway`.
