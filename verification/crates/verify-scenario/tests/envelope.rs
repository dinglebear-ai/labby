//! Envelope, content-identity, path and schema regression contracts.

use serde_json::{Value, json};
use verify_scenario::{EnvelopeError, MAX_SCENARIO_BYTES, MAX_STEPS, Scenario, scenario_schema};

fn fixture() -> Value {
    json!({"schema":1,"project":"example","model":"counter","invariant":"EX-COUNT-001",
        "origin":{"kind":"manual"},"steps":[],"expect":"invariant_holds"})
}

#[test]
fn missing_initial_and_status_default_but_null_is_rejected() {
    let parsed = Scenario::from_json(&fixture().to_string()).unwrap();
    assert!(parsed.scenario().initial.is_empty());
    assert_eq!(parsed.scenario().status, verify_scenario::Status::Active);
    let mut value = fixture();
    value["initial"] = Value::Null;
    assert!(Scenario::from_json(&value.to_string()).is_err());
    value["initial"] = json!([]);
    assert!(Scenario::from_json(&value.to_string()).is_err());
}

#[test]
fn malformed_envelopes_fail_closed() {
    for (field, value) in [
        ("schema", json!(2)),
        ("project", json!(" ")),
        ("model", json!("")),
        ("invariant", json!("bad")),
        ("expect", json!("verified")),
        ("status", json!("pending")),
        ("extra", json!(true)),
        ("bounds", json!({"depth":1.2})),
        ("bounds", json!({"depth":{}})),
        ("origin", json!({"kind":"manual","extra":true})),
        ("origin", json!({"kind":"unknown"})),
    ] {
        let mut input = fixture();
        input[field] = value;
        assert!(Scenario::from_json(&input.to_string()).is_err(), "{input}");
    }
    assert!(Scenario::from_json("{\"schema\":1,\"schema\":1}").is_err());
}

#[test]
fn duplicate_keys_are_rejected_before_opaque_payloads_lose_information() {
    let input = fixture().to_string();
    for replacement in [
        r#""steps":[{"actor":"a","value":3,"value":0}]"#,
        r#""steps":[{"actor":"a","value":3,"\u0076alue":0}]"#,
        r#""steps":[],"initial":{"value":3,"value":0}"#,
        r#""steps":[],"initial":{"nested":[{"a":1,"a":2}]}"#,
        r#""steps":[],"bounds":{"depth":1,"depth":2}"#,
    ] {
        let malformed = input.replace(r#""steps":[]"#, replacement);
        let error = Scenario::from_json(&malformed).unwrap_err();
        assert!(
            error.to_string().contains("duplicate JSON object key"),
            "{error}"
        );
    }
    let ordinary = input.replace(
        r#""steps":[]"#,
        r#""steps":[{"a":null,"b":true,"c":1.25,"d":[-1,18446744073709551615]}]"#,
    );
    let validated = Scenario::from_json(&ordinary).unwrap();
    assert_eq!(
        validated.scenario().steps[0],
        json!({"a":null,"b":true,"c":1.25,"d":[-1,u64::MAX]})
    );
}

#[test]
fn bounds_and_all_origins_round_trip() {
    for origin in [
        "stateright",
        "kani",
        "loom",
        "shuttle",
        "alloy",
        "tla",
        "fuzz",
        "incident",
        "manual",
    ] {
        let mut input = fixture();
        input["origin"] = json!({"kind":origin,"seed":u64::MAX});
        input["bounds"] = json!({"signed":-1,"unsigned":u64::MAX,"symbol":"finite","flag":true});
        let parsed = Scenario::from_json(&input.to_string()).unwrap();
        let reparsed =
            Scenario::from_json(&serde_json::to_string(parsed.scenario()).unwrap()).unwrap();
        assert_eq!(parsed, reparsed);
    }
}

#[test]
fn fingerprint_excludes_provenance_status_bounds_but_includes_expectation_and_order() {
    let mut input = fixture();
    input["steps"] = json!([{"z":1,"a":2},3]);
    let original = Scenario::from_json(&input.to_string()).unwrap();
    input["origin"] = json!({"kind":"incident","reference":"issue-1"});
    input["status"] = json!("quarantined");
    input["bounds"] = json!({"depth":9});
    let alternate = Scenario::from_json(&input.to_string()).unwrap();
    assert_eq!(original.fingerprint(), alternate.fingerprint());
    input["expect"] = json!("invariant_violated");
    assert_ne!(
        original.fingerprint(),
        Scenario::from_json(&input.to_string())
            .unwrap()
            .fingerprint()
    );
    input["expect"] = json!("invariant_holds");
    input["steps"] = json!([3,{"a":2,"z":1}]);
    assert_ne!(
        original.fingerprint(),
        Scenario::from_json(&input.to_string())
            .unwrap()
            .fingerprint()
    );
}

#[test]
fn supplied_hash_is_verified_and_canonical_keys_are_order_independent() {
    let original = Scenario::from_json(&fixture().to_string()).unwrap();
    let mut sealed = serde_json::to_value(original.scenario()).unwrap();
    assert_eq!(original, Scenario::from_json(&sealed.to_string()).unwrap());
    sealed["steps"] = json!([1]);
    assert!(matches!(
        Scenario::from_json(&sealed.to_string()),
        Err(EnvelopeError::FingerprintMismatch)
    ));
    let a = r#"{"schema":1,"project":"example","model":"counter","invariant":"EX-COUNT-001","origin":{"kind":"manual"},"initial":{"a":1,"b":{"c":3,"d":4}},"steps":[],"expect":"invariant_holds"}"#;
    let b = a.replace(r#""c":3,"d":4"#, r#""d":4,"c":3"#);
    assert_eq!(
        Scenario::from_json(a).unwrap().fingerprint(),
        Scenario::from_json(&b).unwrap().fingerprint()
    );
}

#[test]
fn input_and_step_limits_apply_to_constructed_data_too() {
    assert!(matches!(
        Scenario::from_json(&" ".repeat(MAX_SCENARIO_BYTES + 1)),
        Err(EnvelopeError::TooLarge)
    ));
    let mut raw: Scenario = serde_json::from_value(fixture()).unwrap();
    raw.steps = vec![Value::Null; MAX_STEPS + 1];
    assert!(raw.validate().is_err());
    let mut raw: Scenario = serde_json::from_value(fixture()).unwrap();
    raw.initial
        .insert("payload".into(), json!("x".repeat(MAX_SCENARIO_BYTES)));
    assert!(matches!(raw.validate(), Err(EnvelopeError::TooLarge)));
}

#[test]
fn corpus_paths_encode_traversal_and_bound_components() {
    for project in [
        "../escape",
        "/absolute",
        "C:\\outside",
        "a/b",
        "..",
        &"é".repeat(120),
    ] {
        let mut input = fixture();
        input["project"] = json!(project);
        input["model"] = json!("../../x");
        let path = Scenario::from_json(&input.to_string())
            .unwrap()
            .corpus_path();
        assert!(!path.is_absolute());
        assert_eq!(path.components().count(), 3);
        for component in path.components() {
            assert!(matches!(component, std::path::Component::Normal(_)));
            assert!(component.as_os_str().len() < 256);
        }
    }
}

#[test]
fn generated_schema_matches_both_artifacts_and_wire_contract() {
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&scenario_schema()).unwrap()
    );
    assert_eq!(
        rendered,
        include_str!("../../../schemas/scenario.schema.json")
    );
    assert_eq!(
        rendered,
        include_str!("../../../../docs/plans/verification-toolkit/schemas/scenario.schema.json")
    );
    let schema = serde_json::to_value(scenario_schema()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&fixture()));
    let sealed = Scenario::from_json(&fixture().to_string()).unwrap();
    assert!(validator.is_valid(&serde_json::to_value(sealed.scenario()).unwrap()));
    for (key, value) in [
        ("initial", Value::Null),
        ("expect", json!("verified")),
        ("schema", json!(2)),
        ("fingerprint", json!("fake")),
    ] {
        let mut invalid = fixture();
        invalid[key] = value;
        assert!(!validator.is_valid(&invalid), "{invalid}");
    }
}
