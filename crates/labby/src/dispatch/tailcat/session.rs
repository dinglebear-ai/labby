//! One native lifetime owns helper, listener, and credential retirement.
use super::{PairingError, RequestAuthority, grant::GrantLease};
use crate::{access::AccessRuntime, api::tailcat::RestrictedListener};
use labby_tailcat::{Bridge, BridgeConfig, BridgeStatus};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(crate) struct HelperArtifact {
    pub executable: PathBuf,
    pub state_dir: PathBuf,
    pub expected_sha256: [u8; 32],
    pub derp_map_url: String,
}

/// Delivery data is secret; the controller must use authenticated rendezvous.
/// No Debug or Serialize implementation allows accidental log/HTML exposure.
pub(crate) struct SessionDelivery {
    pub address: String,
    pub port: u16,
    pub peer: String,
    pub upstream: String,
    pub envelope: String,
    pub origin: String,
    pub generation: String,
    pub expires_at: u64,
    pub derp_map_url: String,
}

pub(super) struct NativeSession {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl NativeSession {
    pub(super) async fn start<F>(
        runtime: Arc<AccessRuntime>,
        manager: &labby_gateway::gateway::manager::GatewayManager,
        authority: Arc<RequestAuthority>,
        artifact: HelperArtifact,
        projection: F,
    ) -> Result<(Self, SessionDelivery), PairingError>
    where
        F: FnOnce(Arc<RequestAuthority>) -> Result<axum::Router, PairingError>,
    {
        labby_tailcat::ensure_platform_supported().map_err(|_| PairingError)?;
        let grant = GrantLease::issue_checked(runtime, manager, &authority).await?;
        let router = projection(authority.clone())?;
        let listener = RestrictedListener::start_authorized(
            router,
            authority.clone(),
            grant.envelope().to_owned(),
        )
        .await?;
        let config = BridgeConfig {
            executable: artifact.executable,
            state_dir: artifact.state_dir,
            expected_sha256: artifact.expected_sha256,
            derp_map_url: artifact.derp_map_url.clone(),
            target: listener.address(),
            peer: authority.approved.peer().into(),
        };
        let validated = tokio::task::spawn_blocking(move || config.validate())
            .await
            .map_err(|_| PairingError)?
            .map_err(|_| PairingError)?;
        let mut bridge = Bridge::start(validated).await.map_err(|_| PairingError)?;
        let cancel = listener.cancellation();
        if cancel.is_cancelled() {
            return Err(PairingError);
        }
        let delivery = SessionDelivery {
            peer: authority.approved.peer().into(),
            upstream: authority.approved.upstream().into(),
            address: bridge.capability().address().into(),
            port: bridge.capability().port(),
            envelope: grant.envelope().into(),
            origin: authority.approved.origin().into(),
            generation: authority.approved.generation().into(),
            expires_at: authority.approved.expires_at(),
            derp_map_url: artifact.derp_map_url,
        };
        let stopping = cancel.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = stopping.cancelled() => break,
                    () = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
                }
                if bridge.status() != BridgeStatus::Ready {
                    stopping.cancel();
                    break;
                }
            }
            drop(listener);
            // Cancellation of this owner drops the process guard and armed
            // grant even while either graceful cleanup future is pending.
            let _stopped = bridge.stop().await;
            let _retired = grant.revoke().await;
        });
        Ok((Self { cancel, task }, delivery))
    }
    pub(super) fn is_stopped(&self) -> bool {
        self.task.is_finished()
    }
    pub(super) fn phase(&self) -> &'static str {
        if self.task.is_finished() {
            "stopped"
        } else if self.cancel.is_cancelled() {
            "stopping"
        } else {
            "ready"
        }
    }
    pub(super) async fn stop(mut self) {
        self.cancel.cancel();
        let _joined = (&mut self.task).await;
    }
}
impl Drop for NativeSession {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use labby_auth::at_rest::TokenEncryptionKey;
    use labby_primitives::product_credential::{ProductCredential, ProductCredentialVerifier as _};
    use sha2::{Digest as _, Sha256};
    use std::os::unix::fs::PermissionsExt as _;

    #[tokio::test]
    async fn session_stop_owns_helper_listener_and_native_credential() {
        exercise_shutdown(0).await;
    }
    #[tokio::test]
    async fn session_drop_retires_all_native_resources() {
        exercise_shutdown(1).await;
    }
    #[tokio::test]
    async fn helper_failure_retires_listener_and_native_credential() {
        exercise_shutdown(2).await;
    }
    #[tokio::test]
    async fn source_revocation_stops_helper_and_retires_native_child() {
        exercise_shutdown(3).await;
    }
    async fn exercise_shutdown(mode: u8) {
        let (directory, runtime, adapter, approved, manager) =
            super::super::testing::fixture_with_manager().await;
        let key = Arc::new(TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap());
        let authority = Arc::new(RequestAuthority {
            adapter: adapter.clone(),
            approved,
            key: key.clone(),
            source: ProductCredential::parse(&format!(
                "lby_pc_v1_source_{}",
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
            ))
            .unwrap(),
        });
        let executable = directory.path().join("helper");
        let start_file = directory.path().join("start.json");
        let pid_file = directory.path().join("pid");
        let bytes = format!("#!/bin/sh\nread start\necho \"$start\" > '{}'\necho $$ > '{}'\necho '{{\"version\":1,\"type\":\"ready\",\"address\":\"tcpFixture\",\"port\":1}}'\nread stop\necho '{{\"version\":1,\"type\":\"stopped\"}}'\n", start_file.display(), pid_file.display()).into_bytes();
        std::fs::write(&executable, &bytes).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let artifact = HelperArtifact {
            executable,
            state_dir: directory.path().into(),
            expected_sha256: Sha256::digest(bytes).into(),
            derp_map_url: "https://fixture.example/map".into(),
        };
        // Synthetic helper/projection qualify lifecycle only, not MCP or VM work.
        let (session, delivery) = NativeSession::start(
            runtime.clone(),
            &manager,
            authority.clone(),
            artifact,
            |_| Ok(axum::Router::new()),
        )
        .await
        .unwrap();
        assert!(!session.is_stopped());
        let credential = labby_auth::transport_grant::open(
            &key,
            &delivery.envelope,
            &authority.binding(),
            labby_auth::util::now_unix() as u64,
        )
        .unwrap();
        assert!(adapter.verify(&credential).await.is_ok());
        let start: serde_json::Value =
            serde_json::from_slice(&std::fs::read(start_file).unwrap()).unwrap();
        let target = start["target"].as_str().unwrap();
        assert!(tokio::net::TcpStream::connect(target).await.is_ok());
        let pid = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        match mode {
            0 => session.stop().await,
            1 => drop(session),
            2 => {
                labby_gateway::process::unix::send_signal(
                    pid,
                    Some(nix::sys::signal::Signal::SIGTERM),
                )
                .unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(7), async {
                    while !session.is_stopped() {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .unwrap();
            }
            _ => {
                let wire = format!(
                    "lby_pc_v1_source_{}",
                    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
                );
                runtime
                    .store()
                    .await
                    .unwrap()
                    .tombstone_access_artifact(
                        "machine".into(),
                        "credential".into(),
                        "source".into(),
                        Sha256::digest(wire.as_bytes()).into(),
                        1,
                        "test_revocation".into(),
                        labby_auth::util::now_unix(),
                    )
                    .await
                    .unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(7), async {
                    while !session.is_stopped() {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    }
                })
                .await
                .unwrap();
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(7), async {
            while adapter.verify(&credential).await.is_ok() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(adapter.verify(&credential).await.is_err());
        assert!(tokio::net::TcpStream::connect(target).await.is_err());
        assert!(labby_gateway::process::unix::send_signal(pid, None).is_err());
    }
}
