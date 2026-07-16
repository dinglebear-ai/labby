//! Aggregation for `audit.full`.
//!
//! The slim product has no built-in upstream service clients, so this module
//! intentionally aggregates only the checks that actually exist. It must not
//! advertise or silently emit an empty "all configured services" phase.

use std::sync::Arc;

use super::types::Finding;
use crate::dispatch::clients::ServiceClients;

/// Run the local portion of `audit.full`: system and auth checks.
pub async fn stream_audit_full(
    _clients: Arc<ServiceClients>,
    tx: tokio::sync::mpsc::Sender<Finding>,
) {
    // Emit system and auth checks immediately (no network I/O).
    for finding in super::system::run_system_checks() {
        if tx.send(finding).await.is_err() {
            return;
        }
    }
    for finding in super::system::run_auth_checks() {
        if tx.send(finding).await.is_err() {
            return;
        }
    }
}

pub async fn stream_audit_full_with_relay(
    clients: Arc<ServiceClients>,
    public_relay: Option<Arc<crate::oauth::public_relay::PublicRelayRegistryManager>>,
    tx: tokio::sync::mpsc::Sender<Finding>,
) {
    stream_audit_full(clients, tx.clone()).await;

    for finding in super::gateway::check_gateway_upstreams().await.findings {
        if tx.send(finding).await.is_err() {
            return;
        }
    }

    for finding in super::relay::check_public_relay(public_relay, false)
        .await
        .findings
    {
        if tx.send(finding).await.is_err() {
            return;
        }
    }
}
