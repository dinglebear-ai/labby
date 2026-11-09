use std::{
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use rmcp::{RoleServer, Service, service::RunningService};
use tokio::sync::Mutex;

use crate::{
    dispatch::{
        agent_messages::{
            SessionBinding, SessionKind,
            proof::{BRIDGE_PROOF_MS, BridgeProof, MessageKey},
        },
        codemode_notices::{NoticeConsumer, NoticeError, NoticeRecipient, NoticeStore, now_ms},
    },
    live_gateway::LiveGateway,
};

use super::super::claude_channel::ChannelSession;

pub(crate) struct LocalMessageSession {
    pub(in crate::mcp) store: NoticeStore,
    binding: SessionBinding,
    bridge: Option<BridgeAuthority>,
}

struct BridgeAuthority {
    gateway: LiveGateway,
    key: MessageKey,
    installation: String,
    daemon_epoch: String,
    cached: std::sync::Mutex<Option<CachedProof>>,
    refresh: Mutex<()>,
}

struct CachedProof {
    signed_expiry: i64,
    deadline: Instant,
}

impl CachedProof {
    fn new(signed_expiry: i64) -> Self {
        let remaining = signed_expiry
            .saturating_sub(now_ms())
            .clamp(0, BRIDGE_PROOF_MS);
        Self {
            signed_expiry,
            deadline: Instant::now() + Duration::from_millis(remaining as u64),
        }
    }

    fn is_current(&self) -> bool {
        self.signed_expiry > now_ms() && self.deadline > Instant::now()
    }
}

impl LocalMessageSession {
    pub(crate) fn standalone(
        store: NoticeStore,
        key: &MessageKey,
        installation: &str,
        recipient: NoticeRecipient,
    ) -> Result<Arc<Self>, NoticeError> {
        let owner = key.digest("local-owner", &[installation.as_bytes()]);
        Self::new(store, key, recipient, owner, None)
    }

    pub(crate) async fn bridge(
        store: NoticeStore,
        key: MessageKey,
        installation: String,
        recipient: NoticeRecipient,
        gateway: LiveGateway,
    ) -> Result<Arc<Self>, NoticeError> {
        let BridgeProof {
            owner_key,
            daemon_epoch,
            expires_at,
            ..
        } = gateway.prove_message_bridge(&key, &installation).await?;
        let bridge = BridgeAuthority {
            gateway,
            key: key.clone(),
            installation,
            daemon_epoch,
            cached: std::sync::Mutex::new(Some(CachedProof::new(expires_at))),
            refresh: Mutex::new(()),
        };
        Self::new(store, &key, recipient, owner_key, Some(bridge))
    }

    fn new(
        store: NoticeStore,
        key: &MessageKey,
        recipient: NoticeRecipient,
        owner_key: String,
        bridge: Option<BridgeAuthority>,
    ) -> Result<Arc<Self>, NoticeError> {
        if !store.is_enabled() {
            return Err(NoticeError::Unavailable);
        }
        let NoticeConsumer::Stdio { session } = &recipient.consumer else {
            return Err(NoticeError::Identity);
        };
        let binding_key = key.digest("stdio-session", &[owner_key.as_bytes(), session.as_bytes()]);
        Ok(Arc::new(Self {
            store,
            binding: SessionBinding {
                owner_key,
                binding_key,
                recipient,
                kind: SessionKind::Stdio,
                stdio_closed: Some(Arc::new(AtomicBool::new(false))),
            },
            bridge,
        }))
    }

    pub(in crate::mcp) fn visible(&self) -> bool {
        !self.binding.is_closed()
    }

    pub(in crate::mcp) async fn authorize(&self) -> Result<SessionBinding, NoticeError> {
        self.require_current(false).await?;
        Ok(self.binding.clone())
    }

    pub(in crate::mcp) fn can_send_reference(&self, recipient: &NoticeRecipient) -> bool {
        &self.binding.recipient == recipient && self.has_current_proof()
    }

    pub(in crate::mcp) async fn can_offer_reference(&self, recipient: &NoticeRecipient) -> bool {
        if &self.binding.recipient != recipient || !self.visible() {
            return false;
        }
        if self.has_current_proof() {
            return true;
        }
        self.store.message_reference_due(recipient).await == Ok(true)
            && self.require_current(true).await.is_ok()
    }

    fn has_current_proof(&self) -> bool {
        self.visible()
            && self.bridge.as_ref().is_none_or(|bridge| {
                bridge
                    .cached
                    .lock()
                    .is_ok_and(|proof| proof.as_ref().is_some_and(CachedProof::is_current))
            })
    }

    async fn require_current(&self, allow_cached: bool) -> Result<(), NoticeError> {
        if !self.visible() {
            return Err(NoticeError::Identity);
        }
        let Some(bridge) = &self.bridge else {
            return Ok(());
        };
        if allow_cached && self.has_current_proof() {
            return Ok(());
        }
        // Bound proof traffic and serialize definitive revocation with refreshes.
        let _refresh = bridge.refresh.try_lock().map_err(|_| NoticeError::Busy)?;
        if !self.visible() {
            return Err(NoticeError::Identity);
        }
        let error = match bridge
            .gateway
            .prove_message_bridge(&bridge.key, &bridge.installation)
            .await
        {
            Ok(proof)
                if proof.owner_key == self.binding.owner_key
                    && proof.daemon_epoch == bridge.daemon_epoch =>
            {
                *bridge.cached.lock().map_err(|_| NoticeError::Unavailable)? =
                    Some(CachedProof::new(proof.expires_at));
                return Ok(());
            }
            Ok(_) | Err(NoticeError::Identity) => NoticeError::Identity,
            Err(error) => error,
        };
        if let Ok(mut cached) = bridge.cached.lock() {
            *cached = None;
        }
        if error == NoticeError::Identity {
            self.close().await;
        }
        Err(error)
    }

    async fn close(&self) {
        self.binding.close();
        // Keep the in-process close fence even if the durable store is busy.
        // The last lease will expire without further renewals.
        if let Err(error) = self
            .store
            .renew_message_session(self.binding.clone(), true)
            .await
        {
            tracing::warn!(
                kind = error.kind(),
                "message session close could not update the durable lease"
            );
        }
    }

    async fn keep_present(&self) -> std::convert::Infallible {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if !self.visible() {
                return std::future::pending().await;
            }
            if self.require_current(true).await.is_ok() {
                let _ = self
                    .store
                    .renew_message_session(self.binding.clone(), false)
                    .await;
            }
        }
    }

    pub(crate) async fn wait<S: Service<RoleServer>>(
        &self,
        running: RunningService<RoleServer, S>,
        channel: Option<&ChannelSession>,
    ) -> Result<(), tokio::task::JoinError> {
        let peer = running.peer().clone();
        let channel_push = async {
            match channel {
                Some(channel) => channel.run(peer, Some(self)).await,
                None => std::future::pending().await,
            }
        };
        let result = tokio::select! {
            result = running.waiting() => result.map(|_| ()),
            never = self.keep_present() => match never {},
            never = channel_push => match never {},
        };
        self.close().await;
        result
    }
}
