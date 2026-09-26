use serde_json::{Value, json};

use super::*;
use crate::config::CodeModeResultShapePolicy;
use labby_codemode::CodeModeResultShapeMetadata;

fn response(result: Option<Value>) -> CodeModeExecutionResponse {
    CodeModeExecutionResponse {
        execution_id: None,
        result,
        result_shaping: None,
        ui: None,
        calls: vec![],
        logs: vec![],
        artifacts: vec![],
    }
}

fn shaped_display_response() -> CodeModeExecutionResponse {
    CodeModeExecutionResponse {
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
    }
}

fn resolved_snippet_with_tools(
    tools: Option<Vec<&str>>,
) -> crate::dispatch::snippets::store::ResolvedSnippet {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crate::dispatch::snippets::store::{ResolvedSnippet, SnippetSource};
    use labby_codemode::snippet::tool_declarations::SnippetToolDeclarations;

    ResolvedSnippet {
        tools: tools.map(|tools| {
            SnippetToolDeclarations::try_from(
                tools.into_iter().map(ToOwned::to_owned).collect::<Vec<_>>(),
            )
            .expect("valid exact tool declarations")
        }),
        name: "scoped".to_string(),
        description: None,
        tags: Vec::new(),
        inputs: BTreeMap::new(),
        source: SnippetSource::User,
        path: PathBuf::from("scoped.md"),
        body: "async () => ({ ok: true })".to_string(),
    }
}

#[test]
fn saved_snippet_tool_declarations_narrow_but_never_widen_route_authority() {
    let snippet = resolved_snippet_with_tools(Some(vec!["alpha::tool1", "beta::tool2"]));
    let caller_scope = ToolScope::scoped_namespaces(vec!["alpha".to_string()], Vec::new());

    let scope = snippet_execution_scope(&snippet, &caller_scope);

    assert!(scope.is_scoped());
    assert!(scope.allows("alpha", "tool1"));
    assert!(
        !scope.allows("alpha", "other_tool"),
        "an exact snippet declaration must deny undeclared siblings on an allowed upstream"
    );
    assert!(
        !scope.allows("beta", "tool2"),
        "a snippet declaration must never restore an upstream removed by the route"
    );
}

#[test]
fn saved_snippet_without_declarations_inherits_route_scope_exactly() {
    let snippet = resolved_snippet_with_tools(None);
    let caller_scope = ToolScope::scoped_namespaces(vec!["alpha".to_string()], Vec::new());

    let scope = snippet_execution_scope(&snippet, &caller_scope);

    assert_eq!(scope, caller_scope);
    assert!(scope.allows("alpha", "other_tool"));
    assert!(!scope.allows("beta", "tool2"));
}

#[test]
fn trusted_local_saved_snippet_retains_declared_exact_tool_scope() {
    let snippet =
        resolved_snippet_with_tools(Some(vec!["claude-macpoo::Bash", "claude-macpoo::Read"]));

    let scope = snippet_execution_scope(&snippet, &ToolScope::default());

    assert!(scope.is_scoped());
    assert!(scope.allows("claude-macpoo", "Bash"));
    assert!(scope.allows("claude-macpoo", "Read"));
    assert!(!scope.allows("github", "search_issues"));
}

#[test]
fn saved_snippet_invocation_checks_final_wrapped_source_size() {
    let code = "async () => ({ ok: true })";
    let input = json!({ "payload": "x".repeat(256) });

    let error = wrap_snippet_with_input_bounded(code, &input, 128)
        .expect_err("serialized params must count toward the runtime source limit");

    assert_eq!(error.kind(), "invalid_param");
    let message = format!("{error}");
    assert!(message.contains("saved snippet invocation"));
    assert!(message.contains("128"));
}

#[test]
fn snippets_test_checks_raw_result_and_rejects_truncated_display() {
    let result = snippet_test_result(
        "shape-pass".to_owned(),
        SnippetExecutionOutcome {
            raw_response: response(Some(json!({"ok": true, "payload": "x".repeat(5000)}))),
            display_response: shaped_display_response(),
        },
        1,
        &SnippetTestBudgets::default(),
    )
    .unwrap();
    assert_eq!(result["passed"], false);
    assert_eq!(result["metrics"]["truncated"], true);
    assert_eq!(result["result"]["ok"], true);
    assert_eq!(result["metrics"]["output_bytes"], 5024);
}

#[test]
fn snippets_test_rejects_false_ok_and_undefined() {
    for value in [None, Some(json!({"ok": false}))] {
        let result = snippet_test_result(
            "failure".to_owned(),
            SnippetExecutionOutcome {
                raw_response: response(value.clone()),
                display_response: response(value),
            },
            1,
            &SnippetTestBudgets::default(),
        )
        .unwrap();
        assert_eq!(result["passed"], false);
    }
}

#[tokio::test]
async fn snippets_test_requires_an_explicit_mode_before_resolving_or_connecting() {
    for params in [
        json!({"name": "missing"}),
        json!({"all": true}),
        json!({"name": "missing", "fixture": {}, "live": true}),
    ] {
        let error = dispatch_inner(None, "snippets.test", params, None)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), "invalid_param");
        assert!(error.to_string().contains("exactly one"));
    }
}

#[tokio::test]
async fn all_live_tests_reject_explicit_input_before_connecting() {
    let error = dispatch_inner(
        None,
        "snippets.test",
        json!({"all": true, "live": true, "params": {"ignored": true}}),
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind(), "invalid_param");
    assert!(error.to_string().contains("defaults"));
}

#[test]
fn native_input_wrapper_preserves_json_instead_of_object_literal_semantics() {
    let wrapped = wrap_snippet_with_input_bounded(
        "async input => input",
        &json!({"__proto__": {"surprise": true}}),
        16000,
    )
    .unwrap();
    assert!(wrapped.contains("const __labSnippetInput = JSON.parse("));
    assert!(!wrapped.contains("const __labSnippetInput = {"));
}
