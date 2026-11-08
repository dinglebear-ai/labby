use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use rmcp::{RoleServer, Service, service::RunningService};
use tokio::sync::Mutex;

use crate::{
    dispatch::{
        agent_messages::{
            SessionBinding, SessionKind,
            proof::{BridgeProof, MessageKey},
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
    expires_at: std::sync::atomic::AtomicI64,
    refresh: Mutex<()>,
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
            expires_at: expires_at.into(),
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

    async fn require_current(&self, allow_cached: bool) -> Result<(), NoticeError> {
        if !self.visible() {
            return Err(NoticeError::Identity);
        }
        let Some(bridge) = &self.bridge else {
            return Ok(());
        };
        if allow_cached && bridge.expires_at.load(Ordering::Acquire) > now_ms() {
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
                bridge.expires_at.store(proof.expires_at, Ordering::Release);
                return Ok(());
            }
            Ok(_) | Err(NoticeError::Identity) => NoticeError::Identity,
            Err(error) => error,
        };
        bridge.expires_at.store(0, Ordering::Release);
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
                Some(channel) => channel.run(peer).await,
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
