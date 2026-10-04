use super::*;
use serde_json::json;

fn fixture(value: Value) -> SnippetFixture {
    serde_json::from_value(value).expect("valid test fixture")
}

fn raw(result: Value) -> RawReport {
    RawReport {
        contract_calls: Vec::new(),
        result,
        exception: None,
        calls: Vec::new(),
        attempted: 0,
        unexpected: 0,
        max_in_flight: 0,
        unused: Vec::new(),
    }
}

#[test]
fn fixture_defaults_and_unknown_keys() {
    let f = fixture(json!({}));
    f.validate().unwrap();
    assert_eq!(f.budgets.tool_calls, 40);
    assert_eq!(f.budgets.output_bytes, 16_000);
    assert!(serde_json::from_value::<SnippetFixture>(json!({"budgest": {}})).is_err());
}

#[test]
fn saved_contracts_validate_results_and_actual_arguments_without_echoing_values() {
    let contract = json!({"input_schema":{"type":"object","required":["id"],"properties":{"id":{"type":"integer"}}},
        "output_schema":{"type":"object","required":["items"],"properties":{"items":{"type":"array"}}}});
    let f = fixture(
        json!({"calls":[{"tool":"synthetic::lookup","result":{"items":[]}}],
        "schemas":{"synthetic::lookup":contract}}),
    );
    f.validate().unwrap();
    let mut outcome = raw(json!({"ok":true}));
    outcome.contract_calls.push(ContractCall {
        tool: "synthetic::lookup".into(),
        params: json!({"id":"private-value"}),
    });
    let report = evaluate("test", outcome, &f, 1, false).unwrap();
    assert!(!report.passed);
    assert!(report.failures[0].contains("input schema"));
    assert!(
        !serde_json::to_string(&report)
            .unwrap()
            .contains("private-value")
    );
    let bad = fixture(
        json!({"calls":[{"tool":"synthetic::lookup","result":{"items":1}}],
        "schemas":{"synthetic::lookup":contract}}),
    );
    assert!(bad.validate().is_err());
    let rejection = fixture(
        json!({"calls":[{"tool":"synthetic::lookup","error":{"kind":"timeout","message":"synthetic"}}],
        "schemas":{"synthetic::lookup":contract}}),
    );
    rejection.validate().unwrap();
}

#[tokio::test]
async fn configured_live_source_limit_is_applied_before_fixture_wrapping() {
    let snippet: ResolvedSnippet = serde_json::from_value(json!({
        "name": "bounded",
        "description": null,
        "tags": [],
        "source": "user",
        "path": "/tmp/bounded.js",
        "body": "async () => true"
    }))
    .unwrap();
    let error = run_fixture_with_source_limit(&snippet, json!({}), &SnippetFixture::default(), 32)
        .await
        .expect_err("mock must enforce the lower live source ceiling");
    assert_eq!(error.kind(), "invalid_param");
}

#[test]
fn rejects_bad_rules_and_budgets() {
    for value in [
        json!({"calls": [{"tool": "not-qualified"}]}),
        json!({"calls": [{"tool": "state::readFile"}]}),
        json!({"calls": [{"tool": "__lab_internal::describe_types"}]}),
        json!({"calls": [{"tool": "github::get_me", "times": 0}]}),
        json!({"calls": [{"tool": "github::get_me", "match": []}]}),
        json!({"calls": [{"tool": "github::get_me", "result": 1,
            "error": {"kind": "test", "message": "synthetic"}}]}),
        json!({"budgets": {"wall_clock_ms": 30_001}}),
        json!({"budgets": {"tool_calls": 513}}),
        json!({"budgets": {"output_bytes": 0}}),
        json!({"expect": {"not-a-pointer": true}}),
    ] {
        assert!(
            fixture(value.clone()).validate().is_err(),
            "accepted {value}"
        );
    }
}

#[test]
fn assertions_compare_exact_values_and_distinguish_missing_from_null() {
    let f = fixture(json!({"expect": {"/count": 2, "/nullable": null}}));
    assert!(
        evaluate(
            "test",
            raw(json!({"count": 2, "nullable": null})),
            &f,
            1,
            false
        )
        .unwrap()
        .passed
    );
    assert!(
        !evaluate("test", raw(json!({"count": 2})), &f, 1, false)
            .unwrap()
            .passed
    );
    assert!(
        !evaluate(
            "test",
            raw(json!({"count": "2", "nullable": null})),
            &f,
            1,
            false
        )
        .unwrap()
        .passed
    );
}

#[test]
fn snapshots_normalize_only_selected_values() {
    let f = fixture(json!({"snapshot": {"count": 2, "time": 1}, "ignore_paths": ["/time"]}));
    assert!(
        evaluate("test", raw(json!({"count": 2, "time": 99})), &f, 1, false)
            .unwrap()
            .passed
    );
    assert!(
        !evaluate("test", raw(json!({"count": 3, "time": 99})), &f, 1, false)
            .unwrap()
            .passed
    );
}

#[test]
fn swallowed_unexpected_calls_and_unused_rules_fail() {
    let f = SnippetFixture::default();
    let mut r = raw(json!({"ok": true}));
    r.unexpected = 1;
    assert!(!evaluate("test", r, &f, 1, false).unwrap().passed);
    let mut r = raw(json!({"ok": true}));
    r.unused.push(json!({"index": 0, "count": 1}));
    assert!(!evaluate("test", r, &f, 1, false).unwrap().passed);
    assert!(
        !evaluate("test", raw(json!({"ok": false})), &f, 1, false)
            .unwrap()
            .passed
    );
    assert!(
        !evaluate("test", raw(json!({"ok": true})), &f, 1, true)
            .unwrap()
            .passed
    );
}

#[test]
fn raw_utf8_output_is_measured_before_shaping() {
    let f = fixture(json!({"budgets": {"output_bytes": 4}}));
    let report = evaluate("test", raw(json!("éé")), &f, 1, false).unwrap();
    assert_eq!(report.metrics.output_bytes, 6);
    assert!(!report.passed);
    assert!(report.result.is_none());
}

#[test]
fn runtime_and_call_budgets_are_independent() {
    let f = fixture(json!({"budgets": {"wall_clock_ms": 5, "tool_calls": 1}}));
    let mut r = raw(json!(null));
    r.attempted = 2;
    let report = evaluate("test", r, &f, 6, false).unwrap();
    assert!(
        report
            .failures
            .iter()
            .any(|s| s == "tool_calls budget exceeded")
    );
    assert!(
        report
            .failures
            .iter()
            .any(|s| s == "wall_clock_ms budget exceeded")
    );
}

#[test]
fn trace_is_bounded_but_full_count_remains() {
    let f = fixture(json!({"budgets": {"tool_calls": 40}}));
    let mut r = raw(json!(true));
    r.attempted = 40;
    r.calls = (0..40)
        .map(|_| FixtureTrace {
            tool: "test::read".into(),
            ok: true,
            fixture_index: Some(0),
            elapsed_ms: 0,
        })
        .collect();
    let report = evaluate("test", r, &f, 1, false).unwrap();
    assert!(report.passed);
    assert_eq!(report.calls.len(), 32);
    assert!(report.trace_truncated);
    assert_eq!(report.metrics.tool_calls, 40);
}

#[test]
fn absent_paths_distinguish_missing_from_null() {
    let f = fixture(json!({"absent": ["/optional"]}));
    assert!(
        evaluate("test", raw(json!({})), &f, 1, false)
            .unwrap()
            .passed
    );
    assert!(
        !evaluate("test", raw(json!({"optional": null})), &f, 1, false)
            .unwrap()
            .passed
    );
    assert!(
        !evaluate("test", raw(json!({"optional": false})), &f, 1, false)
            .unwrap()
            .passed
    );
}

#[test]
fn invalid_pointer_escapes_cannot_pass_absence_assertions() {
    for value in [
        json!({"absent": ["/missing~2field"]}),
        json!({"expect": {"/value~": 1}}),
        json!({"ignore_paths": ["/time~3"]}),
    ] {
        assert!(fixture(value).validate().is_err());
    }
    let f = fixture(json!({"expect": {"/a~1b/~0key": 7}}));
    f.validate().unwrap();
    assert!(
        evaluate("escaped", raw(json!({"a/b": {"~key": 7}})), &f, 1, false)
            .unwrap()
            .passed
    );
}

#[test]
fn explicit_null_snapshot_is_an_assertion_not_an_omission() {
    let f = fixture(json!({"snapshot": null}));
    assert_eq!(f.snapshot, Some(Value::Null));
    assert!(
        !evaluate("null", raw(json!({"unexpected": true})), &f, 1, false)
            .unwrap()
            .passed
    );
    assert!(
        evaluate("null", raw(Value::Null), &f, 1, false)
            .unwrap()
            .passed
    );
    let omitted = fixture(json!({}));
    assert_eq!(omitted.snapshot, None);
    let encoded = serde_json::to_value(&omitted).unwrap();
    assert!(encoded.get("snapshot").is_none());
    assert_eq!(fixture(encoded).snapshot, None);
    assert_eq!(
        fixture(serde_json::to_value(&f).unwrap()).snapshot,
        Some(Value::Null)
    );
    assert!(
        evaluate(
            "omitted",
            raw(json!({"unexpected": true})),
            &omitted,
            1,
            false
        )
        .unwrap()
        .passed
    );
}

#[test]
fn nested_and_artifact_rules_are_bounded_and_portable() {
    let f = fixture(json!({
        "params": {"alias": "fixture-host"},
        "snippets": [{"name": "docker-host-inventory", "match": {"alias": "fixture-host"}, "result": {"ok": true}}],
        "artifacts": [{"path": "report/result.json", "content_type": "application/json", "contains": ["fixture-host"]}]
    }));
    f.validate().expect("portable synthetic rules");
    for value in [
        json!({"snippets": [{"name": "../escape"}]}),
        json!({"snippets": [{"name": "child", "times": 0}]}),
        json!({"snippets": [{"name": "child", "match": []}]}),
        json!({"artifacts": [{"path": "../escape"}]}),
        json!({"artifacts": [{"path": "/absolute"}]}),
        json!({"artifacts": [{"path": "report", "times": 0}]}),
        json!({"calls": [{"tool": "github::get_me", "times": 512}], "snippets": [{"name": "child"}]}),
    ] {
        assert!(fixture(value).validate().is_err());
    }
}

#[test]
fn fixture_contract_validation_shares_work_across_responses_and_arguments() {
    let schema =
        json!({"type":"object","properties":{"items":{"type":"array","items":{"type":"integer"}}}});
    let value = json!({"items":vec![0;128]});
    let rules = (0..130)
        .map(|_| json!({"tool":"synthetic::lookup","result":value}))
        .collect::<Vec<_>>();
    let responses =
        fixture(json!({"calls":rules,"schemas":{"synthetic::lookup":{"output_schema":schema}}}));
    assert!(responses.validate().is_err());
    let arguments = fixture(json!({"calls":[{"tool":"synthetic::lookup","times":130}],
        "schemas":{"synthetic::lookup":{"input_schema":schema}}}));
    arguments.validate().unwrap();
    let mut outcome = raw(json!({"ok":true}));
    outcome.contract_calls = (0..130)
        .map(|_| ContractCall {
            tool: "synthetic::lookup".into(),
            params: value.clone(),
        })
        .collect();
    let report = evaluate("bounded", outcome, &arguments, 1, false).unwrap();
    assert!(!report.passed);
    assert_eq!(report.failures.len(), 1);
    assert!(report.failures[0].contains("work budget"));
}

#[test]
fn argument_validation_stops_at_the_fixture_deadline() {
    let f = fixture(json!({"budgets":{"wall_clock_ms":1},
        "calls":[{"tool":"synthetic::lookup"}],
        "schemas":{"synthetic::lookup":{"input_schema":{"type":"object"}}}}));
    let mut outcome = raw(json!({"ok":true}));
    outcome.contract_calls.push(ContractCall {
        tool: "synthetic::lookup".into(),
        params: json!({}),
    });
    let report = evaluate("deadline", outcome, &f, 1, false).unwrap();
    assert!(!report.passed);
    assert!(
        report
            .failures
            .iter()
            .any(|f| f.contains("validation deadline"))
    );
    assert!(report.metrics.wall_clock_ms >= 1);
}

#[test]
fn fixture_admission_rejects_defects_in_absent_optional_inputs() {
    for input_schema in [
        json!({"type":"object","properties":{"optional":{"$ref":"#/$defs/missing"}}}),
        json!({"type":"object","patternProperties":{"[":{"type":"string"}}}),
    ] {
        let f = fixture(json!({"calls":[{"tool":"synthetic::lookup","result":true}],
            "schemas":{"synthetic::lookup":{"input_schema":input_schema,"output_schema":{"type":"boolean"}}}}));
        assert!(f.validate().is_err());
    }
}

#[test]
fn runner_reports_must_include_argument_contract_metadata() {
    let report = json!({"result":{"ok":true},"exception":null,"calls":[],
        "attempted":0,"unexpected":0,"max_in_flight":0,"unused":[]});
    assert!(serde_json::from_value::<RawReport>(report.clone()).is_err());
    let mut complete = report;
    complete["contract_calls"] = json!([]);
    assert!(serde_json::from_value::<RawReport>(complete).is_ok());
}
