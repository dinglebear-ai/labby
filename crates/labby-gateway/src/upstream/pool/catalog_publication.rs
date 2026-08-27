//! Immutable publication of the pool's routable tool projection.
//!
//! The snapshot deliberately excludes prompts, resources, skills, connections,
//! OAuth subjects, and capability health other than tool routability. Those
//! concerns have separate lifecycles and must not perturb tool generations.

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tokio::sync::{RwLockReadGuard, RwLockWriteGuard};

use crate::upstream::types::{UpstreamEntry, UpstreamTool};

use super::UpstreamPool;
use super::tools::MAX_UPSTREAM_TOOLS;

// A single gateway projection is intentionally tighter than the 512 KiB
// per-tool Code Mode admission limit: one valid tool cannot crowd every other
// route out of the immutable snapshot.
const MAX_AGGREGATE_SCHEMA_BYTES: usize = 16 * 1024 * 1024;
const MAX_PUBLICATION_RETRIES: usize = 3;

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
    TooManySchemaBytes,
    InvalidTool,
    ConcurrentMutation,
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
    published: Result<Arc<PublishedToolCatalogSnapshot>, ToolCatalogPublicationError>,
    determinant: ProjectionDeterminant,
    tool_revision: u64,
    published_revision: u64,
    #[cfg(test)]
    rebuild_count: u64,
    #[cfg(test)]
    snapshot_clone_count: AtomicU64,
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
    pub(super) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            published: Ok(Arc::new(PublishedToolCatalogSnapshot {
                generation: next_generation(),
                routes: Arc::from([]),
            })),
            determinant: ProjectionDeterminant::Ready(Vec::new()),
            tool_revision: 0,
            published_revision: 0,
            #[cfg(test)]
            rebuild_count: 0,
            #[cfg(test)]
            snapshot_clone_count: AtomicU64::new(0),
        }
    }

    fn projection(
        entries: &HashMap<String, UpstreamEntry>,
    ) -> Result<(Vec<RouteDeterminant>, Arc<[PublishedToolRoute]>), ToolCatalogPublicationError>
    {
        let mut upstreams = entries.iter().collect::<Vec<_>>();
        upstreams.sort_unstable_by_key(|(name, _)| name.as_str());
        let mut determinant = Vec::new();
        let mut routes = Vec::new();
        let mut schema_bytes = 0usize;

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
                let input_bytes = tool.input_schema.as_ref().map_or(Ok(0), |schema| {
                    serde_json::to_vec(schema).map(|bytes| bytes.len())
                });
                let output_bytes = tool.output_schema.as_ref().map_or(Ok(0), |schema| {
                    serde_json::to_vec(schema).map(|bytes| bytes.len())
                });
                schema_bytes = schema_bytes
                    .checked_add(
                        input_bytes.map_err(|_| ToolCatalogPublicationError::InvalidTool)?
                            + output_bytes.map_err(|_| ToolCatalogPublicationError::InvalidTool)?,
                    )
                    .ok_or(ToolCatalogPublicationError::TooManySchemaBytes)?;
                if schema_bytes > MAX_AGGREGATE_SCHEMA_BYTES {
                    return Err(ToolCatalogPublicationError::TooManySchemaBytes);
                }
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

    fn publish_if_changed(
        &mut self,
        revision: u64,
        projection: Result<
            (Vec<RouteDeterminant>, Arc<[PublishedToolRoute]>),
            ToolCatalogPublicationError,
        >,
    ) {
        let determinant = match &projection {
            Ok((determinant, _)) => ProjectionDeterminant::Ready(determinant.clone()),
            Err(error) => ProjectionDeterminant::Failed(*error),
        };
        // A single dirty revision that resolves to the same determinant is a
        // true no-op. Multiple unseen revisions may be an ABA transition, so
        // publish a fresh identity even when the final bytes match.
        if determinant != self.determinant || revision > self.published_revision.saturating_add(1) {
            self.determinant = determinant;
            self.published = projection.map(|(_, routes)| {
                Arc::new(PublishedToolCatalogSnapshot {
                    generation: next_generation(),
                    routes,
                })
            });
        }
        self.published_revision = revision;
    }

    fn mark_tool_projection_dirty(&mut self) {
        self.tool_revision = self.tool_revision.saturating_add(1);
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

pub(super) struct CatalogWriteGuard<'a> {
    guard: RwLockWriteGuard<'a, CatalogState>,
    tool_projection_dirty: bool,
}

impl Deref for CatalogWriteGuard<'_> {
    type Target = CatalogState;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for CatalogWriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl Drop for CatalogWriteGuard<'_> {
    fn drop(&mut self) {
        if self.tool_projection_dirty {
            self.guard.mark_tool_projection_dirty();
        }
    }
}

impl CatalogWriteGuard<'_> {
    /// Declare that this mutation may change tool routes or their schemas.
    pub(super) fn mark_tool_projection_dirty(&mut self) {
        self.tool_projection_dirty = true;
    }
}

impl UpstreamPool {
    pub(super) async fn catalog_write(&self) -> CatalogWriteGuard<'_> {
        CatalogWriteGuard {
            guard: self.catalog.write().await,
            // Unit-test fixtures historically insert complete entries through
            // this low-level seam. Production mutations must opt in explicitly.
            tool_projection_dirty: cfg!(test),
        }
    }

    pub(super) async fn catalog_tools_write(&self) -> CatalogWriteGuard<'_> {
        let mut guard = self.catalog_write().await;
        guard.mark_tool_projection_dirty();
        guard
    }

    pub(super) async fn catalog_metadata_write(&self) -> CatalogWriteGuard<'_> {
        CatalogWriteGuard {
            guard: self.catalog.write().await,
            tool_projection_dirty: false,
        }
    }

    /// Observe generation and routes from the same locked catalog state.
    pub async fn published_tool_catalog(
        &self,
    ) -> Result<Arc<PublishedToolCatalogSnapshot>, ToolCatalogPublicationError> {
        let started = Instant::now();
        for attempt in 1..=MAX_PUBLICATION_RETRIES {
            {
                let state: RwLockReadGuard<'_, CatalogState> = self.catalog.read().await;
                if state.published_revision == state.tool_revision {
                    return state.published.clone();
                }
            }

            let wait_started = Instant::now();
            let _publication = self.catalog_publication.lock().await;
            let publication_wait = wait_started.elapsed();
            let lock_started = Instant::now();
            let (revision, entries) = {
                let state = self.catalog.read().await;
                if state.published_revision == state.tool_revision {
                    tracing::debug!(
                        action = "tool_catalog.rebuild",
                        outcome = "coalesced",
                        attempt,
                        wait_ms = publication_wait.as_millis(),
                        lock_ms = lock_started.elapsed().as_millis(),
                        total_ms = started.elapsed().as_millis(),
                        "coalesced behind an in-flight upstream tool projection rebuild"
                    );
                    return state.published.clone();
                }
                #[cfg(test)]
                state.snapshot_clone_count.fetch_add(1, Ordering::Relaxed);
                (state.tool_revision, state.entries.clone())
            };
            let rebuild_started = Instant::now();
            let projection = CatalogState::projection(&entries);
            let rebuild_elapsed = rebuild_started.elapsed();
            let projected_routes = projection.as_ref().map_or(0, |(_, routes)| routes.len());
            let mut state = self.catalog.write().await;
            if state.tool_revision != revision {
                tracing::debug!(
                    action = "tool_catalog.rebuild",
                    outcome = "retry",
                    attempt,
                    wait_ms = publication_wait.as_millis(),
                    lock_ms = lock_started.elapsed().as_millis(),
                    rebuild_ms = rebuild_elapsed.as_millis(),
                    total_ms = started.elapsed().as_millis(),
                    "upstream tool projection changed during rebuild"
                );
                continue;
            }
            #[cfg(test)]
            {
                state.rebuild_count = state.rebuild_count.saturating_add(1);
            }
            state.publish_if_changed(revision, projection);
            let outcome = match &state.published {
                Ok(_) => "ready",
                Err(ToolCatalogPublicationError::TooManyRoutes) => "too_many_routes",
                Err(ToolCatalogPublicationError::TooManySchemaBytes) => "too_many_schema_bytes",
                Err(ToolCatalogPublicationError::InvalidTool) => "invalid_tool",
                Err(ToolCatalogPublicationError::ConcurrentMutation) => "concurrent_mutation",
            };
            tracing::debug!(
                action = "tool_catalog.rebuild",
                outcome,
                attempt,
                projected_routes,
                wait_ms = publication_wait.as_millis(),
                lock_ms = lock_started.elapsed().as_millis(),
                rebuild_ms = rebuild_elapsed.as_millis(),
                total_ms = started.elapsed().as_millis(),
                "rebuilt upstream tool projection"
            );
            return state.published.clone();
        }
        tracing::warn!(
            action = "tool_catalog.rebuild",
            outcome = "concurrent_mutation",
            attempts = MAX_PUBLICATION_RETRIES,
            total_ms = started.elapsed().as_millis(),
            "upstream tool projection rebuild exhausted its retry budget"
        );
        Err(ToolCatalogPublicationError::ConcurrentMutation)
    }

    #[cfg(test)]
    async fn publication_test_state(&self) -> (u64, u64, u64, u64) {
        let state = self.catalog.read().await;
        (
            state.tool_revision,
            state.published_revision,
            state.rebuild_count,
            state.snapshot_clone_count.load(Ordering::Relaxed),
        )
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

    fn large_entry(upstream: &str, tool_count: usize) -> UpstreamEntry {
        let upstream_name: Arc<str> = Arc::from(upstream);
        let schema = serde_json::json!({
            "type": "object",
            "description": "x".repeat(8 * 1024),
        });
        let tools = (0..tool_count)
            .map(|index| {
                let name = format!("tool-{index:04}");
                let tool = Tool::new(
                    name.clone(),
                    "large catalog tool",
                    Arc::new(serde_json::Map::new()),
                );
                (
                    name,
                    UpstreamTool {
                        input_schema: Some(schema.clone()),
                        output_schema: None,
                        destructive: false,
                        upstream_name: Arc::clone(&upstream_name),
                        tool,
                    },
                )
            })
            .collect();
        healthy_in_process_entry(upstream_name, tools)
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

    #[tokio::test]
    async fn unrelated_and_noop_writes_do_not_rebuild_but_tool_change_rebuilds_once() {
        let pool = UpstreamPool::new();
        pool.catalog_tools_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let original = snapshot(&pool).await;
        let baseline = pool.publication_test_state().await;

        {
            let mut catalog = pool.catalog_metadata_write().await;
            let entry = catalog.get_mut("alpha").expect("entry");
            entry.prompt_count = 7;
            entry.resource_count = 9;
            entry.skill_names.push("unrelated".into());
        }
        drop(pool.catalog_metadata_write().await);
        let unchanged = snapshot(&pool).await;
        assert!(Arc::ptr_eq(&unchanged, &original));
        assert_eq!(pool.publication_test_state().await, baseline);

        {
            let mut catalog = pool.catalog_tools_write().await;
            catalog
                .get_mut("alpha")
                .expect("entry")
                .tools
                .get_mut("read")
                .expect("tool")
                .destructive = true;
        }
        let changed = snapshot(&pool).await;
        let after = pool.publication_test_state().await;
        assert_ne!(changed.generation(), original.generation());
        assert_eq!(after.0, baseline.0 + 1, "one dirty tool revision");
        assert_eq!(after.1, baseline.1 + 1, "one published tool revision");
        assert_eq!(after.2, baseline.2 + 1, "one projection rebuild");
        assert_eq!(after.3, baseline.3 + 1, "one entry snapshot clone");
    }

    #[tokio::test]
    async fn aggregate_schema_byte_cap_rejects_the_projection_fail_closed() {
        let pool = UpstreamPool::new();
        let mut oversized = entry("alpha", "read");
        oversized.tools.get_mut("read").expect("tool").input_schema = Some(serde_json::json!({
            "type": "string",
            "description": "x".repeat(MAX_AGGREGATE_SCHEMA_BYTES + 1),
        }));
        pool.catalog_tools_write()
            .await
            .insert("alpha".into(), oversized);

        assert!(matches!(
            pool.published_tool_catalog().await,
            Err(ToolCatalogPublicationError::TooManySchemaBytes)
        ));
        assert_eq!(pool.publication_test_state().await.2, 1);
    }

    #[tokio::test]
    async fn concurrent_readers_clone_one_snapshot_for_one_dirty_revision() {
        const READERS: usize = 32;

        let pool = UpstreamPool::new();
        pool.catalog_tools_write()
            .await
            .insert("alpha".into(), entry("alpha", "read"));
        let baseline = pool.publication_test_state().await;
        let barrier = Arc::new(tokio::sync::Barrier::new(READERS + 1));
        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            let pool = pool.clone();
            let barrier = Arc::clone(&barrier);
            readers.push(tokio::spawn(async move {
                barrier.wait().await;
                pool.published_tool_catalog().await.expect("published")
            }));
        }
        barrier.wait().await;

        let mut snapshots = Vec::with_capacity(READERS);
        for reader in readers {
            snapshots.push(reader.await.expect("reader task"));
        }
        assert!(
            snapshots[1..]
                .iter()
                .all(|snapshot| Arc::ptr_eq(snapshot, &snapshots[0]))
        );
        let after = pool.publication_test_state().await;
        assert_eq!(after.2, baseline.2 + 1, "one projection rebuild");
        assert_eq!(after.3, baseline.3 + 1, "one entry snapshot clone");
    }

    #[tokio::test]
    async fn large_catalog_noop_reads_and_metadata_mutation_skip_projection_work() {
        let pool = UpstreamPool::new();
        pool.catalog_tools_write()
            .await
            .insert("large".into(), large_entry("large", MAX_UPSTREAM_TOOLS));
        let published = snapshot(&pool).await;
        assert_eq!(published.routes().len(), MAX_UPSTREAM_TOOLS);
        let baseline = pool.publication_test_state().await;

        for _ in 0..32 {
            let observed = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                pool.published_tool_catalog(),
            )
            .await
            .expect("clean readers do not wait for projection work")
            .expect("published catalog");
            assert!(Arc::ptr_eq(&observed, &published));
        }
        assert_eq!(pool.publication_test_state().await, baseline);

        {
            let mut catalog = pool.catalog_metadata_write().await;
            let entry = catalog.get_mut("large").expect("large entry");
            entry.prompt_count = entry.prompt_count.saturating_add(1);
            entry.resource_count = entry.resource_count.saturating_add(1);
        }
        let after_metadata = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            pool.published_tool_catalog(),
        )
        .await
        .expect("unrelated metadata does not trigger a full projection")
        .expect("published catalog");
        assert!(Arc::ptr_eq(&after_metadata, &published));
        assert_eq!(pool.publication_test_state().await, baseline);
    }
}
