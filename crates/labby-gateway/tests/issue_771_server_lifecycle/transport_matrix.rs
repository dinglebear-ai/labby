//! C4 is a qualification checklist, not runtime authorization or a durability claim.
//! Exhaustive matching makes a newly added configured transport require review.
use labby_runtime::gateway_config::UpstreamTransport;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Loss {
    CreatorPeer,
    GatewayRestart,
    UpstreamRestart,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Proof {
    CurrentRouteOwnerAndExposure,
    CurrentCredentialAndConfigBinding,
    DurableRevocationChecked,
    UpstreamTaskStoreIndependentOfPeer,
    AdvertisedRetentionHonored,
    RouteDatabaseReopened,
    ChildReexecutionPreservesTaskIdentity,
    BackendTaskStoreSurvivesRestart,
    HostedTaskImplementationIsDurable,
    ExplicitLegacyTaskCompatibility,
}

fn base_requirements() -> Vec<Proof> {
    vec![
        Proof::CurrentRouteOwnerAndExposure,
        Proof::CurrentCredentialAndConfigBinding,
        Proof::DurableRevocationChecked,
        Proof::UpstreamTaskStoreIndependentOfPeer,
        Proof::AdvertisedRetentionHonored,
    ]
}

fn required_proofs(transport: UpstreamTransport, loss: Loss) -> Vec<Proof> {
    let mut proofs = base_requirements();
    match transport {
        UpstreamTransport::Http | UpstreamTransport::UnixSocket | UpstreamTransport::Websocket => {}
        // Both local stdio and SSH-spawned stdio may create a new child/store.
        UpstreamTransport::Stdio => proofs.push(Proof::ChildReexecutionPreservesTaskIdentity),
    }
    match loss {
        Loss::CreatorPeer => {}
        Loss::GatewayRestart => proofs.push(Proof::RouteDatabaseReopened),
        Loss::UpstreamRestart => proofs.push(Proof::BackendTaskStoreSurvivesRestart),
    }
    proofs
}

fn hosted_requirements(loss: Loss) -> Vec<Proof> {
    let mut proofs = base_requirements();
    match loss {
        Loss::CreatorPeer => {}
        Loss::GatewayRestart => {
            proofs.push(Proof::RouteDatabaseReopened);
            proofs.push(Proof::HostedTaskImplementationIsDurable);
        }
        Loss::UpstreamRestart => proofs.push(Proof::HostedTaskImplementationIsDurable),
    }
    proofs
}

fn with_legacy_qualification(mut proofs: Vec<Proof>) -> Vec<Proof> {
    // Legacy is a lifecycle overlay, not a fifth configured transport or a
    // session model for modern peers. Never infer task support from initialize.
    proofs.push(Proof::ExplicitLegacyTaskCompatibility);
    proofs
}

const TRANSPORTS: [UpstreamTransport; 4] = [
    UpstreamTransport::Http,
    UpstreamTransport::Websocket,
    UpstreamTransport::Stdio,
    UpstreamTransport::UnixSocket,
];

#[test]
fn every_transport_and_loss_requires_reauthorization_and_peer_independent_tasks() {
    for transport in TRANSPORTS {
        for loss in [
            Loss::CreatorPeer,
            Loss::GatewayRestart,
            Loss::UpstreamRestart,
        ] {
            let proofs = required_proofs(transport, loss);
            for requirement in base_requirements() {
                assert!(proofs.contains(&requirement), "{transport:?}/{loss:?}");
            }
        }
    }
}

#[test]
fn stdio_including_ssh_requires_proven_task_identity_across_child_reexecution() {
    for loss in [
        Loss::CreatorPeer,
        Loss::GatewayRestart,
        Loss::UpstreamRestart,
    ] {
        assert!(
            required_proofs(UpstreamTransport::Stdio, loss)
                .contains(&Proof::ChildReexecutionPreservesTaskIdentity)
        );
    }
}

#[test]
fn a_durable_route_database_alone_does_not_qualify_gateway_restart() {
    for transport in TRANSPORTS {
        let proofs = required_proofs(transport, Loss::GatewayRestart);
        assert!(proofs.contains(&Proof::RouteDatabaseReopened));
        assert!(proofs.contains(&Proof::UpstreamTaskStoreIndependentOfPeer));
        assert!(proofs.contains(&Proof::AdvertisedRetentionHonored));
        assert!(proofs.contains(&Proof::CurrentCredentialAndConfigBinding));
    }
}

#[test]
fn every_upstream_restart_requires_persistent_backend_task_state() {
    for transport in TRANSPORTS {
        assert!(
            required_proofs(transport, Loss::UpstreamRestart)
                .contains(&Proof::BackendTaskStoreSurvivesRestart)
        );
    }
}

#[test]
fn in_process_handles_do_not_qualify_host_restart() {
    assert!(
        !hosted_requirements(Loss::CreatorPeer).contains(&Proof::HostedTaskImplementationIsDurable)
    );
    assert!(
        hosted_requirements(Loss::GatewayRestart)
            .contains(&Proof::HostedTaskImplementationIsDurable)
    );
    assert!(hosted_requirements(Loss::GatewayRestart).contains(&Proof::RouteDatabaseReopened));
    assert!(
        hosted_requirements(Loss::UpstreamRestart)
            .contains(&Proof::HostedTaskImplementationIsDurable)
    );
}

#[test]
fn explicit_legacy_qualification_cannot_erase_modern_security_requirements() {
    for transport in TRANSPORTS {
        let original = required_proofs(transport, Loss::CreatorPeer);
        let legacy = with_legacy_qualification(original.clone());
        assert!(
            original
                .iter()
                .all(|requirement| legacy.contains(requirement))
        );
        assert!(legacy.contains(&Proof::ExplicitLegacyTaskCompatibility));
    }
}

#[test]
fn unsupported_transport_is_not_silently_given_http_durability() {
    assert!(serde_json::from_value::<UpstreamTransport>(serde_json::json!("raw_tcp")).is_err());
}
