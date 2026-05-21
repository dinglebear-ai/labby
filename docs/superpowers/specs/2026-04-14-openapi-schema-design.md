# OpenAPI Schema — Design Spec

**Date:** 2026-04-14
**Goals:** Documentation (Scalar UI) + contract testing (schemathesis)
**Approach:** utoipa with catalog-generated action components

---

## Context

The lab HTTP API is a uniform axum service:

- `GET /health` — liveness probe (no auth)
- `GET /ready` — readiness probe (no auth)
- `GET /v1/{service}/actions` — list available actions for a service
- `POST /v1/{service}` — dispatch an action (bearer auth)

All dispatch endpoints share a single request body shape (`ActionRequest`) and a shared error envelope (`ToolError`). Service-level action metadata is already encoded in static `&[ActionSpec]` slices (one per service), which drive MCP, CLI, and HTTP discovery.

---

## Dependencies

Add to `crates/lab/Cargo.toml`:

```toml
utoipa = { version = "5.4", features = ["axum_extras"] }
utoipa-scalar = { version = "0.3", features = ["axum"] }
```

No changes to `lab-apis` — utoipa annotations live only in the `lab` crate (the HTTP surface).

---

## Schema Components

### `ActionRequest` (shared request body)

```rust
#[derive(ToSchema, Deserialize)]
pub struct ActionRequest {
    /// Dotted action name, e.g. "movie.search"
    pub action: String,
    /// Action-specific parameters. See per-action schemas in components.
    #[schema(additional_properties)]
    pub params: serde_json::Value,
}
```

### `HealthResponse`

```rust
#[derive(ToSchema, Serialize)]
pub struct HealthResponse {
    pub status: String,
}
```

### Error shapes

`ToolError` variants become named components. Each has a stable `kind` string discriminant:

| Component | `kind` value | Extra fields |
|---|---|---|
| `ErrorUnknownAction` | `unknown_action` | `valid: string[]`, `hint: string\|null` |
| `ErrorMissingParam` | `missing_param` | `param: string` |
| `ErrorInvalidParam` | `invalid_param` | `param: string` |
| `ErrorConfirmationRequired` | `confirmation_required` | — |
| `ErrorSdk` | *(sdk_kind promoted to `kind`)* | — |

All share `message: string`.

### Per-action request schemas (generated from catalog)

At startup, `build_action_schemas(catalog: &Catalog) -> Vec<(String, Schema)>` walks every service's `&[ActionSpec]` and produces named components:

- Name: `{PascalService}{PascalAction}Params` — e.g. `RadarrMovieSearchParams`
- Shape: `object` with properties derived from `ParamSpec`:
  - `ty` → JSON Schema type using the same mapping the MCP layer uses
  - `required` → listed in `required[]` if true
  - `description` → schema description

These are registered into the `OpenApi` components map and referenced in each service's endpoint description. They are informational — the actual HTTP contract validates `ActionRequest`, not per-action params.

**Type mapping** (`ParamSpec.ty` → JSON Schema):

| `ty` value | JSON Schema |
|---|---|
| `string` | `{"type":"string"}` |
| `integer` | `{"type":"integer"}` |
| `number` | `{"type":"number"}` |
| `boolean` | `{"type":"boolean"}` |
| `object` | `{"type":"object"}` |
| `array` | `{"type":"array"}` |
| `string[]` | `{"type":"array","items":{"type":"string"}}` |
| `integer[]` | `{"type":"array","items":{"type":"integer"}}` |
| `string\|null` | `{"type":["string","null"]}` |
| `"a\|b\|c"` (enum literals) | `{"type":"string","enum":["a","b","c"]}` |

---

## Handler Annotations

Every handler gets `#[utoipa::path(...)]`. The pattern is identical for all service dispatch endpoints:

```rust
#[utoipa::path(
    post,
    path = "/v1/radarr",
    tag = "radarr",
    request_body = ActionRequest,
    responses(
        (status = 200, description = "Action result", body = serde_json::Value),
        (status = 400, description = "Unknown or invalid action", body = ErrorUnknownAction),
        (status = 401, description = "Missing or invalid bearer token", body = ErrorSdk),
        (status = 422, description = "Confirmation required", body = ErrorConfirmationRequired),
    ),
    security(("bearer" = [])),
)]
```

Health endpoints have no security requirement and return `HealthResponse`.

The actions endpoint (`GET /v1/{service}/actions`) returns `array` of `ActionSpec`-shaped objects.

---

## OpenApi Assembly

A top-level `ApiDoc` struct is derived with `#[derive(OpenApi)]`:

```rust
#[derive(OpenApi)]
#[openapi(
    info(title = "lab API", version = env!("CARGO_PKG_VERSION")),
    paths(/* all handlers */),
    components(schemas(
        ActionRequest, HealthResponse,
        ErrorUnknownAction, ErrorMissingParam, ErrorInvalidParam,
        ErrorConfirmationRequired, ErrorSdk,
    )),
    security(("bearer" = [])),
    tags(/* one per service */),
    modifiers(&ActionSchemaInjector),
)]
pub struct ApiDoc;
```

`ActionSchemaInjector` implements `utoipa::Modify` and injects the catalog-generated per-action schemas into the components map at startup.

---

## Serving

Two new routes added to the **outer router** (outside bearer auth, same level as `/health`):

```
GET /openapi.json   → serves Arc<String> of serialized OpenApi
GET /docs           → serves Scalar UI (HTML) pointing at /openapi.json
```

The spec is generated once at server startup via `ApiDoc::openapi()` (plus `ActionSchemaInjector`), serialized to JSON, and wrapped in `Arc<String>`. No per-request generation cost.

---

## Contract Testing

schemathesis runs against the live server from CI or locally:

```bash
schemathesis run http://localhost:8080/openapi.json \
  --checks all \
  --header "Authorization: Bearer $LAB_API_TOKEN"
```

schemathesis validates:
- All documented endpoints return documented status codes
- Error responses conform to their declared schema shapes
- `ActionRequest` body is accepted/rejected correctly

Per-action param schemas are tested via example-based unit tests in the existing dispatch test suite — not via schemathesis (which would need a live upstream service per action).

---

## File Changes

| File | Change |
|---|---|
| `crates/lab/Cargo.toml` | Add `utoipa`, `utoipa-scalar` |
| `crates/lab/src/api/openapi.rs` | New: `ApiDoc`, `ActionSchemaInjector`, error schema types |
| `crates/lab/src/api/router.rs` | Mount `/openapi.json` + `/docs` routes; add `ApiDoc::openapi()` call |
| `crates/lab/src/api/health.rs` | Add `utoipa::path` annotation + `HealthResponse` schema |
| `crates/lab/src/api/services/*.rs` | Add `utoipa::path` annotation to each handler (20 files) |
| `crates/lab/src/api.rs` | Export `openapi` module |
| `docs/upstream-api/lab.openapi.yaml` | Generated artifact — `lab openapi` CLI subcommand output |

---

## Out of Scope

- Authentication scheme changes (bearer token stays as-is)
- Per-action HTTP endpoints (dispatch stays action+params)
- OpenAPI client SDK generation (not requested)
- Versioning strategy beyond the current single `/v1` prefix
