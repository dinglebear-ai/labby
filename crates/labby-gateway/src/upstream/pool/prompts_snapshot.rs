//! Cache-backed prompt metadata for callers that must not fan out live.
//!
//! Code Mode builds its discovery catalog on every run and every sandbox
//! `search()`. A live `prompts/list` fan-out there puts the slowest upstream's
//! prompt listing on every run's latency floor, and one upstream that stalls on
//! `prompts/list` while serving tools normally can time out every run. Such
//! callers read [`UpstreamPool::cached_prompt_routes_allowed`] instead.
//!
//! Freshness: an upstream's prompt source is re-listed on connect, on
//! `prompts/list_changed`, and whenever a client calls MCP `prompts/list`.
//! An upstream that was never listed on its current connection gets a
//! single-flight warm-up with the same bounded cold wait as resources. A failed
//! warm-up is retried only after [`PROMPT_WARMUP_RETRY_AFTER`], so an upstream
//! whose `prompts/list` keeps failing does not cost every run the cold wait.
//! OAuth subject-scoped prompts are not cached here.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;

use super::helpers::{
    COLD_SNAPSHOT_WAIT, RESOURCE_SNAPSHOT_MAX_AGE, upstream_discovery_concurrency,
};
use super::incarnation::ConnectionIncarnation;
use super::resources_list::catalog_listing_timeout;
use super::{PublishedPromptRoute, UpstreamPool};

/// How long a failed prompt warm-up suppresses another attempt on the same
/// connection. Matches the resource snapshot age bound.
const PROMPT_WARMUP_RETRY_AFTER: Duration = RESOURCE_SNAPSHOT_MAX_AGE;

impl UpstreamPool {
    /// Connected, prompt-routable upstreams in `allowed` with no settled prompt
    /// listing for their current connection, keyed to that connection.
    /// Allocates only for cold names.
    async fn cold_prompt_snapshots(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> BTreeMap<String, ConnectionIncarnation> {
        // Same lock order as `observe_routable_prompt_connections`.
        let _binding = self.connection_catalog_binding.read().await;
        let connections = self.connections.read().await;
        let catalog = self.catalog.read().await;
        catalog
            .iter()
            .filter(|(name, entry)| {
                allowed.is_none_or(|allowed| allowed.contains(*name))
                    && entry
                        .health_for(super::super::types::UpstreamCapability::Prompts)
                        .is_routable()
            })
            .filter_map(|(name, _)| {
                let incarnation = connections.get(name)?.incarnation?;
                (catalog.incarnation(name) == Some(incarnation)
                    && !catalog.prompt_snapshot_is_current(name, incarnation))
                .then(|| (name.clone(), incarnation))
            })
            .collect()
    }

    /// Cold upstreams minus those whose warm-up failed recently on the same
    /// connection.
    async fn prompt_snapshots_to_warm(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> BTreeMap<String, ConnectionIncarnation> {
        let mut cold = self.cold_prompt_snapshots(allowed).await;
        if cold.is_empty() {
            return cold;
        }
        let mut failures = self
            .prompt_warmup_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        failures.retain(|_, (_, at)| at.elapsed() < PROMPT_WARMUP_RETRY_AFTER);
        cold.retain(|name, incarnation| {
            failures
                .get(name)
                .is_none_or(|(failed_on, _)| failed_on != incarnation)
        });
        cold
    }

    /// List cold upstreams once each; overlapping callers share one flight and
    /// recheck after acquiring it. A listing that leaves the upstream cold is
    /// recorded so later runs skip it until the retry window passes.
    async fn warm_cold_prompt_snapshots(&self, names: BTreeSet<String>) {
        let mut work = futures::stream::iter(names)
            .map(|name| async move {
                let flight = self.prompt_snapshot_warmup.acquire(&name).await;
                let _turn = flight.lock().await;
                let names = BTreeSet::from([name]);
                if self.prompt_snapshots_to_warm(Some(&names)).await.is_empty() {
                    return;
                }
                let deadline_at =
                    tokio::time::Instant::now() + catalog_listing_timeout(self.request_timeout);
                drop(
                    self.list_upstream_prompts_allowed_until(&[], Some(&names), deadline_at)
                        .await,
                );
                let still_cold = self.cold_prompt_snapshots(Some(&names)).await;
                let mut failures = self
                    .prompt_warmup_failures
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                for upstream in names {
                    let Some(incarnation) = still_cold.get(&upstream) else {
                        failures.remove(&upstream);
                        continue;
                    };
                    tracing::warn!(
                        surface = "dispatch",
                        service = "upstream.pool",
                        action = "catalog.prompts.warm",
                        upstream = %upstream,
                        retry_after_ms = PROMPT_WARMUP_RETRY_AFTER.as_millis(),
                        "prompt snapshot warm-up did not settle a listing; cached prompt metadata omits this upstream until a later listing succeeds"
                    );
                    failures.insert(upstream, (*incarnation, Instant::now()));
                }
            })
            .buffer_unordered(upstream_discovery_concurrency(None));
        while work.next().await.is_some() {}
    }

    /// Start a detached warm-up for never-listed prompt snapshots in `allowed`
    /// and wait at most the 250 ms `COLD_SNAPSHOT_WAIT` for it. The task always runs to
    /// completion so a short caller wait never cuts a listing short of
    /// publishing its rows or recording its failure.
    pub async fn ensure_prompt_snapshots_allowed(
        self: &Arc<Self>,
        allowed: Option<&BTreeSet<String>>,
    ) {
        let cold = self.prompt_snapshots_to_warm(allowed).await;
        if cold.is_empty() {
            return;
        }
        let pool = Arc::clone(self);
        let names = cold.into_keys().collect();
        let task = tokio::spawn(async move { pool.warm_cold_prompt_snapshots(names).await });
        if let Ok(Err(error)) =
            tokio::time::timeout(self.request_timeout.min(COLD_SNAPSHOT_WAIT), task).await
        {
            tracing::warn!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "catalog.prompts.warm",
                error = %error,
                "prompt snapshot warm-up task failed"
            );
        }
    }

    /// Exposed prompts from each routable upstream's settled listing, filtered
    /// to `allowed`. Issues no upstream RPC. Degrades per upstream: one
    /// upstream whose listing was rejected is skipped instead of hiding every
    /// other upstream's prompts, and the fleet caps truncate.
    pub async fn cached_prompt_routes_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<PublishedPromptRoute> {
        let (routes, skipped) = self.catalog.read().await.degraded_prompt_routes(allowed);
        if !skipped.is_empty() {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "catalog.prompts.cached",
                skipped = ?skipped,
                "cached prompt metadata omits upstreams whose listing was rejected or truncated"
            );
        }
        routes
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::super::testsupport::StaticCatalogServer;
    use super::*;

    #[tokio::test]
    async fn seeded_but_unconnected_upstreams_are_not_cold() {
        // Only a live connection whose incarnation lacks a settled listing is
        // cold; the warm-up must never try to list an unconnected upstream.
        let pool = Arc::new(UpstreamPool::new());
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![rmcp::model::Prompt::new("brief", Some("Alpha brief"), None)],
        )
        .await;
        assert!(pool.cold_prompt_snapshots(None).await.is_empty());
        pool.ensure_prompt_snapshots_allowed(None).await;
        let routes = pool.cached_prompt_routes_allowed(None).await;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].upstream_name.as_ref(), "alpha");
    }

    #[tokio::test]
    async fn cold_upstream_is_listed_once_and_then_served_from_cache() {
        let pool = Arc::new(UpstreamPool::new());
        let server = StaticCatalogServer::default();
        pool.install_prompt_server_for_tests("alpha", server.clone())
            .await;
        assert_eq!(pool.cold_prompt_snapshots(None).await.len(), 1);

        pool.ensure_prompt_snapshots_allowed(None).await;
        let routes = pool.cached_prompt_routes_allowed(None).await;
        let names = routes
            .iter()
            .map(|route| route.native_name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(names, ["upstream.prompt.one", "upstream.prompt.two"]);
        assert_eq!(server.list_prompts_count.load(Ordering::SeqCst), 1);

        // Settled: a second run reads the cache and issues no RPC.
        pool.ensure_prompt_snapshots_allowed(None).await;
        assert_eq!(pool.cached_prompt_routes_allowed(None).await.len(), 2);
        assert_eq!(server.list_prompts_count.load(Ordering::SeqCst), 1);

        let other = BTreeSet::from(["beta".to_string()]);
        assert!(
            pool.cached_prompt_routes_allowed(Some(&other))
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn prompt_list_changed_relists_the_cached_source() {
        let pool = Arc::new(UpstreamPool::new());
        let server = StaticCatalogServer::default();
        pool.install_prompt_server_for_tests("alpha", server.clone())
            .await;
        pool.ensure_prompt_snapshots_allowed(None).await;
        assert_eq!(server.list_prompts_count.load(Ordering::SeqCst), 1);

        pool.refresh_after_list_changed("alpha", super::super::ListChangedKinds::PROMPTS)
            .await;
        assert_eq!(
            server.list_prompts_count.load(Ordering::SeqCst),
            2,
            "prompts/list_changed must refresh the cached prompt source"
        );
        assert_eq!(pool.cached_prompt_routes_allowed(None).await.len(), 2);
    }

    #[tokio::test]
    async fn failed_warm_up_is_not_retried_on_every_run() {
        let pool = Arc::new(UpstreamPool::new());
        let server = StaticCatalogServer::default();
        server.fail_list_prompts.store(true, Ordering::SeqCst);
        pool.install_prompt_server_for_tests("alpha", server.clone())
            .await;

        pool.ensure_prompt_snapshots_allowed(None).await;
        let after_first = server.list_prompts_count.load(Ordering::SeqCst);
        assert_eq!(after_first, 1);
        assert!(pool.cached_prompt_routes_allowed(None).await.is_empty());
        // One failure is below the breaker threshold, so only the backoff can
        // keep later runs from re-listing.
        assert!(
            pool.catalog
                .read()
                .await
                .get("alpha")
                .is_some_and(|entry| entry.prompt_health.is_routable())
        );

        for _ in 0..3 {
            pool.ensure_prompt_snapshots_allowed(None).await;
        }
        assert_eq!(
            server.list_prompts_count.load(Ordering::SeqCst),
            after_first,
            "a failed warm-up must back off instead of re-listing every run"
        );
    }

    #[tokio::test]
    async fn rejected_upstream_listing_does_not_hide_other_upstreams() {
        let pool = UpstreamPool::new();
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![rmcp::model::Prompt::new("brief", Some("Alpha brief"), None)],
        )
        .await;
        // Duplicate names fail admission, which fails the published projection
        // for the whole fleet.
        pool.insert_prompt_routes_for_tests(
            "beta",
            vec![
                rmcp::model::Prompt::new("dup", Some("one"), None),
                rmcp::model::Prompt::new("dup", Some("two"), None),
            ],
        )
        .await;
        assert!(pool.published_prompt_catalog().await.is_err());

        let routes = pool.cached_prompt_routes_allowed(None).await;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].upstream_name.as_ref(), "alpha");
    }
}
