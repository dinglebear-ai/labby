//! Tests for the Code Mode gateway meta-tool helpers. Distributed from
//! `server.rs` (bead `lab-kvji.24.1.6`).

use super::{
    CODE_MODE_DESCRIPTION_MAX_BYTES, CodeModeDescriptionVariant, CodeModeExampleCall,
    CodeModeUpstreamDescription, InflightCodeModeRole, await_code_mode_execution,
    begin_code_mode_execution, code_arg, code_mode_execute_trace, code_mode_result,
    code_mode_tool_description, route_scoped_capability_filter, string_array_arg,
};
use crate::config::CodeModeResultShapePolicy;
use labby_codemode::{
    CodeModeExecutedCall, CodeModeExecutionResponse, CodeModeResultShapeMetadata, MAX_SOURCE_BYTES,
    UiLink,
};
use serde_json::{Value, json};

#[test]
fn code_mode_result_preserves_captured_upstream_mcp_app_metadata() {
    let response = CodeModeExecutionResponse {
        execution_id: Some("execution-1".to_string()),
        result: Some(json!({"session_id": "quick-shell-session"})),
        result_shaping: None,
        ui: Some(UiLink {
            ui_meta: json!({
                "resourceUri": "ui://quick-shell/mcp-app.v5.html",
                "visibility": ["model", "app"]
            }),
        }),
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    };

    let result = code_mode_result("result text".to_string(), json!({}), &response, true);
    let meta = result
        .meta
        .expect("captured MCP App metadata must pass through");

    assert_eq!(
        meta.0["ui"],
        json!({
            "resourceUri": "ui://quick-shell/mcp-app.v5.html",
            "visibility": ["model", "app"]
        })
    );

    let hidden = code_mode_result("result text".to_string(), json!({}), &response, false);
    assert!(
        hidden.meta.is_none(),
        "resource-disabled routes must not return nested MCP App metadata"
    );
}

#[test]
fn code_mode_filter_arg_rejects_malformed_values() {
    let mut args = serde_json::Map::new();
    args.insert(
        "tools".to_string(),
        Value::String("github::search_issues".to_string()),
    );
    let err = string_array_arg(&args, "tools")
        .expect_err("string filter must not be treated as allow-all");
    assert_eq!(err.kind(), "invalid_param");

    let mut args = serde_json::Map::new();
    args.insert("upstreams".to_string(), serde_json::json!(["github", 42]));
    let err = string_array_arg(&args, "upstreams")
        .expect_err("non-string filter entries must not be dropped");
    assert_eq!(err.kind(), "invalid_param");
}

#[test]
fn code_mode_filter_arg_accepts_absent_and_string_arrays() {
    let args = serde_json::Map::new();
    assert_eq!(
        string_array_arg(&args, "tools").expect("absent ok"),
        Vec::<String>::new()
    );

    let mut args = serde_json::Map::new();
    args.insert("tools".to_string(), serde_json::json!(["a", "b"]));
    assert_eq!(
        string_array_arg(&args, "tools").expect("array ok"),
        vec!["a".to_string(), "b".to_string()]
    );
}

#[test]
fn code_arg_rejects_missing_or_blank_code() {
    let args = serde_json::Map::new();
    let err = code_arg(&args, 128 * 1024).expect_err("missing code must be invalid");
    assert_eq!(err.kind(), "invalid_param");

    let mut args = serde_json::Map::new();
    args.insert("code".to_string(), Value::String("  \n\t ".to_string()));
    let err = code_arg(&args, 128 * 1024).expect_err("blank code must be invalid");
    assert_eq!(err.kind(), "invalid_param");
}

#[test]
fn code_arg_respects_configured_source_limit_and_hard_ceiling() {
    let configured_limit = crate::config::CodeModeConfig::default().max_source_bytes;
    assert!(configured_limit > 20_000);

    let mut args = serde_json::Map::new();
    args.insert(
        "code".to_string(),
        Value::String("a".repeat(configured_limit)),
    );
    assert!(code_arg(&args, configured_limit).is_ok());

    let mut args = serde_json::Map::new();
    args.insert(
        "code".to_string(),
        Value::String("a".repeat(configured_limit + 1)),
    );
    let err =
        code_arg(&args, configured_limit).expect_err("configured over-limit code must be invalid");
    assert_eq!(err.kind(), "invalid_param");
    assert!(err.to_string().contains(&configured_limit.to_string()));

    let mut args = serde_json::Map::new();
    args.insert(
        "code".to_string(),
        Value::String("a".repeat(MAX_SOURCE_BYTES + 1)),
    );
    assert!(code_arg(&args, MAX_SOURCE_BYTES * 2).is_err());
}

#[test]
fn scoped_capability_filter_rejects_disallowed_requested_upstreams() {
    let mut args = serde_json::Map::new();
    args.insert("upstreams".to_string(), json!(["beta"]));
    let allowed = std::collections::BTreeSet::from(["alpha".to_string()]);
    let available = std::collections::BTreeSet::from(["alpha".to_string(), "beta".to_string()]);

    let err = route_scoped_capability_filter(&args, Some(&allowed), &available)
        .expect_err("disallowed explicit upstream must fail");

    assert_eq!(err.kind(), "route_scope_denied");
    let message = err.to_string();
    assert!(
        message.contains("`beta`") && message.contains("`alpha`"),
        "{message}"
    );
}

#[test]
fn scoped_capability_filter_canonicalizes_unique_case_variant() {
    let mut args = serde_json::Map::new();
    args.insert("upstreams".to_string(), json!(["axon"]));
    let allowed = std::collections::BTreeSet::from(["Axon".to_string()]);
    let available = allowed.clone();

    let filter = route_scoped_capability_filter(&args, Some(&allowed), &available)
        .expect("unique case variant should canonicalize before authorization");

    assert!(filter.allows("Axon", "axon"));
    assert!(!filter.allows("axon", "axon"));
}

#[test]
fn capability_filter_canonicalizes_namespaced_tool_filter() {
    let mut args = serde_json::Map::new();
    args.insert("tools".to_string(), json!(["axon::axon"]));
    let available = std::collections::BTreeSet::from(["Axon".to_string()]);

    let filter = route_scoped_capability_filter(&args, None, &available)
        .expect("namespaced tool filter should canonicalize its namespace");

    assert!(filter.allows("Axon", "axon"));
    assert!(!filter.allows("axon", "axon"));
}

#[test]
fn capability_filter_rejects_ambiguous_case_alias() {
    let mut args = serde_json::Map::new();
    args.insert("upstreams".to_string(), json!(["aXoN"]));
    let available = std::collections::BTreeSet::from(["Axon".to_string(), "axon".to_string()]);

    let err = route_scoped_capability_filter(&args, None, &available)
        .expect_err("ambiguous case-only alias must fail closed");

    assert_eq!(err.kind(), "invalid_param");
    assert!(err.to_string().contains("is ambiguous"));
}

#[test]
fn capability_filter_prefers_exact_case_over_alias_matching() {
    let mut args = serde_json::Map::new();
    args.insert("upstreams".to_string(), json!(["Axon"]));
    let available = std::collections::BTreeSet::from(["Axon".to_string(), "axon".to_string()]);

    let filter = route_scoped_capability_filter(&args, None, &available)
        .expect("exact namespace must remain usable even with a case-only sibling");

    assert!(filter.allows("Axon", "axon"));
    assert!(!filter.allows("axon", "axon"));
}

fn available(names: &[&str]) -> std::collections::BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn upstreams_args(names: Value) -> serde_json::Map<String, Value> {
    let mut args = serde_json::Map::new();
    args.insert("upstreams".to_string(), names);
    args
}

#[test]
fn capability_filter_matches_underscore_alias_of_hyphenated_upstream() {
    // Search results render namespaces as `claude_macpoo`, so agents echo
    // that spelling back; it must resolve to the configured `claude-macpoo`.
    let args = upstreams_args(json!(["claude_macpoo"]));
    let filter = route_scoped_capability_filter(&args, None, &available(&["claude-macpoo"]))
        .expect("separator-only alias should canonicalize");

    assert!(filter.allows("claude-macpoo", "Bash"));
    assert!(!filter.allows("claude_macpoo", "Bash"));
}

#[test]
fn capability_filter_matches_case_and_separator_alias_together() {
    let args = upstreams_args(json!(["Claude_MacPoo"]));
    let filter = route_scoped_capability_filter(&args, None, &available(&["claude-macpoo"]))
        .expect("case plus separator alias should canonicalize");

    assert!(filter.allows("claude-macpoo", "Bash"));
}

#[test]
fn capability_filter_canonicalizes_separator_alias_in_namespaced_tool_filter() {
    let mut args = serde_json::Map::new();
    args.insert("tools".to_string(), json!(["claude_macpoo::Bash"]));
    let filter = route_scoped_capability_filter(&args, None, &available(&["claude-macpoo"]))
        .expect("namespaced tool filter should canonicalize a separator alias");

    assert!(filter.allows("claude-macpoo", "Bash"));
}

#[test]
fn capability_filter_rejects_ambiguous_separator_alias_and_lists_candidates() {
    let args = upstreams_args(json!(["A-b"]));
    let err = route_scoped_capability_filter(&args, None, &available(&["a-b", "a_b"]))
        .expect_err("alias matching two upstreams must fail closed");

    assert_eq!(err.kind(), "invalid_param");
    let message = err.to_string();
    assert!(
        message.contains("`a-b`") && message.contains("`a_b`"),
        "{message}"
    );
}

#[test]
fn capability_filter_rejects_unknown_upstream_with_similar_name_suggestion() {
    let args = upstreams_args(json!(["claude-macpo"]));
    let err = route_scoped_capability_filter(
        &args,
        None,
        &available(&["claude-macpoo", "claude-squirts", "github"]),
    )
    .expect_err("unknown upstream must not silently yield an empty catalog");

    assert_eq!(err.kind(), "unknown_upstream");
    let message = err.to_string();
    assert!(message.contains("`claude-macpo`"), "{message}");
    assert!(
        message.contains("Did you mean `claude-macpoo`"),
        "{message}"
    );
}

#[test]
fn capability_filter_unknown_upstream_without_similar_name_lists_known_upstreams() {
    let args = upstreams_args(json!(["zzz"]));
    let err = route_scoped_capability_filter(&args, None, &available(&["alpha", "beta"]))
        .expect_err("unknown upstream must fail");

    assert_eq!(err.kind(), "unknown_upstream");
    let message = err.to_string();
    assert!(!message.contains("Did you mean"), "{message}");
    assert!(
        message.contains("`alpha`") && message.contains("`beta`"),
        "{message}"
    );
}

#[test]
fn capability_filter_unknown_upstream_suggestions_stay_inside_route_scope() {
    let args = upstreams_args(json!(["secret-bet"]));
    let allowed = available(&["alpha"]);
    let err = route_scoped_capability_filter(
        &args,
        Some(&allowed),
        &available(&["alpha", "secret-beta"]),
    )
    .expect_err("unknown upstream must fail");

    assert_eq!(err.kind(), "unknown_upstream");
    assert!(!err.to_string().contains("secret-beta"), "{err}");
}

#[test]
fn capability_filter_keeps_builtin_provider_namespaces() {
    let args = upstreams_args(json!(["unraid"]));
    let filter = route_scoped_capability_filter(&args, None, &available(&["alpha"]))
        .expect("built-in provider namespace is not a configured upstream but is valid");

    assert!(filter.allows("unraid", "anything"));
}

#[test]
fn scoped_capability_filter_defaults_to_route_allowed_upstreams() {
    let args = serde_json::Map::new();
    let allowed = std::collections::BTreeSet::from(["alpha".to_string()]);
    let available = allowed.clone();

    let filter = route_scoped_capability_filter(&args, Some(&allowed), &available)
        .expect("omitted upstreams should default to route scope");

    assert!(filter.allows("alpha", "search"));
    assert!(!filter.allows("beta", "search"));
}

/// Clients such as Claude Code show roughly the first 2 KB of a description.
const CLIENT_VISIBLE_BYTES: usize = 2048;

fn realistic_upstreams() -> Vec<CodeModeUpstreamDescription> {
    let mut upstreams = (0..20)
        .map(|index| CodeModeUpstreamDescription {
            name: format!("upstream-{index:02}"),
            hint: Some(format!(
                "Fixture service number {index} for description budget tests"
            )),
            example: None,
        })
        .collect::<Vec<_>>();
    upstreams[0].name = "claude-macpoo".to_string();
    upstreams[0].example = Some(CodeModeExampleCall::from_tool(
        "Bash",
        json!({
            "type": "object",
            "properties": { "command": { "type": "string" }, "timeout": { "type": "number" } },
            "required": ["command"]
        })
        .as_object()
        .expect("schema"),
        false,
    ));
    upstreams[1].example = Some(CodeModeExampleCall::from_tool(
        "list_issues",
        json!({
            "type": "object",
            "properties": {
                "state": { "type": "string", "enum": ["open", "closed"] },
                "per-page": { "type": ["integer", "null"] }
            },
            "required": ["state", "per-page"]
        })
        .as_object()
        .expect("schema"),
        true,
    ));
    upstreams
}

fn visible_prefix(description: &str) -> &str {
    let mut end = description.len().min(CLIENT_VISIBLE_BYTES);
    while !description.is_char_boundary(end) {
        end -= 1;
    }
    &description[..end]
}

#[test]
fn code_mode_variants_differ_within_the_client_visible_prefix() {
    let upstreams = realistic_upstreams();
    let full = code_mode_tool_description(CodeModeDescriptionVariant::Full, &upstreams, "");
    let read = code_mode_tool_description(CodeModeDescriptionVariant::Read, &upstreams, "");
    let ui = code_mode_tool_description(CodeModeDescriptionVariant::Ui, &upstreams, "");

    assert!(full.starts_with("Write-capable Code Mode"), "{full}");
    assert!(read.starts_with("Read-only Code Mode"), "{read}");
    assert!(
        ui.starts_with("Code Mode with a visual trace inspector"),
        "{ui}"
    );
    assert_ne!(visible_prefix(&full), visible_prefix(&read));
    assert!(visible_prefix(&read).contains("use `codemode`"));
    assert!(visible_prefix(&full).contains("`codemode_read`"));
}

#[test]
fn code_mode_rules_example_and_first_upstream_fit_in_the_visible_prefix() {
    let upstreams = realistic_upstreams();
    for variant in [
        CodeModeDescriptionVariant::Full,
        CodeModeDescriptionVariant::Read,
        CodeModeDescriptionVariant::Ui,
    ] {
        let description = code_mode_tool_description(variant, &upstreams, "");
        let visible = visible_prefix(&description);
        for required in [
            "async () => {",
            "codemode.search",
            "codemode.describe",
            "codemode.batch",
            "24 KB",
            "30 s",
            "recovery.guidance",
            "Never guess",
            "Example:",
            "## Upstreams",
            "- `claude-macpoo`",
        ] {
            assert!(
                visible.contains(required),
                "{variant:?} prefix lacks {required:?}:\n{visible}"
            );
        }
        assert!(description.len() <= CODE_MODE_DESCRIPTION_MAX_BYTES);
    }
}

#[test]
fn read_variant_never_advertises_writes_or_a_mutating_example() {
    let upstreams = realistic_upstreams();
    let read = code_mode_tool_description(CodeModeDescriptionVariant::Read, &upstreams, "");
    let full = code_mode_tool_description(CodeModeDescriptionVariant::Full, &upstreams, "");

    assert!(!read.contains("Globals: `codemode`, `callTool`, `writeArtifact`"));
    assert!(!read.contains("reuse idempotency keys"));
    // Read skips the non-read-only Bash example and uses the read-only tool.
    assert!(
        read.contains(r#"callTool("upstream-01::list_issues", { state: "open", "per-page": 1 })"#),
        "{read}"
    );
    // Write-capable variants still prefer a read-only example when one exists.
    assert!(
        full.contains(r#"callTool("upstream-01::list_issues""#),
        "{full}"
    );
    assert!(full.contains("reuse idempotency keys"));
}

#[test]
fn write_variant_falls_back_to_a_mutating_example_only_without_read_only_tools() {
    let mut upstreams = realistic_upstreams();
    upstreams[1].example = None;
    let full = code_mode_tool_description(CodeModeDescriptionVariant::Full, &upstreams, "");
    let read = code_mode_tool_description(CodeModeDescriptionVariant::Read, &upstreams, "");

    assert!(
        full.contains(r#"callTool("claude-macpoo::Bash", { command: "..." })"#),
        "{full}"
    );
    assert!(!read.contains("claude-macpoo::Bash"), "{read}");
    assert!(read.contains("hits.results.map"), "{read}");
}

#[test]
fn description_without_live_examples_teaches_a_search_first_run() {
    let upstreams = vec![CodeModeUpstreamDescription {
        name: "github".to_string(),
        hint: None,
        example: None,
    }];
    let description = code_mode_tool_description(CodeModeDescriptionVariant::Full, &upstreams, "");
    assert!(description.contains("hits.results.map"), "{description}");
    assert!(description.contains("- `github`"));

    let empty = code_mode_tool_description(CodeModeDescriptionVariant::Read, &[], "");
    assert!(empty.contains("- none currently configured"));
}

#[test]
fn oversized_upstream_lists_are_trimmed_by_whole_lines_with_a_search_pointer() {
    let upstreams = (0..400)
        .map(|index| CodeModeUpstreamDescription {
            name: format!("upstream-{index:03}"),
            hint: Some("💩".repeat(40)),
            example: None,
        })
        .collect::<Vec<_>>();
    let description = code_mode_tool_description(
        CodeModeDescriptionVariant::Full,
        &upstreams,
        "low-priority trailer",
    );

    assert!(description.len() <= CODE_MODE_DESCRIPTION_MAX_BYTES);
    assert!(description.starts_with("Write-capable Code Mode"));
    assert!(description.contains("- `upstream-000`"));
    assert!(description.contains("more; use `codemode.search()` to discover them"));
    assert!(!description.contains("low-priority trailer"));
    assert!(std::str::from_utf8(description.as_bytes()).is_ok());
}

#[test]
fn trailer_is_appended_when_it_fits() {
    let description = code_mode_tool_description(
        CodeModeDescriptionVariant::Full,
        &[],
        "low-priority trailer",
    );
    assert!(description.ends_with("low-priority trailer"));
}

#[test]
fn codemode_input_schema_includes_optional_filters() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "code": { "type": "string", "minLength": 1 },
            "upstreams": { "type": "array", "items": { "type": "string" } },
            "tools": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["code"]
    });
    let props = schema["properties"].as_object().expect("properties object");
    let prop_names: std::collections::BTreeSet<&str> = props.keys().map(String::as_str).collect();
    assert_eq!(
        prop_names,
        std::collections::BTreeSet::from(["code", "tools", "upstreams"])
    );
    for forbidden_control in ["resume_token", "confirm", "pause", "reject", "rollback"] {
        assert!(
            !prop_names.contains(forbidden_control),
            "Code Mode must not expose a {forbidden_control} lifecycle control"
        );
    }
    assert_eq!(schema["properties"]["code"]["minLength"], json!(1));
    assert_eq!(
        schema["properties"]["upstreams"]["items"]["type"],
        json!("string")
    );
    assert_eq!(
        schema["properties"]["tools"]["items"]["type"],
        json!("string")
    );
}

#[test]
fn code_mode_execute_trace_includes_shape_metadata_and_shaped_result() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        result: Some(json!("[code mode result truncated]\n{}")),
        result_shaping: Some(CodeModeResultShapeMetadata {
            policy: CodeModeResultShapePolicy::Truncate,
            changed: true,
            truncated: true,
            original_size_bytes: 5000,
            shaped_size_bytes: 256,
            warning: None,
        }),
        ui: None,
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    };

    let text_json = serde_json::to_value(&response).expect("response serializes");
    let structured_json = code_mode_execute_trace(&response);

    assert_eq!(
        text_json.get("result"),
        structured_json.get("result"),
        "MCP text JSON and structuredContent must use the same shaped result"
    );
    assert_eq!(
        text_json.get("result_shaping"),
        structured_json.get("result_shaping"),
        "MCP text JSON and structuredContent must expose the same shaping metadata"
    );
    assert!(text_json.get("result_shape").is_none());
    assert_eq!(structured_json["result_shape"]["type"], json!("string"));
    assert_eq!(
        structured_json["result_shaping"]["policy"],
        json!("truncate")
    );
    assert_eq!(structured_json["result_shaping"]["changed"], json!(true));
    assert_eq!(structured_json["result_shaping"]["truncated"], json!(true));
    assert_eq!(
        structured_json["result_shaping"]["original_size_bytes"],
        json!(5000)
    );
    assert_eq!(
        structured_json["result_shaping"]["shaped_size_bytes"],
        json!(256)
    );
    assert!(structured_json["result_shape"].get("policy").is_none());
    assert!(structured_json["result_shape"].get("truncated").is_none());
}

#[test]
fn execute_trace_embeds_result_and_redacts_call_params() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: Some(json!({
            "answer": "the full research answer the model asked for",
            "items": ["a", "b", "c"]
        })),
        result_shaping: None,
        calls: vec![CodeModeExecutedCall {
            id: "github::search_issues".to_string(),
            ok: true,
            elapsed_ms: 12,
            start_ms: Some(3),
            params: Some(json!({
                "action": "issues.search",
                "query": "bug",
                "token": "[redacted]"
            })),
            error_kind: None,
            ui: None,
        }],
        logs: vec!["one".to_string()],
        artifacts: vec![],
    };

    let trace = code_mode_execute_trace(&response);
    assert_eq!(trace["kind"], json!("code_mode_execute_trace"));
    // Neutral vocabulary after the lab-codemode extraction: the call trace
    // namespace field is `namespace` (was `upstream`).
    assert_eq!(trace["calls"][0]["namespace"], json!("github"));
    assert_eq!(trace["calls"][0]["tool"], json!("search_issues"));
    assert_eq!(
        trace["calls"][0]["params"]["action"],
        json!("issues.search")
    );
    // Per-call params remain redacted — that is the secret-bearing channel.
    assert_eq!(trace["calls"][0]["params"]["token"], json!("[redacted]"));
    assert!(
        trace["calls"][0].get("error_kind").is_none(),
        "successful calls must omit optional error_kind instead of emitting null"
    );
    assert!(
        trace["calls"][0].get("ui").is_none(),
        "non-UI calls must omit optional ui instead of emitting null"
    );

    // The real return value is now embedded verbatim so structured-content-only
    // clients (e.g. Claude Code) actually receive it, not just its shape. The
    // value is already response-budget-capped upstream by
    // `truncate_execution_response`, so it is not re-truncated here.
    assert_eq!(
        trace["result"]["answer"],
        json!("the full research answer the model asked for")
    );
    assert_eq!(trace["result"]["items"], json!(["a", "b", "c"]));

    // result_shape is retained for the inline UI app / quick inspection.
    assert_eq!(trace["result_shape"]["type"], json!("object"));
    assert_eq!(trace["result_shape"]["key_count"], json!(2));
}

#[test]
fn execute_trace_omits_result_when_function_returns_undefined() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: None,
        result_shaping: None,
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    };

    let trace = code_mode_execute_trace(&response);
    // `undefined` return omits the field entirely (parity with the response
    // envelope), and the shape descriptor reports `"undefined"`.
    assert!(
        trace.get("result").is_none(),
        "an undefined return must omit `result`, not emit null"
    );
    assert_eq!(trace["result_shape"]["type"], json!("undefined"));
    assert_eq!(trace["logs_count"], json!(0));
}

#[test]
fn execute_trace_preserves_explicit_null_result() {
    let response = CodeModeExecutionResponse {
        execution_id: None,
        ui: None,
        result: Some(Value::Null),
        result_shaping: None,
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    };

    let trace = code_mode_execute_trace(&response);
    // Explicit JS `null` is distinct from `undefined`: the field is present and
    // null, matching the response envelope's null-vs-undefined contract.
    assert!(
        trace.get("result").is_some(),
        "explicit null must emit `result`, not omit it"
    );
    assert!(trace["result"].is_null());
    assert_eq!(trace["result_shape"]["type"], json!("null"));
}

#[tokio::test]
async fn identical_inflight_code_mode_runs_share_one_leader_result() {
    let key = format!("dedup-test-{}", ulid::Ulid::new());
    let leader = match begin_code_mode_execution(key.clone(), "exec_leader".to_string()) {
        InflightCodeModeRole::Leader(leader) => leader,
        InflightCodeModeRole::Follower(_) => panic!("first caller must lead"),
    };
    let follower = match begin_code_mode_execution(key.clone(), "exec_follower".to_string()) {
        InflightCodeModeRole::Follower(entry) => entry,
        InflightCodeModeRole::Leader(_) => panic!("identical concurrent caller must join"),
    };
    assert_eq!(follower.leader_execution_id, "exec_leader");

    let waiter = tokio::spawn(await_code_mode_execution(follower));
    let response = CodeModeExecutionResponse {
        execution_id: None,
        result: Some(json!({ "ok": true })),
        result_shaping: None,
        ui: None,
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    };
    leader.complete(&Ok(response.clone()));

    assert_eq!(waiter.await.unwrap().unwrap(), response);
    match begin_code_mode_execution(key, "exec_next".to_string()) {
        InflightCodeModeRole::Leader(_) => {}
        InflightCodeModeRole::Follower(_) => panic!("completed entry must be removed"),
    }
}

#[tokio::test]
async fn cancelled_code_mode_leader_releases_duplicate_waiters() {
    let key = format!("dedup-cancel-test-{}", ulid::Ulid::new());
    let leader = match begin_code_mode_execution(key.clone(), "exec_leader".to_string()) {
        InflightCodeModeRole::Leader(leader) => leader,
        InflightCodeModeRole::Follower(_) => panic!("first caller must lead"),
    };
    let follower = match begin_code_mode_execution(key, "exec_follower".to_string()) {
        InflightCodeModeRole::Follower(entry) => entry,
        InflightCodeModeRole::Leader(_) => panic!("identical concurrent caller must join"),
    };

    drop(leader);
    let error = await_code_mode_execution(follower)
        .await
        .expect_err("cancelled leader must release follower with an error");
    assert_eq!(error.kind(), "service_unavailable");
}
