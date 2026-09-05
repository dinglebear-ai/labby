//! Aggregation for `audit.full`.
//!
//! The slim product has no built-in upstream service clients, so this module
//! intentionally aggregates only the checks that actually exist. It must not
//! advertise or silently emit an empty "all configured services" phase.

use std::sync::Arc;

use super::types::Finding;
use crate::dispatch::clients::ServiceClients;

/// Run the local portion of `audit.full`: system, auth, and access-store checks.
pub async fn stream_audit_full(
    _clients: Arc<ServiceClients>,
    tx: tokio::sync::mpsc::Sender<Finding>,
) {
    send_local_findings(&tx, super::system::run_auth_checks()).await;
}

pub async fn stream_audit_full_with_relay(
    _clients: Arc<ServiceClients>,
    public_relay: Option<Arc<crate::oauth::public_relay::PublicRelayRegistryManager>>,
    tx: tokio::sync::mpsc::Sender<Finding>,
) {
    if send_local_findings(&tx, super::system::run_auth_checks()).await {
        send_remote_findings(&tx, public_relay).await;
    }
}

/// Run the full audit using the caller's already-resolved auth configuration.
pub async fn stream_audit_full_with_relay_and_auth(
    _clients: Arc<ServiceClients>,
    public_relay: Option<Arc<crate::oauth::public_relay::PublicRelayRegistryManager>>,
    auth: Option<labby_auth::config::AuthConfig>,
    tx: tokio::sync::mpsc::Sender<Finding>,
) {
    if send_local_findings(
        &tx,
        super::system::run_auth_checks_with_config(auth.as_ref()),
    )
    .await
    {
        send_remote_findings(&tx, public_relay).await;
    }
}

async fn send_local_findings(
    tx: &tokio::sync::mpsc::Sender<Finding>,
    auth_findings: Vec<Finding>,
) -> bool {
    if !send_findings(tx, super::system::run_system_checks().await).await
        || !send_findings(tx, auth_findings).await
    {
        return false;
    }
    send_findings(tx, super::access::check_access_store().await.findings).await
}

async fn send_remote_findings(
    tx: &tokio::sync::mpsc::Sender<Finding>,
    public_relay: Option<Arc<crate::oauth::public_relay::PublicRelayRegistryManager>>,
) {
    if !send_findings(tx, super::gateway::check_gateway_upstreams().await.findings).await {
        return;
    }
    send_findings(
        tx,
        super::relay::check_public_relay(public_relay, false)
            .await
            .findings,
    )
    .await;
}

async fn send_findings(
    tx: &tokio::sync::mpsc::Sender<Finding>,
    findings: impl IntoIterator<Item = Finding>,
) -> bool {
    for finding in findings {
        if tx.send(finding).await.is_err() {
            return false;
        }
    }
    true
}
