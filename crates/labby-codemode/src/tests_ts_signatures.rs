//! Tests for `ts_signatures` (TypeScript type generation from JSON Schema).
#![cfg(test)]

use serde_json::json;

#[test]
fn json_schema_to_type_handles_refs_unions_arrays_and_required_properties() {
    let schema = json!({
        "$defs": {
            "Issue": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "number": { "type": "integer" }
                },
                "required": ["title"]
            }
        },
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": { "$ref": "#/$defs/Issue" }
            },
            "state": {
                "enum": ["open", "closed"]
            },
            "cursor": {
                "anyOf": [{ "type": "string" }, { "type": "null" }]
            }
        },
        "required": ["items"]
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(ts.contains("items: Array<{"));
    assert!(ts.contains("title: string;"));
    assert!(ts.contains("number?: number;"));
    assert!(ts.contains("state?: \"open\" | \"closed\";"));
    assert!(ts.contains("cursor?: string | null;"));
}

#[test]
fn json_schema_to_type_matches_cloudflare_edge_cases() {
    let schema = json!({
        "type": "object",
        "properties": {
            "tuple": {
                "type": "array",
                "items": [{ "type": "string" }, { "type": "integer" }]
            },
            "exact": {
                "type": "object",
                "additionalProperties": false
            },
            "when": {
                "type": "string",
                "format": "date-time",
                "description": "Timestamp"
            },
            "anything": true,
            "nothing": false
        }
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(ts.contains("tuple?: [string, number];"), "{ts}");
    assert!(ts.contains("exact?: Record<string, never>;"), "{ts}");
    assert!(ts.contains("* Timestamp"), "{ts}");
    assert!(ts.contains("* @format date-time"), "{ts}");
    assert!(ts.contains("anything?: unknown;"), "{ts}");
    assert!(ts.contains("nothing?: never;"), "{ts}");
}

#[test]
fn json_schema_to_type_preserves_root_object_with_conditional_all_of() {
    let schema = json!({
        "type": "object",
        "properties": {
            "action": {
                "enum": ["project_context", "list_ai_projects"]
            },
            "project": { "type": "string" },
            "tool": { "type": "string" },
            "limit": { "type": "integer" },
            "since": { "type": "string" }
        },
        "required": ["action"],
        "additionalProperties": false,
        "allOf": [
            {
                "if": {
                    "properties": { "action": { "const": "project_context" } },
                    "required": ["action"]
                },
                "then": { "required": ["project"] }
            },
            {
                "if": {
                    "properties": { "action": { "const": "list_ai_projects" } },
                    "required": ["action"]
                },
                "then": { "not": { "required": ["limit"] } }
            }
        ]
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(
        ts.contains(r#"action: "project_context" | "list_ai_projects";"#),
        "{ts}"
    );
    assert!(ts.contains("project?: string;"), "{ts}");
    assert!(ts.contains("limit?: number;"), "{ts}");
    assert!(!ts.contains("unknown & unknown"), "{ts}");
    assert!(!ts.starts_with("unknown"), "{ts}");
}

#[test]
fn json_schema_to_type_composes_root_object_with_one_of_variants() {
    let schema = json!({
        "type": "object",
        "properties": {
            "trace_id": { "type": "string" }
        },
        "oneOf": [
            {
                "type": "object",
                "properties": { "action": { "const": "alpha" } },
                "required": ["action"]
            },
            {
                "type": "object",
                "properties": { "action": { "const": "beta" } },
                "required": ["action"]
            }
        ]
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(ts.contains("trace_id?: string;"), "{ts}");
    assert!(ts.contains(r#"action: "alpha";"#), "{ts}");
    assert!(ts.contains(r#"action: "beta";"#), "{ts}");
    assert!(ts.contains(" & ("), "{ts}");
}

#[test]
fn json_schema_to_type_maps_binary_strings_to_runtime_buffer_types() {
    let schema = json!({
        "type": "object",
        "properties": {
            "payload": {
                "type": "string",
                "format": "binary"
            }
        },
        "required": ["payload"]
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(ts.contains("payload: Uint8Array | ArrayBuffer;"), "{ts}");
}

#[test]
fn json_schema_to_type_does_not_emit_conflicting_index_signatures() {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": { "type": "string" },
            "count": { "type": "integer" }
        },
        "required": ["id"],
        "additionalProperties": { "type": "number" }
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(ts.contains("id: string;"), "{ts}");
    assert!(ts.contains("count?: number;"), "{ts}");
    assert!(
        ts.contains("* Additional properties match: number"),
        "additional properties should preserve the schema type in documentation: {ts}"
    );
    assert!(
        ts.contains("[key: string]: number | string | undefined;"),
        "additional properties should be widened to avoid conflicting with explicit properties: {ts}"
    );
    assert!(!ts.contains("[key: string]: number;"), "{ts}");
}

#[test]
fn generate_tool_types_emits_composable_codemode_declarations() {
    let first = super::ts_signatures::generate_tool_types(
        "github",
        "list_tags",
        "List tags",
        Some(&json!({"type": "object"})),
        None,
    );
    let second = super::ts_signatures::generate_tool_types(
        "github",
        "create_issue",
        "Create issue",
        Some(&json!({"type": "object"})),
        None,
    );
    let combined = format!("{}\n{}", first.dts, second.dts);

    assert!(!combined.contains("declare const codemode"), "{combined}");
    assert_eq!(combined.matches("declare var codemode").count(), 2);
    assert!(combined.contains("interface CodemodeGithubTools"));
    assert!(combined.contains("list_tags(params:"), "{combined}");
    assert!(combined.contains("create_issue(params:"), "{combined}");
}

#[test]
fn generate_tool_types_quotes_sanitized_namespace_and_method_names() {
    let types = super::ts_signatures::generate_tool_types(
        "github chat",
        "list tags",
        "List tags",
        Some(&json!({"type": "object"})),
        None,
    );

    assert!(
        types.signature.contains("codemode.github_chat.list_tags"),
        "{types:?}"
    );
    assert!(types.dts.contains("github_chat"), "{types:?}");
    assert!(types.dts.contains("list_tags(params:"), "{types:?}");
    assert!(!types.dts.contains("github chat: {"), "{types:?}");
    assert!(!types.dts.contains("list tags(params:"), "{types:?}");
}

#[test]
fn generate_tool_types_sanitizes_reserved_digits_empty_dollar_and_collision_adjacent_names() {
    let cases = [
        ("await", "delete", "codemode.await_.delete_"),
        ("9lives", "2fa setup", "codemode._9lives._2fa_setup"),
        ("", "", "codemode._._"),
        ("cash$box", "$charge", "codemode.cash$box.$charge"),
        ("status.get", "list-tags", "codemode.status_get.list_tags"),
        ("status_get", "list.tags", "codemode.status_get.list_tags"),
    ];

    for (namespace, tool, expected) in cases {
        let types = super::ts_signatures::generate_tool_types(
            namespace,
            tool,
            "Description",
            Some(&json!({"type": "object"})),
            None,
        );

        assert!(
            types.signature.contains(expected),
            "expected {expected} in {:?}",
            types.signature
        );
        assert!(!types.dts.contains(".."), "{types:?}");
        assert!(!types.dts.contains("  : {"), "{types:?}");
        assert!(!types.dts.contains(" (params:"), "{types:?}");
    }
}

#[test]
fn json_schema_to_type_renders_openapi_nullable_as_null_union() {
    // Pairs with `code_mode_schema_validator_honors_openapi_nullable`: the
    // `.d.ts` advertises `T | null` for `nullable: true`, and the schema
    // validator accepts the null the signature promises.
    let schema = json!({
        "type": "object",
        "properties": {
            "note": { "type": "string", "nullable": true },
            "strict": { "type": "string" }
        }
    });

    let ts = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert!(ts.contains("note?: string | null;"), "{ts}");
    assert!(ts.contains("strict?: string;"), "{ts}");
}

// ── FR-9b (issue #210, lab-41e7m.7): bounded `$ref` expansion ───────────────

/// Build a wide-and-deep `$defs` graph: each level is an object whose three
/// properties each `$ref` the next level. Shared refs re-expand at every
/// occurrence, so unbudgeted expansion is O(3^depth) — with depth 12 that is
/// ~531k leaf renders and a rendered `String` in the hundreds of MB range.
fn hostile_ref_bomb(levels: usize) -> serde_json::Value {
    let mut defs = serde_json::Map::new();
    for level in 0..levels {
        let child = if level + 1 == levels {
            json!({ "type": "string" })
        } else {
            let next = format!("#/$defs/L{}", level + 1);
            json!({
                "type": "object",
                "properties": {
                    "a": { "$ref": next },
                    "b": { "$ref": next },
                    "c": { "$ref": next }
                }
            })
        };
        defs.insert(format!("L{level}"), child);
    }
    json!({ "$defs": defs, "$ref": "#/$defs/L0" })
}

/// The budget must bound the rendered OUTPUT and the node count, not merely
/// the wall clock: the pre-budget failure mode was a multi-GB `String`
/// allocation (OOM kill), reachable N-fold concurrently because the render
/// cache rebuilds outside its lock.
#[test]
fn hostile_ref_graph_collapses_to_unknown_with_bounded_output() {
    let schema = hostile_ref_bomb(12);

    let rendered = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert_eq!(
        rendered, "unknown",
        "an exhausted budget must publish `unknown`, never a partial render"
    );
}

/// The budget must be invisible to legitimate schemas: realistic nesting and
/// legitimate shared refs render fully.
#[test]
fn legitimate_nested_schema_renders_fully_under_budget() {
    let schema = json!({
        "$defs": {
            "Page": {
                "type": "object",
                "properties": {
                    "cursor": { "type": "string" },
                    "size": { "type": "integer" }
                }
            }
        },
        "type": "object",
        "properties": {
            "query": { "type": "string" },
            "page": { "$ref": "#/$defs/Page" },
            "fallback_page": { "$ref": "#/$defs/Page" },
            "filters": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "field": { "type": "string" },
                        "values": { "type": "array", "items": { "type": "string" } }
                    }
                }
            }
        },
        "required": ["query"]
    });

    let rendered = super::ts_signatures::json_schema_to_type(Some(&schema));

    assert_ne!(rendered, "unknown");
    assert!(rendered.contains("query: string;"), "{rendered}");
    // Both occurrences of the shared ref expand — budget, not memoization.
    assert_eq!(
        rendered.matches("cursor?: string;").count(),
        2,
        "shared refs must still expand at every occurrence: {rendered}"
    );
}
