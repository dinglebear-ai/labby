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
use labby_gateway::upstream::pool::{ResourceCatalogGeneration, ToolCatalogGeneration};
use rmcp::model::{Prompt, Resource, ResourceTemplate};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

const MAX_CATALOG_SNAPSHOTS_PER_KIND: usize = 8;

#[cfg(feature = "gateway")]
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ProjectShadowSnapshotKey {
    pub(crate) credential_instance_fingerprint: String,
    pub(crate) credential_binding_fingerprint: String,
    pub(crate) route_binding_fingerprint: String,
    pub(crate) access_global_revision: u64,
    pub(crate) runtime: GatewayRuntimeConfigGeneration,
    pub(crate) pool: PoolPublicationGeneration,
    pub(crate) tools: ToolCatalogGeneration,
    pub(crate) resources: ResourceCatalogGeneration,
    pub(crate) services: ServiceRegistryPublicationGeneration,
}

#[cfg(not(feature = "gateway"))]
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ProjectShadowSnapshotKey;

struct CatalogSnapshot<T> {
    audience: String,
    revision: String,
    items: Arc<[T]>,
    resource_provenance: Arc<[ResourceProvenance]>,
    project_shadow_key: Option<ProjectShadowSnapshotKey>,
}

#[derive(Clone)]
pub(crate) struct ResourceProvenance {
    pub(crate) upstream: String,
    pub(crate) native_uri: String,
}

struct CatalogSnapshotStore<T> {
    entries: VecDeque<CatalogSnapshot<T>>,
}

impl<T> Default for CatalogSnapshotStore<T> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }
}

impl<T> CatalogSnapshotStore<T> {
    fn get(&self, audience: &str, revision: &str) -> Option<Arc<[T]>> {
        self.entries
            .iter()
            .rev()
            .find(|snapshot| snapshot.audience == audience && snapshot.revision == revision)
            .map(|snapshot| Arc::clone(&snapshot.items))
    }

    fn insert(&mut self, audience: String, revision: String, items: Arc<[T]>) {
        self.insert_with_provenance(audience, revision, items, Arc::from([]), None);
    }

    fn insert_with_provenance(
        &mut self,
        audience: String,
        revision: String,
        items: Arc<[T]>,
        resource_provenance: Arc<[ResourceProvenance]>,
        project_shadow_key: Option<ProjectShadowSnapshotKey>,
    ) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|snapshot| snapshot.audience == audience && snapshot.revision == revision)
        {
            self.entries.remove(index);
        }
        self.entries.push_back(CatalogSnapshot {
            audience,
            revision,
            items,
            resource_provenance,
            project_shadow_key,
        });
        while self.entries.len() > MAX_CATALOG_SNAPSHOTS_PER_KIND {
            self.entries.pop_front();
        }
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
    resources: RwLock<CatalogSnapshotStore<Resource>>,
    resource_templates: RwLock<CatalogSnapshotStore<ResourceTemplate>>,
    prompts: RwLock<CatalogSnapshotStore<Prompt>>,
}

impl McpRouteRuntime {
    pub(crate) async fn resource_snapshot(
        &self,
        audience: &str,
        revision: &str,
    ) -> Option<(
        Arc<[Resource]>,
        Arc<[ResourceProvenance]>,
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
                    Arc::clone(&snapshot.resource_provenance),
                    snapshot.project_shadow_key.clone(),
                )
            })
    }

    pub(crate) async fn store_resource_snapshot(
        &self,
        audience: String,
        revision: String,
        resources: Arc<[Resource]>,
        resource_provenance: Arc<[ResourceProvenance]>,
        project_shadow_key: Option<ProjectShadowSnapshotKey>,
    ) {
        self.resources.write().await.insert_with_provenance(
            audience,
            revision,
            resources,
            resource_provenance,
            project_shadow_key,
        );
    }

    pub(crate) async fn resource_template_snapshot(
        &self,
        audience: &str,
        revision: &str,
    ) -> Option<Arc<[ResourceTemplate]>> {
        self.resource_templates.read().await.get(audience, revision)
    }

    pub(crate) async fn store_resource_template_snapshot(
        &self,
        audience: String,
        revision: String,
        templates: Arc<[ResourceTemplate]>,
    ) {
        self.resource_templates
            .write()
            .await
            .insert(audience, revision, templates);
    }

    pub(crate) async fn prompt_snapshot(
        &self,
        audience: &str,
        revision: &str,
    ) -> Option<Arc<[Prompt]>> {
        self.prompts.read().await.get(audience, revision)
    }

    pub(crate) async fn store_prompt_snapshot(
        &self,
        audience: String,
        revision: String,
        prompts: Arc<[Prompt]>,
    ) {
        self.prompts
            .write()
            .await
            .insert(audience, revision, prompts);
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
    async fn snapshots_are_isolated_by_audience_and_kind() {
        let runtime = McpRouteRuntime::default();
        runtime
            .store_resource_snapshot(
                "alice".to_string(),
                "r1".to_string(),
                Arc::from(vec![Resource::new("file:///one", "one")]),
                Arc::from([]),
                None,
            )
            .await;
        runtime
            .store_prompt_snapshot(
                "alice".to_string(),
                "r1".to_string(),
                Arc::from(vec![Prompt::new("prompt-one", None::<String>, None)]),
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
        assert_eq!(
            runtime
                .prompt_snapshot("alice", "r1")
                .await
                .expect("prompt snapshot")[0]
                .name,
            "prompt-one"
        );
    }
}
