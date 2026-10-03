//! Bounded native pairing and session registry. No browser identity is trusted.
use super::{
    ApprovedPairing, PairingError, PairingRequest, PendingPairing, RequestAuthority,
    session::{HelperArtifact, NativeSession, SessionDelivery},
};
use crate::{
    access::{AccessCredentialAdapter, AccessRuntime},
    config::ProtectedMcpRouteConfig,
};
use labby_auth::{ProductAccessGrantResolver as _, at_rest::TokenEncryptionKey};
use labby_gateway::gateway::manager::GatewayManager;
use labby_primitives::product_credential::{ProductCredential, ProductCredentialVerifier as _};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type Projection = dyn Fn(Arc<RequestAuthority>, ProtectedMcpRouteConfig) -> Result<axum::Router, PairingError>
    + Send
    + Sync;

pub(crate) struct NativeOwner {
    pub oauth_enabled: bool,
    pub runtime: Arc<AccessRuntime>,
    pub adapter: Arc<AccessCredentialAdapter>,
    pub gateway: Arc<GatewayManager>,
    pub installation: String,
    pub key: Arc<TokenEncryptionKey>,
    pub projection: Arc<Projection>,
}

pub(crate) struct PreparedPairing {
    pub id: String,
    pub nonce: [u8; 32],
    pub origin: String,
    pub peer: String,
    pub upstream: String,
}

struct NativePending {
    pairing: PendingPairing,
    source: ProductCredential,
    route: ProtectedMcpRouteConfig,
    started: Instant,
}
enum Slot {
    Starting(tokio_util::sync::CancellationToken),
    Live(NativeSession),
}
type Sessions = Arc<Mutex<HashMap<String, Slot>>>;

pub(crate) struct Manager {
    owner: NativeOwner,
    artifact: HelperArtifact,
    pending: Arc<Mutex<HashMap<String, NativePending>>>,
    sessions: Sessions,
    maintenance: tokio::task::JoinHandle<()>,
}
impl Manager {
    /// Construct only after the host has validated opt-in OAuth/key custody.
    pub(crate) fn new(owner: NativeOwner, artifact: HelperArtifact) -> Result<Self, PairingError> {
        if !owner.oauth_enabled || owner.installation.is_empty() {
            return Err(PairingError);
        }
        let pending = Arc::new(Mutex::new(HashMap::<String, NativePending>::new()));
        let expiring = pending.clone();
        let handle = tokio::runtime::Handle::try_current().map_err(|_| PairingError)?;
        let maintenance = handle.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let Ok(mut entries) = expiring.lock() else {
                    break;
                };
                entries.retain(|_, entry| entry.started.elapsed() < Duration::from_mins(5));
            }
        });
        Ok(Self {
            owner,
            artifact,
            pending,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            maintenance,
        })
    }
    pub(crate) async fn prepare(
        &self,
        request: PairingRequest,
        source: ProductCredential,
    ) -> Result<PreparedPairing, PairingError> {
        let verified = self
            .owner
            .adapter
            .verify(&source)
            .await
            .map_err(|_| PairingError)?;
        let current = self
            .owner
            .adapter
            .resolve(&verified)
            .await
            .map_err(|_| PairingError)?;
        let config =
            tokio::time::timeout(Duration::from_secs(1), self.owner.gateway.current_config())
                .await
                .map_err(|_| PairingError)?;
        let route = config
            .protected_mcp_routes
            .iter()
            .find(|route| route.name == current.route_id)
            .ok_or(PairingError)?
            .clone();
        let loadout = config
            .loadouts
            .iter()
            .find(|loadout| loadout.name == current.loadout_id)
            .ok_or(PairingError)?;
        let view = PreparedPairing {
            id: uuid::Uuid::new_v4().to_string(),
            nonce: [0; 32],
            origin: request.origin.clone(),
            peer: request.peer.clone(),
            upstream: request.upstream.clone(),
        };
        let pairing = PendingPairing::prepare(
            request,
            current,
            &self.owner.installation,
            &route,
            loadout,
            now()?,
        )?;
        let mut pending = self.pending.lock().map_err(|_| PairingError)?;
        pending.retain(|_, entry| entry.started.elapsed() < Duration::from_mins(5));
        if pending.len() >= 16 {
            return Err(PairingError);
        }
        let mut view = view;
        view.nonce = pairing.nonce().ok_or(PairingError)?;
        pending.insert(
            view.id.clone(),
            NativePending {
                pairing,
                source,
                route,
                started: Instant::now(),
            },
        );
        Ok(view)
    }
    pub(crate) async fn approve(
        &self,
        id: &str,
        nonce: &[u8; 32],
        request: PairingRequest,
        exchange_id: Option<String>,
    ) -> Result<(String, SessionDelivery), PairingError> {
        if exchange_id.as_ref().is_some_and(|id| {
            id.len() != 43
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        }) {
            return Err(PairingError);
        }
        // Removal burns the pending approval even when verification/start fails.
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| PairingError)?
            .remove(id)
            .ok_or(PairingError)?;
        let verified = self
            .owner
            .adapter
            .verify(&pending.source)
            .await
            .map_err(|_| PairingError)?;
        let current = self
            .owner
            .adapter
            .resolve(&verified)
            .await
            .map_err(|_| PairingError)?;
        let approved: ApprovedPairing =
            pending.pairing.approve(nonce, &request, &current, now()?)?;
        let session_id = approved.generation().to_owned();
        let reservation = Reservation::acquire(self.sessions.clone(), session_id.clone())?;
        let authority = Arc::new(RequestAuthority {
            adapter: self.owner.adapter.clone(),
            source: pending.source,
            approved,
            key: self.owner.key.clone(),
        });
        let projection = self.owner.projection.clone();
        let route = pending.route;
        let (session, delivery) = tokio::select! {
            () = reservation.cancel.cancelled() => return Err(PairingError),
            result = NativeSession::start(self.owner.runtime.clone(), &self.owner.gateway,
                authority, self.artifact.clone(), exchange_id, move |authority| projection(authority, route)) => result?,
        };
        reservation.publish(session)?;
        Ok((session_id, delivery))
    }
    pub(crate) fn status(&self) -> Result<Vec<(String, &'static str)>, PairingError> {
        let mut sessions = self.sessions.lock().map_err(|_| PairingError)?;
        sessions.retain(|_, slot| !matches!(slot, Slot::Live(session) if session.is_stopped()));
        let mut result: Vec<_> = sessions
            .iter()
            .map(|(id, slot)| {
                (
                    id.clone(),
                    match slot {
                        Slot::Starting(_) => "starting",
                        Slot::Live(session) => session.phase(),
                    },
                )
            })
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(result)
    }
    pub(crate) async fn stop(&self, id: &str) -> Result<(), PairingError> {
        let slot = self
            .sessions
            .lock()
            .map_err(|_| PairingError)?
            .remove(id)
            .ok_or(PairingError)?;
        match slot {
            Slot::Live(session) => {
                tokio::time::timeout(Duration::from_secs(8), session.stop())
                    .await
                    .map_err(|_| PairingError)?;
                Ok(())
            }
            Slot::Starting(cancel) => {
                cancel.cancel();
                Ok(())
            }
        }
    }
}
impl Drop for Manager {
    fn drop(&mut self) {
        self.maintenance.abort();
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
        if let Ok(mut sessions) = self.sessions.lock() {
            sessions.clear();
        }
    }
}
fn now() -> Result<u64, PairingError> {
    u64::try_from(labby_auth::util::now_unix()).map_err(|_| PairingError)
}

/// Reserves capacity before startup. Cancellation cannot strand a Starting slot.
struct Reservation {
    sessions: Sessions,
    id: String,
    armed: bool,
    cancel: tokio_util::sync::CancellationToken,
}
impl Reservation {
    fn acquire(sessions: Sessions, id: String) -> Result<Self, PairingError> {
        let cancel = tokio_util::sync::CancellationToken::new();
        {
            let mut entries = sessions.lock().map_err(|_| PairingError)?;
            entries.retain(|_, slot| !matches!(slot, Slot::Live(session) if session.is_stopped()));
            if entries.len() >= 8 || entries.contains_key(&id) {
                return Err(PairingError);
            }
            entries.insert(id.clone(), Slot::Starting(cancel.clone()));
        }
        Ok(Self {
            sessions,
            id,
            armed: true,
            cancel,
        })
    }
    fn publish(mut self, session: NativeSession) -> Result<(), PairingError> {
        let mut entries = self.sessions.lock().map_err(|_| PairingError)?;
        if !matches!(entries.get(&self.id), Some(Slot::Starting(_))) {
            return Err(PairingError);
        }
        entries.insert(self.id.clone(), Slot::Live(session));
        self.armed = false;
        Ok(())
    }
}
impl Drop for Reservation {
    fn drop(&mut self) {
        if self.armed {
            self.cancel.cancel();
            if let Ok(mut entries) = self.sessions.lock() {
                entries.remove(&self.id);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::Engine as _;
    fn source() -> ProductCredential {
        ProductCredential::parse(&format!(
            "lby_pc_v1_source_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0xA5; 32])
        ))
        .unwrap()
    }
    fn request() -> PairingRequest {
        PairingRequest {
            origin: "https://depot.example".into(),
            peer: format!("nodekey:{}", "1".repeat(64)),
            upstream: "msb".into(),
        }
    }
    pub(crate) async fn fixture() -> (tempfile::TempDir, Manager) {
        let (directory, runtime, adapter, _approved, gateway) =
            super::super::testing::fixture_with_manager().await;
        let manager = Manager::new(
            NativeOwner {
                oauth_enabled: true,
                runtime,
                adapter,
                gateway: Arc::new(gateway),
                installation: "machine".into(),
                key: Arc::new(TokenEncryptionKey::from_encoded(&"01".repeat(32)).unwrap()),
                projection: Arc::new(|_, _| Ok(axum::Router::new())),
            },
            HelperArtifact {
                executable: directory.path().join("absent-helper"),
                state_dir: directory.path().into(),
                expected_sha256: [0; 32],
                derp_map_url: "https://fixture.example/map".into(),
            },
        )
        .unwrap();
        (directory, manager)
    }
    #[tokio::test]
    async fn failed_approval_burns_native_nonce_without_starting_helper() {
        let (_directory, manager) = fixture().await;
        let prepared = manager.prepare(request(), source()).await.unwrap();
        let mut wrong = prepared.nonce;
        wrong[0] ^= 1;
        assert!(
            manager
                .approve(&prepared.id, &wrong, request(), None)
                .await
                .is_err()
        );
        assert!(
            manager
                .approve(&prepared.id, &prepared.nonce, request(), None)
                .await
                .is_err()
        );
        assert!(manager.status().unwrap().is_empty());
    }
    #[tokio::test]
    async fn disabled_oauth_refuses_native_control_owner() {
        let (_directory, manager) = fixture().await;
        let owner = NativeOwner {
            oauth_enabled: false,
            runtime: manager.owner.runtime.clone(),
            adapter: manager.owner.adapter.clone(),
            gateway: manager.owner.gateway.clone(),
            installation: manager.owner.installation.clone(),
            key: manager.owner.key.clone(),
            projection: manager.owner.projection.clone(),
        };
        assert!(Manager::new(owner, manager.artifact.clone()).is_err());
    }
    #[tokio::test]
    async fn pending_pairings_have_a_hard_capacity() {
        let (_directory, manager) = fixture().await;
        for _ in 0..16 {
            manager.prepare(request(), source()).await.unwrap();
        }
        assert!(manager.prepare(request(), source()).await.is_err());
        assert_eq!(manager.pending.lock().unwrap().len(), 16);
    }
    #[test]
    fn canceled_startup_releases_bounded_session_reservation() {
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        let mut reservations = Vec::new();
        for id in 0..8 {
            reservations.push(Reservation::acquire(sessions.clone(), id.to_string()).unwrap());
        }
        assert!(Reservation::acquire(sessions.clone(), "overflow".into()).is_err());
        let cancel = reservations[0].cancel.clone();
        drop(reservations);
        assert!(cancel.is_cancelled());
        assert!(sessions.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn stop_cancels_in_progress_startup() {
        let (_directory, manager) = fixture().await;
        let reservation =
            Reservation::acquire(manager.sessions.clone(), "starting".into()).unwrap();
        manager.stop("starting").await.unwrap();
        assert!(reservation.cancel.is_cancelled());
        assert!(manager.status().unwrap().is_empty());
    }
}
