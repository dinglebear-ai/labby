//! Pure assertions shared by mock and live acceptance tests.
use super::model::SnippetTestCase;
use serde_json::Value;

/// Evaluate assertions without embedding result/fixture values in diagnostics.
pub(super) fn check_result(case: &SnippetTestCase, result: Option<&Value>) -> Vec<String> {
    let mut failures = Vec::new();
    if result.is_none() {
        failures.push("snippet returned undefined".to_string());
    }
    for (pointer, expected) in &case.expect {
        if result.and_then(|v| v.pointer(pointer)) != Some(expected) {
            failures.push(format!("result assertion failed at {pointer}"));
        }
    }
    for pointer in &case.absent {
        if result.and_then(|v| v.pointer(pointer)).is_some() {
            failures.push(format!("result unexpectedly contains {pointer}"));
        }
    }
    if let Some(snapshot) = &case.snapshot {
        let normalize = |value: &Value| {
            let mut value = value.clone();
            for p in &case.normalize {
                if let Some(field) = value.pointer_mut(p) {
                    *field = Value::Null;
                }
            }
            value
        };
        if result.map(normalize) != Some(normalize(snapshot)) {
            failures.push("normalized result snapshot differs".to_string());
        }
    }
    if result.and_then(|v| v.get("ok")).and_then(Value::as_bool) == Some(false)
        && case.expect.get("/ok") != Some(&Value::Bool(false))
    {
        failures.push(
            "snippet returned ok=false without an explicit negative-test assertion".to_string(),
        );
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn case(v: Value) -> SnippetTestCase {
        serde_json::from_value(v).unwrap()
    }
    #[test]
    fn distinguishes_missing_from_null() {
        let c = case(json!({"expect":{"/a":null},"absent":["/b"]}));
        assert!(check_result(&c, Some(&json!({"a":null}))).is_empty());
        assert_eq!(check_result(&c, Some(&json!({"b":null}))).len(), 2);
    }
    #[test]
    fn normalized_snapshot_is_symmetric_and_does_not_hide_assertions() {
        let c = case(json!({"snapshot":{"at":"old","n":1},"normalize":["/at"],"expect":{"/n":1}}));
        assert!(check_result(&c, Some(&json!({"at":"new","n":1}))).is_empty());
        assert_eq!(check_result(&c, Some(&json!({"at":"new","n":2}))).len(), 2);
    }
    #[test]
    fn failure_requires_explicit_negative_test() {
        assert!(!check_result(&case(json!({})), Some(&json!({"ok":false}))).is_empty());
        assert!(
            check_result(
                &case(json!({"expect":{"/ok":false}})),
                Some(&json!({"ok":false}))
            )
            .is_empty()
        );
        assert!(!check_result(&case(json!({})), None).is_empty());
    }
    #[test]
    fn validates_fixture_limits_and_reserved_tools() {
        for v in [
            json!({"calls":[{"tool":"state::readFile"}]}),
            json!({"budgets":{"wall_clock_ms":30001}}),
            json!({"expect":{"not/a/pointer":true}}),
            json!({"expect":{"/~2":true}}),
            json!({"calls":[{"tool":"example::read","times":0}]}),
        ] {
            assert!(case(v).validate().is_err());
        }
        assert!(case(json!({"expect":{"/a~1b~0":1}})).validate().is_ok());
    }
}
