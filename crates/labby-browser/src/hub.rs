//! Live browser connection registry and bounded invocation routing.

use std::collections::HashMap;
use std::path::Path;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier as _, VerifyingKey};
use serde_json::Value;
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
use uuid::Uuid;

use crate::error::{BrowserError, Result};
use crate::protocol::{BrowserEnvelope, BrowserMessage, CatalogObservation};
use crate::store::{PairingRequest, Store, decode_public_key};

const MAX_PENDING_CALLS: usize = 100;
const DEFAULT_TOOL_TIMEOUT: Duration = Duration::from_secs(15);

/// Outbound event delivered to an authenticated extension connection.
#[derive(Clone, Debug)]
pub struct BrowserEvent(pub BrowserEnvelope);

/// Connection handle owned by an HTTP/WebSocket adapter.
pub struct BrowserConnection {
    pub browser_id: String,
    /// Opaque identity used to avoid an old socket disconnecting its replacement.
    pub connection_id: String,
    pub receiver: mpsc::Receiver<BrowserEvent>,
    owner: BrowserBridge,
}

impl Drop for BrowserConnection {
    fn drop(&mut self) {
        // Cancellation may bypass the adapter's explicit shutdown path.
        if let Err(error) = self.owner.disconnect(&self.browser_id, &self.connection_id) {
            tracing::warn!(
                error_kind = error.kind(),
                "browser connection cleanup failed"
            );
        }
    }
}

struct LiveConnection {
    generation: Uuid,
    sender: mpsc::Sender<BrowserEvent>,
}

struct PendingCall {
    browser_id: String,
    tab_id: i64,
    document_id: String,
    catalog_digest: String,
    generation: Uuid,
    reply: oneshot::Sender<Result<Value>>,
}

struct CallGuard {
    bridge: BrowserBridge,
    call_id: String,
    generation: Uuid,
    audit_id: Option<String>,
    started: Instant,
    armed: bool,
}

impl CallGuard {
    fn set_audit_id(&mut self, audit_id: String) {
        self.audit_id = Some(audit_id);
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CallGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(error) = self.bridge.cancel_pending(&self.call_id, self.generation) {
            tracing::warn!(
                call_id = self.call_id,
                error_kind = error.kind(),
                "cancelled browser call cleanup failed"
            );
        }
        let Some(audit_id) = self.audit_id.clone() else {
            return;
        };
        let Some(cleanup_permit) = self.bridge.store.try_acquire_cancellation_cleanup() else {
            tracing::warn!(
                audit_id,
                "cancelled browser call audit cleanup deferred until process restart: backlog full"
            );
            return;
        };
        let store = self.bridge.store.clone();
        let duration = i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(
                audit_id,
                "cancelled browser call audit cleanup deferred until process restart"
            );
            return;
        };
        runtime.spawn(async move {
            let _cleanup_permit = cleanup_permit;
            if let Err(error) = store.abandon_invocation(&audit_id, duration).await {
                tracing::warn!(
                    audit_id,
                    error_kind = error.kind(),
                    "cancelled browser call audit cleanup failed"
                );
            }
        });
    }
}

#[cfg(test)]
#[derive(Default)]
struct CallTestHooks {
    pause_after_pending: AtomicBool,
    pending_inserted: tokio::sync::Notify,
    resume_after_pending: tokio::sync::Notify,
}

#[derive(Default)]
struct HubState {
    connections: HashMap<String, LiveConnection>,
    pending: HashMap<String, PendingCall>,
}

/// Shared Rust browser bridge runtime.
#[derive(Clone)]
pub struct BrowserBridge {
    store: Store,
    state: Arc<Mutex<HubState>>,
    authority: Arc<AsyncMutex<()>>,
    #[cfg(test)]
    call_test_hooks: Arc<CallTestHooks>,
}

impl BrowserBridge {
    /// Open a durable browser bridge.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: Store::open(path).await?,
            state: Arc::new(Mutex::new(HubState::default())),
            authority: Arc::new(AsyncMutex::new(())),
            #[cfg(test)]
            call_test_hooks: Arc::new(CallTestHooks::default()),
        })
    }

    /// Build an in-memory bridge for tests.
    pub async fn memory() -> Result<Self> {
        Ok(Self {
            store: Store::memory().await?,
            state: Arc::new(Mutex::new(HubState::default())),
            authority: Arc::new(AsyncMutex::new(())),
            #[cfg(test)]
            call_test_hooks: Arc::new(CallTestHooks::default()),
        })
    }

    /// Durable store used by dispatch adapters.
    #[must_use]
    pub const fn store(&self) -> &Store {
        &self.store
    }

    /// Accept an unauthenticated pairing request from a loopback-gated adapter.
    pub async fn request_pairing(
        &self,
        display_name: &str,
        extension_id: &str,
        public_key: &str,
    ) -> Result<PairingRequest> {
        self.store
            .request_pairing(display_name, extension_id, decode_public_key(public_key)?)
            .await
    }

    /// Issue a one-time challenge.
    pub async fn issue_challenge(&self, browser_id: &str) -> Result<BrowserEnvelope> {
        let challenge = self.store.create_challenge(browser_id).await?;
        Ok(BrowserEnvelope::new(
            None,
            BrowserMessage::AuthNonce {
                challenge_id: challenge.id,
                nonce: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(challenge.nonce),
                expires_at: challenge.expires_at,
            },
        ))
    }

    /// Verify and consume a challenge, then install this connection as current.
    pub async fn authenticate(
        &self,
        challenge_id: &str,
        signature: &str,
    ) -> Result<BrowserConnection> {
        let challenge = self.store.take_challenge(challenge_id).await?;
        let browser = self
            .store
            .browser(&challenge.browser_id)
            .await?
            .ok_or(BrowserError::AuthenticationFailed)?;
        if browser.revoked_at.is_some() {
            return Err(BrowserError::AuthenticationFailed);
        }
        let public_key: [u8; 32] = browser
            .public_key
            .as_slice()
            .try_into()
            .map_err(|_| BrowserError::AuthenticationFailed)?;
        let signature_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| BrowserError::AuthenticationFailed)?;
        let signature = Signature::from_slice(&signature_bytes)
            .map_err(|_| BrowserError::AuthenticationFailed)?;
        VerifyingKey::from_bytes(&public_key)
            .map_err(|_| BrowserError::AuthenticationFailed)?
            .verify(&challenge.nonce, &signature)
            .map_err(|_| BrowserError::AuthenticationFailed)?;
        let _authority = self.authority.lock().await;
        self.store.touch_browser(&browser.id).await?;
        let (sender, receiver) = mpsc::channel(128);
        let generation = Uuid::new_v4();
        if self
            .store
            .browser(&browser.id)
            .await?
            .is_none_or(|current| current.revoked_at.is_some())
        {
            return Err(BrowserError::AuthenticationFailed);
        }
        let mut state = self.lock_state()?;
        if let Some(replaced) = state
            .connections
            .insert(browser.id.clone(), LiveConnection { generation, sender })
        {
            finish_generation(&mut state, &replaced);
        }
        let connection = BrowserConnection {
            browser_id: browser.id,
            connection_id: generation.to_string(),
            receiver,
            owner: self.clone(),
        };
        drop(state);
        Ok(connection)
    }

    /// Remove exactly the connection generation owned by an adapter.
    pub fn disconnect(&self, browser_id: &str, connection_id: &str) -> Result<()> {
        let mut state = self.lock_state()?;
        let owns_current = state
            .connections
            .get(browser_id)
            .is_some_and(|connection| connection.generation.to_string() == connection_id);
        if owns_current && let Some(connection) = state.connections.remove(browser_id) {
            finish_generation(&mut state, &connection);
        }
        Ok(())
    }

    /// Persist an authenticated catalog observation.
    pub async fn observe(
        &self,
        browser_id: &str,
        connection_id: &str,
        observation: &CatalogObservation,
    ) -> Result<()> {
        let _authority = self.authority.lock().await;
        self.ensure_current(browser_id, connection_id)?;
        let catalog = serde_json::to_string(&observation.tools)?;
        let digest = crate::store::digest_catalog(
            observation.catalog_revision,
            &observation.origin,
            &observation.catalog_fingerprint,
            &catalog,
        );
        self.cancel_document_calls(browser_id, observation.tab_id, |call| {
            call.document_id != observation.document_id || call.catalog_digest != digest
        })?;
        self.store.observe(browser_id, observation).await
    }

    /// Close one exact document owned by an authenticated browser.
    pub async fn close_document(
        &self,
        browser_id: &str,
        connection_id: &str,
        tab_id: i64,
        document_id: &str,
    ) -> Result<()> {
        let _authority = self.authority.lock().await;
        self.ensure_current(browser_id, connection_id)?;
        self.cancel_document_calls(browser_id, tab_id, |call| call.document_id == document_id)?;
        self.store
            .close_document(browser_id, tab_id, document_id)
            .await
    }

    /// Complete a call only from its owning browser and current generation.
    pub fn complete(
        &self,
        browser_id: &str,
        connection_id: &str,
        message: BrowserMessage,
    ) -> Result<bool> {
        let (call_id, outcome) = match message {
            BrowserMessage::ToolResult { call_id, result } => (call_id, Ok(result)),
            BrowserMessage::ToolError {
                call_id,
                kind,
                message,
            } => (
                call_id,
                Err(BrowserError::InvalidRequest(format!("{kind}: {message}"))),
            ),
            _ => {
                return Err(BrowserError::InvalidRequest(
                    "expected tool completion".to_string(),
                ));
            }
        };
        let mut state = self.lock_state()?;
        let Some(pending) = state.pending.get(&call_id) else {
            return Ok(false);
        };
        let Some(connection) = state.connections.get(browser_id) else {
            return Ok(false);
        };
        if connection.generation.to_string() != connection_id {
            return Ok(false);
        }
        if pending.browser_id != browser_id || pending.generation != connection.generation {
            return Ok(false);
        }
        let pending = state.pending.remove(&call_id).expect("pending call exists");
        drop(pending.reply.send(outcome));
        Ok(true)
    }

    /// Invoke one exact document/catalog tuple with bounded capacity and time.
    pub async fn call(
        &self,
        browser_id: &str,
        tab_id: i64,
        document_id: String,
        catalog_revision: i64,
        catalog_digest: String,
        tool_name: String,
        arguments: Value,
        timeout: Option<Duration>,
    ) -> Result<Value> {
        let started = Instant::now();
        let call_id = Uuid::new_v4().to_string();
        let (reply, wait) = oneshot::channel();
        let (sender, generation, catalog_fingerprint) = {
            let _authority = self.authority.lock().await;
            let catalog_fingerprint = self
                .store
                .validate_call(
                    browser_id,
                    tab_id,
                    &document_id,
                    catalog_revision,
                    &catalog_digest,
                    &tool_name,
                )
                .await?;
            let mut state = self.lock_state()?;
            if state.pending.len() >= MAX_PENDING_CALLS {
                return Err(BrowserError::ServerBusy);
            }
            let connection = state
                .connections
                .get(browser_id)
                .ok_or(BrowserError::BrowserOffline)?;
            let sender = connection.sender.clone();
            let generation = connection.generation;
            state.pending.insert(
                call_id.clone(),
                PendingCall {
                    browser_id: browser_id.to_string(),
                    tab_id,
                    document_id: document_id.clone(),
                    catalog_digest: catalog_digest.clone(),
                    generation,
                    reply,
                },
            );
            (sender, generation, catalog_fingerprint)
        };
        // Install cancellation cleanup before any await that follows pending
        // publication. The audit id is attached only after persistence starts.
        let mut guard = CallGuard {
            bridge: self.clone(),
            call_id: call_id.clone(),
            generation,
            audit_id: None,
            started,
            armed: true,
        };
        #[cfg(test)]
        if self
            .call_test_hooks
            .pause_after_pending
            .swap(false, Ordering::SeqCst)
        {
            self.call_test_hooks.pending_inserted.notify_one();
            self.call_test_hooks.resume_after_pending.notified().await;
        }
        let event = BrowserEvent(BrowserEnvelope::new(
            None,
            BrowserMessage::ToolCall {
                call_id: call_id.clone(),
                tab_id,
                document_id: document_id.clone(),
                catalog_revision,
                catalog_fingerprint,
                tool_name: tool_name.clone(),
                arguments,
            },
        ));
        let audit_id = self
            .store
            .begin_invocation(
                browser_id,
                tab_id,
                &document_id,
                &tool_name,
                catalog_revision,
            )
            .await?;
        guard.set_audit_id(audit_id.clone());
        let send_result = {
            let _authority = self.authority.lock().await;
            self.store
                .validate_call(
                    browser_id,
                    tab_id,
                    &document_id,
                    catalog_revision,
                    &catalog_digest,
                    &tool_name,
                )
                .await?;
            // Observation or disconnect may have cancelled admission while audit IO ran.
            if !self
                .lock_state()?
                .pending
                .get(&call_id)
                .is_some_and(|call| call.generation == generation)
            {
                return Err(BrowserError::StaleDocument);
            }
            sender.try_send(event)
        };
        if let Err(error) = send_result {
            self.remove_pending(&call_id, generation)?;
            let result = Err(match error {
                mpsc::error::TrySendError::Full(_) => BrowserError::ServerBusy,
                mpsc::error::TrySendError::Closed(_) => BrowserError::BrowserOffline,
            });
            self.finish_audit(&audit_id, &result, started).await;
            guard.disarm();
            return result;
        }
        let result = match tokio::time::timeout(timeout.unwrap_or(DEFAULT_TOOL_TIMEOUT), wait).await
        {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(BrowserError::ConnectionClosed),
            Err(_) => {
                self.remove_pending(&call_id, generation)?;
                if sender
                    .send(BrowserEvent(BrowserEnvelope::new(
                        None,
                        BrowserMessage::ToolCancel {
                            call_id: call_id.clone(),
                        },
                    )))
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        browser_id,
                        call_id,
                        "browser timeout cancellation delivery failed"
                    );
                }
                Err(BrowserError::ToolTimeout)
            }
        };
        self.finish_audit(&audit_id, &result, started).await;
        guard.disarm();
        result
    }

    /// Current connected browser ids.
    pub fn connected_browser_ids(&self) -> Result<Vec<String>> {
        let mut ids: Vec<_> = self.lock_state()?.connections.keys().cloned().collect();
        ids.sort();
        Ok(ids)
    }

    /// Revoke a browser, close its live connection, and fail its pending calls.
    pub async fn revoke_browser(&self, browser_id: &str) -> Result<crate::store::BrowserRecord> {
        let _authority = self.authority.lock().await;
        let browser = self.store.revoke_browser(browser_id).await?;
        let mut state = self.lock_state()?;
        if let Some(connection) = state.connections.remove(browser_id) {
            finish_generation(&mut state, &connection);
        }
        Ok(browser)
    }

    /// Approve pairing and evict every superseded identity for its extension.
    pub async fn approve_pairing(&self, pairing_id: &str) -> Result<crate::store::BrowserRecord> {
        let _authority = self.authority.lock().await;
        let extension_id = self
            .store
            .pairing(pairing_id)
            .await?
            .ok_or(BrowserError::NotFound)?
            .extension_id;
        let superseded: Vec<_> = self
            .store
            .browsers()
            .await?
            .into_iter()
            .filter(|browser| browser.extension_id == extension_id && browser.revoked_at.is_none())
            .map(|browser| browser.id)
            .collect();
        let browser = self.store.approve_pairing(pairing_id).await?;
        let mut state = self.lock_state()?;
        for browser_id in superseded {
            if let Some(connection) = state.connections.remove(&browser_id) {
                finish_generation(&mut state, &connection);
            }
        }
        Ok(browser)
    }

    fn cancel_document_calls(
        &self,
        browser_id: &str,
        tab_id: i64,
        stale: impl Fn(&PendingCall) -> bool,
    ) -> Result<()> {
        let mut state = self.lock_state()?;
        let ids: Vec<_> = state
            .pending
            .iter()
            .filter(|(_, call)| {
                call.browser_id == browser_id && call.tab_id == tab_id && stale(call)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let Some(call) = state.pending.remove(&id) else {
                continue;
            };
            if let Some(connection) = state.connections.get(browser_id)
                && connection.generation == call.generation
                && connection
                    .sender
                    .try_send(BrowserEvent(BrowserEnvelope::new(
                        None,
                        BrowserMessage::ToolCancel {
                            call_id: id.clone(),
                        },
                    )))
                    .is_err()
            {
                tracing::warn!(
                    call_id = id,
                    "browser stale-document cancellation delivery failed"
                );
            }
            drop(call.reply.send(Err(BrowserError::StaleDocument)));
        }
        Ok(())
    }

    fn cancel_pending(&self, call_id: &str, generation: Uuid) -> Result<()> {
        let mut state = self.lock_state()?;
        if state
            .pending
            .get(call_id)
            .is_some_and(|call| call.generation == generation)
            && let Some(call) = state.pending.remove(call_id)
            && let Some(connection) = state.connections.get(&call.browser_id)
            && connection.generation == generation
            && connection
                .sender
                .try_send(BrowserEvent(BrowserEnvelope::new(
                    None,
                    BrowserMessage::ToolCancel {
                        call_id: call_id.to_string(),
                    },
                )))
                .is_err()
        {
            tracing::warn!(call_id, "browser caller cancellation delivery failed");
        }
        Ok(())
    }

    fn remove_pending(&self, call_id: &str, generation: Uuid) -> Result<()> {
        let mut state = self.lock_state()?;
        if state
            .pending
            .get(call_id)
            .is_some_and(|pending| pending.generation == generation)
        {
            state.pending.remove(call_id);
        }
        Ok(())
    }

    fn ensure_current(&self, browser_id: &str, connection_id: &str) -> Result<()> {
        let current = self
            .lock_state()?
            .connections
            .get(browser_id)
            .is_some_and(|connection| connection.generation.to_string() == connection_id);
        if current {
            Ok(())
        } else {
            Err(BrowserError::AuthenticationFailed)
        }
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, HubState>> {
        self.state
            .lock()
            .map_err(|_| BrowserError::InvalidRequest("browser hub lock poisoned".to_string()))
    }

    async fn finish_audit(&self, audit_id: &str, result: &Result<Value>, started: Instant) {
        if let Err(error) = self
            .store
            .finish_invocation(
                audit_id,
                result,
                i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX),
            )
            .await
        {
            tracing::warn!(
                audit_id,
                error_kind = error.kind(),
                "browser invocation audit write failed"
            );
        }
    }
}

fn finish_generation(state: &mut HubState, connection: &LiveConnection) {
    let call_ids: Vec<_> = state
        .pending
        .iter()
        .filter_map(|(id, pending)| {
            (pending.generation == connection.generation).then(|| id.clone())
        })
        .collect();
    for call_id in call_ids {
        if let Some(pending) = state.pending.remove(&call_id) {
            if connection
                .sender
                .try_send(BrowserEvent(BrowserEnvelope::new(
                    None,
                    BrowserMessage::ToolCancel {
                        call_id: call_id.clone(),
                    },
                )))
                .is_err()
            {
                tracing::debug!(
                    call_id,
                    "closing browser generation could not receive cancellation"
                );
            }
            drop(pending.reply.send(Err(BrowserError::BrowserOffline)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer as _, SigningKey};

    const EXTENSION_ID: &str = "abcdefghijklmnopabcdefghijklmnop";

    #[tokio::test]
    async fn dropping_transport_owner_removes_its_exact_connection() {
        let bridge = BrowserBridge::memory().await.unwrap();
        let connection = pair_and_authenticate(&bridge).await;
        assert_eq!(
            bridge.connected_browser_ids().unwrap(),
            vec![connection.browser_id.clone()]
        );
        drop(connection);
        assert!(bridge.connected_browser_ids().unwrap().is_empty());
    }

    #[tokio::test]
    async fn caller_cancellation_reaches_the_exact_page_call() {
        let bridge = BrowserBridge::memory().await.unwrap();
        let mut connection = pair_and_authenticate(&bridge).await;
        enable_tool(
            &bridge,
            &connection.browser_id,
            &connection.connection_id,
            1,
            1,
            "slow",
        )
        .await;
        let task_bridge = bridge.clone();
        let browser_id = connection.browser_id.clone();
        let task = tokio::spawn(async move {
            task_bridge
                .call(
                    &browser_id,
                    1,
                    "doc".into(),
                    1,
                    current_digest(&task_bridge).await,
                    "slow".into(),
                    Value::Null,
                    None,
                )
                .await
        });
        let event = tokio::time::timeout(Duration::from_secs(2), connection.receiver.recv())
            .await
            .unwrap()
            .unwrap()
            .0;
        let call_id = match event.message {
            BrowserMessage::ToolCall { call_id, .. } => Some(call_id),
            _ => None,
        }
        .expect("expected call");
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let event = tokio::time::timeout(Duration::from_secs(2), connection.receiver.recv())
            .await
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(event.message, BrowserMessage::ToolCancel { call_id });
        assert!(bridge.lock_state().unwrap().pending.is_empty());
    }

    async fn pair_and_authenticate(bridge: &BrowserBridge) -> BrowserConnection {
        let signing = SigningKey::from_bytes(&[9; 32]);
        let public_key = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing.verifying_key().as_bytes());
        let pairing = bridge
            .request_pairing("Chrome", EXTENSION_ID, &public_key)
            .await
            .unwrap();
        let browser = bridge.store().approve_pairing(&pairing.id).await.unwrap();
        authenticate_browser(bridge, &browser.id, &signing).await
    }

    async fn authenticate_browser(
        bridge: &BrowserBridge,
        browser_id: &str,
        signing: &SigningKey,
    ) -> BrowserConnection {
        let challenge = bridge.issue_challenge(browser_id).await.unwrap();
        assert!(matches!(
            challenge.message,
            BrowserMessage::AuthNonce { .. }
        ));
        let BrowserMessage::AuthNonce {
            challenge_id,
            nonce,
            ..
        } = challenge.message
        else {
            return bridge.authenticate("invalid", "invalid").await.unwrap();
        };
        let nonce = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(nonce)
            .unwrap();
        let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(signing.sign(&nonce).to_bytes());
        bridge
            .authenticate(&challenge_id, &signature)
            .await
            .unwrap()
    }

    async fn enable_tool(
        bridge: &BrowserBridge,
        browser_id: &str,
        connection_id: &str,
        tab_id: i64,
        revision: i64,
        name: &str,
    ) {
        bridge
            .observe(
                browser_id,
                connection_id,
                &CatalogObservation {
                    tab_id,
                    document_id: "doc".into(),
                    origin: "https://example.com".into(),
                    sanitized_path: "/".into(),
                    page_title: "Example".into(),
                    catalog_revision: revision,
                    catalog_fingerprint: "fingerprint".into(),
                    tools: vec![crate::protocol::ToolDescriptor {
                        name: name.into(),
                        description: String::new(),
                        input_schema: serde_json::json!({"type":"object"}),
                        annotations: Value::Null,
                    }],
                },
            )
            .await
            .unwrap();
        let session = bridge
            .store()
            .sessions(None, None)
            .await
            .unwrap()
            .sessions
            .remove(0);
        bridge
            .store()
            .set_session_enabled(
                &session.id,
                true,
                &bridge
                    .store()
                    .session(&session.id)
                    .await
                    .unwrap()
                    .catalog_digest,
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn routes_call_and_accepts_only_current_browser_completion() {
        let bridge = BrowserBridge::memory().await.unwrap();
        let mut connection = pair_and_authenticate(&bridge).await;
        let browser_id = connection.browser_id.clone();
        let connection_id = connection.connection_id.clone();
        enable_tool(&bridge, &browser_id, &connection_id, 7, 3, "search").await;
        let task_bridge = bridge.clone();
        let task_browser = browser_id.clone();
        let task = tokio::spawn(async move {
            task_bridge
                .call(
                    &task_browser,
                    7,
                    "doc".into(),
                    3,
                    current_digest(&task_bridge).await,
                    "search".into(),
                    serde_json::json!({"q":"rust"}),
                    Some(Duration::from_secs(1)),
                )
                .await
        });
        let event = connection.receiver.recv().await.unwrap().0;
        assert!(matches!(event.message, BrowserMessage::ToolCall { .. }));
        let BrowserMessage::ToolCall { call_id, .. } = event.message else {
            return;
        };
        assert!(
            bridge
                .complete(
                    &browser_id,
                    &connection_id,
                    BrowserMessage::ToolResult {
                        call_id,
                        result: serde_json::json!({"ok":true}),
                    },
                )
                .unwrap()
        );
        assert_eq!(task.await.unwrap().unwrap(), serde_json::json!({"ok":true}));
    }

    #[tokio::test]
    async fn timeout_sends_exact_cancellation() {
        let bridge = BrowserBridge::memory().await.unwrap();
        let mut connection = pair_and_authenticate(&bridge).await;
        let task_bridge = bridge.clone();
        let browser_id = connection.browser_id.clone();
        let connection_id = connection.connection_id.clone();
        enable_tool(&bridge, &browser_id, &connection_id, 1, 1, "slow").await;
        let task = tokio::spawn(async move {
            task_bridge
                .call(
                    &browser_id,
                    1,
                    "doc".into(),
                    1,
                    current_digest(&task_bridge).await,
                    "slow".into(),
                    Value::Null,
                    Some(Duration::from_millis(10)),
                )
                .await
        });
        let first = connection.receiver.recv().await.unwrap().0;
        assert!(matches!(first.message, BrowserMessage::ToolCall { .. }));
        let BrowserMessage::ToolCall { call_id, .. } = first.message else {
            return;
        };
        let cancellation = connection.receiver.recv().await.unwrap().0;
        assert_eq!(cancellation.message, BrowserMessage::ToolCancel { call_id });
        assert_eq!(task.await.unwrap().unwrap_err().kind(), "tool_timeout");
    }

    #[tokio::test]
    async fn cancellation_while_audit_store_is_blocked_reclaims_pending_capacity() {
        let bridge = BrowserBridge::memory().await.unwrap();
        let connection = pair_and_authenticate(&bridge).await;
        enable_tool(
            &bridge,
            &connection.browser_id,
            &connection.connection_id,
            1,
            1,
            "slow",
        )
        .await;
        bridge
            .call_test_hooks
            .pause_after_pending
            .store(true, Ordering::SeqCst);
        let task_bridge = bridge.clone();
        let browser_id = connection.browser_id.clone();
        let task = tokio::spawn(async move {
            task_bridge
                .call(
                    &browser_id,
                    1,
                    "doc".into(),
                    1,
                    current_digest(&task_bridge).await,
                    "slow".into(),
                    Value::Null,
                    Some(Duration::from_secs(1)),
                )
                .await
        });

        bridge.call_test_hooks.pending_inserted.notified().await;
        let store_permit = bridge.store().hold_executor_for_test().await;
        bridge.call_test_hooks.resume_after_pending.notify_one();
        tokio::task::yield_now().await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(bridge.lock_state().unwrap().pending.is_empty());
        drop(store_permit);
    }

    #[tokio::test]
    async fn cancellation_audit_cleanup_backlog_is_bounded() {
        let directory = crate::store::private_test_directory();
        let database = directory.path().join("browser.sqlite3");
        let bridge = BrowserBridge::open(&database).await.unwrap();
        let mut audit_ids = Vec::new();
        for index in 0..=crate::store::MAX_CANCELLATION_AUDIT_CLEANUPS {
            audit_ids.push(
                bridge
                    .store()
                    .begin_invocation("browser", index as i64, "doc", "tool", 1)
                    .await
                    .unwrap(),
            );
        }
        let store_permit = bridge.store().hold_executor_for_test().await;
        let generation = Uuid::new_v4();

        for (index, audit_id) in audit_ids.iter().enumerate() {
            drop(CallGuard {
                bridge: bridge.clone(),
                call_id: format!("call-{index}"),
                generation,
                audit_id: Some(audit_id.clone()),
                started: Instant::now(),
                armed: true,
            });
        }

        assert_eq!(bridge.store().available_cancellation_cleanups(), 0);
        drop(store_permit);
        tokio::time::timeout(
            Duration::from_secs(30),
            bridge.store().wait_for_cancellation_cleanups_for_test(),
        )
        .await
        .unwrap();

        let overflow_id = audit_ids.last().unwrap();
        assert_eq!(
            bridge
                .store()
                .audit_outcome_for_test(overflow_id)
                .await
                .unwrap(),
            ("started".to_string(), None)
        );
        drop(bridge);

        let reopened = BrowserBridge::open(database).await.unwrap();
        assert_eq!(
            reopened
                .store()
                .audit_outcome_for_test(overflow_id)
                .await
                .unwrap(),
            (
                "abandoned".to_string(),
                Some("process_restarted".to_string())
            )
        );
    }

    #[test]
    fn dropping_armed_call_guard_without_runtime_does_not_panic() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let bridge = runtime.block_on(BrowserBridge::memory()).unwrap();
        let generation = Uuid::new_v4();
        let (reply, _wait) = oneshot::channel();
        bridge.lock_state().unwrap().pending.insert(
            "call".into(),
            PendingCall {
                browser_id: "browser".into(),
                tab_id: 1,
                document_id: "doc".into(),
                catalog_digest: "digest".into(),
                generation,
                reply,
            },
        );
        let guard = CallGuard {
            bridge: bridge.clone(),
            call_id: "call".into(),
            generation,
            audit_id: Some("audit".into()),
            started: Instant::now(),
            armed: true,
        };
        drop(runtime);

        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(guard))).is_ok());
        assert!(bridge.lock_state().unwrap().pending.is_empty());
    }

    #[tokio::test]
    async fn replacement_socket_is_the_only_authoritative_generation() {
        let bridge = BrowserBridge::memory().await.unwrap();
        let old = pair_and_authenticate(&bridge).await;
        let signing = SigningKey::from_bytes(&[9; 32]);
        let current = authenticate_browser(&bridge, &old.browser_id, &signing).await;

        assert!(old.receiver.is_closed());
        assert_eq!(
            bridge
                .observe(
                    &old.browser_id,
                    &old.connection_id,
                    &CatalogObservation {
                        tab_id: 1,
                        document_id: "doc".into(),
                        origin: "https://example.com".into(),
                        sanitized_path: "/".into(),
                        page_title: "Example".into(),
                        catalog_revision: 1,
                        catalog_fingerprint: "one".into(),
                        tools: vec![],
                    },
                )
                .await
                .unwrap_err()
                .kind(),
            "auth_failed"
        );
        assert!(
            bridge
                .connected_browser_ids()
                .unwrap()
                .contains(&current.browser_id)
        );
    }

    #[tokio::test]
    async fn stale_generation_cannot_complete_current_call_or_disconnect_it() {
        let bridge = BrowserBridge::memory().await.unwrap();
        let old = pair_and_authenticate(&bridge).await;
        let signing = SigningKey::from_bytes(&[9; 32]);
        let mut current = authenticate_browser(&bridge, &old.browser_id, &signing).await;
        enable_tool(
            &bridge,
            &current.browser_id,
            &current.connection_id,
            4,
            1,
            "search",
        )
        .await;
        let task_bridge = bridge.clone();
        let browser_id = current.browser_id.clone();
        let task_browser_id = browser_id.clone();
        let task = tokio::spawn(async move {
            task_bridge
                .call(
                    &task_browser_id,
                    4,
                    "doc".into(),
                    1,
                    current_digest(&task_bridge).await,
                    "search".into(),
                    Value::Null,
                    Some(Duration::from_secs(1)),
                )
                .await
        });
        let event = current.receiver.recv().await.unwrap().0;
        let BrowserMessage::ToolCall { call_id, .. } = event.message else {
            unreachable!()
        };
        assert!(
            !bridge
                .complete(
                    &browser_id,
                    &old.connection_id,
                    BrowserMessage::ToolResult {
                        call_id: call_id.clone(),
                        result: Value::Null
                    }
                )
                .unwrap()
        );
        bridge.disconnect(&browser_id, &old.connection_id).unwrap();
        assert!(
            bridge
                .complete(
                    &browser_id,
                    &current.connection_id,
                    BrowserMessage::ToolResult {
                        call_id,
                        result: serde_json::json!({"ok": true})
                    }
                )
                .unwrap()
        );
        assert!(task.await.unwrap().is_ok());
        assert!(
            bridge
                .connected_browser_ids()
                .unwrap()
                .contains(&browser_id)
        );
    }

    #[tokio::test]
    async fn challenge_cannot_authenticate_after_revocation() {
        let bridge = BrowserBridge::memory().await.unwrap();
        let connection = pair_and_authenticate(&bridge).await;
        let challenge = bridge
            .issue_challenge(&connection.browser_id)
            .await
            .unwrap();
        let BrowserMessage::AuthNonce {
            challenge_id,
            nonce,
            ..
        } = challenge.message
        else {
            unreachable!()
        };
        let signing = SigningKey::from_bytes(&[9; 32]);
        let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            signing
                .sign(
                    &base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .decode(nonce)
                        .unwrap(),
                )
                .to_bytes(),
        );
        bridge.revoke_browser(&connection.browser_id).await.unwrap();
        assert!(matches!(
            bridge.authenticate(&challenge_id, &signature).await,
            Err(BrowserError::AuthenticationFailed)
        ));
    }
    #[tokio::test]
    async fn schema_replacement_and_document_close_cancel_pending_calls() {
        for close_document in [false, true] {
            let bridge = BrowserBridge::memory().await.unwrap();
            let mut connection = pair_and_authenticate(&bridge).await;
            let browser_id = connection.browser_id.clone();
            let connection_id = connection.connection_id.clone();
            enable_tool(&bridge, &browser_id, &connection_id, 7, 3, "search").await;
            let task_bridge = bridge.clone();
            let task_browser = browser_id.clone();
            let task = tokio::spawn(async move {
                task_bridge
                    .call(
                        &task_browser,
                        7,
                        "doc".into(),
                        3,
                        current_digest(&task_bridge).await,
                        "search".into(),
                        Value::Null,
                        Some(Duration::from_secs(2)),
                    )
                    .await
            });
            let event = connection.receiver.recv().await.unwrap().0;
            let call_id = match event.message {
                BrowserMessage::ToolCall { call_id, .. } => Some(call_id),
                _ => None,
            }
            .expect("expected admitted page call");
            if close_document {
                bridge
                    .close_document(&browser_id, &connection_id, 7, "doc")
                    .await
                    .unwrap();
            } else {
                let listing = bridge.store().sessions(None, None).await.unwrap();
                let detail = bridge
                    .store()
                    .session(&listing.sessions[0].id)
                    .await
                    .unwrap();
                let mut tools = detail.tools;
                tools[0].input_schema = serde_json::json!({"type":"object", "required":["new"]});
                bridge
                    .observe(
                        &browser_id,
                        &connection_id,
                        &CatalogObservation {
                            tab_id: 7,
                            document_id: "doc".into(),
                            origin: detail.origin,
                            sanitized_path: detail.sanitized_path,
                            page_title: detail.page_title,
                            catalog_revision: 3,
                            catalog_fingerprint: detail.catalog_fingerprint,
                            tools,
                        },
                    )
                    .await
                    .unwrap();
            }
            let cancellation = connection.receiver.recv().await.unwrap().0;
            assert!(
                matches!(cancellation.message, BrowserMessage::ToolCancel { call_id: cancelled } if cancelled == call_id)
            );
            assert!(matches!(
                task.await.unwrap(),
                Err(BrowserError::StaleDocument)
            ));
            assert!(
                !bridge
                    .complete(
                        &browser_id,
                        &connection_id,
                        BrowserMessage::ToolResult {
                            call_id,
                            result: serde_json::json!({"late":true}),
                        }
                    )
                    .unwrap()
            );
        }
    }

    async fn current_digest(bridge: &BrowserBridge) -> String {
        let listing = bridge.store().sessions(None, None).await.unwrap();
        bridge
            .store()
            .session(&listing.sessions[0].id)
            .await
            .unwrap()
            .catalog_digest
    }
}
