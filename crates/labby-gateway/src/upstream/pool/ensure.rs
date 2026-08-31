//! Lazy upstream seeding and on-demand tool discovery.
//!
//! These methods seed the catalog from config without connecting, then connect
//! upstreams on demand (`ensure_tools_for_upstream`), single-flighting concurrent
//! requests through a per-upstream lock. `replace_catalog_tools` is the shared
//! catalog mutator after a tools probe; it is `pub(super)` because `probe.rs`
//! (`reprobe_upstream`) calls it across the module boundary (see plan §3.0/§2.1).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Mutex;

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::{UpstreamCapability, UpstreamRuntimeOwner};
#[cfg(test)]
use super::TestUpstreamConnector;
use super::UpstreamPool;
use super::connect::connect_upstream_with_client;
use super::entries::{lazy_upstream_entry, resolve_upstream_exposure_policies};
use super::helpers::{
    cached_upstream_tool, upstream_discovery_timeout, upstream_name_is_uri_safe,
    upstream_target_redacted, upstream_transport,
};
use super::resources_list::catalog_listing_timeout;
use super::skills_list::peer_declares_skills;
use super::tools::tool_has_mcp_app_ui_resource;
use super::validate::validate_upstream_config;

/// Validate an upstream config entry and, if valid, return the catalog entry
/// that should be inserted for it.
///
/// Returns `None` (and emits a `WARN`) when the config should be skipped:
/// disabled, URI-unsafe name, or failing `validate_upstream_config`.
///
/// This helper removes the duplicated validation+entry-build logic that used to
/// live in both `seed_lazy_upstreams` and `ensure_lazy_upstream_entry` (Q-M3).
fn validated_lazy_entry(config: &UpstreamConfig) -> Option<super::super::types::UpstreamEntry> {
    if !config.enabled {
        return None;
    }
    if !upstream_name_is_uri_safe(&config.name) {
        tracing::warn!(
            upstream = %config.name,
            "upstream name contains URI-unsafe characters (/, ?, #) — skipping"
        );
        return None;
    }
    if let Err(msg) = validate_upstream_config(config) {
        tracing::warn!(
            upstream = %config.name,
            "skipping upstream: {msg}"
        );
        return None;
    }
    Some(lazy_upstream_entry(config, Arc::from(config.name.as_str())))
}

impl UpstreamPool {
    pub(super) async fn install_connected_tools(
        &self,
        config: &UpstreamConfig,
        connection: super::UpstreamConnection,
        tools: Vec<rmcp::model::Tool>,
        supports_skills: Option<bool>,
    ) -> anyhow::Result<()> {
        let exposure_policies = resolve_upstream_exposure_policies(config);
        let upstream_name: Arc<str> = Arc::from(config.name.as_str());
        let tools = tools
            .into_iter()
            .map(|tool| cached_upstream_tool(tool, &upstream_name))
            .collect::<HashMap<_, _>>();
        let previous = self
            .install_connection_and_apply_entry(config.name.clone(), connection, |entry| {
                entry.tools = tools;
                entry.exposure_policy = exposure_policies.tools;
                entry.resource_exposure_policy = exposure_policies.resources;
                entry.prompt_exposure_policy = exposure_policies.prompts;
                entry.skill_exposure_policy = exposure_policies.skills;
                entry.proxy_skills = config.proxy_skills;
                if supports_skills.is_some() {
                    entry.supports_skills = supports_skills;
                }
            })
            .await?;
        if let Some(previous) = previous {
            previous
                .shutdown(&config.name, "upstream.connection.replace")
                .await;
        }
        Ok(())
    }

    /// Seed the upstream catalog from config without starting any upstream runtime.
    pub async fn seed_lazy_upstreams(&self, configs: &[UpstreamConfig]) {
        let mut catalog = self.catalog_write().await;
        let mut resource_names = Vec::new();
        let mut processed_names = std::collections::HashSet::new();

        for config in configs {
            if !processed_names.insert(&config.name) {
                continue;
            }
            let Some(entry) = validated_lazy_entry(config) else {
                continue;
            };

            catalog.entry(config.name.clone()).or_insert(entry);

            if config.proxy_resources {
                resource_names.push(config.name.clone());
            }
        }

        resource_names.sort_unstable();
        resource_names.dedup();
        *self.resource_upstreams.write().await = resource_names;
    }

    async fn ensure_lazy_upstream_entry(&self, config: &UpstreamConfig) {
        let Some(entry) = validated_lazy_entry(config) else {
            return;
        };
        self.catalog
            .write()
            .await
            .entry(config.name.clone())
            .or_insert(entry);
        if config.proxy_resources {
            let mut resource_upstreams = self.resource_upstreams.write().await;
            if !resource_upstreams.iter().any(|name| name == &config.name) {
                resource_upstreams.push(config.name.clone());
                resource_upstreams.sort_unstable();
            }
        }
    }

    /// Ensure one upstream has discovered tools, connecting it lazily when needed.
    pub async fn ensure_tools_for_upstream(
        &self,
        config: &UpstreamConfig,
        oauth_subject: Option<&str>,
        runtime_owner: Option<&UpstreamRuntimeOwner>,
    ) -> anyhow::Result<bool> {
        if !config.enabled {
            return Ok(false);
        }
        // OAuth tool discovery is identity-scoped. Keep its peer and tool list
        // in the per-(upstream, subject) cache; publishing either into the
        // process-global connection/catalog maps lets the first authenticated
        // caller shape every later caller's view.
        if config.oauth.is_some()
            && let Some(subject) = oauth_subject
        {
            let started = Instant::now();
            self.ensure_lazy_upstream_entry(config).await;
            let (_peer, tools) = self.acquire_or_connect_subject(config, subject).await?;
            self.record_success_for(&config.name, UpstreamCapability::Tools)
                .await;
            tracing::info!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.subject.ensure",
                event = "finish",
                operation = "connection.acquire",
                upstream = %config.name,
                tool_count = tools.len(),
                elapsed_ms = started.elapsed().as_millis(),
                "subject-scoped upstream tools ready"
            );
            return Ok(true);
        }
        let lifecycle_epoch = config
            .oauth
            .as_ref()
            .and_then(|_| self.oauth_lifecycle_epoch());
        if self.has_healthy_tools_for_upstream(&config.name).await {
            self.refresh_ui_resource_cache_for_healthy_upstream_if_needed(config)
                .await;
            return Ok(false);
        }

        let connect_lock = self.lazy_connect_lock(&config.name).await;
        let _connect_guard = connect_lock.lock().await;
        if self.has_healthy_tools_for_upstream(&config.name).await {
            self.refresh_ui_resource_cache_for_healthy_upstream_if_needed(config)
                .await;
            return Ok(false);
        }

        self.ensure_lazy_upstream_entry(config).await;
        let stale_connection = self.remove_connection_binding(&config.name).await;
        if let Some(connection) = stale_connection {
            connection
                .shutdown(&config.name, "upstream.lazy.ensure.before_connect")
                .await;
        }

        let started = Instant::now();
        let subject = config.oauth.as_ref().and(oauth_subject);
        let runtime_owner = runtime_owner.or(self.runtime_owner.as_ref());
        let discovery_timeout = upstream_discovery_timeout(config, self.request_timeout);
        let connect_result = tokio::time::timeout(
            discovery_timeout,
            connect_upstream_with_client(
                config,
                subject,
                self.oauth_client_cache.as_ref(),
                self.runtime_origin.as_deref(),
                runtime_owner,
                Some(&self.shared_http_client),
            ),
        )
        .await;
        let (conn, tools) = match connect_result {
            Ok(Ok(connected)) => connected,
            Ok(Err(error)) => {
                self.record_failure_for(
                    &config.name,
                    UpstreamCapability::Tools,
                    format!("lazy upstream connect failed: {error}"),
                )
                .await;
                return Err(error);
            }
            Err(_) => {
                let error = anyhow::anyhow!(
                    "lazy upstream connect timed out after {}s waiting for {} MCP list_tools response from {}",
                    discovery_timeout.as_secs(),
                    upstream_transport(config),
                    upstream_target_redacted(config)
                );
                self.record_failure_for(&config.name, UpstreamCapability::Tools, error.to_string())
                    .await;
                return Err(error);
            }
        };
        let tool_count = tools.len();
        let supports_skills = peer_declares_skills(&conn.peer);
        let _oauth_publication = self.oauth_publication_guard(lifecycle_epoch).await?;
        self.install_connected_tools(config, conn, tools, Some(supports_skills))
            .await?;
        if let Some(subject) = subject {
            self.generic_oauth_subjects
                .write()
                .await
                .insert(config.name.clone(), subject.to_string());
        } else {
            self.generic_oauth_subjects
                .write()
                .await
                .remove(&config.name);
        }
        self.record_success_for(&config.name, UpstreamCapability::Tools)
            .await;
        self.refresh_capability_caches_after_connect(config).await;
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "upstream.lazy.ensure",
            event = "finish",
            operation = "connection.acquire",
            upstream = %config.name,
            tool_count,
            elapsed_ms = started.elapsed().as_millis(),
            "lazy upstream tools connected"
        );
        Ok(true)
    }

    #[cfg(test)]
    async fn ensure_tools_for_upstream_with_connector(
        &self,
        config: &UpstreamConfig,
        oauth_subject: Option<&str>,
        connector: TestUpstreamConnector,
    ) -> anyhow::Result<bool> {
        if !config.enabled {
            return Ok(false);
        }
        if config.oauth.is_some() && oauth_subject.is_some() {
            self.ensure_lazy_upstream_entry(config).await;
            let (_connection, _tools) = connector(config.clone()).await?;
            self.record_success_for(&config.name, UpstreamCapability::Tools)
                .await;
            return Ok(true);
        }
        if self.has_healthy_tools_for_upstream(&config.name).await {
            return Ok(false);
        }

        let connect_lock = self.lazy_connect_lock(&config.name).await;
        let _connect_guard = connect_lock.lock().await;
        if self.has_healthy_tools_for_upstream(&config.name).await {
            return Ok(false);
        }

        self.ensure_lazy_upstream_entry(config).await;
        let stale_connection = self.remove_connection_binding(&config.name).await;
        if let Some(connection) = stale_connection {
            connection
                .shutdown(&config.name, "upstream.lazy.ensure.before_connect")
                .await;
        }

        let (connection, tools) = connector(config.clone()).await?;
        let supports_skills = connection
            .as_ref()
            .map(|connection| peer_declares_skills(&connection.peer));
        if let Some(connection) = connection {
            self.install_connected_tools(config, connection, tools, supports_skills)
                .await?;
            if let Some(subject) = oauth_subject {
                self.generic_oauth_subjects
                    .write()
                    .await
                    .insert(config.name.clone(), subject.to_string());
            } else {
                self.generic_oauth_subjects
                    .write()
                    .await
                    .remove(&config.name);
            }
        } else {
            self.replace_catalog_tools(config, tools, supports_skills)
                .await;
        }
        self.record_success_for(&config.name, UpstreamCapability::Tools)
            .await;
        self.refresh_capability_caches_after_connect(config).await;
        Ok(true)
    }

    #[cfg(test)]
    pub async fn install_test_tools_for_upstream(
        &self,
        config: &UpstreamConfig,
        tools: Vec<rmcp::model::Tool>,
    ) -> anyhow::Result<bool> {
        if !config.enabled {
            return Ok(false);
        }
        if self.has_healthy_tools_for_upstream(&config.name).await {
            return Ok(false);
        }
        self.ensure_lazy_upstream_entry(config).await;
        self.replace_catalog_tools(config, tools, None).await;
        self.record_success_for(&config.name, UpstreamCapability::Tools)
            .await;
        Ok(true)
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn install_test_subject_tools_for_upstream(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        tools: Vec<rmcp::model::Tool>,
    ) {
        use std::time::Instant;

        let fixture = super::testsupport::catalog_pool_with_server(
            &config.name,
            super::testsupport::SlowResponseServer,
        )
        .await;
        let connection = fixture
            .connections
            .write()
            .await
            .remove(&config.name)
            .expect("fixture connection present");
        let peer = connection.peer.clone();
        self.seed_lazy_upstreams(std::slice::from_ref(config)).await;
        self.subject_connections.write().await.insert(
            (config.name.clone(), subject.to_string()),
            super::SubjectScopedConnection {
                _connection: connection,
                peer,
                tools,
                last_used: Instant::now(),
            },
        );
    }

    #[cfg(any(test, feature = "testkit"))]
    pub async fn install_test_subject_server_for_upstream<S>(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        server: S,
    ) where
        S: rmcp::ServerHandler,
    {
        use std::time::Instant;

        let fixture = super::testsupport::catalog_pool_with_server(&config.name, server).await;
        let connection = fixture
            .connections
            .write()
            .await
            .remove(&config.name)
            .expect("fixture connection present");
        let peer = connection.peer.clone();
        self.seed_lazy_upstreams(std::slice::from_ref(config)).await;
        self.subject_connections.write().await.insert(
            (config.name.clone(), subject.to_string()),
            super::SubjectScopedConnection {
                _connection: connection,
                peer,
                tools: Vec::new(),
                last_used: Instant::now(),
            },
        );
    }

    async fn lazy_connect_lock(&self, upstream_name: &str) -> Arc<Mutex<()>> {
        if let Some(lock) = self
            .lazy_connect_locks
            .read()
            .await
            .get(upstream_name)
            .cloned()
        {
            return lock;
        }
        let mut locks = self.lazy_connect_locks.write().await;
        locks
            .entry(upstream_name.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn reprobe_tools_for_upstream(
        &self,
        config: &UpstreamConfig,
    ) -> anyhow::Result<bool> {
        self.reprobe_tools_for_upstream_as(config, None, None).await
    }

    pub async fn reprobe_tools_for_upstream_as(
        &self,
        config: &UpstreamConfig,
        oauth_subject: Option<&str>,
        runtime_owner: Option<&UpstreamRuntimeOwner>,
    ) -> anyhow::Result<bool> {
        if !config.enabled {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "upstream.reprobe",
                event = "skipped",
                operation = "health",
                upstream = %config.name,
                reason = "disabled",
                "upstream reprobe skipped"
            );
            return Ok(false);
        }
        if config.oauth.is_some()
            && let Some(subject) = oauth_subject
        {
            self.acquire_or_connect_subject(config, subject).await?;
            return Ok(true);
        }
        let connect_lock = self.lazy_connect_lock(&config.name).await;
        let _connect_guard = connect_lock.lock().await;
        self.reprobe_upstream(config, oauth_subject, runtime_owner)
            .await
    }

    pub(super) async fn replace_catalog_tools(
        &self,
        config: &UpstreamConfig,
        tools: Vec<rmcp::model::Tool>,
        supports_skills: Option<bool>,
    ) {
        let exposure_policies = resolve_upstream_exposure_policies(config);
        let upstream_name: Arc<str> = Arc::from(config.name.as_str());
        let tools = tools
            .into_iter()
            .map(|tool| cached_upstream_tool(tool, &upstream_name))
            .collect::<HashMap<_, _>>();

        let mut catalog = self.catalog_write().await;
        if let Some(entry) = catalog.get_mut(&config.name) {
            entry.tools = tools;
            entry.exposure_policy = exposure_policies.tools;
            entry.resource_exposure_policy = exposure_policies.resources;
            entry.prompt_exposure_policy = exposure_policies.prompts;
            entry.skill_exposure_policy = exposure_policies.skills;
            entry.proxy_skills = config.proxy_skills;
            if supports_skills.is_some() {
                entry.supports_skills = supports_skills;
            }
        }
    }

    async fn refresh_ui_resource_cache_for_healthy_upstream_if_needed(
        &self,
        config: &UpstreamConfig,
    ) {
        if !config.proxy_resources {
            return;
        }
        let should_refresh = {
            let catalog = self.catalog.read().await;
            catalog.get(&config.name).is_some_and(|entry| {
                entry.resource_uris.is_empty()
                    && entry.tool_health.is_routable()
                    && entry.tools.values().any(|tool| {
                        entry.exposure_policy.matches(tool.tool.name.as_ref())
                            && tool_has_mcp_app_ui_resource(tool)
                    })
            })
        };
        if should_refresh {
            self.refresh_resource_cache_for_upstream(&config.name).await;
        }
    }

    /// Refresh every capability cache an upstream opts into, right after a
    /// successful connect.
    ///
    /// This is the single post-connect finalization step, called by BOTH
    /// `ensure_tools_for_upstream` and its `#[cfg(test)]` connector twin. It
    /// exists as one function on purpose: prompts previously went unrefreshed
    /// here (bead lab-zfyxk) and the first fix was applied to only one of two
    /// copies of an inlined block — the production path kept the bug while the
    /// suite stayed green, because no test could tell the two copies apart.
    /// Keep new post-connect refreshes in here rather than at the call sites.
    ///
    /// Scope note: the subject-scoped OAuth branch of `ensure_tools_for_upstream`
    /// returns before reaching this, so OAuth upstreams refresh neither cache.
    /// That gap predates this function and is shared by resources and prompts.
    async fn refresh_capability_caches_after_connect(&self, config: &UpstreamConfig) {
        if config.proxy_resources {
            // Reset before listing, not after: an open circuit would exclude
            // this upstream from the very fan-out being kicked off here, making
            // the refresh a silent no-op that can never close the circuit it is
            // gated on. See `reset_capability_circuit` for the full latch.
            self.reset_capability_circuit(&config.name, UpstreamCapability::Resources)
                .await;
            self.refresh_resource_cache_for_upstream(&config.name).await;
        }
        if config.proxy_prompts {
            self.reset_capability_circuit(&config.name, UpstreamCapability::Prompts)
                .await;
            self.refresh_prompt_cache_for_upstream(&config.name).await;
        }
    }

    async fn refresh_resource_cache_for_upstream(&self, upstream_name: &str) {
        let allowed = BTreeSet::from([upstream_name.to_string()]);
        self.list_upstream_resources_allowed(Some(&allowed)).await;
    }

    /// Refresh one upstream's cached prompt listing after a lazy connect.
    ///
    /// Bounded by the shared `catalog_listing_timeout` (10s cap) rather than
    /// the raw `request_timeout` (30s by default) that
    /// `list_upstream_prompts_allowed` would apply on its own. This runs while
    /// the caller still holds both the
    /// per-upstream lazy-connect mutex and a read guard on
    /// `oauth_invalidation_barrier` — and that barrier is write-preferring, so
    /// a stalled `prompts/list` here delays every queued OAuth credential
    /// mutation and, behind it, every peer acquisition fleet-wide. The
    /// resource sibling is capped the same way for the same reason; keep the
    /// two budgets equal.
    ///
    /// LOCK ORDER: whatever this calls must not `acquire_peer` or re-enter
    /// `ensure_tools_for_upstream`. Re-taking `oauth_invalidation_barrier`
    /// read while a writer is queued would deadlock. `collect_upstream_prompts`
    /// satisfies this today (it resolves peers from the catalog directly).
    ///
    /// The cap is applied here rather than inside `list_upstream_prompts_allowed`
    /// so only this connect-path refresh is bounded; the shared listing helper
    /// keeps its existing budget for its other callers. Use
    /// `catalog_listing_timeout` rather than a local constant so the prompt
    /// resource, and general listing budgets cannot drift apart silently.
    async fn refresh_prompt_cache_for_upstream(&self, upstream_name: &str) {
        let allowed = BTreeSet::from([upstream_name.to_string()]);
        let deadline_at =
            tokio::time::Instant::now() + catalog_listing_timeout(self.request_timeout);
        self.list_upstream_prompts_allowed_until(&[], Some(&allowed), deadline_at)
            .await;
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use labby_runtime::gateway_config::{
        UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
    };
    use rmcp::model::MetaObject;
    use rmcp::{RoleClient, ServiceExt};

    use super::super::testsupport::*;
    use super::super::{
        UpstreamConnection, UpstreamRuntimeMetadata, helpers::IN_PROCESS_PEER_BUFFER_BYTES,
    };
    use super::*;

    #[tokio::test]
    async fn seed_lazy_upstreams_records_enabled_names_without_connections() {
        let pool = UpstreamPool::new();
        let configs = vec![
            named_test_upstream_config("alpha"),
            named_test_upstream_config("beta"),
            named_disabled_test_upstream_config("disabled"),
        ];

        pool.seed_lazy_upstreams(&configs).await;

        assert_eq!(pool.upstream_count().await, 2);
        assert_eq!(pool.connection_count_for_tests().await, 0);
        assert!(pool.cached_upstream_summary("alpha").await.is_some());
        assert!(pool.cached_upstream_summary("beta").await.is_some());
        assert!(pool.cached_upstream_summary("disabled").await.is_none());
    }

    #[tokio::test]
    async fn ensure_tools_for_upstream_connects_only_requested_upstream() {
        let pool = UpstreamPool::new();
        let configs = vec![
            named_test_upstream_config("slow"),
            named_test_upstream_config("fast"),
        ];
        pool.seed_lazy_upstreams(&configs).await;

        let fast_seen = Arc::new(AtomicBool::new(false));
        let slow_seen = Arc::new(AtomicBool::new(false));
        let connector: TestUpstreamConnector = {
            let fast_seen = Arc::clone(&fast_seen);
            let slow_seen = Arc::clone(&slow_seen);
            Arc::new(move |config| {
                let fast_seen = Arc::clone(&fast_seen);
                let slow_seen = Arc::clone(&slow_seen);
                Box::pin(async move {
                    match config.name.as_str() {
                        "fast" => fast_seen.store(true, Ordering::Relaxed),
                        "slow" => slow_seen.store(true, Ordering::Relaxed),
                        other => panic!("unexpected upstream {other}"),
                    }
                    Ok((None, vec![test_tool("ping")]))
                })
            })
        };

        pool.ensure_tools_for_upstream_with_connector(&configs[1], None, connector)
            .await
            .expect("fast connects");

        assert!(fast_seen.load(Ordering::Relaxed));
        assert!(!slow_seen.load(Ordering::Relaxed));
        assert_eq!(pool.connection_count_for_tests().await, 0);
        assert_eq!(pool.healthy_tools_for_upstream("fast").await.len(), 1);
        assert!(pool.healthy_tools_for_upstream("slow").await.is_empty());
    }

    #[tokio::test]
    async fn ensure_tools_for_upstream_singleflights_concurrent_connects() {
        let pool = Arc::new(UpstreamPool::new());
        let config = named_test_upstream_config("alpha");
        pool.seed_lazy_upstreams(std::slice::from_ref(&config))
            .await;

        let connect_count = Arc::new(AtomicUsize::new(0));
        let connector: TestUpstreamConnector = {
            let connect_count = Arc::clone(&connect_count);
            Arc::new(move |_config| {
                let connect_count = Arc::clone(&connect_count);
                Box::pin(async move {
                    connect_count.fetch_add(1, Ordering::Relaxed);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    Ok((None, vec![test_tool("ping")]))
                })
            })
        };

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let pool = Arc::clone(&pool);
            let config = config.clone();
            let connector = Arc::clone(&connector);
            tasks.push(tokio::spawn(async move {
                pool.ensure_tools_for_upstream_with_connector(&config, None, connector)
                    .await
                    .expect("lazy connect succeeds")
            }));
        }

        let results = futures::future::join_all(tasks).await;
        let connected = results
            .into_iter()
            .map(|result| result.expect("task joins"))
            .filter(|connected| *connected)
            .count();
        assert_eq!(connected, 1);
        assert_eq!(connect_count.load(Ordering::Relaxed), 1);
        assert_eq!(pool.healthy_tools_for_upstream("alpha").await.len(), 1);
    }

    #[tokio::test]
    async fn subject_scoped_oauth_ensure_never_publishes_tools_globally() {
        let pool = UpstreamPool::new();
        let config = UpstreamConfig {
            oauth: Some(UpstreamOauthConfig {
                mode: UpstreamOauthMode::AuthorizationCodePkce,
                registration: UpstreamOauthRegistration::Dynamic,
                scopes: None,
                credential: Default::default(),
                prefer_client_metadata_document: None,
            }),
            ..named_test_upstream_config("oauth")
        };
        pool.seed_lazy_upstreams(std::slice::from_ref(&config))
            .await;

        pool.ensure_tools_for_upstream_with_connector(
            &config,
            Some("subject-a"),
            Arc::new(|_config| Box::pin(async { Ok((None, vec![test_tool("private_a")])) })),
        )
        .await
        .expect("subject-scoped discovery succeeds");

        assert!(pool.healthy_tools().await.is_empty());
        assert!(pool.healthy_tools_for_upstream("oauth").await.is_empty());
        assert_eq!(pool.connection_count_for_tests().await, 0);
    }

    #[tokio::test]
    async fn ensure_tools_for_upstream_records_lazy_connect_failures() {
        let pool = UpstreamPool::new();
        let config = UpstreamConfig {
            url: Some("http://127.0.0.1:9/mcp".to_string()),
            command: None,
            ..named_test_upstream_config("broken")
        };
        pool.seed_lazy_upstreams(std::slice::from_ref(&config))
            .await;

        let err = pool
            .ensure_tools_for_upstream(&config, None, None)
            .await
            .expect_err("connect should fail");

        assert!(!err.to_string().is_empty());
        let last_error = pool
            .upstream_tool_last_error("broken")
            .await
            .expect("lazy failure is recorded");
        assert!(
            last_error.contains("lazy upstream connect"),
            "unexpected lazy connect error: {last_error}"
        );
    }

    #[tokio::test]
    async fn ensure_tools_for_upstream_preserves_other_resource_upstreams() {
        let pool = UpstreamPool::new();
        let mut alpha = named_test_upstream_config("alpha");
        alpha.proxy_resources = true;
        let mut beta = named_test_upstream_config("beta");
        beta.proxy_resources = true;
        pool.seed_lazy_upstreams(&[alpha.clone(), beta.clone()])
            .await;

        pool.ensure_tools_for_upstream_with_connector(
            &alpha,
            None,
            Arc::new(|_config| Box::pin(async { Ok((None, vec![test_tool("ping")])) })),
        )
        .await
        .expect("lazy connect succeeds");

        assert_eq!(
            *pool.resource_upstreams.read().await,
            vec!["alpha".to_string(), "beta".to_string()]
        );
    }

    #[tokio::test]
    async fn ensure_tools_for_upstream_refreshes_resource_cache() {
        let pool = UpstreamPool::new();
        let mut alpha = named_test_upstream_config("alpha");
        alpha.proxy_resources = true;
        pool.seed_lazy_upstreams(std::slice::from_ref(&alpha)).await;

        let connection = Arc::new(Mutex::new(Some(static_catalog_connection().await)));
        let connector: TestUpstreamConnector = Arc::new(move |_config| {
            let connection = Arc::clone(&connection);
            Box::pin(async move {
                let connection = connection
                    .lock()
                    .await
                    .take()
                    .expect("connector called once");
                Ok((Some(connection), vec![test_tool("ping")]))
            })
        });

        pool.ensure_tools_for_upstream_with_connector(&alpha, None, connector)
            .await
            .expect("lazy connect succeeds");

        assert_eq!(
            pool.cached_upstream_resource_uris().await,
            vec![(
                "alpha".to_string(),
                vec![
                    "file:///tmp/upstream-one".to_string(),
                    "lab://upstream/old-name/file:///tmp/upstream-two".to_string(),
                ],
            )]
        );
    }

    #[tokio::test]
    async fn ensure_tools_for_upstream_refreshes_stale_ui_resource_cache() {
        let pool = UpstreamPool::new();
        let mut alpha = named_test_upstream_config("alpha");
        alpha.proxy_resources = true;
        pool.seed_lazy_upstreams(std::slice::from_ref(&alpha)).await;

        let connection = static_catalog_connection().await;
        assert!(
            pool.install_connection_and_apply_entry("alpha".to_string(), connection, |_| {})
                .await
                .expect("bind test connection")
                .is_none()
        );
        let mut ui_tool = test_tool("quick_shell_ui");
        ui_tool.meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://quick-shell/component.html" }),
        )])));
        pool.install_test_tools_for_upstream(&alpha, vec![ui_tool])
            .await
            .expect("tools install");
        assert!(pool.cached_upstream_resource_uris().await.is_empty());

        pool.ensure_tools_for_upstream(&alpha, None, None)
            .await
            .expect("stale cache refresh succeeds");

        assert_eq!(
            pool.cached_upstream_resource_uris().await,
            vec![(
                "alpha".to_string(),
                vec![
                    "file:///tmp/upstream-one".to_string(),
                    "lab://upstream/old-name/file:///tmp/upstream-two".to_string(),
                ],
            )]
        );
    }

    /// Drive a lazy connect through the connector seam with a live static
    /// catalog peer, returning the pool for assertions.
    ///
    /// The seam and `ensure_tools_for_upstream` now share one post-connect
    /// step (`refresh_capability_caches_after_connect`), so exercising the seam
    /// exercises the production refresh — the two can no longer diverge the way
    /// they did for bead lab-zfyxk.
    async fn connect_via_seam(config: &UpstreamConfig) -> UpstreamPool {
        let pool = UpstreamPool::new();
        pool.seed_lazy_upstreams(std::slice::from_ref(config)).await;

        let connection = Arc::new(Mutex::new(Some(static_catalog_connection().await)));
        let connector: TestUpstreamConnector = Arc::new(move |_config| {
            let connection = Arc::clone(&connection);
            Box::pin(async move {
                let connection = connection
                    .lock()
                    .await
                    .take()
                    .expect("connector called once");
                Ok((Some(connection), vec![test_tool("ping")]))
            })
        });

        pool.ensure_tools_for_upstream_with_connector(config, None, connector)
            .await
            .expect("lazy connect succeeds");
        pool
    }

    #[tokio::test]
    async fn connecting_an_upstream_refreshes_its_prompt_cache() {
        let mut alpha = named_test_upstream_config("alpha");
        alpha.proxy_prompts = true;

        let pool = connect_via_seam(&alpha).await;

        assert_eq!(
            pool.cached_upstream_prompt_names_by_upstream().await,
            vec![(
                "alpha".to_string(),
                vec![
                    "alpha/upstream.prompt.one".to_string(),
                    "alpha/upstream.prompt.two".to_string(),
                ],
            )],
            "connecting an upstream must populate its prompt cache, not leave \
             discovered_prompt_count at 0 until a client calls prompts/list"
        );
    }

    /// A prompt circuit opened by earlier failures must not survive a reconnect.
    ///
    /// This is a latch, not just staleness: `routable_upstream_peers` filters
    /// open circuits out of the prompt listing fan-out, and that fan-out is the
    /// only place a prompt success is recorded — so without the reset, the
    /// listing that would close the circuit is exactly the one being skipped,
    /// forever. `is_open` has no quarantine expiry to rescue it. Asserting on
    /// the refreshed cache (rather than on the health field) is what makes this
    /// test fail if the reset is removed: a latched circuit yields an empty
    /// cache even though the upstream serves two prompts.
    #[tokio::test]
    async fn connecting_an_upstream_recovers_a_latched_prompt_circuit() {
        let mut alpha = named_test_upstream_config("alpha");
        alpha.proxy_prompts = true;

        let pool = UpstreamPool::new();
        pool.seed_lazy_upstreams(std::slice::from_ref(&alpha)).await;
        // Drive the circuit open the way repeated listing failures would.
        for _ in 0..3 {
            pool.record_failure_for(
                "alpha",
                UpstreamCapability::Prompts,
                "failed to list prompts from upstream: connection reset".to_string(),
            )
            .await;
        }
        assert!(
            !pool
                .upstream_capability_health("alpha", UpstreamCapability::Prompts)
                .await
                .expect("seeded entry exists")
                .is_routable(),
            "fixture must start with an open prompt circuit for this to be a regression"
        );

        let connection = Arc::new(Mutex::new(Some(static_catalog_connection().await)));
        let connector: TestUpstreamConnector = Arc::new(move |_config| {
            let connection = Arc::clone(&connection);
            Box::pin(async move {
                let connection = connection
                    .lock()
                    .await
                    .take()
                    .expect("connector called once");
                Ok((Some(connection), vec![test_tool("ping")]))
            })
        });
        pool.ensure_tools_for_upstream_with_connector(&alpha, None, connector)
            .await
            .expect("lazy connect succeeds");

        assert_eq!(
            pool.cached_upstream_prompt_names_by_upstream().await,
            vec![(
                "alpha".to_string(),
                vec![
                    "alpha/upstream.prompt.one".to_string(),
                    "alpha/upstream.prompt.two".to_string(),
                ],
            )],
            "a reconnect must clear the prompt circuit; otherwise the refresh is \
             filtered out of its own listing pass and the capability stays dark \
             permanently"
        );
    }

    #[tokio::test]
    async fn connecting_an_upstream_skips_prompts_when_proxying_is_off() {
        let alpha = named_test_upstream_config("alpha");
        assert!(!alpha.proxy_prompts, "fixture defaults prompt proxying off");

        let pool = connect_via_seam(&alpha).await;

        assert!(
            pool.cached_upstream_prompt_names_by_upstream()
                .await
                .is_empty(),
            "an upstream that does not proxy prompts must not be listed"
        );
    }

    async fn static_catalog_connection() -> UpstreamConnection {
        let (server_transport, client_transport) = tokio::io::duplex(IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = StaticCatalogServer::default()
                .serve(server_transport)
                .await
                .expect("static catalog server starts");
            running.waiting().await.expect("static catalog server runs");
        });
        let client_service: rmcp::service::RunningService<RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("static catalog client starts");
        let peer = client_service.peer().clone();
        UpstreamConnection::new(
            client_service,
            Some(server_task),
            peer,
            UpstreamRuntimeMetadata::default(),
        )
    }
}
