use super::*;
use serde_json::json;

fn fixture(value: Value) -> SnippetFixture {
    serde_json::from_value(value).expect("valid test fixture")
}

fn raw(result: Value) -> RawReport {
    RawReport {
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
