#![allow(clippy::panic)]

//! Integration-level qualification of the production authority vocabulary.
//!
//! Durable membership resolution and adapter reauthorization are exercised by
//! the `access::authority` and dispatch tests. The assertions here compare the
//! exported production role templates (`labby_primitives::access::RoleTemplate`,
//! a Rust enum) against `docs/access-control/authority-matrix-v1.json` (a
//! hand-maintained product document). The two are independent sources: the
//! previous version of this file restated the capability lists by hand inside
//! the test, so a coordinated mistake in the enum and the test copy would have
//! passed. Now a change to either source without the other fails.

use std::collections::BTreeSet;

use labby_primitives::access::{Capability, CapabilitySchemaVersion, RoleTemplate};
use serde_json::Value;

const MATRIX: &str = include_str!("../../../docs/access-control/authority-matrix-v1.json");

fn matrix() -> Value {
    serde_json::from_str(MATRIX).expect("valid authority matrix")
}

fn strings(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .expect("string array")
        .iter()
        .map(|item| item.as_str().expect("string entry").to_owned())
        .collect()
}

#[test]
fn every_published_role_matches_its_production_capability_set() {
    // Replaces the hand-copied `cases` table: the expected capability set per
    // role now comes from the published matrix document, and the actual set
    // from the compiled enum. Order is compared too, since the document is the
    // wire projection of the enum's registry order.
    let matrix = matrix();
    let roles = matrix["roles"].as_object().expect("role templates");
    assert_eq!(
        roles.keys().cloned().collect::<BTreeSet<_>>(),
        RoleTemplate::ALL
            .iter()
            .map(|role| role.registry_key().to_owned())
            .collect::<BTreeSet<_>>(),
        "published roles and RoleTemplate::ALL must be the same set"
    );
    for role in RoleTemplate::ALL {
        let published = roles[role.registry_key()]
            .as_array()
            .expect("role capability list")
            .iter()
            .map(|value| value.as_str().expect("capability wire name").to_owned())
            .collect::<Vec<_>>();
        let implemented = role
            .capabilities(CapabilitySchemaVersion::V1)
            .expect("v1 schema is supported")
            .iter()
            .map(|capability| capability.as_wire().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            published,
            implemented,
            "{}: published capabilities drifted from RoleTemplate",
            role.registry_key()
        );
    }
}

#[test]
fn published_capability_families_are_closed_over_the_wire_adapter() {
    // Every published capability family must round-trip through the
    // production wire adapter, and every production capability must be
    // published. Unknown names and unknown schema versions fail closed.
    let matrix = matrix();
    let published = strings(&matrix["capabilityFamilies"]);
    let all_roles: BTreeSet<String> = matrix["roles"]
        .as_object()
        .expect("roles")
        .values()
        .flat_map(strings)
        .collect();
    assert!(
        all_roles.is_subset(&published),
        "roles reference unpublished capability families"
    );
    for name in &published {
        let capability = Capability::from_wire(CapabilitySchemaVersion::V1, name)
            .unwrap_or_else(|| panic!("published capability {name} is unknown to production"));
        assert_eq!(capability.as_wire(), name);
    }
    let platform_admin = RoleTemplate::PlatformAdmin
        .capabilities(CapabilitySchemaVersion::V1)
        .expect("v1")
        .iter()
        .map(|capability| capability.as_wire().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        platform_admin, published,
        "platform_admin must hold exactly the published capability families"
    );
    assert_eq!(
        Capability::from_wire(CapabilitySchemaVersion::V1, "scope.superuser"),
        None
    );
    assert_eq!(
        Capability::from_wire(CapabilitySchemaVersion::new(2), "scope.read"),
        None
    );
    assert_eq!(
        RoleTemplate::PlatformAdmin.capabilities(CapabilitySchemaVersion::new(2)),
        None
    );
}

#[test]
fn production_templates_preserve_privilege_boundaries() {
    // Privilege-boundary invariants that must hold regardless of what the
    // document says: these are product rules, not restatements of a table.
    let capabilities = |role: RoleTemplate| role.capabilities(CapabilitySchemaVersion::V1).unwrap();
    assert!(capabilities(RoleTemplate::PlatformAdmin).contains(&Capability::PlatformManage));
    for role in RoleTemplate::ALL
        .iter()
        .filter(|role| **role != RoleTemplate::PlatformAdmin)
    {
        assert!(
            !capabilities(*role).contains(&Capability::PlatformManage),
            "{role:?} must not hold platform.manage"
        );
    }
    assert!(capabilities(RoleTemplate::TeamOwner).contains(&Capability::OwnershipTransfer));
    assert!(!capabilities(RoleTemplate::ProjectOwner).contains(&Capability::OwnershipTransfer));
    assert!(!capabilities(RoleTemplate::TeamAdmin).contains(&Capability::OwnershipTransfer));
    assert!(capabilities(RoleTemplate::TeamAdmin).contains(&Capability::MembershipManage));
    assert!(!capabilities(RoleTemplate::TeamMember).contains(&Capability::MembershipManage));
    assert!(capabilities(RoleTemplate::PersonalUser).contains(&Capability::ScopeDelete));
    assert!(!capabilities(RoleTemplate::PersonalUser).contains(&Capability::MembershipManage));
    assert_eq!(
        capabilities(RoleTemplate::ProjectViewer),
        [Capability::ScopeRead]
    );
}
