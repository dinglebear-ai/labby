use super::*;
#[test]
fn rejects_unvisited_schema_defects_and_bounds_reference_expansion() {
    for schema in [
        json!({"type":"object","properties":{"optional":{"$ref":"#/$defs/missing"}}}),
        json!({"$ref":"#/$defs/invalid","$defs":{"invalid":42}}),
        json!({"$ref":"#/$defs/loop","$defs":{"loop":{"$ref":"#/$defs/loop"}}}),
        json!({"type":"object","patternProperties":{"[":{"type":"string"}}}),
    ] {
        assert!(check_schema(&schema).is_err(), "{schema}");
        assert!(validate_value(&json!({}), &schema).is_err());
    }
    let mut defs = Map::from_iter([("d0".into(), json!({}))]);
    for n in 1..=16 {
        let reference = format!("#/$defs/d{}", n - 1);
        defs.insert(
            format!("d{n}"),
            json!({"allOf":[{"$ref":reference},{"$ref":reference}]}),
        );
    }
    assert!(check_schema(&json!({"$ref":"#/$defs/d16","$defs":defs})).is_err());
}

#[test]
fn shared_validation_allowance_cannot_reset_per_value() {
    let schema = json!({"type":"array","items":{"type":"integer"}});
    let value = json!([0, 1, 2, 3]);
    check_schema(&schema).unwrap();
    let mut budget = crate::schema::SchemaValidationBudget::new(6);
    validate_value_with_budget(&value, &schema, &mut budget).unwrap();
    assert!(validate_value_with_budget(&value, &schema, &mut budget).is_err());
}

#[test]
fn fingerprints_ignore_object_order_and_include_both_contracts() {
    let a: FixtureSchemas = serde_json::from_str(r#"{"input_schema":{"type":"object","properties":{"a":{"type":"string"},"b":{"type":"integer"}}},"output_schema":{"type":"boolean"}}"#).unwrap();
    let mut b: FixtureSchemas = serde_json::from_str(r#"{"output_schema":{"type":"boolean"},"input_schema":{"properties":{"b":{"type":"integer"},"a":{"type":"string"}},"type":"object"}}"#).unwrap();
    assert_eq!(a.contract_fingerprint(), b.contract_fingerprint());
    b.output_schema = None;
    assert_ne!(a.contract_fingerprint(), b.contract_fingerprint());
}

#[test]
fn generates_refs_required_enums_and_bounded_arrays_deterministically() {
    let schema = json!({"type":"object","required":["items"],"properties":{
            "items":{"type":"array","items":{"$ref":"#/$defs/item"}},"extra":{"type":"boolean"}},
            "$defs":{"item":{"type":"object","required":["status"],"properties":{"status":{"enum":["open","closed"]}}}}});
    assert_eq!(
        generate_response(&schema, FixtureVariant::Populated).unwrap(),
        json!({"extra":true,"items":[{"status":"open"}]})
    );
    assert_eq!(
        generate_response(&schema, FixtureVariant::Minimal).unwrap(),
        json!({"items":[]})
    );
}
#[test]
fn does_not_copy_defaults_and_reports_unsupported_or_unsatisfiable_constraints() {
    assert_eq!(
        generate_response(
            &json!({"type":"string","default":"private"}),
            FixtureVariant::Populated
        )
        .unwrap(),
        "synthetic"
    );
    for schema in [
        json!({"format":"email","type":"string"}),
        json!({"$ref":"https://example.com/schema"}),
        json!({"$ref":"#"}),
        json!({"type":"string","pattern":"^[0-9]+$"}),
        json!({"type":"array","minItems":65}),
    ] {
        assert!(generate_response(&schema, FixtureVariant::Populated).is_err());
    }
}

#[test]
fn integer_generation_respects_fractional_bounds_on_both_sides_of_zero() {
    for (bounds, expected) in [
        (json!({"maximum":-0.5}), -1),
        (json!({"minimum":0.5}), 1),
        (json!({"minimum":-1.5,"maximum":-0.5}), -1),
        (json!({"minimum":0.5,"maximum":1.5}), 1),
        (json!({"minimum":-0.5,"maximum":0.5}), 0),
        (json!({"minimum":-2,"maximum":-2}), -2),
    ] {
        let mut schema = bounds;
        schema["type"] = json!("integer");
        for variant in [FixtureVariant::Minimal, FixtureVariant::Populated] {
            let value = generate_response(&schema, variant).unwrap();
            assert_eq!(value, json!(expected), "schema: {schema}");
            validate_value(&value, &schema).unwrap();
        }
    }
    for schema in [
        json!({"type":"integer","minimum":0.1,"maximum":0.9}),
        json!({"type":"integer","minimum":-0.9,"maximum":-0.1}),
        json!({"type":"integer","minimum":2,"maximum":1}),
    ] {
        assert!(generate_response(&schema, FixtureVariant::Populated).is_err());
    }
}

#[test]
fn exclusive_numeric_bounds_remain_explicitly_unsupported() {
    for keyword in ["exclusiveMinimum", "exclusiveMaximum"] {
        let mut schema = json!({"type":"integer"});
        schema[keyword] = json!(0.5);
        assert!(check_schema(&schema).is_err());
        assert!(generate_response(&schema, FixtureVariant::Populated).is_err());
    }
}
