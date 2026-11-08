//! Durable MCP webhook subscriptions for the Code Mode inbox.
mod authority;
mod subscriptions;
mod types;
mod webhook;
mod worker;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use labby_auth::upstream::encryption::{EncryptionKey, load_key, open_with_aad, seal_with_aad};
use sha2::{Digest as _, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use zeroize::Zeroizing;

use super::codemode_notices::{NoticeError, NoticeRecipient, NoticeStore, now_ms};
use types::*;
use webhook::{HttpsWebhookSender, WebhookSender};

pub(crate) use authority::{EventAuthority, EventAuthorityDecision, EventAuthorizer};
pub(crate) use types::{
    EventError, EventsListParams, EventsSubscribeParams, EventsUnsubscribeParams,
};
pub(super) use worker::enqueue;

const MAX_VERIFICATIONS: usize = 8;
const VERIFICATION_TTL: Duration = Duration::from_mins(5);
type VerificationFlight = Arc<AsyncMutex<Option<Instant>>>;

struct RegisteredAuthority {
    expires: i64,
    check: Arc<dyn EventAuthorizer>,
    cancel: tokio_util::sync::CancellationToken,
}

pub(crate) struct AgentEvents {
    store: NoticeStore,
    encryption: EncryptionKey,
    sender: Arc<dyn WebhookSender>,
    authorities: Mutex<HashMap<String, RegisteredAuthority>>,
    verification_slots: Semaphore,
    edits: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
    verifications: Mutex<HashMap<[u8; 32], VerificationFlight>>,
}

impl AgentEvents {
    fn new(store: NoticeStore, encryption: EncryptionKey, sender: Arc<dyn WebhookSender>) -> Self {
        Self {
            store,
            encryption,
            sender,
            authorities: Mutex::new(HashMap::new()),
            verification_slots: Semaphore::new(MAX_VERIFICATIONS),
            edits: Mutex::new(HashMap::new()),
            verifications: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) async fn open(store: NoticeStore) -> Result<Self, EventError> {
        if !store.is_enabled() {
            return Err(NoticeError::Unavailable.into());
        }
        let encryption = tokio::task::spawn_blocking(load_installation_key)
            .await
            .map_err(|_| NoticeError::Unavailable)??;
        Ok(Self::new(
            store,
            encryption,
            Arc::new(HttpsWebhookSender::default()),
        ))
    }

    pub(crate) async fn subscribe(
        &self,
        recipient: NoticeRecipient,
        check: Arc<dyn EventAuthorizer>,
        params: EventsSubscribeParams,
    ) -> Result<serde_json::Value, EventError> {
        let (target, key, ttl) = params.validate()?;
        let id = target.id(&recipient)?;
        let recipient_key = recipient.key()?;
        let _permit = self
            .verification_slots
            .try_acquire()
            .map_err(|_| NoticeError::Busy)?;
        let _edit = self.edit(&id).await?;
        Self::require_current(check.as_ref()).await?;
        self.store
            .check_subscription_capacity(recipient_key.clone(), id.clone(), now_ms())
            .await?;
        let key_fingerprint = key.fingerprint();
        let cache_key = Sha256::digest(
            serde_json::to_vec(&(recipient_key, target.url.as_str(), &key_fingerprint))
                .map_err(|_| EventError::Invalid)?,
        )
        .into();
        self.verify(cache_key, &target, &id, &key).await?;
        // Verification is an external await; a grant revoked during the
        // challenge must not activate a subscription.
        Self::require_current(check.as_ref()).await?;
        let now = now_ms();
        let expires = now.saturating_add(ttl);
        let sealed_key = self.seal_key(&id, key.bytes())?;
        let authority_key = ulid::Ulid::new().to_string();
        let owner = EventOwnerBinding {
            recipient,
            authority_key: authority_key.clone(),
        };
        // Install evidence before activation. The DB cannot yield this key
        // to a worker until the subscription transaction commits.
        {
            let mut authorities = self
                .authorities
                .lock()
                .map_err(|_| NoticeError::Unavailable)?;
            authorities.retain(|_, entry| entry.expires > now);
            if authorities.len() >= 2 * MAX_SUBSCRIPTIONS + MAX_VERIFICATIONS {
                return Err(NoticeError::Full.into());
            }
            authorities.insert(
                authority_key.clone(),
                RegisteredAuthority {
                    expires,
                    check,
                    cancel: tokio_util::sync::CancellationToken::new(),
                },
            );
        }
        let saved = self
            .store
            .activate_event_subscription(
                owner,
                ValidatedSubscription {
                    target,
                    sealed_key,
                    key_fingerprint,
                    expires,
                },
                now,
            )
            .await;
        let saved = match saved {
            Ok(saved) => saved,
            Err(error) => {
                self.forget_authority(&authority_key);
                return Err(error.into());
            }
        };
        if let Some(old) = &saved.replaced_authority {
            self.forget_authority(old);
        }
        Ok(serde_json::json!({
            "id": saved.id,
            "refreshBefore": timestamp(saved.expires)?,
            "cursor": null,
            "truncated": false
        }))
    }

    pub(crate) async fn unsubscribe(
        &self,
        recipient: NoticeRecipient,
        params: EventsUnsubscribeParams,
    ) -> Result<serde_json::Value, EventError> {
        let target = params.validate()?;
        let _permit = self
            .verification_slots
            .try_acquire()
            .map_err(|_| NoticeError::Busy)?;
        let _edit = self.edit(&target.id(&recipient)?).await?;
        let owner = EventOwnerBinding {
            recipient,
            authority_key: String::new(),
        };
        if let Some(old) = self
            .store
            .remove_event_subscription(owner, target, now_ms())
            .await?
        {
            self.forget_authority(&old);
        }
        Ok(serde_json::json!({}))
    }

    async fn edit(&self, id: &str) -> Result<tokio::sync::OwnedMutexGuard<()>, EventError> {
        // A refresh cannot reactivate a subscription after a later
        // unsubscribe. This locks only one identity in the owning daemon;
        // the durable generation still fences worker claims.
        let gate = {
            let mut edits = self.edits.lock().map_err(|_| NoticeError::Unavailable)?;
            edits.retain(|_, gate| gate.strong_count() > 0);
            if let Some(gate) = edits.get(id).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(AsyncMutex::new(()));
                edits.insert(id.to_owned(), Arc::downgrade(&gate));
                gate
            }
        };
        Ok(gate.lock_owned().await)
    }

    async fn require_current(check: &dyn EventAuthorizer) -> Result<(), EventError> {
        match tokio::time::timeout(Duration::from_secs(2), check.check()).await {
            Ok(EventAuthorityDecision::Current) => Ok(()),
            Ok(EventAuthorityDecision::Revoked) => Err(NoticeError::Identity.into()),
            Ok(EventAuthorityDecision::RefreshRequired) | Err(_) => {
                Err(NoticeError::Unavailable.into())
            }
        }
    }

    async fn verify(
        &self,
        cache_key: [u8; 32],
        target: &SubscriptionTarget,
        id: &str,
        key: &WebhookSecret,
    ) -> Result<(), EventError> {
        let flight = {
            let mut cache = self
                .verifications
                .lock()
                .map_err(|_| NoticeError::Unavailable)?;
            cache.retain(|_, flight| {
                Arc::strong_count(flight) > 1
                    || flight
                        .try_lock()
                        .is_ok_and(|expires| expires.is_some_and(|time| time > Instant::now()))
            });
            if cache.len() >= 2 * MAX_SUBSCRIPTIONS && !cache.contains_key(&cache_key) {
                return Err(NoticeError::Full.into());
            }
            cache.entry(cache_key).or_default().clone()
        };
        let mut expires = flight.lock().await;
        if expires.is_some_and(|time| time > Instant::now()) {
            return Ok(());
        }
        self.sender.verify(&target.url, id, key).await?;
        *expires = Some(Instant::now() + VERIFICATION_TTL);
        Ok(())
    }

    fn forget_authority(&self, key: &str) {
        if let Ok(mut authorities) = self.authorities.lock()
            && let Some(entry) = authorities.remove(key)
        {
            entry.cancel.cancel();
        }
    }

    fn authority_keys(&self) -> Option<String> {
        let mut authorities = self.authorities.lock().ok()?;
        let now = now_ms();
        authorities.retain(|_, entry| entry.expires > now);
        if authorities.is_empty() {
            return None;
        }
        serde_json::to_string(&authorities.keys().collect::<Vec<_>>()).ok()
    }

    fn authority_cancellation(&self, key: &str) -> Option<tokio_util::sync::CancellationToken> {
        self.authorities
            .lock()
            .ok()?
            .get(key)
            .map(|entry| entry.cancel.clone())
    }

    async fn check_authority(&self, key: &str) -> EventAuthorityDecision {
        let proof = self.authorities.lock().ok().and_then(|map| {
            map.get(key)
                .filter(|entry| entry.expires > now_ms())
                .map(|entry| entry.check.clone())
        });
        if let Some(proof) = proof {
            tokio::time::timeout(Duration::from_secs(2), proof.check())
                .await
                .unwrap_or(EventAuthorityDecision::RefreshRequired)
        } else {
            EventAuthorityDecision::RefreshRequired
        }
    }

    fn seal_key(&self, id: &str, bytes: &[u8]) -> Result<Vec<u8>, EventError> {
        let aad = format!("labby-event-signing-key/v1/{id}");
        let (ciphertext, mut nonce) = seal_with_aad(&self.encryption, bytes, aad.as_bytes())
            .map_err(|_| NoticeError::Unavailable)?;
        nonce.extend(ciphertext);
        Ok(nonce)
    }

    fn open_key(&self, id: &str, stored: &[u8]) -> Result<Vec<u8>, EventError> {
        let (nonce, ciphertext) = stored
            .split_at_checked(12)
            .ok_or(NoticeError::Unavailable)?;
        let aad = format!("labby-event-signing-key/v1/{id}");
        open_with_aad(&self.encryption, ciphertext, nonce, aad.as_bytes())
            .map_err(|_| NoticeError::Unavailable.into())
    }
}

fn load_installation_key() -> Result<EncryptionKey, NoticeError> {
    use crate::installation::{InstallationPaths, secure_file};
    let paths = InstallationPaths::resolve().map_err(|_| NoticeError::Unavailable)?;
    let path = paths
        .root()
        .join("agent-notifications")
        .join("webhook-encryption.key");
    let encoded = match secure_file::read_private(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut bytes = Zeroizing::new([0u8; 32]);
            getrandom::fill(bytes.as_mut()).map_err(|_| NoticeError::Unavailable)?;
            let new = Zeroizing::new(STANDARD.encode(bytes.as_ref()));
            match secure_file::publish_new(&path, new.as_bytes()) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(NoticeError::Unavailable),
            }
            secure_file::read_private(&path).map_err(|_| NoticeError::Unavailable)?
        }
        Err(_) => return Err(NoticeError::Unavailable),
    };
    let encoded = Zeroizing::new(encoded);
    load_key(std::str::from_utf8(&encoded).map_err(|_| NoticeError::Unavailable)?)
        .map_err(|_| NoticeError::Unavailable)
}

#[cfg(test)]
mod tests;
