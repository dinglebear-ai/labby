use serde_json::{Value, json};

use super::super::execution::snippet_execution_scope;
use super::super::store::wrap_snippet_with_input_bounded;
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

#[test]
fn owner_scoped_snippet_actions_share_the_contextual_surface_contract() {
    for action in [
        "snippets.exec",
        "snippets.test",
        "snippets.fixture",
        "snippets.promote",
        "snippets.preview",
        "snippets.replay",
        "snippets.receipt",
        "snippets.history",
        "snippets.artifact",
    ] {
        assert!(requires_execution_context(action), "{action}");
    }
    for action in [
        "snippets.list",
        "snippets.validate",
        "snippets.create",
        "snippets.get",
    ] {
        assert!(!requires_execution_context(action), "{action}");
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
        content_digest: None,
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
fn snippets_test_uses_raw_result_for_pass_fail_and_returns_shaped_display() {
    let pass = snippet_test_result(
        "shape-pass".to_string(),
        SnippetExecutionOutcome {
            receipt_status: "unavailable".into(),
            raw_response: response(Some(json!({"ok": true, "payload": "x".repeat(5000)}))),
            display_response: shaped_display_response(),
        },
    )
    .expect("passing snippet result");
    assert_eq!(pass["passed"], json!(true));
    assert_eq!(
        pass["response"],
        serde_json::to_value(shaped_display_response()).expect("display response serializes")
    );

    let fail = snippet_test_result(
        "shape-fail".to_string(),
        SnippetExecutionOutcome {
            receipt_status: "unavailable".into(),
            raw_response: response(Some(json!({"ok": false, "payload": "x".repeat(5000)}))),
            display_response: shaped_display_response(),
        },
    )
    .expect("failing snippet result");
    assert_eq!(fail["passed"], json!(false));
    assert_eq!(
        fail["response"],
        serde_json::to_value(shaped_display_response()).expect("display response serializes")
    );
}

#[test]
fn snippets_test_fails_when_a_batched_upstream_call_failed() {
    let mut response = response(Some(json!({
        "requested": 1,
        "succeeded": 0,
        "failed": 1,
        "all_ok": false
    })));
    response.calls.push(labby_codemode::CodeModeExecutedCall {
        id: "team-depot::depot.skills.search".into(),
        ok: false,
        elapsed_ms: 0,
        start_ms: Some(0),
        params: Some(json!({})),
        error_kind: Some("unknown_upstream".into()),
        ui: None,
    });
    assert!(!snippet_response_passed(&response));
}
