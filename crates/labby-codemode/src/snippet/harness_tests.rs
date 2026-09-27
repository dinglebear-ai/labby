use super::super::store::{SnippetSource, validate_snippet_code};
use super::*;

fn fixture(value: Value) -> SnippetFixture {
    serde_json::from_value(value).unwrap()
}
fn snippet(body: &str) -> ResolvedSnippet {
    ResolvedSnippet {
        name: "fixture-demo".into(),
        description: None,
        tags: vec![],
        inputs: BTreeMap::new(),
        tools: None,
        source: SnippetSource::User,
        path: "/unused/fixture-demo.js".into(),
        body: body.into(),
    }
}
fn evidence(result: Value) -> Value {
    json!({"result": result, "exception": null, "calls": [], "attempted": 0, "unexpected": 0,
        "max_in_flight": 0, "unused": []})
}

#[test]
fn defaults_are_offline_and_bounded() {
    let f = fixture(json!({}));
    assert_eq!(f.budgets.tool_calls, 40);
    assert_eq!(f.budgets.output_bytes, 16_000);
    assert!(f.validate().is_ok());
}

#[test]
fn rejects_unknown_fixture_fields_and_unbounded_calls() {
    assert!(serde_json::from_value::<SnippetFixture>(json!({"exepct": {}})).is_err());
    for value in [
        json!({"budgets": {"tool_calls": 513}}),
        json!({"budgets": {"wall_clock_ms": 30_001}}),
        json!({"calls": [{"tool": "github::get_me", "times": 0}]}),
        json!({"calls": [{"tool": "github::get_me", "match": []}]}),
        json!({"input": null}),
        json!({"expect": {"not-a-pointer": true}}),
        json!({"expect": {"/bad~2escape": true}}),
    ] {
        assert!(fixture(value).validate().is_err());
    }
}

#[test]
fn rejects_reserved_capabilities_and_conflicting_error_results() {
    for tool in [
        "lab::gateway",
        "state::get",
        "git::status",
        "openapi::call",
        "__lab_internal::describe_types",
    ] {
        assert!(
            fixture(json!({"calls": [{"tool": tool}]}))
                .validate()
                .is_err()
        );
    }
    assert!(
        fixture(json!({"calls": [{"tool": "github::get_me", "result": {},
        "error": {"kind": "test", "message": "synthetic"}}]}))
        .validate()
        .is_err()
    );
}

#[test]
fn compiles_wrapper_using_real_quickjs_parser_without_executing() {
    let f = fixture(json!({"calls": [{"tool": "github::get_me", "result": {"login": "fixture"}}]}));
    let code = prepare(
        &snippet("async () => await callTool('github::get_me', {})"),
        &f,
        json!({}),
    )
    .unwrap();
    validate_snippet_code(&code).unwrap();
    assert!(code.contains("nativeBatch"));
}

#[test]
fn fixture_tool_must_match_declared_dependencies() {
    let mut s = snippet("async () => true");
    s.tools = Some(SnippetToolDeclarations::try_from(Vec::new()).unwrap());
    let f = fixture(json!({"calls": [{"tool": "github::get_me"}]}));
    assert!(prepare(&s, &f, json!({})).is_err());
}

#[test]
fn verifies_snapshots_after_explicit_normalization() {
    let f =
        fixture(json!({"expect": {"": {"ok": true, "elapsed": 0}}, "normalize": {"/elapsed": 0}}));
    let report = report(&f, &evidence(json!({"ok": true, "elapsed": 123})), 1, 0);
    assert_eq!(report["passed"], true);
}

#[test]
fn asserts_absent_fields_and_detects_snapshot_drift() {
    let f = fixture(json!({"expect": {"/ok": true}, "absent": ["/history"]}));
    assert_eq!(
        report(&f, &evidence(json!({"ok": true})), 1, 0)["passed"],
        true
    );
    assert_eq!(
        report(&f, &evidence(json!({"ok": true, "history": []})), 1, 0)["passed"],
        false
    );
    assert_eq!(
        report(&f, &evidence(json!({"ok": false})), 1, 0)["passed"],
        false
    );
}

#[test]
fn caught_unexpected_calls_cannot_turn_test_green() {
    let f = fixture(json!({"expect": {"/ok": true}}));
    let mut e = evidence(json!({"ok": true}));
    e["unexpected"] = json!(1);
    assert_eq!(report(&f, &e, 1, 0)["passed"], false);
    assert_eq!(
        report(&f, &evidence(json!({"ok": true})), 1, 1)["passed"],
        false
    );
}

#[test]
fn enforces_wall_clock_parallelism_and_utf8_budget() {
    let f = fixture(
        json!({"min_parallel_calls": 2, "budgets": {"output_bytes": 5, "wall_clock_ms": 10}}),
    );
    let r = report(&f, &evidence(json!("🧪")), 11, 0);
    assert_eq!(r["passed"], false);
    assert_eq!(r["metrics"]["output_bytes"], 6);
    assert_eq!(r["failures"].as_array().unwrap().len(), 3);
    assert!(r["result"].is_null());
}

#[test]
fn missing_harness_evidence_and_unused_rules_fail_closed() {
    let f = fixture(json!({}));
    assert_eq!(report(&f, &json!({}), 0, 0)["passed"], false);
    let mut e = evidence(json!(true));
    e["unused"] = json!([{"index": 0, "count": 1}]);
    assert_eq!(report(&f, &e, 0, 0)["passed"], false);
}

#[test]
fn camel_case_inputs_do_not_relax_snippet_filename_rules() {
    use super::super::store::validate_snippet_name;
    for key in ["chunkSize", "includeHandoffs", "repo_name", "repo-name"] {
        let body = format!(
            "---\nname: safe-name\ndescription: Synthetic input validation\ninputs:\n  {key}:\n    type: boolean\n    default: false\n---\n\n```js\nasync () => true\n```"
        );
        assert!(validate_snippet_body("safe-name", &body).is_ok(), "{key}");
    }
    assert!(validate_snippet_name("BadFilename").is_err());
    for key in ["../escape", "bad.key", "bad key", "__proto__"] {
        let body = format!(
            "---\nname: safe-name\ndescription: Synthetic input validation\ninputs:\n  {key}:\n    type: boolean\n---\n\n```js\nasync () => true\n```"
        );
        assert!(validate_snippet_body("safe-name", &body).is_err(), "{key}");
    }
}
