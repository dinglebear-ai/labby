#![allow(clippy::panic, dead_code)]

#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_identity.rs"]
mod live_identity;
#[path = "support/live_labby.rs"]
mod live_labby;
#[path = "support/state_snapshot.rs"]
mod state_snapshot;

mod support {
    pub(crate) use crate::live_labby::{
        CleanupResult, LiveLabbyBuilder, LiveLabbyGuard, isolated_command,
    };
}

use reqwest::StatusCode;
use state_snapshot::{NarrowStorageObservation, OwnedProcessObservation, PERSISTENCE_CONTRACT};

#[test]
fn restart_suite_locks_the_complete_persistence_contract() {
    assert_eq!(PERSISTENCE_CONTRACT.len(), 11);
}

#[tokio::test]
async fn cold_start_and_repeated_restart_preserve_durable_identity_and_replace_process_state() {
    let mut identity = live_identity::LiveIdentity::bootstrap("parity-restart-subject")
        .await
        .expect("cold public bootstrap");
    let root = identity.root().to_path_buf();
    let original_base = identity.base().to_owned();
    let durable_before =
        NarrowStorageObservation::read(&root.join("labby-home"), &["config.toml", "access.sqlite"])
            .unwrap();
    let first = OwnedProcessObservation::read(&root).unwrap();
    assert_eq!(identity.introspect().await.unwrap().0, StatusCode::OK);
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK
    );

    identity.restart().await.expect("first staged restart");
    let second = OwnedProcessObservation::read(&root).unwrap();
    second.assert_restarted_from(&first);
    assert_eq!(identity.base(), original_base);
    assert_eq!(identity.introspect().await.unwrap().0, StatusCode::OK);
    assert_eq!(
        NarrowStorageObservation::read(&root.join("labby-home"), &["config.toml", "access.sqlite"])
            .unwrap(),
        durable_before
    );

    identity.restart().await.expect("repeated restart");
    let third = OwnedProcessObservation::read(&root).unwrap();
    third.assert_restarted_from(&second);
    assert_eq!(identity.introspect().await.unwrap().0, StatusCode::OK);
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK,
        "allowed upstream disappeared after repeated restart"
    );

    let cleanup = identity.cleanup().await.expect("journaled cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
    assert!(!root.exists(), "owned installation survived cleanup");
}

#[tokio::test]
async fn staged_protected_route_change_is_not_half_published_before_restart() {
    let mut identity = live_identity::LiveIdentity::bootstrap("staged-route-subject")
        .await
        .expect("bootstrap");
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK
    );
    let before = OwnedProcessObservation::read(identity.root()).unwrap();
    let disabled =
        live_identity::policy(&["lab:read"]).replace("enabled = true", "enabled = false");
    std::fs::write(identity.root().join("labby-home/config.toml"), disabled).unwrap();

    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::OK,
        "desired config leaked into the running route collection"
    );
    assert_eq!(
        OwnedProcessObservation::read(identity.root()).unwrap(),
        before
    );

    identity.restart().await.expect("activate staged revision");
    OwnedProcessObservation::read(identity.root())
        .unwrap()
        .assert_restarted_from(&before);
    assert_eq!(
        identity.protected_mcp_initialize().await.unwrap(),
        StatusCode::NOT_FOUND,
        "disabled desired route remained mounted after restart"
    );
    let cleanup = identity.cleanup().await.expect("cleanup");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}
