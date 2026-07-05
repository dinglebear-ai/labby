//! Spec → allowlisted operation descriptors. This module only extracts the
//! dispatch metadata Labby needs: operationId, HTTP method, and path template.

use crate::error::OpenApiError;

/// A single dispatchable operation derived from a spec. The `operation_id` is the
/// raw OpenAPI operationId — the SAME key used by the allowlist and
/// path-template substitution. Operations without an operationId are skipped.
#[derive(Debug, Clone)]
pub struct OperationDescriptor {
    /// Raw operationId (allowlist + dispatch key).
    pub operation_id: String,
    /// HTTP method.
    pub method: reqwest::Method,
    /// Path template, e.g. `/users/{id}`.
    pub path_template: String,
}

const HTTP_METHOD_KEYS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Parse a spec into allowlisted operation descriptors. Deny-by-default on the
/// RAW operationId.
///
/// # Errors
/// Returns [`OpenApiError::SpecParse`] if the document is not valid JSON or does
/// not contain a top-level `paths` object. `label` is threaded in so the error is
/// attributable.
pub fn convert_spec(
    label: &str,
    spec_json: &str,
    allowed: &[String],
) -> Result<Vec<OperationDescriptor>, OpenApiError> {
    let value: serde_json::Value =
        serde_json::from_str(spec_json).map_err(|_| OpenApiError::SpecParse {
            label: label.to_string(),
        })?;

    let paths = value
        .get("paths")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| OpenApiError::SpecParse {
            label: label.to_string(),
        })?;

    let mut out = Vec::new();
    for (path_template, path_item) in paths {
        let Some(path_item) = path_item.as_object() else {
            continue;
        };
        for method_key in HTTP_METHOD_KEYS {
            let Some(operation) = path_item
                .get(*method_key)
                .and_then(serde_json::Value::as_object)
            else {
                continue;
            };
            let Some(operation_id) = operation
                .get("operationId")
                .and_then(serde_json::Value::as_str)
            else {
                continue;
            };
            if !allowed.iter().any(|a| a == operation_id) {
                continue; // deny-by-default
            }
            let method = parse_method(label, operation_id, method_key);
            out.push(OperationDescriptor {
                operation_id: operation_id.to_string(),
                method,
                path_template: path_template.clone(),
            });
        }
    }
    Ok(out)
}

/// Map an OpenAPI method string to a `reqwest::Method`. `Method::from_str`
/// accepts any valid HTTP token (including extension verbs), so this only falls
/// back to `GET` for a genuinely malformed token. The fallback is WARN-logged so
/// a malformed spec verb is diagnosable rather than silently dispatched as GET;
/// the allowlist still bounds dispatch.
fn parse_method(label: &str, operation_id: &str, raw: &str) -> reqwest::Method {
    raw.to_ascii_uppercase()
        .parse::<reqwest::Method>()
        .unwrap_or_else(|_| {
            tracing::warn!(
                service = "openapi",
                label = %label,
                operation = %operation_id,
                "openapi: unparseable HTTP method in spec — falling back to GET"
            );
            reqwest::Method::GET
        })
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
                },
                "parameters": [
                    { "name": "id", "in": "path", "required": true,
                      "schema": { "type": "string" } }
                ]
            },
            "/health": {
                "get": {
                    "responses": { "200": { "description": "ok" } }
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

    #[test]
    fn missing_operation_id_is_skipped() {
        let ops = super::convert_spec("vendor", FIXTURE_SPEC, &["GET_/health".to_string()])
            .expect("valid spec");
        assert!(ops.is_empty());
    }

    #[test]
    fn missing_paths_is_spec_parse_error() {
        let err = super::convert_spec(
            "vendor",
            r#"{ "openapi": "3.0.0", "info": { "title": "No paths", "version": "1.0.0" } }"#,
            &["getUser".into()],
        )
        .unwrap_err();
        assert_eq!(err.kind(), "config_error");
    }
}
