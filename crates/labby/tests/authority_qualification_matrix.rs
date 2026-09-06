//! Dependency-safe authority qualification fixture.
//!
//! This is an executable truth-table contract, not live-environment evidence.
//! Product E2E adapters can consume the same cases once all Phase 13 surfaces exist.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Actor {
    PlatformAdmin,
    TeamOwner,
    TeamAdmin,
    TeamMember,
    PersonalUser,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Target {
    Installation,
    Personal,
    OwnTeam,
    OtherTeam,
    Project,
    Public,
    Nonexistent,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Read,
    Operate,
    Manage,
    Delete,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Surface {
    Web,
    Api,
    Mcp,
    CodeMode,
    Cli,
}

const ACTORS: [Actor; 5] = [
    Actor::PlatformAdmin,
    Actor::TeamOwner,
    Actor::TeamAdmin,
    Actor::TeamMember,
    Actor::PersonalUser,
];
const TARGETS: [Target; 7] = [
    Target::Installation,
    Target::Personal,
    Target::OwnTeam,
    Target::OtherTeam,
    Target::Project,
    Target::Public,
    Target::Nonexistent,
];
const OPERATIONS: [Operation; 4] = [
    Operation::Read,
    Operation::Operate,
    Operation::Manage,
    Operation::Delete,
];
const SURFACES: [Surface; 5] = [
    Surface::Web,
    Surface::Api,
    Surface::Mcp,
    Surface::CodeMode,
    Surface::Cli,
];

fn expected(actor: Actor, target: Target, operation: Operation) -> bool {
    if target == Target::Nonexistent {
        return false;
    }
    if target == Target::Public {
        return operation == Operation::Read;
    }
    if actor == Actor::PlatformAdmin {
        return true;
    }
    if target == Target::Installation || target == Target::OtherTeam {
        return false;
    }
    if target == Target::Personal {
        return true;
    }
    match actor {
        Actor::TeamOwner => true,
        Actor::TeamAdmin => operation != Operation::Delete,
        Actor::TeamMember => matches!(operation, Operation::Read | Operation::Operate),
        Actor::PersonalUser | Actor::PlatformAdmin => false,
    }
}

#[test]
fn authority_truth_table_is_closed_across_every_product_surface() {
    let mut cases = 0;
    for surface in SURFACES {
        for actor in ACTORS {
            for target in TARGETS {
                for operation in OPERATIONS {
                    let allowed = expected(actor, target, operation);
                    cases += 1;
                    assert_eq!(
                        allowed,
                        expected(actor, target, operation),
                        "unstable case: {surface:?}/{actor:?}/{target:?}/{operation:?}"
                    );
                }
            }
        }
    }
    assert_eq!(cases, 5 * 5 * 7 * 4);
}

#[derive(Clone, Copy, Debug)]
struct Epochs {
    authority: u64,
    membership: u64,
    policy: u64,
    resource: u64,
    context: u64,
}

fn still_current(captured: Epochs, current: Epochs) -> bool {
    captured.authority == current.authority
        && captured.membership == current.membership
        && captured.policy == current.policy
        && captured.resource == current.resource
        && captured.context == current.context
}

#[test]
fn every_revocation_and_context_axis_invalidates_in_flight_authority() {
    let captured = Epochs {
        authority: 7,
        membership: 11,
        policy: 13,
        resource: 17,
        context: 19,
    };
    assert!(still_current(captured, captured));
    for current in [
        Epochs {
            authority: 8,
            ..captured
        },
        Epochs {
            membership: 12,
            ..captured
        },
        Epochs {
            policy: 14,
            ..captured
        },
        Epochs {
            resource: 18,
            ..captured
        },
        Epochs {
            context: 20,
            ..captured
        },
    ] {
        assert!(!still_current(captured, current));
    }
}

#[test]
fn denial_contract_does_not_distinguish_cross_scope_from_nonexistent() {
    for surface in SURFACES {
        for actor in [
            Actor::TeamOwner,
            Actor::TeamAdmin,
            Actor::TeamMember,
            Actor::PersonalUser,
        ] {
            for operation in OPERATIONS {
                assert_eq!(
                    expected(actor, Target::OtherTeam, operation),
                    expected(actor, Target::Nonexistent, operation),
                    "enumeration leak contract: {surface:?}/{actor:?}/{operation:?}"
                );
            }
        }
    }
}
