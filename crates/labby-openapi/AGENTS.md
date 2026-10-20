# labby-openapi — Charter

This crate extracts allowlisted operations from operator-selected JSON specs and
owns their hardened outbound dispatch for Code Mode's `openapi` provider.
Consumers carry the registry/client through host contracts; the OpenAPI loading
and HTTP implementation stays here.

## External references

| Concern | Source |
| --- | --- |
| Public API and state ownership | [Crate README](README.md), [registry](src/registry.rs) |
| Extraction and dispatch | [Converter](src/convert.rs), [dispatch](src/dispatch.rs) |
| Transport and stable errors | [HTTP guards](src/http.rs), [error mapping](src/error.rs) |
| Product configuration resolution | [Host config](../labby/src/config.rs) |

## Hard rules

- **Keep extraction narrow.** `convert.rs` uses `serde_json` to derive raw operation ID,
  HTTP method, and path template. There is no `rmcp-openapi` dependency. Preserve the
  exact raw-ID allowlist; missing IDs and excluded operations do not become callable.
- **This crate owns the outbound HTTP.** The hardened `reqwest::Client` in `http.rs` is built
  with `redirect::Policy::none()`, `https_only(true)`, `no_proxy()`, connect/request timeouts, and the
  workspace-canonical "resolve → validate every IP → pin one → re-check the connected peer IP"
  SSRF pattern. Pinning is the primary rebinding defense; the peer check fails closed
  on an absent or mismatched address. Preserve bounded DNS resolution and response collection.
- **All SSRF checks go through `labby_primitives::ssrf`** (`parse_validated_https_url`,
  `check_ip_not_private`, …). Do NOT hand-roll RFC1918/loopback/CGNAT checks.
- **`base_url` is mandatory in config.** The converter ignores `servers[]`;
  the operator supplies the base URL. Load-time preflight does not resolve DNS.
  Preserve request-time origin/base-path scope checks and opaque path-param encoding.
- **Credentials are injected server-side after the sandbox boundary.** Static
  `OpenApiCredential` values are resolved by the host, while caller-scoped
  OAuth arrives only as a per-dispatch credential override resolved by the host.
  The registry stores the non-secret `oauth_upstream` reference, never a user's
  bearer token. Static auth and `oauth_upstream` are mutually exclusive. The JS
  snippet never sees a raw key/token. Header-style injection only in v1.
- **Never format a raw parser or `reqwest` error** into any `tracing` field or
  `ToolError`/`OpenApiError` message — always map to a fixed, scrubbed `OpenApiError` variant
  first. Preserve `ResolveFailed` versus forbidden-address classification and
  `UpstreamTimeout`; the [mapping](src/error.rs) owns log and surface kinds. Canary tests enforce redaction.
- **No `mod.rs`.** Use a named `.rs` module file beside its same-name subdirectory. No `#[async_trait]`.
- **Configuration discovery and env resolution are host-owned.** Registry loading
  deliberately reads explicit `SpecSource::Path` files with regular-file checks and
  a pre-parse byte cap. Do not add ambient configuration discovery to this crate.
- **Test bypasses stay test-only.** Production dispatch always pins and checks peers;
  loopback Wiremock helpers must remain behind `cfg(test)`.
- **Dependency direction:** `labby-codemode -> labby-openapi -> labby-runtime`/`labby-primitives`.
  MUST NOT depend on `labby-codemode` or `labby-gateway`.
