//! Tests: tool-id parsing, tool scope, schema validation, tool descriptor.
#![cfg(test)]
#![allow(clippy::panic)]

use std::collections::BTreeMap;

use serde_json::json;

use crate::error::ToolError;
use crate::snippet::store::{SnippetInfo, SnippetInputSpec, SnippetInputType, SnippetSource};
use crate::types::{CodeModeToolId, CodeModeToolRef, ToolDescriptor, ToolScope};

use super::protocol::CodeModeRunnerOutput;
use super::schema::validate_code_mode_params_against_schema;

#[test]
fn local_provider_ids_are_detected_before_upstream_ids() {
    let state = crate::local_provider::try_parse_local_provider_call("state::readFile")
        .expect("parse succeeds")
        .expect("state provider detected");
    assert_eq!(state.provider.as_str(), "state");
    assert_eq!(state.method, "readFile");

    let git = crate::local_provider::try_parse_local_provider_call("git::status")
        .expect("parse succeeds")
        .expect("git provider detected");
    assert_eq!(git.provider.as_str(), "git");
    assert_eq!(git.method, "status");

    assert!(
        crate::local_provider::try_parse_local_provider_call("movie::search")
            .expect("ordinary upstream id is valid")
            .is_none()
    );
}

#[test]
fn local_provider_ids_reject_bad_methods() {
    let err = crate::local_provider::try_parse_local_provider_call("state::")
        .expect_err("empty local method is rejected");
    assert_eq!(err.kind(), "invalid_param");
}

#[test]
fn parses_openapi_provider_call() {
    use crate::local_provider::LocalProviderName;
    let c = crate::local_provider::try_parse_local_provider_call("openapi::vendor.getUser")
        .expect("parse succeeds")
        .expect("openapi provider detected");
    assert_eq!(c.provider, LocalProviderName::Openapi);
    assert_eq!(c.method, "vendor.getUser");
}

#[test]
fn openapi_preserves_dotted_operation_id() {
    use crate::local_provider::LocalProviderName;
    let c = crate::local_provider::try_parse_local_provider_call("openapi::vendor.pets.list")
        .expect("parse succeeds")
        .expect("openapi provider detected");
    assert_eq!(c.provider, LocalProviderName::Openapi);
    assert_eq!(c.method, "vendor.pets.list");
}

#[test]
fn openapi_is_reserved() {
    assert!(crate::local_provider::is_reserved_provider_namespace(
        "openapi"
    ));
}

#[test]
fn artifact_write_protocol_round_trips() {
    let output = CodeModeRunnerOutput::ArtifactWrite {
        seq: 7,
        path: "axon/brief.md".to_string(),
        content: "# Brief".to_string(),
        content_type: Some("text/markdown".to_string()),
    };

    let encoded = serde_json::to_string(&output).expect("serialize protocol");
    assert_eq!(
        encoded,
        r##"{"type":"artifact_write","seq":7,"path":"axon/brief.md","content":"# Brief","content_type":"text/markdown"}"##
    );

    let decoded: CodeModeRunnerOutput =
        serde_json::from_str(&encoded).expect("deserialize protocol");
    assert_eq!(decoded, output);
}

#[test]
fn snippet_catalog_entry_projects_to_codemode_run() {
    let info = SnippetInfo {
        name: "repo-summary".to_string(),
        description: Some("Summarize repo health".to_string()),
        tags: vec!["repo".to_string()],
        inputs: Default::default(),
        tools: Vec::new(),
        source: SnippetSource::User,
        path: "repo-summary.md".into(),
        shadowed: false,
    };
    let entry = ToolDescriptor::snippet(&info);
    assert_eq!(entry.kind, crate::types::CodeModeCatalogKind::Snippet);
    assert_eq!(entry.id, "snippet::repo-summary");
    assert_eq!(entry.namespace, "snippet");
    assert!(entry.signature.contains("codemode.run"));
    assert!(entry.dts.is_empty());
    assert_eq!(entry.safety, None);
    assert!(
        serde_json::to_value(&entry)
            .expect("serialize snippet descriptor")
            .get("safety")
            .is_none(),
        "composite snippets must not receive static tool safety"
    );

    let discovery = crate::types::CodeModeDiscoveryEntry::from_catalog(&entry);
    assert_eq!(discovery.kind, crate::types::CodeModeCatalogKind::Snippet);
    assert_eq!(discovery.path, "snippet.repo-summary");
    assert_eq!(discovery.helper, "codemode.run(\"repo-summary\", input)");
}

#[test]
fn snippet_catalog_json_input_schema_allows_any_json_value() {
    let mut inputs = BTreeMap::new();
    inputs.insert(
        "payload".to_string(),
        SnippetInputSpec {
            ty: SnippetInputType::Json,
            required: true,
            default: None,
            description: Some("Raw payload".to_string()),
        },
    );
    let info = SnippetInfo {
        name: "json-snippet".to_string(),
        description: None,
        tags: Vec::new(),
        inputs,
        tools: Vec::new(),
        source: SnippetSource::User,
        path: "json-snippet.md".into(),
        shadowed: false,
    };

    let entry = ToolDescriptor::snippet(&info);
    let schema = entry.schema.expect("snippet schema");
    let payload = &schema["properties"]["payload"];
    assert!(payload.get("type").is_none(), "{payload}");
    assert_eq!(payload["description"], "Raw payload");
    assert_eq!(schema["required"], json!(["payload"]));
}

#[test]
fn parse_rejects_lab_id() {
    let err = CodeModeToolId::parse("lab::gateway.status.get").expect_err("lab:: ids are rejected");
    match err {
        ToolError::Sdk { sdk_kind, message } => {
            assert_eq!(sdk_kind, "unknown_tool");
            assert!(message.contains("lab::"));
            // Message points callers at the native Lab service tool, not back
            // through Code Mode.
            assert!(message.contains("native Lab service tool"));
            assert!(message.contains("gateway"));
        }
        other => panic!("expected unknown_tool, got {other:?}"),
    }
}

#[test]
fn parses_namespaced_tool_id() {
    let parsed = CodeModeToolId::parse("github::search_issues").unwrap();
    assert_eq!(
        parsed,
        CodeModeToolId {
            raw: "github::search_issues".to_string(),
            reference: CodeModeToolRef::Tool {
                namespace: "github".to_string(),
                tool: "search_issues".to_string(),
            },
        }
    );
}

#[test]
fn rejects_invalid_ids() {
    for id in [
        "",
        "a.a.schema",
        "lab::native",
        "github",
        "::tool",
        "ns::github::search_issues",
    ] {
        assert!(CodeModeToolId::parse(id).is_err(), "{id} should be invalid");
    }
}

#[test]
fn tool_scope_allows_only_selected_namespaces_and_tools() {
    let filter = ToolScope::new(
        vec!["github".to_string()],
        vec!["github::search_issues".to_string()],
    );

    assert!(filter.allows("github", "search_issues"));
    assert!(!filter.allows("github", "delete_repo"));
    assert!(!filter.allows("docker", "search_issues"));
}

#[test]
fn capability_filter_fingerprint_is_structured_and_collision_resistant() {
    let first = ToolScope::new(
        vec!["a,b".to_string(), "c".to_string()],
        vec!["x".to_string()],
    );
    let second = ToolScope::new(
        vec!["a".to_string(), "b,c".to_string()],
        vec!["x".to_string()],
    );

    assert_ne!(first.fingerprint(), second.fingerprint());
    assert!(serde_json::from_str::<serde_json::Value>(&first.fingerprint()).is_ok());
}

#[test]
fn capability_filter_fingerprint_separates_read_only_from_full_access() {
    let full = ToolScope::new(vec!["github".to_string()], Vec::new());
    let read_only = full.clone().read_only();

    assert!(!full.is_read_only());
    assert!(read_only.is_read_only());
    assert_ne!(full.fingerprint(), read_only.fingerprint());
}

#[test]
fn scoped_tool_scope_with_empty_namespaces_denies_all_tool_calls() {
    let filter = ToolScope::scoped_namespaces(Vec::new(), Vec::new());

    assert!(!filter.allows("github", "search_issues"));
    assert!(!filter.allows("docker", "containers"));
}

#[test]
fn validates_code_mode_params_against_input_schema() {
    let schema = json!({
        "type": "object",
        "properties": {
            "query": { "type": "string" },
            "limit": { "type": "integer" }
        },
        "required": ["query"]
    });

    validate_code_mode_params_against_schema(&json!({"query": "rust", "limit": 10}), Some(&schema))
        .expect("valid params pass");

    let missing = validate_code_mode_params_against_schema(&json!({}), Some(&schema))
        .expect_err("missing required field fails");
    assert_eq!(missing.kind(), "missing_param");

    let invalid = validate_code_mode_params_against_schema(&json!({"query": 42}), Some(&schema))
        .expect_err("wrong field type fails");
    assert_eq!(invalid.kind(), "invalid_param");
}

#[test]
fn validates_code_mode_params_recursively_against_schema() {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "state": { "enum": ["open", "closed"] },
            "limit": { "type": ["integer", "null"], "minimum": 1, "maximum": 100 },
            "labels": { "type": "array", "items": { "type": "string" } },
            "owner": {
                "type": "object",
                "properties": { "login": { "type": "string" } },
                "required": ["login"],
                "additionalProperties": false
            }
        },
        "required": ["state", "owner"]
    });

    validate_code_mode_params_against_schema(
        &json!({
            "state": "open",
            "limit": null,
            "labels": ["bug"],
            "owner": {"login": "octo"}
        }),
        Some(&schema),
    )
    .expect("valid nested params pass");

    for params in [
        json!({"state": "merged", "owner": {"login": "octo"}}),
        json!({"state": "open", "owner": {"login": "octo", "extra": true}}),
        json!({"state": "open", "owner": {}, "labels": ["bug"]}),
        json!({"state": "open", "owner": {"login": "octo"}, "labels": [1]}),
        json!({"state": "open", "owner": {"login": "octo"}, "limit": 0}),
        json!({"state": "open", "owner": {"login": "octo"}, "extra": true}),
    ] {
        let err = validate_code_mode_params_against_schema(&params, Some(&schema))
            .expect_err("invalid nested params fail");
        assert_eq!(err.kind(), "invalid_param", "{params}");
    }
}

#[test]
fn validates_code_mode_params_through_local_refs_and_constraints() {
    let schema = json!({
        "$ref": "#/$defs/Params",
        "$defs": {
            "Params": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "minLength": 2,
                        "maxLength": 5,
                        "pattern": "^[a-z]+$"
                    },
                    "tags": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 2,
                        "uniqueItems": true,
                        "items": { "type": "string" }
                    },
                    "meta": {
                        "type": "object",
                        "properties": {
                            "known": { "type": "string" }
                        },
                        "additionalProperties": { "type": "integer" }
                    },
                    "flag": {
                        "oneOf": [
                            { "type": "string", "const": "on" },
                            { "type": "boolean" }
                        ]
                    },
                    "labels": {
                        "type": "object",
                        "patternProperties": {
                            "^x-": { "type": "string" }
                        },
                        "additionalProperties": false
                    }
                },
                "required": ["query", "tags", "flag"]
            }
        }
    });

    validate_code_mode_params_against_schema(
        &json!({
            "query": "abc",
            "tags": ["one", "two"],
            "meta": {"known": "ok", "count": 1},
            "flag": true,
            "labels": {"x-owner": "me"}
        }),
        Some(&schema),
    )
    .expect("valid params through local ref pass");

    for params in [
        json!({"tags": ["one"], "flag": true}),
        json!({"query": "a", "tags": ["one"], "flag": true}),
        json!({"query": "abcdef", "tags": ["one"], "flag": true}),
        json!({"query": "ABC", "tags": ["one"], "flag": true}),
        json!({"query": "abc", "tags": [], "flag": true}),
        json!({"query": "abc", "tags": ["one", "two", "three"], "flag": true}),
        json!({"query": "abc", "tags": ["one", "one"], "flag": true}),
        json!({"query": "abc", "tags": ["one"], "flag": 1}),
        json!({"query": "abc", "tags": ["one"], "flag": true, "meta": {"count": "one"}}),
        json!({"query": "abc", "tags": ["one"], "flag": true, "labels": {"owner": "me"}}),
    ] {
        let err = validate_code_mode_params_against_schema(&params, Some(&schema))
            .expect_err("invalid params fail through local ref");
        assert!(
            matches!(err.kind(), "missing_param" | "invalid_param"),
            "{params}: {err}"
        );
    }
}

#[test]
fn code_mode_schema_validator_ignores_annotations_but_enforces_supported_assertions() {
    let schema = json!({
        "title": "Search params",
        "description": "Common MCP schema annotations are documentation only.",
        "default": { "query": "rust" },
        "examples": [{ "query": "rust" }],
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "Search text",
                "default": "rust",
                "examples": ["rust"]
            }
        },
        "required": ["query"],
        "additionalProperties": false
    });

    validate_code_mode_params_against_schema(&json!({"query": "rust"}), Some(&schema))
        .expect("annotations must not affect validation");

    let err = validate_code_mode_params_against_schema(
        &json!({"query": "rust", "extra": true}),
        Some(&schema),
    )
    .expect_err("supported assertions are still enforced");
    assert_eq!(err.kind(), "invalid_param");
}

#[test]
fn code_mode_schema_validator_enforces_if_then_else_and_not() {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "mode": { "enum": ["basic", "advanced"] },
            "advanced": { "type": "boolean" }
        },
        "required": ["mode"],
        "if": {
            "properties": { "mode": { "const": "advanced" } },
            "required": ["mode"]
        },
        "then": {
            "required": ["advanced"]
        },
        "else": {
            "not": { "required": ["advanced"] }
        },
        "dependentRequired": {
            "mode": ["advanced"]
        }
    });

    validate_code_mode_params_against_schema(
        &json!({"mode": "advanced", "advanced": true}),
        Some(&schema),
    )
    .expect("matching then branch passes");
    validate_code_mode_params_against_schema(&json!({"mode": "basic"}), Some(&schema))
        .expect("matching else branch passes and unsupported dependentRequired stays ignored");

    let missing =
        validate_code_mode_params_against_schema(&json!({"mode": "advanced"}), Some(&schema))
            .expect_err("then branch requires advanced");
    assert_eq!(missing.kind(), "missing_param");

    let forbidden = validate_code_mode_params_against_schema(
        &json!({"mode": "basic", "advanced": false}),
        Some(&schema),
    )
    .expect_err("else branch forbids advanced");
    assert_eq!(forbidden.kind(), "invalid_param");
}

#[test]
fn code_mode_schema_validator_rejects_cortex_action_field_mismatches() {
    let schema = json!({
        "type": "object",
        "properties": {
            "action": {
                "enum": ["project_context", "list_ai_projects"]
            },
            "project": { "type": "string" },
            "tool": { "type": "string" },
            "limit": { "type": "integer" },
            "since": { "type": "string" },
            "until": { "type": "string" }
        },
        "required": ["action"],
        "additionalProperties": false,
        "allOf": [
            {
                "if": {
                    "properties": { "action": { "const": "project_context" } },
                    "required": ["action"]
                },
                "then": {
                    "required": ["project"],
                    "not": {
                        "anyOf": [
                            { "required": ["since"] },
                            { "required": ["until"] }
                        ]
                    }
                }
            },
            {
                "if": {
                    "properties": { "action": { "const": "list_ai_projects" } },
                    "required": ["action"]
                },
                "then": {
                    "not": { "required": ["limit"] }
                }
            }
        ]
    });

    for params in [
        json!({"action": "project_context", "project": "/repo", "limit": 5}),
        json!({"action": "list_ai_projects", "since": "2026-08-01T00:00:00Z"}),
    ] {
        validate_code_mode_params_against_schema(&params, Some(&schema))
            .expect("valid Cortex action shape passes");
    }

    for params in [
        json!({"action": "project_context", "project": "/repo", "since": "2026-08-01T00:00:00Z"}),
        json!({"action": "project_context", "project": "/repo", "until": "2026-08-02T00:00:00Z"}),
        json!({"action": "list_ai_projects", "limit": 20}),
    ] {
        let error = validate_code_mode_params_against_schema(&params, Some(&schema))
            .expect_err("invalid Cortex action fields fail before dispatch");
        assert_eq!(error.kind(), "invalid_param", "{params}: {error}");
    }
}

#[test]
fn code_mode_schema_validator_enforces_boolean_schemas() {
    validate_code_mode_params_against_schema(&json!({"value": 1}), Some(&json!(true)))
        .expect("true schema accepts every value");
    let error = validate_code_mode_params_against_schema(&json!({"value": 1}), Some(&json!(false)))
        .expect_err("false schema rejects every value");
    assert_eq!(error.kind(), "invalid_param");
}

fn schema_error_message(error: ToolError) -> String {
    match error {
        ToolError::Sdk { message, .. } => message,
        other => panic!("expected sdk error, got {other:?}"),
    }
}

#[test]
fn code_mode_schema_validator_rejects_deep_ref_chain_bomb_quickly() {
    // ~100 chained definitions, each fanning out through `oneOf` into the next.
    // Without the depth/budget guard this explores ~2^100 schema paths (the
    // per-branch `seen_refs` clone never sees a cycle on a linear chain).
    let mut defs = serde_json::Map::new();
    for i in 0..100 {
        let next = format!("#/$defs/d{}", i + 1);
        defs.insert(
            format!("d{i}"),
            json!({ "oneOf": [ { "$ref": next }, { "$ref": next } ] }),
        );
    }
    defs.insert("d100".to_string(), json!({ "type": "string" }));
    let schema = json!({ "$ref": "#/$defs/d0", "$defs": defs });

    let start = std::time::Instant::now();
    let error = validate_code_mode_params_against_schema(&json!(7), Some(&schema))
        .expect_err("schema bomb must be rejected, not explored");
    assert_eq!(error.kind(), "invalid_param");
    let message = schema_error_message(error);
    assert!(
        message.contains("nesting depth") || message.contains("work budget"),
        "expected a depth/budget rejection, got: {message}"
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "rejection must be fast, took {:?}",
        start.elapsed()
    );
}

#[test]
fn code_mode_schema_validator_rejects_wide_fanout_bomb_via_visit_budget() {
    // 12 levels of 4-way `anyOf` fan-out = 4^12 ≈ 16.7M schema paths, but only
    // ~25 recursion frames deep — under the depth cap, so this proves the
    // visit budget stops width-driven blowups the depth guard cannot see.
    let mut defs = serde_json::Map::new();
    for i in 0..12 {
        let next = format!("#/$defs/d{}", i + 1);
        defs.insert(
            format!("d{i}"),
            json!({ "anyOf": [
                { "$ref": next }, { "$ref": next }, { "$ref": next }, { "$ref": next }
            ] }),
        );
    }
    // The leaf mismatches the value so every path is fully explored.
    defs.insert("d12".to_string(), json!({ "type": "string" }));
    let schema = json!({ "$ref": "#/$defs/d0", "$defs": defs });

    let start = std::time::Instant::now();
    let error = validate_code_mode_params_against_schema(&json!(7), Some(&schema))
        .expect_err("fan-out bomb must exhaust the visit budget");
    assert_eq!(error.kind(), "invalid_param");
    let message = schema_error_message(error);
    assert!(
        message.contains("work budget"),
        "expected a work-budget rejection, got: {message}"
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "rejection must be fast, took {:?}",
        start.elapsed()
    );
}

#[test]
fn code_mode_schema_validator_fails_closed_on_defective_not_subschema() {
    // A `not` wrapping an unsupported external $ref must be a structured
    // rejection — never a silent accept (the defective subschema previously
    // read as "did not match", satisfying `not`).
    let schema = json!({ "not": { "$ref": "https://external.example/schema.json" } });
    let error = validate_code_mode_params_against_schema(&json!({"x": 1}), Some(&schema))
        .expect_err("defective not subschema must fail closed");
    assert_eq!(error.kind(), "invalid_param");
    assert!(schema_error_message(error).contains("non-local $ref"));
}

#[test]
fn code_mode_schema_validator_fails_closed_on_defective_if_subschema() {
    // A defective `if` condition must not silently route to `else`.
    let schema = json!({
        "if": { "$ref": "#/$defs/missing" },
        "then": { "required": ["a"] },
        "else": { "required": ["b"] }
    });
    let error = validate_code_mode_params_against_schema(&json!({"b": 1}), Some(&schema))
        .expect_err("defective if subschema must fail closed");
    assert_eq!(error.kind(), "invalid_param");
    assert!(schema_error_message(error).contains("unresolved local $ref"));
}

#[test]
fn code_mode_schema_validator_rejects_malformed_subschemas() {
    // A schema position holding a non-object, non-boolean value is a schema
    // defect, mirroring the false-boolean-schema treatment.
    let nested = json!({ "properties": { "a": 42 } });
    let error = validate_code_mode_params_against_schema(&json!({"a": 1}), Some(&nested))
        .expect_err("malformed property subschema must fail closed");
    assert_eq!(error.kind(), "invalid_param");
    assert!(schema_error_message(error).contains("malformed"));

    let top_level = json!("not a schema");
    let error = validate_code_mode_params_against_schema(&json!({"a": 1}), Some(&top_level))
        .expect_err("malformed top-level schema must fail closed");
    assert_eq!(error.kind(), "invalid_param");
    assert!(schema_error_message(error).contains("malformed"));
}

#[test]
fn code_mode_schema_validator_honors_openapi_nullable() {
    let schema = json!({
        "type": "object",
        "properties": {
            "note": { "type": "string", "nullable": true },
            "strict": { "type": "string" }
        }
    });

    for params in [
        json!({"note": null}),
        json!({"note": "hello"}),
        json!({"strict": "hello"}),
    ] {
        validate_code_mode_params_against_schema(&params, Some(&schema))
            .expect("nullable widens the declared type to include null");
    }

    let error = validate_code_mode_params_against_schema(&json!({"strict": null}), Some(&schema))
        .expect_err("null still fails a non-nullable typed property");
    assert_eq!(error.kind(), "invalid_param");
}

#[test]
fn code_mode_schema_validator_leaves_normal_schemas_unaffected_by_guards() {
    // A representative real-world nested schema stays accepted under the new
    // depth/budget guards.
    let schema = json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": { "name": { "type": "string" } },
                    "required": ["name"]
                }
            }
        },
        "required": ["items"]
    });
    let params = json!({
        "items": (0..500).map(|i| json!({"name": format!("n{i}")})).collect::<Vec<_>>()
    });
    validate_code_mode_params_against_schema(&params, Some(&schema))
        .expect("large but ordinary params stay within the visit budget");
}

#[test]
fn builds_catalog_entry_for_tool() {
    let candidate = ToolDescriptor::tool(
        "github",
        "search_issues",
        "Search issues",
        Some(json!({
            "type": "object",
            "properties": {
                "q": {
                    "type": "string",
                    "description": "Search query"
                }
            },
            "required": ["q"]
        })),
        Some(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" }
                        }
                    }
                }
            }
        })),
    );
    assert_eq!(candidate.id, "github::search_issues");
    assert_eq!(candidate.namespace, "github");
    assert_eq!(candidate.name, "search_issues");
    assert_eq!(
        candidate.output_schema,
        Some(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" }
                        }
                    }
                }
            }
        }))
    );
    assert!(
        candidate
            .signature
            .contains("codemode.github.search_issues")
    );
    assert!(candidate.signature.contains("GithubSearchIssuesInput"));
    assert!(candidate.signature.contains("GithubSearchIssuesOutput"));
    assert!(candidate.dts.contains("type GithubSearchIssuesInput"));
    assert!(candidate.dts.contains("/** Search query */"));
    assert!(candidate.dts.contains("q: string;"));
    assert!(candidate.dts.contains("title?: string;"));
    assert!(
        candidate
            .dts
            .contains("declare function callTool(id: \"github::search_issues\"")
    );
    assert!(
        candidate
            .dts
            .contains("project, filter, or slice large tool results")
    );
}
