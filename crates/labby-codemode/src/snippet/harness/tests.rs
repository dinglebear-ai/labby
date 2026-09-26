use super::super::store::SnippetSource;
use super::*;
use serde_json::json;

fn snippet(code: &str) -> ResolvedSnippet {
    ResolvedSnippet {
        name: "fixture-test".to_owned(),
        description: None,
        tags: Vec::new(),
        tools: None,
        inputs: BTreeMap::new(),
        source: SnippetSource::User,
        path: "fixture-test.js".into(),
        body: code.to_owned(),
    }
}

fn fixture(value: Value) -> SnippetFixture {
    SnippetFixture::from_value(value).unwrap()
}

#[test]
fn rejects_ambiguous_or_unbounded_fixtures() {
    for value in [
        json!({"calls": [], "expect": {"/ok": true}, "unknown": true}),
        json!({"calls": [], "expect": {"not-a-pointer": true}}),
        json!({"calls": [], "expect": {"/ok": true}, "budgets": {"wall_clock_ms": 0}}),
        json!({"calls": [], "expect": {"/ok": true}, "budgets": {"tool_calls": 513}}),
        json!({"calls": [], "expect": {"/ok": true}, "ignore_paths": ["/time"]}),
        json!({"calls": []}),
    ] {
        assert!(SnippetFixture::from_value(value).is_err());
    }
    for tool in [
        "state::readFile",
        "lab::gateway",
        "__lab_internal::describe_types",
        "bare",
        " a::b",
    ] {
        assert!(
            SnippetFixture::from_value(json!({
                "calls": [{"tool": tool, "response": {"returns": null}}], "expect": {"/ok": true}
            }))
            .is_err(),
            "{tool}"
        );
    }
}

#[test]
fn parameter_subsets_keep_arrays_exact() {
    assert!(fixture_host::subset(
        &json!({"filter": {"state": "open"}}),
        &json!({"filter": {"state": "open", "team": "U8"}, "limit": 10})
    ));
    assert!(!fixture_host::subset(
        &json!({"ids": [1]}),
        &json!({"ids": [1,2]})
    ));
}

#[test]
fn empty_fixture_is_deny_all_and_read_only() {
    let host = FixtureHost::new(Vec::new(), CodeModeConfig::default()).unwrap();
    assert!(!host.scope().allows("github", "search_pull_requests"));
    assert!(!crate::local_providers_allowed(
        &CodeModeCaller::TrustedLocal,
        &host.scope()
    ));
    assert!(host.scope().is_read_only());
}

#[tokio::test]
async fn fixture_runs_batch_against_real_runner_and_checks_snapshot() {
    let case = fixture(json!({
        "calls": [
            {"tool": "fake::read", "params": {"id": 1}, "response": {"returns": {"n": 7}}},
            {"tool": "fake::read", "params": {"id": 2}, "response": {"returns": {"n": 9}}}
        ],
        "snapshot": {"sum": 16}, "budgets": {"tool_calls": 2}
    }));
    let report = test_fixture(
        &snippet(
            r#"async () => {
        const b = await codemode.batch([() => callTool('fake::read', {id: 1}),
          () => callTool('fake::read', {id: 2})]);
        return {sum: b.ok.reduce((s, x) => s + x.value.n, 0)};
    }"#,
        ),
        json!({}),
        case,
    )
    .await
    .unwrap();
    assert!(report.passed, "{:?}", report);
    assert_eq!(report.metrics.tool_calls, 2);
    assert_eq!(report.metrics.calls_by_upstream["fake"], 2);
}

#[tokio::test]
async fn extra_call_fails_even_when_snippet_catches_it() {
    let case = fixture(json!({
        "calls": [{"tool": "fake::read", "response": {"returns": 1}}],
        "expect": {"/caught": true}
    }));
    let report = test_fixture(
        &snippet(
            r#"async () => {
        await callTool('fake::read', {});
        try { await callTool('fake::read', {}); } catch (_) { return {caught: true}; }
    }"#,
        ),
        json!({}),
        case,
    )
    .await
    .unwrap();
    assert!(!report.passed);
    assert_eq!(report.metrics.failed_calls, 1);
    assert_eq!(report.result.unwrap()["caught"], true);
}

#[tokio::test]
async fn deliberately_injected_errors_are_assertable() {
    let case = fixture(json!({
        "calls": [{"tool": "fake::read", "response": {"error": {
            "kind": "upstream_timeout", "message": "synthetic timeout"}}}],
        "expected_failures": 1, "expect": {"/failed": 1}
    }));
    let report = test_fixture(
        &snippet(
            r#"async () => {
        const b = await codemode.batch([() => callTool('fake::read', {})]);
        return {failed: b.failed.length};
    }"#,
        ),
        json!({}),
        case,
    )
    .await
    .unwrap();
    assert!(report.passed, "{:?}", report);
}

#[tokio::test]
async fn unused_rules_and_snapshot_mismatches_fail() {
    let case = fixture(json!({
        "calls": [{"tool": "fake::read", "response": {"returns": null}}],
        "snapshot": {"ok": true}
    }));
    let report = test_fixture(&snippet("async () => ({ok: false})"), json!({}), case)
        .await
        .unwrap();
    assert!(!report.passed);
    assert!(report.violations.iter().any(|v| v.contains("rule 0")));
    assert!(report.violations.iter().any(|v| v.contains("snapshot")));
}

#[tokio::test]
async fn snapshots_only_ignore_explicit_existing_paths() {
    let case = fixture(
        json!({"calls": [], "snapshot": {"ok": true, "at": "old"}, "ignore_paths": ["/at"]}),
    );
    let report = test_fixture(
        &snippet("async () => ({ok: true, at: 'new'})"),
        json!({}),
        case,
    )
    .await
    .unwrap();
    assert!(report.passed, "{:?}", report);
}

#[tokio::test]
async fn output_budget_counts_utf8_bytes_before_shaping() {
    let case =
        fixture(json!({"calls": [], "expect": {"/text": "éé"}, "budgets": {"output_bytes": 14}}));
    let report = test_fixture(&snippet("async () => ({text: 'éé'})"), json!({}), case)
        .await
        .unwrap();
    assert!(!report.passed);
    assert_eq!(report.metrics.output_bytes, 15);
    assert!(report.result.is_none());
}

#[tokio::test]
async fn input_escaping_is_not_rewritten() {
    let case = fixture(json!({"calls": [], "expect": {"/text": "literal\\n and newline\n"}}));
    let report = test_fixture(
        &snippet("async (input) => input"),
        json!({"text": "literal\\n and newline\n"}),
        case,
    )
    .await
    .unwrap();
    assert!(report.passed, "{:?}", report);
}

#[tokio::test]
async fn hung_snippet_is_killed_and_reported() {
    let case =
        fixture(json!({"calls": [], "expect": {"/ok": true}, "budgets": {"wall_clock_ms": 1000}}));
    let report = test_fixture(&snippet("async () => { while (true) {} }"), json!({}), case)
        .await
        .unwrap();
    assert!(!report.passed);
    assert!(report.error.is_some());
    assert!(report.metrics.elapsed_ms < 5000);
}

#[test]
fn expected_errors_cannot_cover_unmatched_tool_calls() {
    let result = SnippetFixture::from_value(json!({
        "calls": [], "expect": {"/ok": true}, "expected_failures": 1
    }));
    assert!(result.is_err());
}

#[tokio::test]
async fn fixture_input_preserves_proto_as_an_own_json_key() {
    let case = fixture(json!({"calls": [], "expect": {"/own": true, "/inherited": false}}));
    let report = test_fixture(
        &snippet(
            r#"async (input) => ({
        own: Object.prototype.hasOwnProperty.call(input, '__proto__'),
        inherited: input.surprise === true
    })"#,
        ),
        json!({"__proto__": {"surprise": true}}),
        case,
    )
    .await
    .unwrap();
    assert!(report.passed, "{:?}", report);
}

#[tokio::test]
async fn application_truncated_field_is_not_runtime_truncation() {
    let case = fixture(json!({"calls": [], "expect": {"/truncated": true}}));
    let report = test_fixture(&snippet("async () => ({truncated: true})"), json!({}), case)
        .await
        .unwrap();
    assert!(report.passed, "{:?}", report);
    assert!(!report.metrics.truncated);
}
