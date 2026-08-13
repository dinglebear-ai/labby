//! In-process service-peer registration.
//!
//! Built-in lab services are exposed to the gateway as in-process upstream peers
//! over an in-memory transport. These methods register each service concurrently
//! (isolating slow/failing peers), populate the catalog, and record failures as
//! degraded entries. The `InProcessConnector`/`InProcessRegistration` types stay
//! defined in `pool.rs`; this descendant module sees them without annotation.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use futures::stream::FuturesUnordered;

use crate::registry::{InProcessService, InProcessServiceRegistry};

use super::entries::{
    failed_in_process_entry, failed_in_process_entry_from_existing, healthy_in_process_entry,
};
use super::helpers::{
    IN_PROCESS_DISCOVERY_TIMEOUT, cached_upstream_tool, in_process_upstream_name,
};
use super::{InProcessConnector, UpstreamPool};

/// Minimum interval between registration attempts for peers that stay
/// missing/failed. Mirrors the shape of `SEMANTIC_SEARCH_COOLDOWN`: long
/// enough that a broken builtin is not re-spawned on every `codemode.search`,
/// short enough that recovery lands within a working session.
const IN_PROCESS_ENSURE_RETRY_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(30);

impl UpstreamPool {
    pub async fn register_in_process_service_peers(&self, registry: &dyn InProcessServiceRegistry) {
        let services: Vec<Box<dyn InProcessService>> = registry
            .in_process_services()
            .into_iter()
            .filter(|service| service.has_actions())
            .collect();
        self.register_in_process_service_list(services).await;
    }

    /// Idempotent variant for hot paths: register only services whose
    /// in-process entry is missing, circuit-open, or empty. (A re-registration
    /// replaces the entry via `healthy_in_process_entry`, which also resets
    /// breaker state — deliberate for local peers, which self-heal.) Safe to
    /// call on every Code Mode catalog build:
    ///
    /// - Single-flight: concurrent builds serialize on the ensure mutex, so N
    ///   simultaneous cold misses spawn each mini-server once, not N times —
    ///   and never replace a just-registered live connection (which would
    ///   abort in-flight builtin calls).
    /// - Failure cooldown: an attempt that still leaves peers missing is not
    ///   retried within [`IN_PROCESS_ENSURE_RETRY_COOLDOWN`], so a wedged
    ///   peer costs one bounded attempt per window instead of adding up to
    ///   `IN_PROCESS_DISCOVERY_TIMEOUT` to every catalog build.
    pub async fn ensure_in_process_service_peers(&self, registry: &dyn InProcessServiceRegistry) {
        let mut last_attempt = self.in_process_ensure_state.lock().await;
        let missing: Vec<Box<dyn InProcessService>> = {
            let catalog = self.catalog.read().await;
            registry
                .in_process_services()
                .into_iter()
                .filter(|service| service.has_actions())
                .filter(|service| {
                    let upstream_name = in_process_upstream_name(service.service_name());
                    !catalog.get(&upstream_name).is_some_and(|entry| {
                        entry.tool_health.is_routable() && !entry.tools.is_empty()
                    })
                })
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        if last_attempt
            .is_some_and(|attempted| attempted.elapsed() < IN_PROCESS_ENSURE_RETRY_COOLDOWN)
        {
            return;
        }
        *last_attempt = Some(std::time::Instant::now());
        self.register_in_process_service_list(missing).await;
    }

    async fn register_in_process_service_list(&self, services: Vec<Box<dyn InProcessService>>) {
        let Some(connector) = self.in_process_connector.clone() else {
            tracing::warn!(
                service_count = services.len(),
                "in-process peer registration skipped: no connector was provided by the surface"
            );
            return;
        };
        self.register_in_process_service_list_with_connector(services, connector)
            .await;
    }

    async fn register_in_process_service_list_with_connector(
        &self,
        services: Vec<Box<dyn InProcessService>>,
        connector: InProcessConnector,
    ) {
        let mut in_process_resource_names = Vec::new();
        let mut futures = FuturesUnordered::new();
        let mut failed_count = 0usize;
        let mut timeout_count = 0usize;

        for service in services {
            let service_name = service.service_name();
            let upstream_name = in_process_upstream_name(service_name);
            tracing::info!(
                upstream = %upstream_name,
                service = service_name,
                timeout_secs = IN_PROCESS_DISCOVERY_TIMEOUT.as_secs(),
                "starting in-process peer registration"
            );
            let connector = Arc::clone(&connector);
            futures.push(async move {
                let result =
                    tokio::time::timeout(IN_PROCESS_DISCOVERY_TIMEOUT, connector(service)).await;
                (service_name, upstream_name, result)
            });
        }

        while let Some((service_name, upstream_name, result)) = futures.next().await {
            match result {
                Ok(Ok(registration)) => {
                    let mut tool_map = HashMap::new();
                    let tool_count = registration.tools.len();
                    for tool in registration.tools {
                        tool_map.insert(
                            tool.name.to_string(),
                            cached_upstream_tool(tool, &registration.entry_name).1,
                        );
                    }

                    self.catalog.write().await.insert(
                        registration.upstream_name.clone(),
                        healthy_in_process_entry(Arc::clone(&registration.entry_name), tool_map),
                    );
                    if let Some(conn) = registration.connection {
                        self.connections
                            .write()
                            .await
                            .insert(registration.upstream_name.clone(), conn);
                    }
                    in_process_resource_names.push(registration.upstream_name.clone());
                    if tool_count == 0 {
                        // A zero-tool "success" re-qualifies as missing on the
                        // next ensure pass, so without this signal the retry
                        // loop is silent churn (review finding on lab-48z4k).
                        tracing::warn!(
                            upstream = %registration.entry_name,
                            service = service_name,
                            "in-process peer registered ZERO tools — it will be re-registered after the ensure cooldown"
                        );
                    } else {
                        tracing::info!(
                            upstream = %registration.entry_name,
                            service = service_name,
                            tool_count,
                            resource_count = 0,
                            prompt_count = 0,
                            "in-process peer registration succeeded"
                        );
                    }
                }
                Ok(Err(error)) => {
                    failed_count += 1;
                    let error_message =
                        format!("failed to register in-process service peer: {error}");
                    tracing::warn!(
                        upstream = %upstream_name,
                        service = service_name,
                        error = %error_message,
                        "in-process peer registration failed"
                    );
                    let mut catalog = self.catalog.write().await;
                    let name: Arc<str> = Arc::from(upstream_name.as_str());
                    let entry = catalog
                        .remove(&upstream_name)
                        .map(|existing| {
                            failed_in_process_entry_from_existing(existing, error_message.clone())
                        })
                        .unwrap_or_else(|| failed_in_process_entry(name, error_message));
                    catalog.insert(upstream_name, entry);
                }
                Err(_) => {
                    failed_count += 1;
                    timeout_count += 1;
                    let error_message = format!(
                        "in-process peer registration timed out after {}s",
                        IN_PROCESS_DISCOVERY_TIMEOUT.as_secs()
                    );
                    tracing::warn!(
                        upstream = %upstream_name,
                        service = service_name,
                        timeout_secs = IN_PROCESS_DISCOVERY_TIMEOUT.as_secs(),
                        error = %error_message,
                        "in-process peer registration timed out"
                    );
                    let mut catalog = self.catalog.write().await;
                    let name: Arc<str> = Arc::from(upstream_name.as_str());
                    let entry = catalog
                        .remove(&upstream_name)
                        .map(|existing| {
                            failed_in_process_entry_from_existing(existing, error_message.clone())
                        })
                        .unwrap_or_else(|| failed_in_process_entry(name, error_message));
                    catalog.insert(upstream_name, entry);
                }
            }
        }

        if !in_process_resource_names.is_empty() {
            let mut resource_upstreams = self.resource_upstreams.write().await;
            resource_upstreams.extend(in_process_resource_names);
            resource_upstreams.sort_unstable();
            resource_upstreams.dedup();
        }

        if failed_count > 0 {
            tracing::warn!(
                failed_count,
                timeout_count,
                "in-process peer registration completed with degraded services"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use std::any::Any;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::registry::InProcessService;

    use super::super::helpers::in_process_upstream_name;
    use super::super::{InProcessConnector, InProcessRegistration};
    use super::*;

    /// Minimal in-process service stub for registration tests — carries only the
    /// name/has-actions that the pool seam reads, decoupled from Labby's
    /// `RegisteredService`.
    struct StubService {
        name: &'static str,
    }

    impl InProcessService for StubService {
        fn service_name(&self) -> &'static str {
            self.name
        }

        fn has_actions(&self) -> bool {
            true
        }

        fn as_any(self: Box<Self>) -> Box<dyn Any + Send> {
            self
        }
    }

    fn service(name: &'static str) -> Box<dyn InProcessService> {
        Box::new(StubService { name })
    }

    struct StubRegistry(&'static [&'static str]);

    impl crate::registry::InProcessServiceRegistry for StubRegistry {
        fn in_process_services(&self) -> Vec<Box<dyn InProcessService>> {
            self.0.iter().map(|name| service(name)).collect()
        }
    }

    /// FU-1 (issue #210, lab-48z4k): `ensure_in_process_service_peers` runs on
    /// every Code Mode catalog build, so it must not re-spawn mini-servers or
    /// churn connections once a healthy, non-empty entry exists.
    #[tokio::test]
    async fn ensure_in_process_service_peers_is_idempotent() {
        use futures::future::BoxFuture;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let connect_count = Arc::new(AtomicUsize::new(0));
        let connect_count_for_connector = Arc::clone(&connect_count);
        let connector: InProcessConnector = Arc::new(move |service| {
            let connect_count = Arc::clone(&connect_count_for_connector);
            let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                Box::pin(async move {
                    connect_count.fetch_add(1, Ordering::SeqCst);
                    let upstream_name: Arc<str> =
                        Arc::from(in_process_upstream_name(service.service_name()));
                    Ok(InProcessRegistration {
                        connection: None,
                        tools: vec![rmcp::model::Tool::new(
                            "gateway-alpha",
                            "Gateway alpha",
                            Arc::new(serde_json::Map::new()),
                        )],
                        entry_name: Arc::clone(&upstream_name),
                        upstream_name: upstream_name.to_string(),
                    })
                });
            future
        });
        let pool = UpstreamPool::new().with_in_process_connector(connector);
        let registry = StubRegistry(&["gateway-alpha"]);

        pool.ensure_in_process_service_peers(&registry).await;
        pool.ensure_in_process_service_peers(&registry).await;

        assert_eq!(
            connect_count.load(Ordering::SeqCst),
            1,
            "a healthy, non-empty peer entry must not be re-registered"
        );
        let tools = pool.healthy_tools_allowed(None).await;
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].upstream_name.as_ref(),
            in_process_upstream_name("gateway-alpha")
        );
    }

    /// Review fix (lab-48z4k): a peer that keeps failing must not be
    /// re-registered on every catalog build — one attempt per cooldown
    /// window, so a wedged builtin cannot put its connect timeout on every
    /// `codemode.search`.
    #[tokio::test]
    async fn ensure_in_process_service_peers_applies_failure_cooldown() {
        use futures::future::BoxFuture;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let connect_count = Arc::new(AtomicUsize::new(0));
        let connect_count_for_connector = Arc::clone(&connect_count);
        let connector: InProcessConnector = Arc::new(move |_service| {
            let connect_count = Arc::clone(&connect_count_for_connector);
            let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                Box::pin(async move {
                    connect_count.fetch_add(1, Ordering::SeqCst);
                    anyhow::bail!("mini-server refuses to start");
                });
            future
        });
        let pool = UpstreamPool::new().with_in_process_connector(connector);
        let registry = StubRegistry(&["gateway-alpha"]);

        pool.ensure_in_process_service_peers(&registry).await;
        pool.ensure_in_process_service_peers(&registry).await;
        pool.ensure_in_process_service_peers(&registry).await;

        assert_eq!(
            connect_count.load(Ordering::SeqCst),
            1,
            "a failing peer is retried at most once per cooldown window"
        );
    }

    #[tokio::test]
    async fn in_process_registration_isolates_slow_services_from_fast_services() {
        use futures::future::BoxFuture;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let pool = UpstreamPool::new();
        let fast_seen = Arc::new(AtomicUsize::new(0));
        let fast_seen_for_connector = Arc::clone(&fast_seen);
        let connector: InProcessConnector = Arc::new(move |service| {
            let fast_seen = Arc::clone(&fast_seen_for_connector);
            let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                Box::pin(async move {
                    if service.service_name() == "slow" {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        anyhow::bail!("slow service failed to start");
                    }

                    fast_seen.fetch_add(1, Ordering::SeqCst);
                    let upstream_name: Arc<str> =
                        Arc::from(in_process_upstream_name(service.service_name()));
                    Ok(InProcessRegistration {
                        connection: None,
                        tools: Vec::new(),
                        entry_name: Arc::clone(&upstream_name),
                        upstream_name: upstream_name.to_string(),
                    })
                });
            future
        });

        let registration = tokio::spawn({
            let pool = pool.clone();
            async move {
                pool.register_in_process_service_list_with_connector(
                    vec![service("slow"), service("fast")],
                    connector,
                )
                .await;
            }
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(
            fast_seen.load(Ordering::SeqCst),
            1,
            "fast service should register before slow service finishes"
        );

        registration.await.expect("registration task");
        assert_eq!(pool.upstream_count().await, 2);
    }

    #[tokio::test]
    async fn failed_in_process_registration_does_not_hide_healthy_peer_tools() {
        use futures::future::BoxFuture;

        let pool = UpstreamPool::new();
        let connector: InProcessConnector = Arc::new(|service| {
            let future: BoxFuture<'static, anyhow::Result<InProcessRegistration>> =
                Box::pin(async move {
                    if service.service_name() == "bad" {
                        anyhow::bail!("bad service failed to start");
                    }

                    let upstream_name: Arc<str> =
                        Arc::from(in_process_upstream_name(service.service_name()));
                    let tool = rmcp::model::Tool::new(
                        "status.read",
                        "Read status",
                        Arc::new(serde_json::Map::new()),
                    );
                    Ok(InProcessRegistration {
                        connection: None,
                        tools: vec![tool],
                        entry_name: Arc::clone(&upstream_name),
                        upstream_name: upstream_name.to_string(),
                    })
                });
            future
        });

        pool.register_in_process_service_list_with_connector(
            vec![service("bad"), service("good")],
            connector,
        )
        .await;

        let good_tools = pool
            .healthy_tools_for_upstream(&in_process_upstream_name("good"))
            .await;
        let bad_tools = pool
            .healthy_tools_for_upstream(&in_process_upstream_name("bad"))
            .await;

        assert_eq!(good_tools.len(), 1);
        assert_eq!(good_tools[0].tool.name.as_ref(), "status.read");
        assert!(bad_tools.is_empty());
        assert_eq!(pool.upstream_count().await, 2);
    }
}
