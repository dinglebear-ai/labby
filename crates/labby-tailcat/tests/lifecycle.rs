#![cfg(unix)]
#![allow(clippy::panic)] // Bounded fixture failures must report a test failure.
use labby_tailcat::{Bridge, BridgeConfig, BridgeError, BridgeStatus};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, time::Duration};

fn fixture(script: &str) -> (tempfile::TempDir, BridgeConfig) {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("helper");
    let bytes = format!("#!/bin/sh\n{script}\n").into_bytes();
    std::fs::write(&exe, &bytes).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let config = BridgeConfig {
        executable: exe,
        state_dir: dir.path().into(),
        expected_sha256: Sha256::digest(bytes).into(),
        target: "127.0.0.1:12345".parse().unwrap(),
        peer: format!("nodekey:{}", "a".repeat(64)),
        derp_map_url: "https://tailcat.dev/derpmap.json".into(),
    };
    (dir, config)
}

#[tokio::test]
async fn graceful_stop_and_redacted_capability() {
    let (_dir, c) = fixture(
        "read start\necho '{\"version\":1,\"type\":\"ready\",\"address\":\"tcpSecret\",\"port\":1}'\nread stop\necho '{\"version\":1,\"type\":\"stopped\"}'",
    );
    let mut bridge = Bridge::start(c.validate().unwrap()).await.unwrap();
    assert_eq!(bridge.status(), BridgeStatus::Ready);
    assert!(!format!("{:?}", bridge.capability()).contains("tcpSecret"));
    bridge.stop().await.unwrap();
    assert_eq!(bridge.status(), BridgeStatus::Stopped);
    assert_eq!(bridge.capability().address(), "");
    bridge.stop().await.unwrap();
}

#[tokio::test]
async fn rejects_wrong_version_unknown_fields_and_missing_ready_fields() {
    for frame in [
        r#"{"version":2,"type":"ready","address":"tcpSecret","port":1}"#,
        r#"{"version":1,"type":"ready","address":"tcpSecret","port":1,"secret":"value"}"#,
        r#"{"version":1,"type":"ready"}"#,
    ] {
        let (_dir, c) = fixture(&format!("read start\necho '{frame}'\nsleep 30"));
        assert!(matches!(
            Bridge::start(c.validate().unwrap()).await,
            Err(BridgeError::Protocol)
        ));
    }
}

#[tokio::test]
async fn startup_timeout_is_bounded() {
    let (_dir, c) = fixture("read start\nsleep 30");
    assert!(matches!(
        Bridge::start_with_timeout(c.validate().unwrap(), Duration::from_millis(80)).await,
        Err(BridgeError::StartupTimeout)
    ));
}

#[test]
fn checksum_and_symlink_are_rejected() {
    let (dir, mut c) = fixture("exit 0");
    c.expected_sha256 = [1; 32];
    assert!(matches!(c.validate(), Err(BridgeError::ChecksumMismatch)));
    let (_other_dir, mut c) = fixture("exit 0");
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&c.executable, &link).unwrap();
    c.executable = link;
    assert!(matches!(c.validate(), Err(BridgeError::InvalidConfig)));
}

async fn pid_from(path: &std::path::Path) -> u32 {
    for _ in 0..100 {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse() {
                return pid;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("fixture did not write pid");
}

async fn assert_dead(pid: u32) {
    for _ in 0..100 {
        if labby_gateway::process::unix::send_signal(pid, None).is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("owned process survived cleanup");
}

#[tokio::test]
async fn cancellation_during_startup_kills_process_group() {
    let output = tempfile::tempdir().unwrap();
    let pid_file = output.path().join("pid");
    let script = format!("read start\necho $$ > '{}'\nsleep 30", pid_file.display());
    let (_dir, c) = fixture(&script);
    let starting = tokio::spawn(Bridge::start(c.validate().unwrap()));
    let pid = pid_from(&pid_file).await;
    starting.abort();
    let _cancelled = starting.await;
    assert_dead(pid).await;
}

#[tokio::test]
async fn unsolicited_ready_kills_helper_without_status_polling() {
    let output = tempfile::tempdir().unwrap();
    let pid_file = output.path().join("pid");
    let script = format!(
        "read start\necho $$ > '{}'\necho '{{\"version\":1,\"type\":\"ready\",\"address\":\"tcpSecret\",\"port\":1}}'\necho '{{\"version\":1,\"type\":\"ready\"}}'\nsleep 30",
        pid_file.display()
    );
    let (_dir, c) = fixture(&script);
    let mut bridge = Bridge::start(c.validate().unwrap()).await.unwrap();
    let pid = pid_from(&pid_file).await;
    assert_dead(pid).await;
    assert_eq!(bridge.status(), BridgeStatus::Failed);
    bridge.stop().await.unwrap();
}

#[tokio::test]
async fn verified_snapshot_survives_source_replacement() {
    let (_dir, c) = fixture(
        "read start\necho '{\"version\":1,\"type\":\"ready\",\"address\":\"tcpSecret\",\"port\":1}'\nread stop",
    );
    let path = c.executable.clone();
    let validated = c.validate().unwrap();
    std::fs::write(path, b"#!/bin/sh\nexit 9\n").unwrap();
    let mut bridge = Bridge::start(validated).await.unwrap();
    assert_eq!(bridge.status(), BridgeStatus::Ready);
    bridge.stop().await.unwrap();
}

#[tokio::test]
async fn drop_kills_owned_descendant() {
    let output = tempfile::tempdir().unwrap();
    let pid_file = output.path().join("pid");
    let descendant_file = output.path().join("descendant");
    let script = format!(
        "read start\necho $$ > '{}'\nsleep 30 &\necho $! > '{}'\necho '{{\"version\":1,\"type\":\"ready\",\"address\":\"tcpSecret\",\"port\":1}}'\nwait",
        pid_file.display(),
        descendant_file.display()
    );
    let (_dir, c) = fixture(&script);
    let bridge = Bridge::start(c.validate().unwrap()).await.unwrap();
    let pid = pid_from(&pid_file).await;
    let descendant = pid_from(&descendant_file).await;
    drop(bridge);
    assert_dead(pid).await;
    assert_dead(descendant).await;
}

#[tokio::test]
async fn oversized_ready_and_early_exit_fail_closed() {
    for script in [
        "read start\nexit 1".to_owned(),
        format!("read start\necho '{}'\nsleep 30", "x".repeat(65537)),
    ] {
        let (_dir, c) = fixture(&script);
        assert!(matches!(
            Bridge::start(c.validate().unwrap()).await,
            Err(BridgeError::Protocol)
        ));
    }
}

#[tokio::test]
async fn unresponsive_stop_is_forced_and_reaped() {
    let output = tempfile::tempdir().unwrap();
    let pid_file = output.path().join("pid");
    let script = format!(
        "read start\necho $$ > '{}'\necho '{{\"version\":1,\"type\":\"ready\",\"address\":\"tcpSecret\",\"port\":1}}'\nsleep 30",
        pid_file.display()
    );
    let (_dir, c) = fixture(&script);
    let mut bridge = Bridge::start(c.validate().unwrap()).await.unwrap();
    let pid = pid_from(&pid_file).await;
    tokio::time::timeout(Duration::from_secs(5), bridge.stop())
        .await
        .unwrap()
        .unwrap();
    assert_dead(pid).await;
    assert_eq!(bridge.status(), BridgeStatus::Stopped);
}

#[tokio::test]
async fn cancelling_stop_cleans_descendants_while_bridge_is_retained() {
    let output = tempfile::tempdir().unwrap();
    let descendant_file = output.path().join("descendant");
    let script = format!(
        "read start\nsleep 30 &\necho $! > '{}'\necho '{{\"version\":1,\"type\":\"ready\",\"address\":\"tcpSecret\",\"port\":1}}'\nwait",
        descendant_file.display()
    );
    let (_dir, c) = fixture(&script);
    let mut bridge = Bridge::start(c.validate().unwrap()).await.unwrap();
    let descendant = pid_from(&descendant_file).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), bridge.stop())
            .await
            .is_err()
    );
    assert_dead(descendant).await;
    bridge.stop().await.unwrap();
    assert_eq!(bridge.status(), BridgeStatus::Stopped);
}
