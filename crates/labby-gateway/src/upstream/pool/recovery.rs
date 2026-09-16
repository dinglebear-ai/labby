//! Per-upstream recovery and incarnation-fenced tool catalog publication.

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::{UpstreamCapability, UpstreamRuntimeOwner};
use super::UpstreamPool;
use super::helpers::cached_upstream_tool;
use super::incarnation::ObservedConnectionCatalogEntry;

impl UpstreamPool {
    /// Restart the selected runtime without changing its desired enabled state.
    /// The manager owns the task so a disconnected operator request cannot
    /// interrupt the stop/start transaction.
    ///
    /// `between_stop_and_start` runs after the owned connection has been shut
    /// down and before the replacement connects, while the per-upstream
    /// connect gate is still held. The manager uses it to reap stale runtime
    /// processes from earlier generations: running it after the reconnect
    /// would match the replacement child, and running it before the shutdown
    /// would race the graceful process-group teardown. Its output is returned
    /// unchanged so the caller can report it alongside the restart.
    pub async fn restart_upstream<C, F>(
        &self,
        config: &UpstreamConfig,
        oauth_subject: Option<&str>,
        owner: Option<&UpstreamRuntimeOwner>,
        between_stop_and_start: C,
    ) -> anyhow::Result<F::Output>
    where
        C: FnOnce() -> F,
        F: Future,
    {
        anyhow::ensure!(config.enabled, "upstream `{}` is disabled", config.name);
        // A runtime swap may publish a fresh pool before its normal lazy-seed
        // reconciliation has run. Restart remains self-contained in that
        // window instead of failing because the catalog row is absent.
        self.ensure_lazy_upstream_entry(config).await;
        let gate = self.lazy_connect_lock(&config.name).await;
        let _guard = gate.lock().await;
        anyhow::ensure!(
            self.lazy_connect_gate_is_current(&config.name, &gate).await,
            "upstream configuration changed before restart"
        );
        if config.oauth.is_some()
            && let Some(subject) = oauth_subject
        {
            self.invalidate_oauth_subject_sessions(&config.name, subject, "upstream.restart")
                .await;
            let between = between_stop_and_start().await;
            self.acquire_or_connect_subject(config, subject).await?;
            return Ok(between);
        }
        self.begin_subscription_generation(&config.name).await;
        if let Some(connection) = self.remove_connection_binding(&config.name).await {
            connection.shutdown(&config.name, "upstream.restart").await;
        }
        let between = between_stop_and_start().await;
        self.reprobe_upstream(config, oauth_subject, owner).await?;
        Ok(between)
    }

    pub(super) async fn lazy_connect_gate_is_current(
        &self,
        upstream: &str,
        gate: &Arc<tokio::sync::Mutex<()>>,
    ) -> bool {
        self.lazy_connect_locks
            .read()
            .await
            .get(upstream)
            .is_some_and(|current| Arc::ptr_eq(current, gate))
    }

    pub(super) async fn publish_observed_tools(
        &self,
        observed: &ObservedConnectionCatalogEntry,
        tools: Vec<rmcp::model::Tool>,
    ) -> bool {
        let name: Arc<str> = Arc::from(observed.upstream());
        let tools = tools
            .into_iter()
            .map(|tool| cached_upstream_tool(tool, &name))
            .collect::<HashMap<_, _>>();
        self.apply_to_observed_entry(observed, |entry| {
            entry.tools = tools;
            super::health::record_success_on_entry(
                observed.upstream(),
                entry,
                UpstreamCapability::Tools,
            );
        })
        .await
        .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::super::testsupport::{StaticCatalogServer, catalog_pool_with_server, test_tool};

    #[tokio::test]
    async fn late_tool_catalog_cannot_overwrite_replacement() {
        let pool = catalog_pool_with_server("server", StaticCatalogServer::default()).await;
        let old = pool
            .observe_connection_catalog_entry("server")
            .await
            .unwrap();
        let replacement = catalog_pool_with_server("server", StaticCatalogServer::default()).await;
        let (connection, entry) = replacement.remove_connection_catalog_entry("server").await;
        pool.install_connection_catalog_entry("server".into(), connection.unwrap(), entry.unwrap())
            .await
            .unwrap();
        let current = pool
            .observe_connection_catalog_entry("server")
            .await
            .unwrap();
        assert!(
            pool.publish_observed_tools(&current, vec![test_tool("current")])
                .await
        );
        assert!(
            !pool
                .publish_observed_tools(&old, vec![test_tool("obsolete")])
                .await
        );
        assert!(pool.observed_entry_is_current(&current).await);
        let tools = pool.healthy_tools_for_upstream("server").await;
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool.name.as_ref(), "current");
    }
}

#[cfg(test)]
mod catalog_isolation_tests {
    use super::super::testsupport::catalog_pool_with_server;
    use super::UpstreamPool;
    use rmcp::model::{
        ErrorData, ListPromptsResult, ListResourcesResult, PaginatedRequestParams, Prompt,
        Resource, ServerCapabilities, ServerInfo,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};
    use std::sync::Arc;

    struct ManyCatalogs;
    impl ServerHandler for ManyCatalogs {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(
                ServerCapabilities::builder()
                    .enable_resources()
                    .enable_prompts()
                    .build(),
            )
        }
        async fn list_resources(
            &self,
            _: Option<PaginatedRequestParams>,
            _: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult::with_all_items(
                (0..600)
                    .map(|i| Resource::new(format!("test://{i}"), format!("r{i}")))
                    .collect(),
            ))
        }
        async fn list_prompts(
            &self,
            _: Option<PaginatedRequestParams>,
            _: RequestContext<RoleServer>,
        ) -> Result<ListPromptsResult, ErrorData> {
            Ok(ListPromptsResult::with_all_items(
                (0..600)
                    .map(|i| Prompt::new(format!("p{i}"), None::<String>, None))
                    .collect(),
            ))
        }
    }

    async fn two_catalogs() -> Arc<UpstreamPool> {
        let pool = catalog_pool_with_server("alpha", ManyCatalogs).await;
        let second = catalog_pool_with_server("beta", ManyCatalogs).await;
        let (connection, entry) = second.remove_connection_catalog_entry("beta").await;
        pool.install_connection_catalog_entry("beta".into(), connection.unwrap(), entry.unwrap())
            .await
            .unwrap();
        pool.resource_upstreams.write().await.push("beta".into());
        pool
    }

    #[tokio::test]
    async fn fleet_resource_limit_never_poison_other_servers_health_or_counts() {
        let pool = two_catalogs().await;
        let resources = pool.list_upstream_resources_allowed(None).await;
        assert_eq!(resources.len(), 1_000, "merged envelope stays bounded");
        let catalog = pool.catalog.read().await;
        for name in ["alpha", "beta"] {
            let entry = catalog.get(name).unwrap();
            assert_eq!(
                entry.resource_count, 600,
                "{name} retains its complete source count"
            );
            assert_eq!(entry.resource_uris.len(), 600);
            assert!(entry.resource_last_error.is_none());
            assert!(entry.resource_health.is_routable());
        }
    }

    #[tokio::test]
    async fn fleet_prompt_limit_never_poison_other_servers_health_or_counts() {
        let pool = two_catalogs().await;
        let prompts = pool.list_upstream_prompts(&[]).await;
        assert_eq!(prompts.len(), 1_000, "merged envelope stays bounded");
        let catalog = pool.catalog.read().await;
        for name in ["alpha", "beta"] {
            let entry = catalog.get(name).unwrap();
            assert_eq!(
                entry.prompt_count, 600,
                "{name} retains its complete source count"
            );
            assert_eq!(entry.prompt_names.len(), 600);
            assert!(entry.prompt_last_error.is_none());
            assert!(entry.prompt_health.is_routable());
        }
    }
}

#[cfg(all(test, unix))]
mod process_cleanup_tests {
    use super::super::testsupport::{StaticCatalogServer, catalog_pool_with_server};
    use std::time::Duration;
    use tokio::process::Command;

    #[tokio::test]
    async fn shutdown_reaps_group_members_after_leader_exit() {
        let mut leader = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pgid = leader.id().unwrap();
        let mut member = Command::new("sleep")
            .arg("60")
            .process_group(pgid as i32)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        leader.kill().await.unwrap();
        leader.wait().await.unwrap();
        assert!(member.try_wait().unwrap().is_none());
        let pool = catalog_pool_with_server("process", StaticCatalogServer::default()).await;
        let mut connection = pool.remove_connection_binding("process").await.unwrap();
        connection.runtime.pid = Some(pgid);
        connection.runtime.pgid = Some(pgid);
        connection.shutdown("process", "test.leader_exited").await;
        tokio::time::timeout(Duration::from_secs(2), member.wait())
            .await
            .expect("remaining group member must exit after graceful shutdown")
            .unwrap();
    }

    #[tokio::test]
    async fn cancelled_shutdown_keeps_process_group_cleanup_owner() {
        use std::process::Stdio;
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut member = Command::new("sh")
            .args(["-c", "trap '' TERM; echo ready; exec sleep 60"])
            .stdout(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pgid = member.id().unwrap();
        let mut output = BufReader::new(member.stdout.take().unwrap()).lines();
        assert_eq!(output.next_line().await.unwrap().as_deref(), Some("ready"));
        let pool = catalog_pool_with_server("cancel", StaticCatalogServer::default()).await;
        let mut connection = pool.remove_connection_binding("cancel").await.unwrap();
        connection.runtime.pid = Some(pgid);
        connection.runtime.pgid = Some(pgid);
        // Closing this already-live local session reaches the graceful TERM
        // delay. The member ignores TERM, so only cancellation Drop can reap it.
        let result = tokio::time::timeout(
            Duration::from_millis(50),
            connection.shutdown("cancel", "test.cancel_shutdown"),
        )
        .await;
        assert!(
            result.is_err(),
            "shutdown remains in its graceful cleanup window"
        );
        tokio::time::timeout(Duration::from_secs(2), member.wait())
            .await
            .expect("cancelling shutdown must still SIGKILL the owned group")
            .unwrap();
    }
}
