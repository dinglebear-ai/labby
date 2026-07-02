//! Spec → allowlisted operation descriptors, using `rmcp-openapi`'s PARSE-ONLY
//! surface (`Spec::from_value` + `Spec::to_tool_metadata`). This module NEVER
//! constructs an `rmcp_openapi::HttpClient` and NEVER calls `Tool::call`/
//! `Tool::execute`/`generate_openapi_tools` — the outbound HTTP is owned by
//! `http.rs`.
//!
//! Note: `rmcp-openapi` v0.31.2 hardcodes `ToolMetadata.security = None`
//! (a library TODO), so no per-operation security scheme is available here.
//! Credential injection is driven entirely by our own `OpenApiCredential`
//! config in `http::execute_operation`.

use crate::error::OpenApiError;

/// A single dispatchable operation derived from a spec. The `operation_id` is the
/// RAW `ToolMetadata.name` (operationId, or rmcp-openapi's `{METHOD}_{path}`
/// fallback when the spec omits one) — the SAME key used by the allowlist and
/// path-template substitution.
#[derive(Debug, Clone)]
pub struct OperationDescriptor {
    /// Raw operationId (allowlist + dispatch key).
    pub operation_id: String,
    /// HTTP method.
    pub method: reqwest::Method,
    /// Path template, e.g. `/users/{id}`.
    pub path_template: String,
}

/// Parse a spec into allowlisted operation descriptors. Deny-by-default on the
/// RAW operationId.
///
/// # Errors
/// Returns [`OpenApiError::SpecParse`] if the document is not valid JSON or the
/// spec cannot be parsed. `label` is threaded in so the error is attributable.
pub fn convert_spec(
    label: &str,
    spec_json: &str,
    allowed: &[String],
) -> Result<Vec<OperationDescriptor>, OpenApiError> {
    let value: serde_json::Value =
        serde_json::from_str(spec_json).map_err(|_| OpenApiError::SpecParse {
            label: label.to_string(),
        })?;
    let spec = rmcp_openapi::Spec::from_value(value).map_err(|_| OpenApiError::SpecParse {
        label: label.to_string(),
    })?;
    // Parse-only: to_tool_metadata does NOT build an HttpClient.
    let metadata = spec
        .to_tool_metadata(None, false, false, false)
        .map_err(|_| OpenApiError::SpecParse {
            label: label.to_string(),
        })?;

    let mut out = Vec::new();
    for m in metadata {
        // `m.name` is the raw operationId (or rmcp-openapi's method_path fallback).
        if !allowed.iter().any(|a| a == &m.name) {
            continue; // deny-by-default
        }
        let method = parse_method(&m.method);
        out.push(OperationDescriptor {
            operation_id: m.name,
            method,
            path_template: m.path,
        });
    }
    Ok(out)
}

/// Map an OpenAPI method string (upper-case) to a `reqwest::Method`. Unknown
/// verbs fall back to `GET` — the allowlist bounds which operations can even be
/// dispatched, and rmcp-openapi only emits the standard verbs.
fn parse_method(raw: &str) -> reqwest::Method {
    raw.parse::<reqwest::Method>()
        .unwrap_or(reqwest::Method::GET)
}

#[cfg(test)]
mod tests {
    const FIXTURE_SPEC: &str = r#"{
        "openapi": "3.0.0",
        "info": { "title": "Fixture", "version": "1.0.0" },
        "paths": {
            "/users/{id}": {
                "get": {
                    "operationId": "getUser",
                    "parameters": [
                        { "name": "id", "in": "path", "required": true,
                          "schema": { "type": "string" } }
                    ],
                    "responses": { "200": { "description": "ok" } }
                },
                "delete": {
                    "operationId": "deleteUser",
                    "parameters": [
                        { "name": "id", "in": "path", "required": true,
                          "schema": { "type": "string" } }
                    ],
                    "responses": { "204": { "description": "gone" } }
                }
            }
        }
    }"#;

    #[test]
    fn allowlist_filters_on_raw_operation_id() {
        let ops = super::convert_spec("vendor", FIXTURE_SPEC, &["getUser".to_string()]).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].operation_id, "getUser");
        assert_eq!(ops[0].method, reqwest::Method::GET);
        assert_eq!(ops[0].path_template, "/users/{id}");
        assert!(
            !ops.iter().any(|o| o.operation_id == "deleteUser"),
            "deny-by-default"
        );
    }

    #[test]
    fn empty_allowlist_denies_everything() {
        let ops = super::convert_spec("vendor", FIXTURE_SPEC, &[]).unwrap();
        assert!(ops.is_empty(), "deny-by-default with no allowlist");
    }

    #[test]
    fn invalid_json_is_spec_parse_error() {
        let err = super::convert_spec("vendor", "not json", &["getUser".into()]).unwrap_err();
        assert_eq!(err.kind(), "config_error");
    }
}
