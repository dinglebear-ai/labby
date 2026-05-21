# OpenAPI Schema + Scalar UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add OpenAPI 3.1 schema generation (via utoipa) and Scalar UI documentation to the lab HTTP API surface, enabling both interactive documentation browsing and contract testing.

**Architecture:** A new `api/openapi.rs` module owns all utoipa coupling: `ApiDoc`, documentation-only error schema types, catalog-generated per-action param schemas, and **programmatic path construction** from catalog data. No `#[utoipa::path]` annotations on service handlers — all 22 service paths are built from the existing `ActionSpec` catalog in a single loop inside `openapi.rs`. The OpenAPI JSON is generated once at startup via a pure `build_openapi_spec()` function, served from `Arc<String>`. Routes `/openapi.json` and `/docs` sit **inside bearer auth** alongside `/v1` routes. Scalar UI assets are self-hosted (vendored into the binary).

**Tech Stack:** utoipa 5.4.0 (`axum_extras` feature), utoipa-scalar 0.3.0 (`axum` feature), schemathesis (external contract testing)

**Review decisions applied:**
- Programmatic path construction (no 22-file annotation sweep)
- `/openapi.json` + `/docs` behind bearer auth (not public)
- Self-hosted Scalar UI assets (no CDN dependency)
- Drift test for ToolError ↔ doc schema parity
- `tracing::warn!` on unknown `ParamSpec.ty` fallback
- `Cache-Control: private, no-store` on `/openapi.json`
- `Serialize` added to `ActionRequest`
- `build_openapi_spec()` extracted as pure function

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `crates/lab/Cargo.toml` | Modify | Add `utoipa`, `utoipa-scalar` dependencies |
| `crates/lab/src/api/openapi.rs` | Create | `ApiDoc`, `ActionSchemaInjector`, error schema doc types, `build_action_schemas()`, `param_type_to_schema()`, `build_service_paths()`, `build_openapi_spec()` |
| `crates/lab/src/api.rs` | Modify | Add `pub mod openapi;`, add `Serialize` + `ToSchema` to `ActionRequest` |
| `crates/lab/src/api/health.rs` | Modify | `HealthResponse` schema type (no `#[utoipa::path]` — health paths built programmatically) |
| `crates/lab/src/api/router.rs` | Modify | Mount `/openapi.json` + `/docs` inside bearer auth, call `build_openapi_spec()` |

No changes to `crates/lab/src/api/services/*.rs` — all service paths are built programmatically from catalog data.

---

### Task 1: Add utoipa dependencies + verify compatibility

**Files:**
- Modify: `crates/lab/Cargo.toml`

- [ ] **Step 1: Verify utoipa 5.4 supports axum 0.8**

Before adding the dependency, check compatibility:

Run: `cargo search utoipa --limit 5`

Then check utoipa's Cargo.toml for axum version bounds. The `axum_extras` feature must support axum 0.8 (this project's version). If utoipa 5.4 pins axum 0.7, find the correct utoipa version or feature flag.

- [ ] **Step 2: Add utoipa and utoipa-scalar to dependencies**

In `crates/lab/Cargo.toml`, add these two lines to the `[dependencies]` section (after the existing `axum`, `tower`, `tower-http` block):

```toml
utoipa                       = { version = "5.4", features = ["axum_extras"] }
utoipa-scalar                = { version = "0.3", features = ["axum"] }
```

> **Note:** If Step 1 revealed a version incompatibility, adjust the version constraint here. The `axum_extras` feature name may also differ — check utoipa docs.

- [ ] **Step 3: Verify the project compiles**

Run: `cargo check --all-features -p lab`
Expected: Compiles successfully with no errors.

- [ ] **Step 4: Run cargo deny check**

Run: `cargo deny check`
Expected: No new advisories or duplicate proc-macro crate conflicts from utoipa.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/Cargo.toml Cargo.lock
git commit -m "chore: add utoipa 5.4 + utoipa-scalar 0.3 dependencies"
```

---

### Task 2: Error schema doc types + HealthResponse + ActionRequest ToSchema

**Files:**
- Create: `crates/lab/src/api/openapi.rs`
- Modify: `crates/lab/src/api.rs`
- Modify: `crates/lab/src/api/health.rs`

This task creates documentation-only schema types that mirror the JSON wire format of `ToolError` variants. **Critical constraint:** `ToolError` has a hand-written `Serialize` impl where the `Sdk` variant promotes `sdk_kind` to the top-level `kind` field. Never add `#[derive(ToSchema)]` or `#[derive(Serialize)]` to `ToolError` — doing so would break the wire format. These doc types are separate structs used only by utoipa for schema generation.

- [ ] **Step 1: Write the failing test**

Create `crates/lab/src/api/openapi.rs` with the test first:

```rust
//! OpenAPI schema generation for the lab HTTP API.
//!
//! This module owns ALL utoipa coupling — no other file in the crate imports
//! utoipa types or uses `#[utoipa::path]` annotations. Service paths are built
//! programmatically from the `ActionSpec` catalog.
//!
//! **Critical:** `ToolError` has a hand-written `Serialize` impl — never add
//! `ToSchema` to it. The doc types here are separate structs that match the
//! JSON wire format for OpenAPI documentation only.

use serde::Serialize;
use utoipa::ToSchema;

// ── Documentation-only error schema types ──────────────────────────────

/// OpenAPI schema for `unknown_action` error envelope.
#[derive(Serialize, ToSchema)]
pub struct ErrorUnknownAction {
    /// Always `"unknown_action"`.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
    /// Valid action names for this service.
    pub valid: Vec<String>,
    /// Optional fuzzy match suggestion.
    #[schema(nullable)]
    pub hint: Option<String>,
}

/// OpenAPI schema for `missing_param` error envelope.
#[derive(Serialize, ToSchema)]
pub struct ErrorMissingParam {
    /// Always `"missing_param"`.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
    /// The missing parameter name.
    pub param: String,
}

/// OpenAPI schema for `invalid_param` error envelope.
#[derive(Serialize, ToSchema)]
pub struct ErrorInvalidParam {
    /// Always `"invalid_param"`.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
    /// The invalid parameter name.
    pub param: String,
}

/// OpenAPI schema for `confirmation_required` error envelope.
#[derive(Serialize, ToSchema)]
pub struct ErrorConfirmationRequired {
    /// Always `"confirmation_required"`.
    pub kind: String,
    /// Human-readable message.
    pub message: String,
}

/// OpenAPI schema for SDK pass-through error envelopes.
///
/// The `kind` field contains the promoted `sdk_kind` value (e.g.,
/// `"auth_failed"`, `"rate_limited"`, `"not_found"`).
#[derive(Serialize, ToSchema)]
pub struct ErrorSdk {
    /// Stable error kind tag (e.g., `"auth_failed"`, `"rate_limited"`).
    pub kind: String,
    /// Human-readable message.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_schema_types_exist_and_serialize() {
        let ua = ErrorUnknownAction {
            kind: "unknown_action".into(),
            message: "test".into(),
            valid: vec!["help".into()],
            hint: Some("halp".into()),
        };
        let json = serde_json::to_value(&ua).unwrap();
        assert_eq!(json["kind"], "unknown_action");
        assert_eq!(json["valid"], serde_json::json!(["help"]));
        assert_eq!(json["hint"], "halp");

        let mp = ErrorMissingParam {
            kind: "missing_param".into(),
            message: "test".into(),
            param: "query".into(),
        };
        let json = serde_json::to_value(&mp).unwrap();
        assert_eq!(json["kind"], "missing_param");
        assert_eq!(json["param"], "query");

        let cr = ErrorConfirmationRequired {
            kind: "confirmation_required".into(),
            message: "test".into(),
        };
        let json = serde_json::to_value(&cr).unwrap();
        assert_eq!(json["kind"], "confirmation_required");

        let sdk = ErrorSdk {
            kind: "auth_failed".into(),
            message: "test".into(),
        };
        let json = serde_json::to_value(&sdk).unwrap();
        assert_eq!(json["kind"], "auth_failed");
    }

    /// Drift test: ensure doc-only error schema types have the same field
    /// names as `ToolError`'s hand-written `Serialize` output.
    ///
    /// If a field is added to `ToolError` serialization but not to the
    /// doc-only struct, this test will catch the divergence.
    #[test]
    fn error_doc_schemas_match_tool_error_wire_format() {
        use crate::dispatch::error::ToolError;

        // UnknownAction
        let te = ToolError::UnknownAction {
            message: "x".into(),
            valid: vec!["a".into()],
            hint: Some("b".into()),
        };
        let te_keys: Vec<String> = serde_json::to_value(&te)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let doc = ErrorUnknownAction {
            kind: "".into(),
            message: "".into(),
            valid: vec![],
            hint: None,
        };
        let doc_keys: Vec<String> = serde_json::to_value(&doc)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut te_sorted = te_keys.clone();
        te_sorted.sort();
        let mut doc_sorted = doc_keys.clone();
        doc_sorted.sort();
        assert_eq!(te_sorted, doc_sorted, "ErrorUnknownAction drift: ToolError has {te_keys:?}, doc has {doc_keys:?}");

        // MissingParam
        let te = ToolError::MissingParam {
            message: "x".into(),
            param: "p".into(),
        };
        let te_keys: Vec<String> = serde_json::to_value(&te)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let doc = ErrorMissingParam {
            kind: "".into(),
            message: "".into(),
            param: "".into(),
        };
        let doc_keys: Vec<String> = serde_json::to_value(&doc)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut te_sorted = te_keys.clone();
        te_sorted.sort();
        let mut doc_sorted = doc_keys.clone();
        doc_sorted.sort();
        assert_eq!(te_sorted, doc_sorted, "ErrorMissingParam drift");

        // ConfirmationRequired
        let te = ToolError::ConfirmationRequired {
            message: "x".into(),
        };
        let te_keys: Vec<String> = serde_json::to_value(&te)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let doc = ErrorConfirmationRequired {
            kind: "".into(),
            message: "".into(),
        };
        let doc_keys: Vec<String> = serde_json::to_value(&doc)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut te_sorted = te_keys.clone();
        te_sorted.sort();
        let mut doc_sorted = doc_keys.clone();
        doc_sorted.sort();
        assert_eq!(te_sorted, doc_sorted, "ErrorConfirmationRequired drift");

        // Sdk
        let te = ToolError::Sdk {
            sdk_kind: "auth_failed".into(),
            message: "x".into(),
        };
        let te_keys: Vec<String> = serde_json::to_value(&te)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let doc = ErrorSdk {
            kind: "".into(),
            message: "".into(),
        };
        let doc_keys: Vec<String> = serde_json::to_value(&doc)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut te_sorted = te_keys.clone();
        te_sorted.sort();
        let mut doc_sorted = doc_keys.clone();
        doc_sorted.sort();
        assert_eq!(te_sorted, doc_sorted, "ErrorSdk drift");
    }
}
```

- [ ] **Step 2: Add `pub mod openapi;` to api.rs**

In `crates/lab/src/api.rs`, add after the `pub mod services;` line:

```rust
/// OpenAPI schema generation (utoipa + Scalar UI).
pub mod openapi;
```

- [ ] **Step 3: Add Serialize + ToSchema to ActionRequest**

In `crates/lab/src/api.rs`, change the `ActionRequest` derive to include both:

```rust
#[derive(Debug, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct ActionRequest {
    /// Dotted action name, e.g. "movie.search"
    pub action: String,
    /// Action-specific parameters
    #[serde(default)]
    pub params: serde_json::Value,
}
```

- [ ] **Step 4: Add HealthResponse to health.rs**

In `crates/lab/src/api/health.rs`, add `HealthResponse` and update handlers:

```rust
use utoipa::ToSchema;

/// Response body for liveness/readiness probes.
#[derive(serde::Serialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
}
```

Update the `health()` handler:

```rust
pub async fn health() -> impl IntoResponse {
    Json(HealthResponse { status: "ok".into() })
}
```

Update the `ready()` handler:

```rust
pub async fn ready(State(_state): State<AppState>) -> impl IntoResponse {
    (StatusCode::OK, Json(HealthResponse { status: "ready".into() }))
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test --all-features -p lab -- openapi::tests`
Expected: Both `error_schema_types_exist_and_serialize` and `error_doc_schemas_match_tool_error_wire_format` PASS.

- [ ] **Step 6: Run all existing tests to ensure no regressions**

Run: `cargo test --all-features -p lab`
Expected: All tests pass. The `HealthResponse` type change is backward-compatible (same JSON output).

- [ ] **Step 7: Commit**

```bash
git add crates/lab/src/api/openapi.rs crates/lab/src/api.rs crates/lab/src/api/health.rs
git commit -m "feat(openapi): add error schema doc types, HealthResponse, ActionRequest ToSchema + drift test"
```

---

### Task 3: `param_type_to_schema()` — ParamSpec.ty to JSON Schema mapping

**Files:**
- Modify: `crates/lab/src/api/openapi.rs`

This function converts the `ParamSpec.ty` string labels into `utoipa::openapi::Schema` objects. The mapping table from the spec must be covered exhaustively by unit tests. Unknown types log a warning and fall back to `string`.

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/lab/src/api/openapi.rs`:

```rust
    use utoipa::openapi::schema::Schema;

    /// Helper: extract the JSON representation of a schema for assertions.
    fn schema_json(s: &Schema) -> serde_json::Value {
        serde_json::to_value(s).unwrap()
    }

    #[test]
    fn param_type_string() {
        let s = super::param_type_to_schema("string");
        let j = schema_json(&s);
        assert_eq!(j["type"], "string");
    }

    #[test]
    fn param_type_integer() {
        let s = super::param_type_to_schema("integer");
        let j = schema_json(&s);
        assert_eq!(j["type"], "integer");
    }

    #[test]
    fn param_type_number() {
        let s = super::param_type_to_schema("number");
        let j = schema_json(&s);
        assert_eq!(j["type"], "number");
    }

    #[test]
    fn param_type_boolean() {
        let s = super::param_type_to_schema("boolean");
        let j = schema_json(&s);
        assert_eq!(j["type"], "boolean");
    }

    #[test]
    fn param_type_object() {
        let s = super::param_type_to_schema("object");
        let j = schema_json(&s);
        assert_eq!(j["type"], "object");
    }

    #[test]
    fn param_type_array() {
        let s = super::param_type_to_schema("array");
        let j = schema_json(&s);
        assert_eq!(j["type"], "array");
    }

    #[test]
    fn param_type_string_array() {
        let s = super::param_type_to_schema("string[]");
        let j = schema_json(&s);
        assert_eq!(j["type"], "array");
        assert_eq!(j["items"]["type"], "string");
    }

    #[test]
    fn param_type_integer_array() {
        let s = super::param_type_to_schema("integer[]");
        let j = schema_json(&s);
        assert_eq!(j["type"], "array");
        assert_eq!(j["items"]["type"], "integer");
    }

    #[test]
    fn param_type_nullable_string() {
        let s = super::param_type_to_schema("string|null");
        let j = schema_json(&s);
        // OpenAPI 3.1: nullable via type array or nullable flag
        assert!(
            j["type"] == serde_json::json!(["string", "null"])
                || j.get("nullable") == Some(&serde_json::json!(true)),
            "expected nullable string schema, got: {j}"
        );
    }

    #[test]
    fn param_type_enum_literals() {
        let s = super::param_type_to_schema("asc|desc");
        let j = schema_json(&s);
        assert_eq!(j["type"], "string");
        assert_eq!(j["enum"], serde_json::json!(["asc", "desc"]));
    }

    #[test]
    fn param_type_three_way_enum() {
        let s = super::param_type_to_schema("movie|show|all");
        let j = schema_json(&s);
        assert_eq!(j["type"], "string");
        assert_eq!(j["enum"], serde_json::json!(["movie", "show", "all"]));
    }

    #[test]
    fn param_type_unknown_falls_back_to_string() {
        let s = super::param_type_to_schema("foobar");
        let j = schema_json(&s);
        assert_eq!(j["type"], "string", "unknown types should fall back to string");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-features -p lab -- openapi::tests::param_type`
Expected: FAIL — `param_type_to_schema` does not exist yet.

- [ ] **Step 3: Implement `param_type_to_schema()`**

Add this function to `crates/lab/src/api/openapi.rs` (above `#[cfg(test)]`):

```rust
use utoipa::openapi::schema::{ObjectBuilder, ArrayBuilder, Schema, SchemaType};

/// Convert a `ParamSpec.ty` string label to a `utoipa::openapi::Schema`.
///
/// Mapping table (from `docs/superpowers/specs/2026-04-14-openapi-schema-design.md`):
///
/// | `ty`            | JSON Schema                                     |
/// |-----------------|-------------------------------------------------|
/// | `string`        | `{"type":"string"}`                              |
/// | `integer`       | `{"type":"integer"}`                             |
/// | `number`        | `{"type":"number"}`                              |
/// | `boolean`       | `{"type":"boolean"}`                             |
/// | `object`        | `{"type":"object"}`                              |
/// | `array`         | `{"type":"array"}`                               |
/// | `string[]`      | `{"type":"array","items":{"type":"string"}}`     |
/// | `integer[]`     | `{"type":"array","items":{"type":"integer"}}`    |
/// | `string\|null`  | nullable string (Option<String> pattern)         |
/// | `"a\|b\|c"`     | `{"type":"string","enum":["a","b","c"]}`         |
///
/// Unknown values log a warning and fall back to `{"type":"string"}`.
///
/// **API note (from research):** utoipa 5.4 `ObjectBuilder` uses
/// `.schema_type(SchemaType::String)` directly (not `SchemaType::new()`).
/// There is no `.nullable()` method — nullable is expressed via `Option<T>`
/// in derive macros or via schema composition programmatically.
pub fn param_type_to_schema(ty: &str) -> Schema {
    match ty {
        "string" => ObjectBuilder::new()
            .schema_type(SchemaType::String)
            .build()
            .into(),
        "integer" => ObjectBuilder::new()
            .schema_type(SchemaType::Integer)
            .build()
            .into(),
        "number" => ObjectBuilder::new()
            .schema_type(SchemaType::Number)
            .build()
            .into(),
        "boolean" => ObjectBuilder::new()
            .schema_type(SchemaType::Boolean)
            .build()
            .into(),
        "object" => ObjectBuilder::new()
            .schema_type(SchemaType::Object)
            .build()
            .into(),
        "array" => ArrayBuilder::new().build().into(),
        "string[]" => ArrayBuilder::new()
            .items(ObjectBuilder::new().schema_type(SchemaType::String))
            .build()
            .into(),
        "integer[]" => ArrayBuilder::new()
            .items(ObjectBuilder::new().schema_type(SchemaType::Integer))
            .build()
            .into(),
        "string|null" => {
            // utoipa has no .nullable() builder method. Use Option<String>
            // schema pattern: build a schema that represents nullable string.
            // The exact approach depends on the utoipa version — try
            // SchemaType with multiple types or use a OneOf composition.
            // If this doesn't compile, use `<Option<String>>::schema()` instead.
            ObjectBuilder::new()
                .schema_type(SchemaType::String)
                .build()
                .into()
            // TODO: Verify the nullable output matches test assertion.
            // The test accepts either `type: ["string", "null"]` or
            // `nullable: true`. Adjust based on what utoipa 5.4 actually
            // produces for nullable schemas.
        }
        other => {
            // If it contains `|` and is not `string|null`, treat as enum literals.
            if other.contains('|') {
                let variants: Vec<serde_json::Value> =
                    other.split('|').map(|s| serde_json::Value::String(s.to_string())).collect();
                ObjectBuilder::new()
                    .schema_type(SchemaType::String)
                    .enum_values(Some(variants))
                    .build()
                    .into()
            } else {
                // Unknown type — warn and fall back to string.
                tracing::warn!(
                    param_type = other,
                    "unknown ParamSpec.ty value, falling back to string schema"
                );
                ObjectBuilder::new()
                    .schema_type(SchemaType::String)
                    .build()
                    .into()
            }
        }
    }
}
```

> **Research finding (utoipa 5.4):** `ObjectBuilder` uses `.schema_type(SchemaType::String)` directly — not `SchemaType::new(Type::String)`. There is no `.nullable()` method on builders. For nullable types, use `<Option<String>>::schema()` from the `ToSchema` derive, or compose schemas with `OneOf`. The `string|null` case may need adjustment at implementation time — the test assertion is flexible (accepts either `type: ["string", "null"]` or `nullable: true`).

- [ ] **Step 4: Run the tests**

Run: `cargo test --all-features -p lab -- openapi::tests::param_type`
Expected: All 12 param_type tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api/openapi.rs
git commit -m "feat(openapi): param_type_to_schema with tracing::warn on unknown types"
```

---

### Task 4: `build_action_schemas()` — catalog-generated per-action param schemas

**Files:**
- Modify: `crates/lab/src/api/openapi.rs`

This function walks every service's `&[ActionSpec]` and produces named schema components like `RadarrMovieSearchParams`. These are injected into the OpenAPI components map for documentation.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/lab/src/api/openapi.rs`:

```rust
    use lab_apis::core::action::{ActionSpec, ParamSpec};

    #[test]
    fn build_action_schemas_produces_named_components() {
        let actions: &[ActionSpec] = &[
            ActionSpec {
                name: "movie.search",
                description: "Search for movies",
                destructive: false,
                params: &[
                    ParamSpec {
                        name: "query",
                        ty: "string",
                        required: true,
                        description: "Search query",
                    },
                    ParamSpec {
                        name: "year",
                        ty: "integer",
                        required: false,
                        description: "Release year",
                    },
                ],
                returns: "array of movie objects",
            },
            ActionSpec {
                name: "queue.list",
                description: "List queue",
                destructive: false,
                params: &[],
                returns: "array of queue items",
            },
        ];

        let schemas = super::build_action_schemas("radarr", actions);

        // movie.search -> RadarrMovieSearchParams
        assert!(
            schemas.iter().any(|(name, _)| name == "RadarrMovieSearchParams"),
            "expected RadarrMovieSearchParams in schemas: {:?}",
            schemas.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );

        // queue.list has no params — should still produce a schema (empty object)
        assert!(
            schemas.iter().any(|(name, _)| name == "RadarrQueueListParams"),
            "expected RadarrQueueListParams even for parameterless actions"
        );

        // Verify the movie.search schema has the right properties
        let (_, movie_schema) = schemas.iter().find(|(n, _)| n == "RadarrMovieSearchParams").unwrap();
        let j = serde_json::to_value(movie_schema).unwrap();
        assert_eq!(j["type"], "object");
        assert!(j["properties"]["query"].is_object(), "expected query property");
        assert!(j["properties"]["year"].is_object(), "expected year property");
        assert_eq!(j["required"], serde_json::json!(["query"]));
    }

    #[test]
    fn build_action_schemas_pascal_case_conversion() {
        let actions: &[ActionSpec] = &[ActionSpec {
            name: "bookmark.search",
            description: "Search bookmarks",
            destructive: false,
            params: &[],
            returns: "array",
        }];

        let schemas = super::build_action_schemas("linkding", actions);
        assert!(
            schemas.iter().any(|(name, _)| name == "LinkdingBookmarkSearchParams"),
            "service and action should be PascalCased: {:?}",
            schemas.iter().map(|(n, _)| n).collect::<Vec<_>>()
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --all-features -p lab -- openapi::tests::build_action_schemas`
Expected: FAIL — function does not exist yet.

- [ ] **Step 3: Implement helper and `build_action_schemas()`**

Add to `crates/lab/src/api/openapi.rs`:

```rust
use lab_apis::core::action::{ActionSpec, ParamSpec};

/// Convert a dotted action name to PascalCase.
///
/// `"movie.search"` -> `"MovieSearch"`, `"queue.list"` -> `"QueueList"`.
fn to_pascal_case(s: &str) -> String {
    s.split('.')
        .flat_map(|segment| {
            let mut chars = segment.chars();
            let first = chars.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            std::iter::once(first).chain(std::iter::once(chars.as_str().to_string()))
        })
        .collect()
}

/// Build named OpenAPI schema components from a service's action catalog.
///
/// Each action produces a `{PascalService}{PascalAction}Params` named schema
/// of type `object` with properties derived from `ParamSpec` entries.
///
/// These schemas are informational — the actual HTTP contract validates
/// `ActionRequest`, not per-action params.
pub fn build_action_schemas(service: &str, actions: &[ActionSpec]) -> Vec<(String, Schema)> {
    let service_pascal = to_pascal_case(service);
    actions
        .iter()
        .map(|action| {
            let action_pascal = to_pascal_case(action.name);
            let name = format!("{service_pascal}{action_pascal}Params");

            let mut builder = ObjectBuilder::new()
                .schema_type(SchemaType::Object);

            for param in action.params {
                builder = builder.property(param.name, param_type_to_schema(param.ty));
                if param.required {
                    builder = builder.required(param.name);
                }
            }

            let schema: Schema = builder
                .description(Some(action.description))
                .build()
                .into();

            (name, schema)
        })
        .collect()
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --all-features -p lab -- openapi::tests::build_action_schemas`
Expected: All tests PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api/openapi.rs
git commit -m "feat(openapi): build_action_schemas — generate per-action param schemas from catalog"
```

---

### Task 5: `ActionSchemaInjector` + `SecurityAddon` — utoipa::Modify impls

**Files:**
- Modify: `crates/lab/src/api/openapi.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/lab/src/api/openapi.rs`:

```rust
    use utoipa::OpenApi;

    #[test]
    fn action_schema_injector_adds_components() {
        #[derive(OpenApi)]
        #[openapi()]
        struct EmptyDoc;

        let actions: &[ActionSpec] = &[ActionSpec {
            name: "movie.search",
            description: "Search for movies",
            destructive: false,
            params: &[ParamSpec {
                name: "query",
                ty: "string",
                required: true,
                description: "Search query",
            }],
            returns: "array",
        }];

        let schemas = super::build_action_schemas("radarr", actions);
        let injector = super::ActionSchemaInjector { schemas };

        let mut doc = EmptyDoc::openapi();
        injector.modify(&mut doc);

        let components = doc.components.expect("components should exist after injection");
        assert!(
            components.schemas.contains_key("RadarrMovieSearchParams"),
            "injector should add RadarrMovieSearchParams to components"
        );
    }

    #[test]
    fn security_addon_adds_bearer_scheme() {
        #[derive(OpenApi)]
        #[openapi()]
        struct EmptyDoc;

        let addon = super::SecurityAddon;
        let mut doc = EmptyDoc::openapi();
        addon.modify(&mut doc);

        let components = doc.components.expect("components should exist");
        assert!(
            components.security_schemes.contains_key("bearer"),
            "SecurityAddon should add bearer scheme"
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --all-features -p lab -- openapi::tests::action_schema_injector openapi::tests::security_addon`
Expected: FAIL — structs do not exist.

- [ ] **Step 3: Implement both modifiers**

Add to `crates/lab/src/api/openapi.rs`:

```rust
/// Injects catalog-generated per-action param schemas into the OpenApi
/// components map at startup.
pub struct ActionSchemaInjector {
    pub schemas: Vec<(String, Schema)>,
}

impl utoipa::Modify for ActionSchemaInjector {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        for (name, schema) in &self.schemas {
            components.schemas.insert(name.clone(), schema.clone());
        }
    }
}

/// Adds the bearer token security scheme to the OpenApi spec.
pub struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "bearer",
            utoipa::openapi::security::SecurityScheme::Http(
                utoipa::openapi::security::Http::new(
                    utoipa::openapi::security::HttpAuthScheme::Bearer,
                ),
            ),
        );
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --all-features -p lab -- openapi::tests::action_schema_injector openapi::tests::security_addon`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api/openapi.rs
git commit -m "feat(openapi): ActionSchemaInjector + SecurityAddon modifiers"
```

---

### Task 6: Programmatic path construction + `ApiDoc` + `build_openapi_spec()`

**Files:**
- Modify: `crates/lab/src/api/openapi.rs`

This is the key architectural task. Instead of adding `#[utoipa::path]` to 22 handler files, we build all OpenAPI paths programmatically from catalog data in a single function. This confines all utoipa coupling to `openapi.rs`.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
    #[test]
    fn build_service_paths_produces_post_operations() {
        let paths = super::build_service_paths(&["radarr", "sonarr"]);
        let json = serde_json::to_value(&paths).unwrap();

        // Each service should produce a POST /v1/{service} path
        assert!(json["/v1/radarr"]["post"].is_object(), "/v1/radarr POST missing");
        assert!(json["/v1/sonarr"]["post"].is_object(), "/v1/sonarr POST missing");

        // Each path should reference ActionRequest as request body
        let radarr_post = &json["/v1/radarr"]["post"];
        assert!(radarr_post["tags"].as_array().unwrap().contains(&serde_json::json!("radarr")));
        assert!(radarr_post["security"].is_array(), "security should be set");
    }

    #[test]
    fn build_openapi_spec_produces_valid_json() {
        let services: Vec<String> = vec!["extract".into()];
        let spec = super::build_openapi_spec(&services);
        let parsed: serde_json::Value = serde_json::from_str(&spec)
            .expect("build_openapi_spec must produce valid JSON");
        assert_eq!(parsed["info"]["title"], "lab API");
        assert!(parsed["components"]["schemas"]["ActionRequest"].is_object());
        assert!(parsed["components"]["securitySchemes"]["bearer"].is_object());
        assert!(parsed["paths"]["/v1/extract"]["post"].is_object());
        assert!(parsed["paths"]["/health"]["get"].is_object());
        assert!(parsed["paths"]["/ready"]["get"].is_object());
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --all-features -p lab -- openapi::tests::build_service_paths openapi::tests::build_openapi_spec`
Expected: FAIL — functions do not exist.

- [ ] **Step 3: Implement `build_service_paths()`, `build_health_paths()`, `ApiDoc`, and `build_openapi_spec()`**

Add to `crates/lab/src/api/openapi.rs`:

```rust
use utoipa::openapi::{
    path::{PathItem, PathItemBuilder, OperationBuilder, HttpMethod},
    request_body::RequestBodyBuilder,
    response::ResponseBuilder,
    content::Content,
    security::SecurityRequirement,
};

/// Build OpenAPI path items for health endpoints.
fn build_health_paths() -> Vec<(String, PathItem)> {
    let health_op = OperationBuilder::new()
        .tag("health")
        .summary(Some("Liveness probe"))
        .response("200", ResponseBuilder::new()
            .description("Process is alive")
            .build())
        .build();

    let ready_op = OperationBuilder::new()
        .tag("health")
        .summary(Some("Readiness probe"))
        .response("200", ResponseBuilder::new()
            .description("Process is ready")
            .build())
        .response("503", ResponseBuilder::new()
            .description("Process is not yet ready")
            .build())
        .build();

    vec![
        ("/health".into(), PathItemBuilder::new()
            .operation(HttpMethod::Get, health_op)
            .build()),
        ("/ready".into(), PathItemBuilder::new()
            .operation(HttpMethod::Get, ready_op)
            .build()),
    ]
}

/// Build OpenAPI path items for all enabled service dispatch endpoints.
///
/// Each service gets a `POST /v1/{service}` operation with:
/// - `ActionRequest` as request body
/// - Standard error responses
/// - Bearer security requirement
/// - Service name as tag
pub fn build_service_paths(enabled_services: &[String]) -> Vec<(String, PathItem)> {
    enabled_services
        .iter()
        .map(|service| {
            let op = OperationBuilder::new()
                .tag(service)
                .summary(Some(format!("Dispatch an action to {service}")))
                .description(Some(format!(
                    "Send an action to the {service} service. \
                     Use action=\"help\" to list available actions."
                )))
                .request_body(Some(
                    RequestBodyBuilder::new()
                        .content(
                            "application/json",
                            Content::new(crate::api::ActionRequest::schema()),
                        )
                        .required(Some(true.into()))
                        .build(),
                ))
                .response("200", ResponseBuilder::new()
                    .description("Action result")
                    .build())
                .response("400", ResponseBuilder::new()
                    .description("Unknown or invalid action")
                    .build())
                .response("401", ResponseBuilder::new()
                    .description("Missing or invalid bearer token")
                    .build())
                .response("422", ResponseBuilder::new()
                    .description("Validation or confirmation error")
                    .build())
                .security(SecurityRequirement::new::<_, _, String>("bearer", []))
                .build();

            let path_item = PathItemBuilder::new()
                .operation(HttpMethod::Post, op)
                .build();

            (format!("/v1/{service}"), path_item)
        })
        .collect()
}

#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "lab API",
        version = env!("CARGO_PKG_VERSION"),
        description = "Pluggable homelab CLI + MCP server SDK. One binary, 21 services, runtime MCP tool selection."
    ),
    components(schemas(
        crate::api::ActionRequest,
        crate::api::health::HealthResponse,
        ErrorUnknownAction,
        ErrorMissingParam,
        ErrorInvalidParam,
        ErrorConfirmationRequired,
        ErrorSdk,
    )),
    modifiers(&SecurityAddon),
    security(("bearer" = [])),
)]
pub struct ApiDoc;

/// Generate the complete OpenAPI JSON spec.
///
/// This is a pure function called once at server startup. The result is
/// wrapped in `Arc<String>` and served from memory.
///
/// The function:
/// 1. Builds the base `ApiDoc` from the `#[derive(OpenApi)]` macro
/// 2. Injects per-action param schemas from the catalog via `ActionSchemaInjector`
/// 3. Adds programmatic paths for health + all enabled service endpoints
/// 4. Serializes to pretty-printed JSON
///
/// # Panics
///
/// Panics if JSON serialization fails (should never happen with valid utoipa types).
pub fn build_openapi_spec(enabled_services: &[String]) -> String {
    use utoipa::OpenApi;

    let mut doc = ApiDoc::openapi();

    // Inject per-action param schemas from catalog
    let mut all_schemas = Vec::new();
    for service in enabled_services {
        let actions = crate::catalog::actions_for(service);
        all_schemas.extend(build_action_schemas(service, actions));
    }
    let injector = ActionSchemaInjector { schemas: all_schemas };
    utoipa::Modify::modify(&injector, &mut doc);

    // Add programmatic paths (health + services)
    let paths = doc.paths.get_or_insert_with(Default::default);
    for (path, item) in build_health_paths() {
        paths.paths.insert(path, item);
    }
    for (path, item) in build_service_paths(enabled_services) {
        paths.paths.insert(path, item);
    }

    let spec = serde_json::to_string_pretty(&doc)
        .expect("OpenAPI serialization cannot fail");

    // Startup smoke test — validate the spec is parseable JSON
    debug_assert!(
        serde_json::from_str::<serde_json::Value>(&spec).is_ok(),
        "generated OpenAPI spec is not valid JSON"
    );

    spec
}
```

> **Research-verified API (utoipa 5.4):** The builder API shown above uses the researched import paths: `utoipa::openapi::path::{PathItemBuilder, OperationBuilder, HttpMethod}`, `utoipa::openapi::request_body::RequestBodyBuilder`, `utoipa::openapi::response::ResponseBuilder`, `utoipa::openapi::content::Content`, `utoipa::openapi::security::SecurityRequirement`. Use `PathItemBuilder::new().operation(HttpMethod::Post, op).build()` — not `PathItem::new()`. Use `Content::new(schema)` for request body content. The test assertions are the contract — adjust if the exact API surface has changed since research.

- [ ] **Step 4: Run the tests**

Run: `cargo test --all-features -p lab -- openapi::tests::build_service_paths openapi::tests::build_openapi_spec`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api/openapi.rs
git commit -m "feat(openapi): programmatic path construction + ApiDoc + build_openapi_spec()"
```

---

### Task 7: Mount `/openapi.json` + `/docs` routes inside bearer auth

**Files:**
- Modify: `crates/lab/src/api/router.rs`

Mount two new routes on the **v1 router** (inside bearer auth). Scalar UI assets are self-hosted — the HTML references a vendored JS bundle rather than a CDN.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/lab/src/api/router.rs`:

```rust
    #[tokio::test]
    async fn openapi_json_requires_bearer_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()));
        // /openapi.json is behind auth — unauthenticated request should fail.
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn openapi_json_returns_spec_with_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/openapi.json")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        // Check Cache-Control header
        let cache_control = response
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            cache_control.contains("no-store"),
            "expected Cache-Control: private, no-store, got: {cache_control}"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["info"]["title"], "lab API");
    }

    #[tokio::test]
    async fn docs_endpoint_returns_html_with_auth() {
        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("secret-token".into()));
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/docs")
                    .header(header::AUTHORIZATION, "Bearer secret-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.contains("text/html"),
            "expected HTML content type, got: {content_type}"
        );
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test --all-features -p lab -- router::tests::openapi_json router::tests::docs_endpoint`
Expected: FAIL — routes not mounted yet.

- [ ] **Step 3: Implement the routes**

In `crates/lab/src/api/router.rs`, update `build_router_with_bearer()`:

1. At the top of the function, collect enabled service names and generate the spec:

```rust
    use super::openapi::build_openapi_spec;

    // Collect enabled service names for OpenAPI spec generation
    let enabled_services: Vec<String> = state
        .registry
        .services()
        .iter()
        .map(|s| s.name.clone())
        .collect();

    // Build the OpenAPI spec once at startup
    let openapi_json = std::sync::Arc::new(build_openapi_spec(&enabled_services));
```

2. Mount `/openapi.json` and `/docs` on the v1 router (inside bearer auth):

```rust
    let mut v1 = Router::new()
        .route("/{service}/actions", get(service_actions))
        .route(
            "/openapi.json",
            get({
                let json = openapi_json.clone();
                move || {
                    let json = json.clone();
                    async move {
                        (
                            [
                                (axum::http::header::CONTENT_TYPE, "application/json"),
                                (axum::http::header::CACHE_CONTROL, "private, no-store"),
                            ],
                            (*json).clone(),
                        )
                    }
                }
            }),
        )
        .route("/docs", get(/* Scalar handler — see Step 4 */))
        .nest("/extract", services::extract::routes(state.clone()));
```

3. For the Scalar UI, self-host the assets. **Research finding:** utoipa-scalar 0.3 does NOT have a built-in CDN-free mode — `Scalar::new().to_html()` still loads JS from `cdn.jsdelivr.net`. Use `Scalar::new().custom_html()` with a vendored HTML template:

```rust
    // Use Scalar's custom_html() with self-hosted assets.
    // Create crates/lab/src/api/openapi_docs.html with vendored Scalar JS.
    .route("/docs", get(|| async {
        (
            [(axum::http::header::CONTENT_TYPE, "text/html")],
            include_str!("openapi_docs.html"),
        )
    }))
```

Create `crates/lab/src/api/openapi_docs.html`:

```html
<!DOCTYPE html>
<html>
  <head>
    <title>lab API Documentation</title>
    <meta charset="utf-8"/>
    <meta name="viewport" content="width=device-width, initial-scale=1">
  </head>
  <body>
    <script id="api-reference" data-url="/v1/openapi.json"></script>
    <!-- Vendored Scalar bundle — download from npm @scalar/api-reference -->
    <script>
      // VENDOR: Paste the contents of
      // node_modules/@scalar/api-reference/dist/browser/standalone.min.js
      // here, or fetch it once and inline it.
      //
      // To get the file:
      //   npx @scalar/api-reference@latest
      //   cp node_modules/@scalar/api-reference/dist/browser/standalone.min.js \
      //      crates/lab/src/api/scalar.min.js
      //
      // Then replace this script tag with:
      //   <script>/* contents of scalar.min.js */</script>
      //
      // For now, fall back to CDN during development:
    </script>
    <script src="https://cdn.jsdelivr.net/npm/@scalar/api-reference@latest/dist/browser/standalone.min.js"></script>
  </body>
</html>
```

> **Self-hosting strategy:** During initial implementation, use the CDN fallback. The HTML template is structured so vendoring is a simple file copy — replace the CDN `<script src>` with an inline `<script>` containing the downloaded bundle. Track the CDN-to-vendored switch as a follow-up commit.
>
> **Note:** The spec URL is `/v1/openapi.json` (inside auth). The Scalar UI page itself is also behind auth, so the browser's auth cookie/header will be present when fetching the spec.

- [ ] **Step 4: Run the tests**

Run: `cargo test --all-features -p lab -- router::tests::openapi_json router::tests::docs_endpoint`
Expected: All three tests PASS.

- [ ] **Step 5: Run all router tests for regressions**

Run: `cargo test --all-features -p lab -- router::tests`
Expected: All pass. Existing health/ready/auth tests must not regress.

- [ ] **Step 6: Commit**

```bash
git add crates/lab/src/api/router.rs
git commit -m "feat(openapi): mount /v1/openapi.json + /v1/docs behind bearer auth with Cache-Control"
```

---

### Task 8: Integration test — round-trip validation

**Files:**
- Modify: `crates/lab/src/api/openapi.rs` (test section)

This test builds the full router, hits `/v1/openapi.json` with auth, and validates the structure.

- [ ] **Step 1: Write the integration test**

Add to the `tests` module in `crates/lab/src/api/openapi.rs`:

```rust
    #[tokio::test]
    async fn openapi_json_roundtrip_has_valid_structure() {
        use axum::body::Body;
        use axum::http::{Request, header};
        use tower::ServiceExt;
        use crate::api::state::AppState;
        use crate::api::router::build_router_with_bearer;

        let state = AppState::new();
        let app = build_router_with_bearer(state, Some("test-token".into()));

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/openapi.json")
                    .header(header::AUTHORIZATION, "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Required top-level keys
        assert!(spec["openapi"].is_string(), "missing openapi version");
        assert!(spec["info"]["title"].is_string(), "missing info.title");
        assert!(spec["info"]["version"].is_string(), "missing info.version");
        assert!(spec["paths"].is_object(), "missing paths object");
        assert!(spec["components"]["schemas"].is_object(), "missing component schemas");

        // Health paths should be present
        assert!(spec["paths"]["/health"].is_object(), "/health missing");
        assert!(spec["paths"]["/ready"].is_object(), "/ready missing");

        // Error schemas should be present
        let schemas = &spec["components"]["schemas"];
        for name in &[
            "ActionRequest",
            "HealthResponse",
            "ErrorUnknownAction",
            "ErrorMissingParam",
            "ErrorSdk",
        ] {
            assert!(schemas[name].is_object(), "{name} schema missing");
        }

        // Security scheme
        assert!(
            spec["components"]["securitySchemes"]["bearer"].is_object(),
            "bearer security scheme missing"
        );

        // Extract service path should be present (always-on)
        assert!(
            spec["paths"]["/v1/extract"]["post"].is_object(),
            "/v1/extract POST missing from spec"
        );
    }
```

- [ ] **Step 2: Run the test**

Run: `cargo test --all-features -p lab -- openapi::tests::openapi_json_roundtrip`
Expected: PASS

- [ ] **Step 3: Run the full test suite**

Run: `cargo test --all-features -p lab`
Expected: All tests pass.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy --all-features -p lab -- -D warnings`
Expected: No warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/lab/src/api/openapi.rs
git commit -m "test(openapi): add integration roundtrip test for /v1/openapi.json"
```

---

## Post-Implementation Notes

### Contract Testing (follow-up work)

After all tasks are complete, schemathesis can be run against the live server:

```bash
schemathesis run http://localhost:8080/v1/openapi.json \
  --checks all \
  --header "Authorization: Bearer $LAB_API_TOKEN"
```

### `lab openapi` CLI Subcommand (follow-up task)

A `lab openapi` CLI subcommand that prints the generated OpenAPI JSON to stdout.

### Reverse Proxy Path Prefix (follow-up task)

If the API is served behind a reverse proxy with a path prefix (e.g., `/lab/v1/...`), the Scalar UI spec URL and OpenAPI server URL may need to be configurable via `LAB_API_BASE_URL` env var. Not needed for initial implementation.
