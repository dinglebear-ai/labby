//! Immutable publication of the pool's routable tool projection.
//!
//! The snapshot deliberately excludes prompts, resources, skills, connections,
//! OAuth subjects, and capability health other than tool routability. Those
//! concerns have separate lifecycles and must not perturb tool generations.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{RwLockReadGuard, RwLockWriteGuard};

use crate::upstream::types::{UpstreamEntry, UpstreamTool};

use super::UpstreamPool;
use super::tools::MAX_UPSTREAM_TOOLS;

static NEXT_TOOL_CATALOG_GENERATION: AtomicU64 = AtomicU64::new(1);

fn next_generation() -> ToolCatalogGeneration {
    ToolCatalogGeneration(
        NEXT_TOOL_CATALOG_GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("tool catalog generation exhausted"),
    )
}

/// Opaque process-local identity of one published tool catalog revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToolCatalogGeneration(u64);

/// Fail-closed reason that no routable tool snapshot can be published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolCatalogPublicationError {
    TooManyRoutes,
    InvalidTool,
}

/// One immutable, routable tool and its owning upstream.
#[derive(Debug, Clone)]
pub struct PublishedToolRoute {
    pub upstream_name: Arc<str>,
    pub tool_name: Arc<str>,
    pub tool: UpstreamTool,
}

/// A coherent point-in-time projection of every routable pool tool.
#[derive(Debug, Clone)]
pub struct PublishedToolCatalogSnapshot {
    generation: ToolCatalogGeneration,
    routes: Arc<[PublishedToolRoute]>,
}

impl PublishedToolCatalogSnapshot {
    #[must_use]
    pub fn generation(&self) -> ToolCatalogGeneration {
        self.generation
    }

    #[must_use]
    pub fn routes(&self) -> &[PublishedToolRoute] {
        &self.routes
    }
}

pub(super) struct CatalogState {
    entries: HashMap<String, UpstreamEntry>,
    incarnations: HashMap<String, super::incarnation::ConnectionIncarnation>,
    published: Result<Arc<PublishedToolCatalogSnapshot>, ToolCatalogPublicationError>,
    determinant: ProjectionDeterminant,
}

#[derive(Debug, PartialEq, Eq)]
enum ProjectionDeterminant {
    Ready(Vec<RouteDeterminant>),
    Failed(ToolCatalogPublicationError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RouteDeterminant {
    upstream_name: String,
    tool_name: String,
    tool: serde_json::Value,
    input_schema: Option<serde_json::Value>,
    output_schema: Option<serde_json::Value>,
    destructive: bool,
}

impl CatalogState {
    pub(super) fn bind_incarnation(
        &mut self,
        upstream: &str,
        incarnation: super::incarnation::ConnectionIncarnation,
    ) {
        self.incarnations.insert(upstream.to_string(), incarnation);
    }

    pub(super) fn incarnation(
        &self,
        upstream: &str,
    ) -> Option<super::incarnation::ConnectionIncarnation> {
        self.incarnations.get(upstream).copied()
    }

    pub(super) fn remove_incarnation(&mut self, upstream: &str) {
        self.incarnations.remove(upstream);
    }

    pub(super) fn clear_incarnations(&mut self) {
        self.incarnations.clear();
    }

    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            incarnations: HashMap::new(),
            published: Ok(Arc::new(PublishedToolCatalogSnapshot {
                generation: next_generation(),
                routes: Arc::from([]),
            })),
            determinant: ProjectionDeterminant::Ready(Vec::new()),
        }
    }

    fn projection(
        &self,
    ) -> Result<(Vec<RouteDeterminant>, Arc<[PublishedToolRoute]>), ToolCatalogPublicationError>
    {
        let mut upstreams = self.entries.iter().collect::<Vec<_>>();
        upstreams.sort_unstable_by_key(|(name, _)| name.as_str());
        let mut determinant = Vec::new();
        let mut routes = Vec::new();

        for (upstream, entry) in upstreams {
            if !entry.tool_health.is_routable() {
                continue;
            }
            if entry.name.as_ref() != upstream {
                return Err(ToolCatalogPublicationError::InvalidTool);
            }
            let mut tools = entry
                .tools
                .iter()
                .filter(|(name, _)| entry.exposure_policy.matches(name))
                .collect::<Vec<_>>();
            tools.sort_unstable_by_key(|(name, _)| name.as_str());
            for (name, source_tool) in tools {
                if routes.len() == MAX_UPSTREAM_TOOLS {
                    return Err(ToolCatalogPublicationError::TooManyRoutes);
                }
                if source_tool.tool.name.as_ref() != name
                    || source_tool.upstream_name.as_ref() != upstream
                {
                    return Err(ToolCatalogPublicationError::InvalidTool);
                }
                let tool = source_tool.clone();
                let tool_value = serde_json::to_value(&tool.tool)
                    .map_err(|_| ToolCatalogPublicationError::InvalidTool)?;
                determinant.push(RouteDeterminant {
                    upstream_name: upstream.clone(),
                    tool_name: name.clone(),
                    tool: tool_value,
                    input_schema: tool.input_schema.clone(),
                    output_schema: tool.output_schema.clone(),
                    destructive: tool.destructive,
                });
                routes.push(PublishedToolRoute {
                    upstream_name: Arc::from(upstream.as_str()),
                    tool_name: Arc::from(name.as_str()),
                    tool,
                });
            }
        }
        Ok((determinant, Arc::from(routes)))
    }

    fn publish_if_changed(&mut self) {
        let projection = self.projection();
        let determinant = match &projection {
            Ok((determinant, _)) => ProjectionDeterminant::Ready(determinant.clone()),
            Err(error) => ProjectionDeterminant::Failed(*error),
        };
        if determinant != self.determinant {
            self.determinant = determinant;
            self.published = projection.map(|(_, routes)| {
                Arc::new(PublishedToolCatalogSnapshot {
                    generation: next_generation(),
                    routes,
                })
            });
        }
    }
}

impl Deref for CatalogState {
    type Target = HashMap<String, UpstreamEntry>;

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl DerefMut for CatalogState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entries
    }
}

pub(super) struct CatalogWriteGuard<'a>(RwLockWriteGuard<'a, CatalogState>);

impl Deref for CatalogWriteGuard<'_> {
    type Target = CatalogState;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for CatalogWriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for CatalogWriteGuard<'_> {
    fn drop(&mut self) {
        self.0.publish_if_changed();
    }
}

impl UpstreamPool {
    pub(super) async fn catalog_write(&self) -> CatalogWriteGuard<'_> {
        CatalogWriteGuard(self.catalog.write().await)
    }

    /// Observe generation and routes from the same locked catalog state.
    pub async fn published_tool_catalog(
        &self,
    ) -> Result<Arc<PublishedToolCatalogSnapshot>, ToolCatalogPublicationError> {
        let state: RwLockReadGuard<'_, CatalogState> = self.catalog.read().await;
        state.published.clone()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use rmcp::model::Tool;

    use super::*;
    use crate::upstream::pool::entries::healthy_in_process_entry;
    use crate::upstream::types::ToolExposurePolicy;

    fn entry(upstream: &str, tool_name: &str) -> UpstreamEntry {
        let upstream_name: Arc<str> = Arc::from(upstream);
        let tool = Tool::new(
            tool_name.to_string(),
            "test tool",
            Arc::new(serde_json::Map::new()),
        );
        let upstream_tool = UpstreamTool {
            input_schema: Some(serde_json::Value::Object((*tool.input_schema).clone())),
            output_schema: None,
            destructive: false,
            upstream_name: Arc::clone(&upstream_name),
            tool,
        };
        healthy_in_process_entry(
            upstream_name,
            HashMap::from([(tool_name.to_string(), upstream_tool)]),
        )
    }

    fn route_names(snapshot: &PublishedToolCatalogSnapshot) -> Vec<(&str, &str)> {
        snapshot
            .routes()
            .iter()
            .map(|route| (route.upstream_name.as_ref(), route.tool_name.as_ref()))
            .collect()
    }

    async fn snapshot(pool: &UpstreamPool) -> Arc<PublishedToolCatalogSnapshot> {
        pool.published_tool_catalog()
            .await
            .expect("published catalog")
    }

    #[tokio::test]
    async fn add_remove_and_identical_replacement_publish_only_semantic_changes() {
        let pool = UpstreamPool::new();
        let empty = snapshot(&pool).await;

        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let added = snapshot(&pool).await;
        assert_ne!(added.generation(), empty.generation());
        assert_eq!(route_names(&added), [("alpha", "read")]);

        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let identical = snapshot(&pool).await;
        assert_eq!(identical.generation(), added.generation());
        assert!(Arc::ptr_eq(&identical, &added));

        pool.catalog_write().await.remove("alpha");
        let removed = snapshot(&pool).await;
        assert_ne!(removed.generation(), added.generation());
        assert!(removed.routes().is_empty());
    }

    #[tokio::test]
    async fn aba_content_receives_a_fresh_generation() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "a"));
        let first_a = snapshot(&pool).await;
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "b"));
        let b = snapshot(&pool).await;
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "a"));
        let second_a = snapshot(&pool).await;

        assert_ne!(first_a.generation(), b.generation());
        assert_ne!(first_a.generation(), second_a.generation());
        assert_eq!(route_names(&first_a), route_names(&second_a));
    }

    #[tokio::test]
    async fn clones_share_publication_but_distinct_pools_never_share_generation() {
        let pool = UpstreamPool::new();
        let clone = pool.clone();
        let other = UpstreamPool::new();
        assert_ne!(
            snapshot(&pool).await.generation(),
            snapshot(&other).await.generation()
        );

        clone
            .catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let original_snapshot = snapshot(&pool).await;
        let clone_snapshot = snapshot(&clone).await;
        assert!(Arc::ptr_eq(&original_snapshot, &clone_snapshot));
        assert_ne!(
            original_snapshot.generation(),
            snapshot(&other).await.generation()
        );
    }

    #[tokio::test]
    async fn reader_cannot_observe_new_routes_with_the_old_generation() {
        let pool = UpstreamPool::new();
        let old = snapshot(&pool).await;
        let mut writer = pool.catalog_write().await;
        writer.insert("alpha".into(), entry("alpha", "read"));

        let reader_pool = pool.clone();
        let reader = tokio::spawn(async move { reader_pool.published_tool_catalog().await });
        tokio::task::yield_now().await;
        assert!(
            !reader.is_finished(),
            "reader must wait for publication lock"
        );

        drop(writer);
        let observed = reader
            .await
            .expect("reader task")
            .expect("published catalog");
        assert_ne!(observed.generation(), old.generation());
        assert_eq!(route_names(&observed), [("alpha", "read")]);
    }

    #[tokio::test]
    async fn exposure_transitions_remove_and_restore_routes() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let visible = snapshot(&pool).await;
        pool.catalog_write()
            .await
            .get_mut("alpha")
            .expect("entry")
            .exposure_policy = ToolExposurePolicy::AllowList(Vec::new());
        let hidden = snapshot(&pool).await;
        assert!(hidden.routes().is_empty());
        assert_ne!(hidden.generation(), visible.generation());

        pool.catalog_write()
            .await
            .get_mut("alpha")
            .expect("entry")
            .exposure_policy = ToolExposurePolicy::All;
        let restored = snapshot(&pool).await;
        assert_eq!(route_names(&restored), [("alpha", "read")]);
        assert_ne!(restored.generation(), visible.generation());
    }

    #[tokio::test]
    async fn only_tool_routability_threshold_changes_generation() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let healthy = snapshot(&pool).await;
        pool.record_failure("alpha", "one").await;
        pool.record_failure("alpha", "two").await;
        assert_eq!(snapshot(&pool).await.generation(), healthy.generation());

        pool.record_failure("alpha", "three").await;
        let open = snapshot(&pool).await;
        assert!(open.routes().is_empty());
        assert_ne!(open.generation(), healthy.generation());
        pool.record_success("alpha").await;
        assert_eq!(
            route_names(snapshot(&pool).await.as_ref()),
            [("alpha", "read")]
        );
    }

    #[tokio::test]
    async fn schema_and_destructive_metadata_changes_advance_generation() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let original = snapshot(&pool).await;
        {
            let mut catalog = pool.catalog_write().await;
            let tool = catalog
                .get_mut("alpha")
                .and_then(|entry| entry.tools.get_mut("read"))
                .expect("tool");
            tool.output_schema = Some(serde_json::json!({"type": "string"}));
            tool.destructive = true;
        }
        assert_ne!(snapshot(&pool).await.generation(), original.generation());
    }

    #[tokio::test]
    async fn mismatched_route_identity_fails_the_whole_publication_closed() {
        let entry_name_pool = UpstreamPool::new();
        let mut wrong_entry_name = entry("wrong", "read");
        wrong_entry_name
            .tools
            .get_mut("read")
            .expect("tool")
            .upstream_name = Arc::from("canonical");
        entry_name_pool
            .catalog_write()
            .await
            .insert("canonical".into(), wrong_entry_name);

        let tool_name_pool = UpstreamPool::new();
        let mut wrong_tool_name = entry("alpha", "read");
        let tool = wrong_tool_name.tools.remove("read").expect("tool");
        wrong_tool_name.tools.insert("alias".into(), tool);
        tool_name_pool
            .catalog_write()
            .await
            .insert("alpha".into(), wrong_tool_name);

        let owner_pool = UpstreamPool::new();
        let mut wrong_owner = entry("alpha", "read");
        wrong_owner
            .tools
            .get_mut("read")
            .expect("tool")
            .upstream_name = Arc::from("wrong");
        owner_pool
            .catalog_write()
            .await
            .insert("alpha".into(), wrong_owner);

        for pool in [&entry_name_pool, &tool_name_pool, &owner_pool] {
            assert!(matches!(
                pool.published_tool_catalog().await,
                Err(ToolCatalogPublicationError::InvalidTool)
            ));
        }
    }

    #[tokio::test]
    async fn non_tool_metadata_does_not_advance_generation() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let original = snapshot(&pool).await;
        {
            let mut catalog = pool.catalog_write().await;
            let entry = catalog.get_mut("alpha").expect("entry");
            entry.prompt_count = 7;
            entry.resource_count = 9;
            entry.skill_names.push("unrelated".into());
        }
        let unchanged = snapshot(&pool).await;
        assert_eq!(unchanged.generation(), original.generation());
        assert!(Arc::ptr_eq(&unchanged, &original));
    }

    #[tokio::test]
    async fn aggregate_overflow_fails_closed_and_recovery_gets_new_generation() {
        let pool = UpstreamPool::new();
        pool.catalog_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let before = snapshot(&pool).await;
        {
            let mut catalog = pool.catalog_write().await;
            let entry = catalog.get_mut("alpha").expect("entry");
            let template = entry.tools.get("read").expect("tool").clone();
            for index in 0..MAX_UPSTREAM_TOOLS {
                let name = format!("extra-{index}");
                let mut tool = template.clone();
                tool.tool.name = name.clone().into();
                entry.tools.insert(name, tool);
            }
        }
        assert!(matches!(
            pool.published_tool_catalog().await,
            Err(ToolCatalogPublicationError::TooManyRoutes)
        ));
        pool.catalog_write()
            .await
            .get_mut("alpha")
            .expect("entry")
            .tools
            .retain(|name, _| name == "read");
        let recovered = snapshot(&pool).await;
        assert_ne!(recovered.generation(), before.generation());
    }
}
