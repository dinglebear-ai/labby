//! Bounded inbound MCP client observation registry — surface-neutral metadata type
//! + shared handle so the gateway dispatch layer (`labby-gateway`) can read
//! observed metadata that `labby`'s MCP transport (`rmcp`-dependent, cannot be
//! imported here) writes.
//!
//! Mirrors the existing `GatewayRuntimeHandle` pattern (a thin
//! `Arc<RwLock<...>>` swap/read handle bridging the binary crate's live
//! transport state into the extracted dispatch crate) rather than inventing a
//! new cross-crate wiring shape.
//!
//! Records show retained observations, not active transport sessions. In
//! particular, stateless HTTP requests never establish a durable connection.
//!
//! `push` is a hardened boundary against a hostile/buggy peer: entries are a
//! drop-oldest ring bounded at `MAX_CLIENTS`, and every attacker-controlled
//! string field (the peer's self-declared `clientInfo.name`/`version`) is
//! truncated to `MAX_FIELD_LEN` bytes. Without this, a client that
//! reconnects repeatedly — or one that declares an oversized `clientInfo` —
//! grows this registry unbounded for the life of the daemon. Same class of
//! bug the codebase has already been burned by once: bead lab-l9yv.6
//! (LEARNED) — "Ingest endpoints need per-batch count + per-event size caps;
//! no rate limit = DoS via authenticated node."

use std::collections::VecDeque;
use std::sync::Arc;

use tokio::sync::RwLock;

/// Drop-oldest cap on the number of tracked client entries.
const MAX_CLIENTS: usize = 500;

/// Byte cap applied to every peer-controlled string field before storage.
const MAX_FIELD_LEN: usize = 256;

fn truncate_field(mut value: String) -> String {
    if value.len() > MAX_FIELD_LEN {
        // Truncate on a char boundary so we never split a multi-byte UTF-8
        // sequence (peer-controlled string, must not panic).
        let mut end = MAX_FIELD_LEN;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    value
}

/// One bounded observation of an inbound MCP client identity. This is not a
/// persistent-session or current-connection record, especially for HTTP.
///
/// `subject_tag` is a pre-redacted display tag (see
/// `redact_subject_for_logging` in `labby`'s `mcp::context`) — this type must
/// never carry a raw auth subject. The writer (`labby`'s transport layer) is
/// responsible for redacting before constructing this record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectedClient {
    /// Redacted actor display tag, e.g. `"sub:ab12cd34ef56..."`. `None` for
    /// unauthenticated/loopback-dev sessions.
    pub subject_tag: Option<String>,
    /// Verified OAuth authorized party from the signed access-token `azp` claim.
    /// Never derived from MCP `clientInfo`.
    pub authorized_client_id: Option<String>,
    /// MCP `clientInfo.name` declared by the client. Descriptive only, not authenticated.
    pub client_name: Option<String>,
    /// MCP `clientInfo.version` declared during the initialize handshake.
    pub client_version: Option<String>,
    /// `"stdio"`, `"http"`, `"in-process"` (built-in service peers), or
    /// `"test"` — set from `LabMcpServer::transport_label` at construction.
    pub transport: String,
    /// RFC 3339 timestamp of the first retained observation. Kept under the
    /// historical wire name for compatibility; it does not prove connection.
    pub connected_at: String,
    /// Most recent observation of this identity, including later requests.
    #[serde(default)]
    pub last_seen_at: Option<String>,
    /// Number of observations folded into this row.
    #[serde(default)]
    pub observation_count: u64,
}

struct RegistryEntry {
    client: ConnectedClient,
    // Full authenticated actor key, or a transport-local connection key.
    // Never serialized. A shortened display tag must not merge users.
    scope_key: Option<String>,
}

/// Thin shared handle — cloneable, cheap, `Default` yields an empty registry
/// (the no-op case for CLI/dev paths that never wire a real transport).
#[derive(Clone, Default)]
pub struct ClientRegistryHandle {
    clients: Arc<RwLock<VecDeque<RegistryEntry>>>,
}

impl ClientRegistryHandle {
    /// Truncates every attacker-controlled string field to `MAX_FIELD_LEN`
    /// and drops the oldest entry once the registry holds `MAX_CLIENTS`.
    pub async fn push(&self, client: ConnectedClient) {
        self.observe(client, None).await;
    }

    /// Fold repeat observations only when the caller supplies a private,
    /// collision-resistant actor or connection key. Anonymous stateless HTTP
    /// calls remain separate bounded observations.
    pub async fn observe(&self, client: ConnectedClient, scope_key: Option<String>) {
        let client = ConnectedClient {
            subject_tag: client.subject_tag.map(truncate_field),
            authorized_client_id: client.authorized_client_id.map(truncate_field),
            client_name: client.client_name.map(truncate_field),
            client_version: client.client_version.map(truncate_field),
            transport: client.transport,
            last_seen_at: client
                .last_seen_at
                .or_else(|| Some(client.connected_at.clone())),
            connected_at: client.connected_at,
            observation_count: client.observation_count.max(1),
        };
        let mut guard = self.clients.write().await;
        if let Some(key) = scope_key.as_deref()
            && let Some(index) = guard.iter().position(|entry| {
                entry.scope_key.as_deref() == Some(key)
                    && entry.client.client_name == client.client_name
                    && entry.client.client_version == client.client_version
                    && entry.client.transport == client.transport
                    && entry.client.authorized_client_id == client.authorized_client_id
            })
        {
            let mut entry = guard.remove(index).expect("position came from registry");
            entry.client.last_seen_at = Some(client.connected_at);
            entry.client.observation_count = entry.client.observation_count.saturating_add(1);
            guard.push_back(entry);
            return;
        }
        if guard.len() >= MAX_CLIENTS {
            guard.pop_front();
        }
        guard.push_back(RegistryEntry { client, scope_key });
    }

    /// Return a snapshot of currently retained client records in connection order.
    pub async fn list(&self) -> Vec<ConnectedClient> {
        self.clients
            .read()
            .await
            .iter()
            .map(|entry| entry.client.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> ConnectedClient {
        ConnectedClient {
            subject_tag: Some("sub:deadbeef".to_string()),
            authorized_client_id: Some("oauth-client".to_string()),
            client_name: Some(name.to_string()),
            client_version: Some("1.0.0".to_string()),
            transport: "mcp".to_string(),
            connected_at: "2026-01-01T00:00:00Z".to_string(),
            last_seen_at: None,
            observation_count: 1,
        }
    }

    #[tokio::test]
    async fn default_handle_starts_empty() {
        let handle = ClientRegistryHandle::default();
        assert!(handle.list().await.is_empty());
    }

    #[tokio::test]
    async fn push_then_list_round_trips_in_order() {
        let handle = ClientRegistryHandle::default();
        handle.push(sample("claude-code")).await;
        handle.push(sample("claude-desktop")).await;

        let clients = handle.list().await;
        assert_eq!(clients.len(), 2);
        assert_eq!(clients[0].client_name.as_deref(), Some("claude-code"));
        assert_eq!(clients[1].client_name.as_deref(), Some("claude-desktop"));
    }

    #[tokio::test]
    async fn cloned_handle_shares_the_same_underlying_registry() {
        let handle = ClientRegistryHandle::default();
        let cloned = handle.clone();
        cloned.push(sample("codex")).await;

        assert_eq!(handle.list().await.len(), 1);
    }

    #[tokio::test]
    async fn push_drops_the_oldest_entry_once_at_capacity() {
        let handle = ClientRegistryHandle::default();
        for i in 0..MAX_CLIENTS {
            handle.push(sample(&format!("client-{i}"))).await;
        }
        // One more push past the cap must evict the very first entry rather
        // than growing unbounded or silently dropping the new one.
        handle.push(sample("client-overflow")).await;

        let clients = handle.list().await;
        assert_eq!(clients.len(), MAX_CLIENTS);
        assert_eq!(clients[0].client_name.as_deref(), Some("client-1"));
        assert_eq!(
            clients[MAX_CLIENTS - 1].client_name.as_deref(),
            Some("client-overflow")
        );
    }

    #[tokio::test]
    async fn push_truncates_oversized_peer_controlled_fields() {
        let handle = ClientRegistryHandle::default();
        let mut oversized = sample("victim");
        oversized.client_name = Some("x".repeat(MAX_FIELD_LEN * 4));
        oversized.subject_tag = Some("y".repeat(MAX_FIELD_LEN * 4));

        handle.push(oversized).await;

        let clients = handle.list().await;
        assert_eq!(
            clients[0].client_name.as_ref().unwrap().len(),
            MAX_FIELD_LEN
        );
        assert_eq!(
            clients[0].subject_tag.as_ref().unwrap().len(),
            MAX_FIELD_LEN
        );
    }

    #[tokio::test]
    async fn push_truncation_never_splits_a_multibyte_char() {
        let handle = ClientRegistryHandle::default();
        let mut oversized = sample("victim");
        // 3-byte UTF-8 char repeated past the cap so a naive byte-index
        // truncate would land mid-character and panic.
        oversized.client_name = Some("€".repeat(MAX_FIELD_LEN));

        handle.push(oversized).await;

        let clients = handle.list().await;
        let name = clients[0].client_name.as_ref().unwrap();
        assert!(name.len() <= MAX_FIELD_LEN);
        assert!(std::str::from_utf8(name.as_bytes()).is_ok());
    }

    #[tokio::test]
    async fn observations_update_last_seen_without_merging_distinct_actors() {
        let handle = ClientRegistryHandle::default();
        let first = sample("same-client");
        let mut later = first.clone();
        later.connected_at = "2026-01-01T00:00:10Z".to_string();
        handle
            .observe(first.clone(), Some("full-actor-key-a".into()))
            .await;
        handle
            .observe(later.clone(), Some("full-actor-key-a".into()))
            .await;
        handle.observe(later, Some("full-actor-key-b".into())).await;

        let clients = handle.list().await;
        assert_eq!(clients.len(), 2);
        assert_eq!(clients[0].observation_count, 2);
        assert_eq!(
            clients[0].last_seen_at.as_deref(),
            Some("2026-01-01T00:00:10Z")
        );
        assert_eq!(clients[1].observation_count, 1);
        assert_eq!(clients[0].subject_tag, clients[1].subject_tag);
    }

    #[tokio::test]
    async fn anonymous_observations_remain_bounded_and_unmerged() {
        let handle = ClientRegistryHandle::default();
        for _ in 0..=MAX_CLIENTS {
            handle.observe(sample("anonymous"), None).await;
        }
        assert_eq!(handle.list().await.len(), MAX_CLIENTS);
    }
}
