//! Catalog validation tests.
//!
//! Every rule is tested on the input it must *reject*. A validator exercised
//! only on valid input is untested: it would pass just as happily if the rule
//! were deleted.

use std::collections::BTreeMap;

use crate::backend::{BackendId, Capabilities};
use crate::catalog::{Catalog, CatalogError};
use crate::invariant::Kind;

fn backend(id: &str, kinds: &[Kind]) -> (BackendId, Capabilities) {
    (
        BackendId::parse(id).expect("valid backend id"),
        Capabilities {
            kinds: kinds.iter().copied().collect(),
            bounded: false,
            produces_counterexamples: true,
        },
    )
}

fn registry(entries: Vec<(BackendId, Capabilities)>) -> BTreeMap<BackendId, Capabilities> {
    entries.into_iter().collect()
}

const VALID: &str = r#"
schema = 1
project = "labby"
namespace = "LABBY"

[[invariant]]
id = "LABBY-REQ-001"
title = "A request has at most one authoritative terminal outcome."
kind = "safety"
severity = "critical"
model = "gateway"

[invariant.checks]
stateright = ["request_lifecycle"]
"#;

#[test]
fn a_well_formed_catalog_validates() {
    let catalog = Catalog::parse(VALID).expect("parse");
    let backends = registry(vec![backend("stateright", &[Kind::Safety])]);
    assert_eq!(catalog.validate(&backends), Ok(()));
    assert_eq!(catalog.invariants.len(), 1);
    assert!(catalog.uncovered().is_empty());
}

#[test]
fn rule_1_rejects_a_duplicate_id() {
    let text = VALID.to_owned()
        + r#"
[[invariant]]
id = "LABBY-REQ-001"
title = "A second property reusing the first one's id."
kind = "safety"
severity = "high"
model = "gateway"
"#;
    let catalog = Catalog::parse(&text).expect("parse");
    let backends = registry(vec![backend("stateright", &[Kind::Safety])]);
    let errors = catalog
        .validate(&backends)
        .expect_err("duplicate must fail");
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, CatalogError::DuplicateId { .. })),
        "expected DuplicateId, got {errors:?}"
    );
}

#[test]
fn rule_2_rejects_an_id_from_another_projects_namespace() {
    // Without this rule DRIVE-PERM-003 validates cleanly inside Labby's
    // catalog, and two projects can collide on one id with both CIs green.
    let text = VALID.to_owned()
        + r#"
[[invariant]]
id = "DRIVE-PERM-003"
title = "Borrowed from another project's namespace."
kind = "safety"
severity = "high"
model = "gateway"
"#;
    let catalog = Catalog::parse(&text).expect("parse");
    let backends = registry(vec![backend("stateright", &[Kind::Safety])]);
    let errors = catalog.validate(&backends).expect_err("mismatch must fail");
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, CatalogError::NamespaceMismatch { .. })),
        "expected NamespaceMismatch, got {errors:?}"
    );
}

#[test]
fn rule_3_rejects_a_liveness_property_bound_to_a_safety_only_backend() {
    let text = r#"
schema = 1
project = "labby"
namespace = "LABBY"

[[invariant]]
id = "LABBY-REQ-002"
title = "Every accepted request eventually reaches a terminal outcome."
kind = "liveness"
severity = "high"
model = "gateway"

[invariant.checks]
kani = ["request_termination"]
"#;
    let catalog = Catalog::parse(text).expect("parse");
    // Kani is bounded and cannot express fairness, so it claims safety only.
    let backends = registry(vec![backend("kani", &[Kind::Safety])]);
    let errors = catalog
        .validate(&backends)
        .expect_err("capability mismatch must fail");
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, CatalogError::CapabilityMismatch { .. })),
        "expected CapabilityMismatch, got {errors:?}"
    );
}

#[test]
fn rule_4_rejects_an_unknown_backend_and_names_what_is_registered() {
    let text = VALID.replace("stateright = ", "statewrong = ");
    let catalog = Catalog::parse(&text).expect("parse");
    let backends = registry(vec![backend("stateright", &[Kind::Safety])]);
    let errors = catalog.validate(&backends).expect_err("typo must fail");
    let message = errors
        .iter()
        .find_map(|error| match error {
            CatalogError::UnknownBackend { .. } => Some(error.to_string()),
            _ => None,
        })
        .expect("expected UnknownBackend");
    // The error has to name the real ids, or a typo costs a debugging session.
    assert!(message.contains("statewrong"), "{message}");
    assert!(message.contains("stateright"), "{message}");
}

#[test]
fn an_empty_handle_list_is_rejected_rather_than_read_as_covered() {
    let text = VALID.replace(r#"stateright = ["request_lifecycle"]"#, "stateright = []");
    let catalog = Catalog::parse(&text).expect("parse");
    let backends = registry(vec![backend("stateright", &[Kind::Safety])]);
    let errors = catalog.validate(&backends).expect_err("empty must fail");
    assert!(
        errors
            .iter()
            .any(|error| matches!(error, CatalogError::EmptyHandles { .. })),
        "expected EmptyHandles, got {errors:?}"
    );
}

#[test]
fn validation_reports_every_violation_not_just_the_first() {
    let text = r#"
schema = 9
project = "labby"
namespace = "LABBY"

[[invariant]]
id = "DRIVE-PERM-003"
title = "Wrong namespace and an unknown backend at once."
kind = "safety"
severity = "high"
model = "gateway"

[invariant.checks]
statewrong = ["nope"]
"#;
    let catalog = Catalog::parse(text).expect("parse");
    let errors = catalog
        .validate(&BTreeMap::new())
        .expect_err("multiple failures");
    assert!(
        errors.len() >= 3,
        "expected schema, namespace and backend errors together, got {errors:?}"
    );
}

#[test]
fn an_uncovered_invariant_is_reported_by_id() {
    let text = r#"
schema = 1
project = "labby"
namespace = "LABBY"

[[invariant]]
id = "LABBY-CAT-009"
title = "Nobody checks this yet."
kind = "safety"
severity = "medium"
model = "gateway"
"#;
    let catalog = Catalog::parse(text).expect("parse");
    assert_eq!(catalog.validate(&BTreeMap::new()), Ok(()));
    let uncovered = catalog.uncovered();
    assert_eq!(uncovered.len(), 1);
    assert_eq!(uncovered[0].as_str(), "LABBY-CAT-009");
}

#[test]
fn a_retired_invariant_is_not_reported_as_uncovered() {
    let text = r#"
schema = 1
project = "labby"
namespace = "LABBY"

[[invariant]]
id = "LABBY-OLD-001"
title = "Withdrawn, but the id stays reserved forever."
kind = "safety"
severity = "medium"
model = "gateway"
status = "retired"
"#;
    let catalog = Catalog::parse(text).expect("parse");
    assert!(catalog.uncovered().is_empty());
}

#[test]
fn an_unknown_field_is_rejected_rather_than_silently_ignored() {
    // A typo'd key that parses and does nothing is how a binding goes missing.
    let text = VALID.replace("severity =", "severty =");
    assert!(Catalog::parse(&text).is_err());
}
