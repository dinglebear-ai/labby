# core/ — Cross-cutting primitives

This directory owns shared SDK HTTP primitives and contract re-exports. Changes affect SDK consumers; read the owning contracts below before editing.

## External references

| Concern | Contract |
| --- | --- |
| Dependency boundaries and conventions | [Architecture](../../../../docs/ARCH.md), [conventions](../../../../docs/CONVENTIONS.md) |
| Request events and error mapping | [Observability](../../../../docs/dev/OBSERVABILITY.md), [errors](../../../../docs/dev/ERRORS.md) |
| Wire serialization | [Serialization](../../../../docs/design/SERIALIZATION.md) |

## Files

| File | Purpose |
|------|---------|
| `auth.rs` | `Auth` enum: `ApiKey { header, key }`, `Bearer { token }`, `Token { token }`, `Basic { username, password }`, `Session { cookie }`, `None`. Debug impl **must** redact secrets. |
| `http.rs` | `HttpClient` wrapper around `reqwest::Client` — auth injection, `request.start`/`request.finish`/`request.error` tracing events, JSON and GraphQL helpers. No retry logic, no backoff, no spans — callers own those. |
| `error.rs` | `ApiError` canonical taxonomy + `kind()` method; stable tags appear in the error-kind table. |
| `status.rs` | `ServiceStatus { reachable, auth_ok, version, latency_ms, message }` — returned by `ServiceClient::health()`. |
| `action.rs` | Re-exports `ActionSpec` and `ParamSpec` from their [canonical definitions](../../../labby-primitives/src/action.rs). Preserve admin/destructive policy separation and complete output schemas. |
| `plugin.rs` | Re-exports `Category`, `EnvVar`, and `PluginMeta` from their [canonical definitions](../../../labby-primitives/src/plugin.rs). |
| `plugin_ui.rs` | Re-exports `UiSchema` and wizard field types from their [canonical definitions](../../../labby-primitives/src/plugin_ui.rs). |
| `ssrf.rs` | Re-exports static SSRF preflight checks from their [canonical implementation](../../../labby-primitives/src/ssrf.rs). |
| `traits.rs` | `ServiceClient::health` returns `impl Future<Output = Result<ServiceStatus, ApiError>> + Send`; no `#[async_trait]` or `dyn ServiceClient`. |

Shared vocabulary lives in `labby-primitives`, an internal leaf crate with no workspace dependencies; it has third-party dependencies. Edit those types there, not in these shims. `labby-gateway` also depends on this SDK and uses its SSH configuration parser; preserve the separation from product routing and surfaces.

## ApiError.kind() — canonical stable tags

These strings appear verbatim in MCP and HTTP error surfaces. Adding a new kind is a **spec change** — update `docs/dev/ERRORS.md`, `docs/surfaces/MCP.md`, and `docs/CONVENTIONS.md` first.

| Variant | `kind()` |
|---------|----------|
| `Auth` | `"auth_failed"` |
| `NotFound` | `"not_found"` |
| `RateLimited { retry_after }` | `"rate_limited"` |
| `Validation { field, message }` | `"validation_failed"` |
| `Network(_)` | `"network_error"` |
| `Server { status, body }` | `"server_error"` |
| `Decode(_)` | `"decode_error"` |
| `Internal(_)` | `"internal_error"` |

Dispatchers in `crates/labby/src/dispatch/` layer additional kinds on top: `unknown_action`, `unknown_subaction`, `missing_param`, `invalid_param`, `unknown_instance`. See `docs/dev/ERRORS.md` for the canonical vocabulary and envelope rules.

## Invariants

- **No `clap`, `rmcp`, `tabled`, `anyhow`** in this directory — ever.
- **No file or env I/O.** Callers resolve configuration and credentials, then inject values. `Auth` does not read environment variables.
- **Debug impls for anything holding secrets must redact.** Test this.
- **Keep `ParamSpec.ty` as `&'static str`** (for example, `"string"`, `"integer"`, `"boolean"`). Do not reintroduce a `ParamType` enum — the topic docs standardize on string labels.
- **`ActionSpec.destructive` is the single source of truth** for elicitation + CLI confirm flows. Keep `requires_admin` independent; an admin-only mutation is not automatically destructive. Never hide destructive ops behind a non-destructive action.
