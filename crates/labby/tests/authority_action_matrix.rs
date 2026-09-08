//! D-C1: closed-world per-action authority expectations.
//!
//! `fixtures/authority_action_expectations.json` holds one reviewed row per
//! `service:action` for every catalog action of the capability-evaluated
//! services (`access`, `agents`, `tasks`, `dev_containers`, `gateway`,
//! `stash`, `projects`), giving an allow/deny decision for six principals
//! relative to one Team-owned resource:
//!
//! * `team_member`, `team_admin`, `team_owner` — hold that role in the owning
//!   Team;
//! * `other_team_member` — holds `member` in a different Team only;
//! * `personal_no_membership` — holds no Team membership at all;
//! * `platform_admin` — installation-level platform administrator.
//!
//! What is and is not live-dispatched here:
//!
//! * NOT live-dispatched. In-process dispatch is not reachable from an
//!   integration test: `labby::access` is a private module and
//!   `labby::dispatch::{access,projects,agents,tasks,dev_containers}::dispatch`
//!   are `pub(crate)`; the only exported entrypoints are the `dispatch_unbound`
//!   registry fallbacks, which deny every non-probe action without an identity.
//!   The live-binary harness (`support/live_labby`, `support/live_identity`)
//!   bootstraps exactly one owner bearer per process and has no way to mint
//!   bearers for additional principals with different Team roles, so a
//!   multi-principal HTTP variant would require a new harness, which this test
//!   deliberately does not build. Per-principal *runtime* denial is covered by
//!   the crate-internal `access::authority` and dispatch unit tests.
//! * WHAT IS CHECKED. (a) Closed-world coverage: the fixture's key set must
//!   equal the catalog's key set for the listed services — an action without a
//!   row fails, and a row for an unknown action fails. (b) Every row's
//!   decisions are validated against an independent source: the catalog's
//!   `required_capability` column combined with the production role templates
//!   (`labby_primitives::access::RoleTemplate`). A row that claims a decision
//!   the vocabulary cannot produce fails, so the fixture cannot silently encode
//!   a privilege escalation or a bogus denial. (c) The fixture is deserialized
//!   with `deny_unknown_fields`, so a mistyped principal or extra column fails.

#![allow(clippy::panic)]

#[path = "support/lib.rs"]
mod support;

use std::collections::{BTreeMap, BTreeSet};

use labby_primitives::access::{Capability, CapabilitySchemaVersion, RoleTemplate};
use serde::Deserialize;
use support::action_matrix::CatalogAction;
use support::authority_matrix::ActionRef;

const ACTION_CATALOG: &str = include_str!("../../../docs/generated/action-catalog.json");
const EXPECTATIONS: &str = include_str!("fixtures/authority_action_expectations.json");

/// Services whose actions are authorized through the shared capability
/// evaluator and therefore need a per-action expectations row.
const MATRIX_SERVICES: [&str; 7] = [
    "access",
    "agents",
    "tasks",
    "dev_containers",
    "gateway",
    "stash",
    "projects",
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Decision {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ExpectationRow {
    /// Capability the row was reviewed against; must match the catalog.
    required_capability: Option<String>,
    team_member: Decision,
    team_admin: Decision,
    team_owner: Decision,
    other_team_member: Decision,
    personal_no_membership: Decision,
    platform_admin: Decision,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectationsFixture {
    schema: String,
    #[allow(dead_code)]
    resource_under_test: String,
    actions: BTreeMap<String, ExpectationRow>,
}

fn catalog() -> Vec<CatalogAction> {
    serde_json::from_str(ACTION_CATALOG).expect("generated action catalog must parse")
}

fn expectations() -> ExpectationsFixture {
    let fixture: ExpectationsFixture =
        serde_json::from_str(EXPECTATIONS).expect("authority expectations fixture must parse");
    assert_eq!(fixture.schema, "labby.authority-action-expectations/v1");
    fixture
}

fn matrix_catalog() -> BTreeMap<String, CatalogAction> {
    catalog()
        .into_iter()
        .filter(|action| MATRIX_SERVICES.contains(&action.service.as_str()))
        .map(|action| (ActionRef::from(&action).key(), action))
        .collect()
}

/// The decision the production vocabulary yields for `role` over the owning
/// Team: allowed exactly when the role template carries the capability. Rows
/// without a capability are builtin probes or caller-membership projections,
/// which every authenticated principal may call (visibility is filtered
/// inside the store).
fn vocabulary_decision(role: RoleTemplate, capability: Option<Capability>) -> Decision {
    match capability {
        None => Decision::Allow,
        Some(capability) => {
            if role
                .capabilities(CapabilitySchemaVersion::V1)
                .expect("v1 schema supported")
                .contains(&capability)
            {
                Decision::Allow
            } else {
                Decision::Deny
            }
        }
    }
}

#[test]
fn expectations_fixture_is_closed_over_the_catalog() {
    let catalog = matrix_catalog();
    let fixture = expectations();
    let catalog_keys = catalog.keys().cloned().collect::<BTreeSet<_>>();
    let fixture_keys = fixture.actions.keys().cloned().collect::<BTreeSet<_>>();
    let missing = catalog_keys.difference(&fixture_keys).collect::<Vec<_>>();
    let unknown = fixture_keys.difference(&catalog_keys).collect::<Vec<_>>();
    assert!(
        missing.is_empty() && unknown.is_empty(),
        "authority expectations must cover every matrix-service action exactly once\n\
         actions without an expectations row: {missing:#?}\n\
         rows for unknown actions: {unknown:#?}"
    );
    for service in MATRIX_SERVICES {
        assert!(
            catalog.values().any(|action| action.service == service),
            "{service} is a matrix service but has no catalog actions"
        );
    }
}

#[test]
fn every_row_is_consistent_with_the_production_authority_vocabulary() {
    let catalog = matrix_catalog();
    let fixture = expectations();
    let mut failures = Vec::new();
    for (key, row) in &fixture.actions {
        let Some(action) = catalog.get(key) else {
            // Reported by the closed-world test; skip here.
            continue;
        };
        if row.required_capability != action.required_capability {
            failures.push(format!(
                "{key}: fixture reviewed against {:?} but the catalog demands {:?}",
                row.required_capability, action.required_capability
            ));
            continue;
        }
        let capability = match action.required_capability.as_deref() {
            None => None,
            Some(name) => match Capability::from_wire(CapabilitySchemaVersion::V1, name) {
                Some(capability) => Some(capability),
                None => {
                    failures.push(format!(
                        "{key}: catalog capability {name} is not production vocabulary"
                    ));
                    continue;
                }
            },
        };
        if action.builtin && capability.is_some() {
            failures.push(format!("{key}: builtin probe must not demand a capability"));
        }

        let checks = [
            (
                "team_member",
                row.team_member,
                vocabulary_decision(RoleTemplate::TeamMember, capability),
            ),
            (
                "team_admin",
                row.team_admin,
                vocabulary_decision(RoleTemplate::TeamAdmin, capability),
            ),
            (
                "team_owner",
                row.team_owner,
                vocabulary_decision(RoleTemplate::TeamOwner, capability),
            ),
            (
                "platform_admin",
                row.platform_admin,
                vocabulary_decision(RoleTemplate::PlatformAdmin, capability),
            ),
        ];
        for (principal, expected, derived) in checks {
            if expected != derived {
                failures.push(format!(
                    "{key}: {principal} expects {expected:?} but the vocabulary yields {derived:?} for {:?}",
                    action.required_capability
                ));
            }
        }

        // Principals with no role in the owning Team hold no capability over
        // it: any capability-gated action must be denied, and only
        // capability-free probes/projections may be allowed.
        let no_membership = if capability.is_some() {
            Decision::Deny
        } else {
            Decision::Allow
        };
        for (principal, expected) in [
            ("other_team_member", row.other_team_member),
            ("personal_no_membership", row.personal_no_membership),
        ] {
            if expected != no_membership {
                failures.push(format!(
                    "{key}: {principal} has no role in the owning Team and must be {no_membership:?}"
                ));
            }
        }

        // Surface policy must agree with the vocabulary: `requires_admin`
        // means platform-scoped, which no Team role may satisfy.
        if action.requires_admin
            && [row.team_member, row.team_admin, row.team_owner].contains(&Decision::Allow)
        {
            failures.push(format!("{key}: requires_admin action allows a Team role"));
        }
        if row.platform_admin == Decision::Deny {
            failures.push(format!(
                "{key}: platform_admin must never be denied by the vocabulary"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_capability_class_is_represented_in_the_matrix() {
    // The matrix must exercise each capability the evaluator can demand, so a
    // vocabulary change cannot leave a class silently untested.
    let catalog = matrix_catalog();
    let demanded = catalog
        .values()
        .filter_map(|action| action.required_capability.clone())
        .collect::<BTreeSet<_>>();
    for required in [
        "platform.manage",
        "scope.read",
        "scope.operate",
        "scope.create",
        "scope.manage",
        "scope.delete",
        "membership.manage",
    ] {
        assert!(
            demanded.contains(required),
            "no matrix action demands {required}"
        );
    }
    let unknown = demanded
        .iter()
        .filter(|name| Capability::from_wire(CapabilitySchemaVersion::V1, name).is_none())
        .collect::<Vec<_>>();
    assert!(
        unknown.is_empty(),
        "catalog demands unknown capabilities: {unknown:?}"
    );
}

#[test]
fn fixture_rejects_unknown_principals_and_columns() {
    let bad_principal = r#"{"schema":"labby.authority-action-expectations/v1","resource_under_test":"x","actions":{"agents:agents.get":{"required_capability":"scope.read","team_member":"allow","team_admin":"allow","team_owner":"allow","other_team_member":"deny","personal_no_membership":"deny","platform_admin":"allow","team_viewer":"allow"}}}"#;
    let error = serde_json::from_str::<ExpectationsFixture>(bad_principal)
        .expect_err("unknown principal must fail");
    assert!(error.to_string().contains("team_viewer"), "{error}");

    let missing_principal = r#"{"schema":"labby.authority-action-expectations/v1","resource_under_test":"x","actions":{"agents:agents.get":{"required_capability":"scope.read","team_member":"allow","team_admin":"allow","team_owner":"allow","other_team_member":"deny","personal_no_membership":"deny"}}}"#;
    let error = serde_json::from_str::<ExpectationsFixture>(missing_principal)
        .expect_err("missing principal must fail");
    assert!(error.to_string().contains("platform_admin"), "{error}");

    let bad_decision = r#"{"schema":"labby.authority-action-expectations/v1","resource_under_test":"x","actions":{"agents:agents.get":{"required_capability":"scope.read","team_member":"maybe","team_admin":"allow","team_owner":"allow","other_team_member":"deny","personal_no_membership":"deny","platform_admin":"allow"}}}"#;
    assert!(serde_json::from_str::<ExpectationsFixture>(bad_decision).is_err());
}
