//! Catalog acceptance, rejection, history and generated-schema contracts.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use verify_core::*;

const CATALOG: &str = r#"{
  "schema": 1, "project": "fixture", "namespace": "TEST",
  "invariant": [{"id": "TEST-REQ-001", "title": "One terminal outcome",
    "kind": "safety", "severity": "critical", "model": "request",
    "checks": {"fixture": ["terminal"]}}]
}"#;

struct FixtureBackend {
    fairness: bool,
}
impl Backend for FixtureBackend {
    fn id(&self) -> BackendId {
        "fixture".to_owned().try_into().unwrap()
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            kinds: BTreeSet::from([Kind::Safety, Kind::Liveness]),
            fairness: self.fairness,
            concurrency: false,
            bounded: true,
        }
    }
    fn has_handle(&self, model: &str, handle: &str) -> bool {
        model == "request" && handle == "terminal"
    }
    fn availability(&self) -> Availability {
        panic!("validation must not probe tools")
    }
    fn run(&self, _: &CheckPlan) -> BackendReport {
        panic!("validation must not execute")
    }
}

fn parse(value: &Value) -> Result<ValidatedCatalog, CatalogError> {
    let backend = FixtureBackend { fairness: false };
    let mut registry = BackendRegistry::default();
    registry.register(&backend).unwrap();
    Catalog::from_json(&value.to_string(), &registry)
}

fn fixture() -> Value {
    serde_json::from_str(CATALOG).unwrap()
}

#[test]
fn validates_binding_without_tool_probe_or_execution() {
    let catalog = parse(&fixture()).unwrap();
    assert_eq!(catalog.catalog().project, "fixture");
    assert_eq!(
        catalog.catalog().invariant[0].status,
        InvariantStatus::Active
    );
    assert_eq!(catalog.uncovered().count(), 0);
}

#[test]
fn parses_toml_with_same_defaults_and_binding() {
    let backend = FixtureBackend { fairness: false };
    let mut registry = BackendRegistry::default();
    registry.register(&backend).unwrap();
    let text = r#"
schema = 1
project = "fixture"
namespace = "TEST"
[[invariant]]
id = "TEST-REQ-001"
title = "One terminal outcome"
kind = "safety"
severity = "critical"
model = "request"
[invariant.checks]
fixture = ["terminal"]
"#;
    let catalog = Catalog::from_toml(text, &registry).unwrap();
    assert_eq!(catalog.catalog().invariant[0].id.as_str(), "TEST-REQ-001");
    assert_eq!(catalog.catalog().invariant[0].owner, "");
    assert_eq!(catalog.uncovered().count(), 0);
}

#[test]
fn missing_and_empty_checks_are_explicitly_uncovered() {
    for checks in [None, Some(json!({}))] {
        let mut value = fixture();
        value["invariant"][0]
            .as_object_mut()
            .unwrap()
            .remove("checks");
        if let Some(checks) = checks {
            value["invariant"][0]["checks"] = checks;
        }
        let catalog = parse(&value).unwrap();
        assert_eq!(
            catalog
                .uncovered()
                .map(|i| i.id.as_str())
                .collect::<Vec<_>>(),
            ["TEST-REQ-001"]
        );
    }
}

#[test]
fn namespace_mismatch_is_not_a_schema_only_convention() {
    let mut value = fixture();
    value["invariant"][0]["id"] = json!("OTHER-REQ-001");
    assert!(
        matches!(parse(&value), Err(CatalogError::NamespaceMismatch { id, namespace })
        if id.as_str() == "OTHER-REQ-001" && namespace == "TEST")
    );
}

#[test]
fn retired_ids_still_collide_with_active_ids() {
    let mut value = fixture();
    let mut retired = value["invariant"][0].clone();
    retired["status"] = json!("retired");
    value["invariant"].as_array_mut().unwrap().push(retired);
    assert!(
        matches!(parse(&value), Err(CatalogError::DuplicateId(id)) if id.as_str() == "TEST-REQ-001")
    );
}

#[test]
fn unknown_backend_error_names_available_keys() {
    let mut value = fixture();
    value["invariant"][0]["checks"] = json!({"missing": ["terminal"]});
    assert!(
        matches!(parse(&value), Err(CatalogError::UnknownBackend { backend, available })
        if backend.as_str() == "missing" && available.iter().map(BackendId::as_str).collect::<Vec<_>>() == ["fixture"])
    );
}

#[test]
fn unsupported_kind_is_rejected_even_with_valid_handle() {
    let mut value = fixture();
    value["invariant"][0]["kind"] = json!("security");
    assert!(matches!(
        parse(&value),
        Err(CatalogError::UnsupportedKind {
            kind: Kind::Security,
            ..
        })
    ));
}

#[test]
fn liveness_requires_fairness_not_just_kind_flag() {
    let mut value = fixture();
    value["invariant"][0]["kind"] = json!("liveness");
    assert!(matches!(
        parse(&value),
        Err(CatalogError::UnsupportedKind {
            kind: Kind::Liveness,
            ..
        })
    ));
    let backend = FixtureBackend { fairness: true };
    let mut registry = BackendRegistry::default();
    registry.register(&backend).unwrap();
    assert!(Catalog::from_json(&value.to_string(), &registry).is_ok());
}

#[test]
fn unresolved_handle_is_not_silently_uncovered() {
    let mut value = fixture();
    value["invariant"][0]["checks"] = json!({"fixture": ["typo"]});
    assert!(
        matches!(parse(&value), Err(CatalogError::UnresolvedHandle { model, handle, .. })
        if model == "request" && handle == "typo")
    );
}

#[test]
fn handle_resolution_is_model_scoped() {
    let mut value = fixture();
    value["invariant"][0]["model"] = json!("other_model");
    assert!(
        matches!(parse(&value), Err(CatalogError::UnresolvedHandle { model, .. }) if model == "other_model")
    );
}

#[test]
fn empty_and_whitespace_handles_fail() {
    for handles in [json!([]), json!([""]), json!(["  "])] {
        let mut value = fixture();
        value["invariant"][0]["checks"]["fixture"] = handles;
        assert!(matches!(parse(&value), Err(CatalogError::Empty(_))));
    }
}

#[test]
fn duplicate_handles_fail() {
    let mut value = fixture();
    value["invariant"][0]["checks"]["fixture"] = json!(["terminal", "terminal"]);
    assert!(
        matches!(parse(&value), Err(CatalogError::DuplicateHandle { handle, .. }) if handle == "terminal")
    );
}

#[test]
fn unknown_fields_are_rejected_at_both_levels() {
    for path in ["", "/invariant/0"] {
        let mut value = fixture();
        value.pointer_mut(path).unwrap()["typo"] = json!(true);
        assert!(matches!(parse(&value), Err(CatalogError::Json(_))));
    }
}

#[test]
fn malformed_identifiers_are_rejected_during_deserialization() {
    for id in [
        "",
        "test-REQ-001",
        "TEST-req-001",
        "TEST-REQ-01",
        "TEST-REQ-001-extra",
        "TEST-REQ-１２３",
    ] {
        let mut value = fixture();
        value["invariant"][0]["id"] = json!(id);
        assert!(matches!(parse(&value), Err(CatalogError::Json(_))), "{id}");
    }
    for backend in ["", "Upper", "../bad", "a-b"] {
        let mut value = fixture();
        value["invariant"][0]["checks"] = json!({backend: ["terminal"]});
        assert!(
            matches!(parse(&value), Err(CatalogError::Json(_))),
            "{backend}"
        );
    }
}

#[test]
fn malformed_envelopes_fail_validation() {
    let mut value = fixture();
    value["schema"] = json!(2);
    assert!(matches!(parse(&value), Err(CatalogError::Schema(2))));
    value = fixture();
    value["namespace"] = json!("bad-prefix");
    assert!(matches!(parse(&value), Err(CatalogError::Namespace(_))));
    for path in ["/project", "/invariant/0/title", "/invariant/0/model"] {
        value = fixture();
        *value.pointer_mut(path).unwrap() = json!("  ");
        assert!(matches!(parse(&value), Err(CatalogError::Empty(_))));
    }
    value = fixture();
    value["invariant"] = json!([]);
    assert!(matches!(parse(&value), Err(CatalogError::Empty(_))));
}

#[test]
fn duplicate_registry_registration_does_not_replace_original() {
    let first = FixtureBackend { fairness: false };
    let second = FixtureBackend { fairness: true };
    let mut registry = BackendRegistry::default();
    registry.register(&first).unwrap();
    assert!(matches!(
        registry.register(&second),
        Err(RegistryError::Duplicate(_))
    ));
    assert!(!registry.get(&first.id()).unwrap().capabilities().fairness);
}

#[test]
fn history_requires_tombstones_and_forbids_reuse() {
    let old = parse(&fixture()).unwrap();
    let mut value = fixture();
    value["invariant"][0]["id"] = json!("TEST-REQ-002");
    let new = parse(&value).unwrap();
    assert!(
        matches!(new.validate_evolution(&old), Err(CatalogError::RemovedId(id)) if id.as_str() == "TEST-REQ-001")
    );
    value = fixture();
    value["invariant"][0]["status"] = json!("retired");
    let retired = parse(&value).unwrap();
    assert!(retired.validate_evolution(&old).is_ok());
    assert!(matches!(
        old.validate_evolution(&retired),
        Err(CatalogError::ReusedId(_))
    ));
    value = fixture();
    value["invariant"][0]["checks"] = json!({});
    value["invariant"][0]["model"] = json!("repurposed");
    assert!(matches!(
        parse(&value).unwrap().validate_evolution(&old),
        Err(CatalogError::ReusedId(_))
    ));
}

#[test]
fn history_allows_clarifications_but_rejects_other_projects() {
    let old = parse(&fixture()).unwrap();
    let mut value = fixture();
    value["invariant"][0]["title"] = json!("Clarified terminal outcome statement");
    assert!(parse(&value).unwrap().validate_evolution(&old).is_ok());
    value["project"] = json!("different");
    assert!(matches!(
        parse(&value).unwrap().validate_evolution(&old),
        Err(CatalogError::HistoryIdentity)
    ));
}

#[test]
fn duplicate_backend_keys_cannot_hide_an_invalid_binding() {
    let backend = FixtureBackend { fairness: false };
    let mut registry = BackendRegistry::default();
    registry.register(&backend).unwrap();
    let input = CATALOG.replace(
        "\"fixture\": [\"terminal\"]",
        "\"fixture\": [\"typo\"], \"fixture\": [\"terminal\"]",
    );
    assert!(matches!(
        Catalog::from_json(&input, &registry),
        Err(CatalogError::Json(_))
    ));
}

#[test]
fn generated_schema_matches_both_committed_artifacts() {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let generated = format!(
        "{}\n",
        serde_json::to_string_pretty(&catalog_schema()).unwrap()
    );
    for path in [
        "schemas/invariants.schema.json",
        "../docs/plans/verification-toolkit/schemas/invariants.schema.json",
    ] {
        assert_eq!(
            std::fs::read_to_string(workspace.join(path)).unwrap(),
            generated,
            "regenerate with just verify-schema"
        );
    }
}

#[test]
fn generated_schema_validates_wire_shape_without_enumerating_backends() {
    let schema = serde_json::to_value(catalog_schema()).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&fixture()));
    let mut value = fixture();
    value["invariant"][0]["checks"] = json!({"future_backend": ["future_handle"]});
    assert!(validator.is_valid(&value)); // Runtime registry resolves extensible keys.
    let invalid = [
        ("/schema", json!(2)),
        ("/namespace", json!("bad-prefix")),
        ("/invariant", json!([])),
        ("/invariant/0/title", json!("")),
        ("/invariant/0/id", json!("TEST-bad-01")),
        ("/invariant/0/checks", json!({"fixture": []})),
        ("/invariant/0/checks", json!({"fixture": [""]})),
        ("/invariant/0/checks", json!({"Bad-Key": ["terminal"]})),
    ];
    for (path, replacement) in invalid {
        let mut value = fixture();
        *value.pointer_mut(path).unwrap() = replacement;
        assert!(
            !validator.is_valid(&value),
            "accepted invalid {path}: {value}"
        );
    }
}
