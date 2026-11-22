//! Shared state for stateless MCP request handlers.
//!
//! Streamable HTTP creates a fresh `LabMcpServer` for every POST. Anything
//! needed to continue a protocol operation across POSTs belongs here rather
//! than on the request handler itself.

use std::collections::VecDeque;
use std::sync::Arc;

#[cfg(feature = "gateway")]
use labby_gateway::gateway::ServiceRegistryPublicationGeneration;
#[cfg(feature = "gateway")]
use labby_gateway::gateway::manager::{GatewayRuntimeConfigGeneration, PoolPublicationGeneration};
#[cfg(feature = "gateway")]
use labby_gateway::upstream::pool::{
    PromptCatalogGeneration, ResourceCatalogGeneration, ResourceTemplateCatalogGeneration,
    ToolCatalogGeneration,
};
use rmcp::model::{Prompt, Resource, ResourceTemplate};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

const MAX_CATALOG_SNAPSHOTS_PER_KIND: usize = 64;
const MAX_CATALOG_SNAPSHOTS_PER_AUDIENCE: usize = 8;

#[cfg(feature = "gateway")]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectShadowSnapshotKey {
    pub(crate) credential_instance_fingerprint: String,
    pub(crate) credential_binding_fingerprint: String,
    pub(crate) route_binding_fingerprint: String,
    pub(crate) access_global_revision: u64,
    pub(crate) runtime: GatewayRuntimeConfigGeneration,
    pub(crate) pool: PoolPublicationGeneration,
    pub(crate) tools: ToolCatalogGeneration,
    pub(crate) resources: ResourceCatalogGeneration,
    pub(crate) resource_templates: ResourceTemplateCatalogGeneration,
    pub(crate) prompts: PromptCatalogGeneration,
    pub(crate) services: ServiceRegistryPublicationGeneration,
}

#[cfg(feature = "gateway")]
impl ProjectShadowSnapshotKey {
    pub(crate) fn tools_cursor_fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"labby.mcp.project-tools-cursor.v1\0");
        for value in [
            self.credential_instance_fingerprint.as_bytes(),
            self.credential_binding_fingerprint.as_bytes(),
            self.route_binding_fingerprint.as_bytes(),
        ] {
            hasher.update(value.len().to_be_bytes());
            hasher.update(value);
        }
        hasher.update(self.access_global_revision.to_be_bytes());
        hasher.update(self.runtime.fingerprint_bytes());
        hasher.update(self.pool.fingerprint_bytes());
        hasher.update(self.tools.fingerprint_bytes());
        hasher.update(self.resources.fingerprint_bytes());
        hasher.update(self.resource_templates.fingerprint_bytes());
        hasher.update(self.prompts.fingerprint_bytes());
        hasher.update(self.services.fingerprint_bytes());
        hex::encode(hasher.finalize())
    }
}

#[cfg(not(feature = "gateway"))]
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ProjectShadowSnapshotKey;

#[derive(Clone, Default)]
pub(crate) struct ResourceSnapshotDiagnostics {
    pub(crate) checked: usize,
    pub(crate) would_suppress: usize,
    pub(crate) owned_app_uris: Arc<[String]>,
}

struct ToolSnapshotMetadata {
    peer: super::peer_contract::PeerContract,
    contract: super::catalog::ToolCatalogSnapshot,
    source_revision: String,
}

struct CatalogSnapshot<T, M = ()> {
    metadata: M,
    audience: String,
    revision: String,
    items: Arc<[T]>,
    diagnostics: ResourceSnapshotDiagnostics,
    prompt_provenance: Arc<[PromptProvenance]>,
    project_shadow_key: Option<ProjectShadowSnapshotKey>,
}

#[cfg(any(feature = "gateway", test))]
#[derive(Clone)]
pub(crate) struct ResourceProvenance {
    pub(crate) upstream: String,
    pub(crate) native_uri: String,
}

#[cfg(any(feature = "gateway", test))]
#[derive(Clone)]
pub(crate) struct ResourceTemplateProvenance {
    pub(crate) upstream: String,
    pub(crate) native_uri_template: String,
}

#[derive(Clone)]
pub(crate) struct PromptProvenance {
    pub(crate) upstream: String,
    pub(crate) native_name: String,
}

struct CatalogSnapshotStore<T, M = ()> {
    entries: VecDeque<CatalogSnapshot<T, M>>,
}

impl<T, M> Default for CatalogSnapshotStore<T, M> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }
}

impl<T, M> CatalogSnapshotStore<T, M> {
    fn insert(&mut self, entry: CatalogSnapshot<T, M>) {
        self.entries
            .retain(|old| old.audience != entry.audience || old.revision != entry.revision);
        self.entries.push_back(entry);
        self.retain_capacity();
    }

    fn retain_capacity(&mut self) {
        let Some(newest) = self.entries.back() else {
            return;
        };
        let audience = &newest.audience;
        if self
            .entries
            .iter()
            .filter(|entry| &entry.audience == audience)
            .count()
            > MAX_CATALOG_SNAPSHOTS_PER_AUDIENCE
            && let Some(oldest) = self
                .entries
                .iter()
                .position(|entry| &entry.audience == audience)
        {
            self.entries.remove(oldest);
        }
        if self.entries.len() > MAX_CATALOG_SNAPSHOTS_PER_KIND {
            self.entries.pop_front();
        }
    }
}

impl<T> CatalogSnapshotStore<T> {
    fn insert_with_resource_diagnostics(
        &mut self,
        audience: String,
        revision: String,
        items: Arc<[T]>,
        diagnostics: ResourceSnapshotDiagnostics,
        project_shadow_key: Option<ProjectShadowSnapshotKey>,
    ) {
        self.insert(CatalogSnapshot {
            metadata: (),
            audience,
            revision,
            items,
            diagnostics,
            prompt_provenance: Arc::from([]),
            project_shadow_key,
        });
    }

    fn insert_with_prompt_provenance(
        &mut self,
        audience: String,
        revision: String,
        items: Arc<[T]>,
        prompt_provenance: Arc<[PromptProvenance]>,
        project_shadow_key: Option<ProjectShadowSnapshotKey>,
    ) {
        self.insert(CatalogSnapshot {
            metadata: (),
            audience,
            revision,
            items,
            diagnostics: ResourceSnapshotDiagnostics::default(),
            prompt_provenance,
            project_shadow_key,
        });
    }
}

/// Long-lived state shared by every request handler mounted on one MCP route.
///
/// The route boundary matters: protected subset routes can expose a different
/// catalog than the root route. Authentication is an additional key inside the
/// snapshot store so callers with different effective scopes never share a
/// paginated result set.
#[derive(Default)]
pub(crate) struct McpRouteRuntime {
    /// Durable agent inbox shared with the authenticated publication API.
    #[cfg(feature = "gateway")]
    pub(crate) code_mode_notifications: crate::dispatch::codemode_notices::NoticeStore,
    #[cfg(feature = "gateway")]
    notification_stdio_session: std::sync::OnceLock<String>,
    #[cfg(feature = "gateway")]
    pub(crate) agent_events: Option<Arc<crate::dispatch::agent_events::AgentEvents>>,
    #[cfg(feature = "gateway")]
    pub(crate) http_messages:
        Option<Arc<crate::dispatch::agent_messages::http::HttpMessageRuntime>>,
    #[cfg(feature = "gateway")]
    pub(crate) local_messages: Option<Arc<super::session_messages::LocalMessageSession>>,
    #[cfg(feature = "gateway")]
    pub(crate) event_credential_adapter: Option<Arc<crate::access::AccessCredentialAdapter>>,
    #[cfg(feature = "gateway")]
    pub(crate) claude_channel: Option<Arc<super::claude_channel::ChannelSession>>,
    tools: RwLock<CatalogSnapshotStore<rmcp::model::Tool, ToolSnapshotMetadata>>,
    resources: RwLock<CatalogSnapshotStore<Resource>>,
    resource_templates: RwLock<CatalogSnapshotStore<ResourceTemplate>>,
    prompts: RwLock<CatalogSnapshotStore<Prompt>>,
    depot: std::sync::OnceLock<Arc<crate::dispatch::depot::DepotClient>>,
}

impl McpRouteRuntime {
    #[cfg(feature = "gateway")]
    pub(crate) fn with_notification_store(
        store: crate::dispatch::codemode_notices::NoticeStore,
    ) -> Self {
        Self {
            code_mode_notifications: store,
            ..Self::default()
        }
    }

    #[cfg(feature = "gateway")]
    pub(crate) fn notification_stdio_session(&self) -> &str {
        self.notification_stdio_session
            .get_or_init(|| ulid::Ulid::new().to_string())
    }

    pub(crate) fn configure_depot(&self, depot: Arc<crate::dispatch::depot::DepotClient>) {
        drop(self.depot.set(depot));
    }

    pub(crate) fn depot(&self) -> Option<&Arc<crate::dispatch::depot::DepotClient>> {
        self.depot.get()
    }
    pub(crate) async fn tool_snapshot(
        &self,
        audience: &str,
        revision: &str,
        sources: &str,
        peer: &super::peer_contract::PeerContract,
    ) -> Option<(
        Arc<[rmcp::model::Tool]>,
        super::catalog::ToolCatalogSnapshot,
    )> {
        self.tools
            .read()
            .await
            .entries
            .iter()
            .rev()
            .find(|snapshot| {
                snapshot.audience == audience
                    && snapshot.revision == revision
                    && snapshot.metadata.source_revision == sources
                    && snapshot.metadata.peer.same_inputs(peer)
            })
            .map(|snapshot| {
                (
                    Arc::clone(&snapshot.items),
                    snapshot.metadata.contract.clone(),
                )
            })
    }

    pub(crate) async fn store_tool_snapshot(
        &self,
        audience: String,
        revision: String,
        sources: String,
        tools: Vec<rmcp::model::Tool>,
        peer: super::peer_contract::PeerContract,
        contract: super::catalog::ToolCatalogSnapshot,
    ) {
        self.tools.write().await.insert(CatalogSnapshot {
            audience,
            revision,
            items: tools.into(),
            metadata: ToolSnapshotMetadata {
                peer,
                contract,
                source_revision: sources,
            },
            diagnostics: Default::default(),
            prompt_provenance: Arc::from([]),
            project_shadow_key: None,
        });
    }

    pub(crate) async fn resource_snapshot(
        &self,
        audience: &str,
        revision: &str,
    ) -> Option<(
        Arc<[Resource]>,
        ResourceSnapshotDiagnostics,
        Option<ProjectShadowSnapshotKey>,
    )> {
        let resources = self.resources.read().await;
        resources
            .entries
            .iter()
            .rev()
            .find(|snapshot| snapshot.audience == audience && snapshot.revision == revision)
            .map(|snapshot| {
                (
                    Arc::clone(&snapshot.items),
                    snapshot.diagnostics.clone(),
                    snapshot.project_shadow_key.clone(),
                )
            })
    }

    pub(crate) async fn store_resource_snapshot(
        &self,
        audience: String,
        revision: String,
        resources: Arc<[Resource]>,
        diagnostics: ResourceSnapshotDiagnostics,
        project_shadow_key: Option<ProjectShadowSnapshotKey>,
    ) {
        self.resources
            .write()
            .await
            .insert_with_resource_diagnostics(
                audience,
                revision,
                resources,
                diagnostics,
                project_shadow_key,
            );
    }

    pub(crate) async fn resource_template_snapshot(
        &self,
        audience: &str,
        revision: &str,
    ) -> Option<(
        Arc<[ResourceTemplate]>,
        ResourceSnapshotDiagnostics,
        Option<ProjectShadowSnapshotKey>,
    )> {
        let templates = self.resource_templates.read().await;
        templates
            .entries
            .iter()
            .rev()
            .find(|snapshot| snapshot.audience == audience && snapshot.revision == revision)
            .map(|snapshot| {
                (
                    Arc::clone(&snapshot.items),
                    snapshot.diagnostics.clone(),
                    snapshot.project_shadow_key.clone(),
                )
            })
    }

    pub(crate) async fn store_resource_template_snapshot(
        &self,
        audience: String,
        revision: String,
        templates: Arc<[ResourceTemplate]>,
        provenance: ResourceSnapshotDiagnostics,
        project_shadow_key: Option<ProjectShadowSnapshotKey>,
    ) {
        self.resource_templates
            .write()
            .await
            .insert_with_resource_diagnostics(
                audience,
                revision,
                templates,
                provenance,
                project_shadow_key,
            );
    }

    pub(crate) async fn prompt_snapshot(
        &self,
        audience: &str,
        revision: &str,
    ) -> Option<(
        Arc<[Prompt]>,
        Arc<[PromptProvenance]>,
        Option<ProjectShadowSnapshotKey>,
    )> {
        let prompts = self.prompts.read().await;
        prompts
            .entries
            .iter()
            .rev()
            .find(|snapshot| snapshot.audience == audience && snapshot.revision == revision)
            .map(|snapshot| {
                (
                    Arc::clone(&snapshot.items),
                    Arc::clone(&snapshot.prompt_provenance),
                    snapshot.project_shadow_key.clone(),
                )
            })
    }

    pub(crate) async fn store_prompt_snapshot(
        &self,
        audience: String,
        revision: String,
        prompts: Arc<[Prompt]>,
        provenance: Arc<[PromptProvenance]>,
        project_shadow_key: Option<ProjectShadowSnapshotKey>,
    ) {
        self.prompts.write().await.insert_with_prompt_provenance(
            audience,
            revision,
            prompts,
            provenance,
            project_shadow_key,
        );
    }
}

/// Stable in-memory authorization key for one visible catalog audience.
///
/// The value never leaves the process. Hashing keeps raw subjects and issuers
/// out of cache keys while scopes ensure privilege changes cannot reuse a
/// snapshot produced under a different authorization context.
pub(crate) fn catalog_snapshot_audience(
    auth: Option<&labby_auth::auth_context::AuthContext>,
) -> String {
    let Some(auth) = auth else {
        return "trusted-local".to_string();
    };

    let mut scopes = auth.scopes.clone();
    scopes.sort_unstable();
    scopes.dedup();

    let mut hasher = Sha256::new();
    hasher.update(auth.issuer.as_bytes());
    hasher.update([0]);
    hasher.update(auth.sub.as_bytes());
    hasher.update([u8::from(auth.via_session)]);
    for scope in scopes {
        hasher.update([0]);
        hasher.update(scope.as_bytes());
    }
    format!("auth:{}", hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_audiences_revision_churn_preserves_other_clients_cursors() {
        let runtime = McpRouteRuntime::default();
        let items: Arc<[Resource]> = Arc::from([Resource::new("file:///one", "one")]);
        for client in 0..16 {
            runtime
                .store_resource_snapshot(
                    format!("client-{client}"),
                    "initial".into(),
                    items.clone(),
                    Default::default(),
                    None,
                )
                .await;
        }
        for revision in 0..16 {
            runtime
                .store_resource_snapshot(
                    "client-0".into(),
                    revision.to_string(),
                    items.clone(),
                    Default::default(),
                    None,
                )
                .await;
        }
        assert!(
            runtime
                .resource_snapshot("client-0", "initial")
                .await
                .is_none()
        );
        for client in 1..16 {
            assert!(
                runtime
                    .resource_snapshot(&format!("client-{client}"), "initial")
                    .await
                    .is_some()
            );
        }
    }

    #[tokio::test]
    async fn snapshots_are_isolated_by_audience_and_kind() {
        let runtime = McpRouteRuntime::default();
        runtime
            .store_resource_snapshot(
                "alice".to_string(),
                "r1".to_string(),
                Arc::from(vec![Resource::new("file:///one", "one")]),
                ResourceSnapshotDiagnostics::default(),
                None,
            )
            .await;
        runtime
            .store_prompt_snapshot(
                "alice".to_string(),
                "r1".to_string(),
                Arc::from(vec![Prompt::new("prompt-one", None::<String>, None)]),
                Arc::from([]),
                None,
            )
            .await;

        assert!(runtime.resource_snapshot("bob", "r1").await.is_none());
        assert_eq!(
            runtime
                .resource_snapshot("alice", "r1")
                .await
                .expect("resource snapshot")
                .0[0]
                .uri,
            "file:///one"
        );
        let (prompts, provenance, key) = runtime
            .prompt_snapshot("alice", "r1")
            .await
            .expect("prompt snapshot");
        assert_eq!(prompts[0].name, "prompt-one");
        assert!(provenance.is_empty());
        assert!(key.is_none());
    }
}
