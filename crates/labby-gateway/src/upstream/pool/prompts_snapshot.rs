//! Bounded warm-up for the published prompt snapshot.
//!
//! Code Mode builds its discovery catalog on every run and every sandbox
//! `search()`. A live `prompts/list` fan-out there puts the slowest upstream's
//! prompt listing on every run's latency floor, and one upstream that stalls on
//! `prompts/list` while serving tools normally can time out every run. Callers
//! read `published_prompt_catalog()` instead; this module gives never-listed
//! upstreams a single-flight warm-up with a bounded wait, matching the resource
//! contract (`ensure_resource_snapshots_allowed`).

use std::collections::BTreeSet;
use std::sync::Arc;

use futures::StreamExt as _;

use super::UpstreamPool;
use super::helpers::{COLD_SNAPSHOT_WAIT, snapshot_flight, upstream_discovery_concurrency};
use super::resources_list::catalog_listing_timeout;

impl UpstreamPool {
    /// Connected, prompt-routable upstreams in `allowed` with no settled prompt
    /// listing for their current connection. Allocates only for cold names.
    pub(super) async fn cold_prompt_snapshots(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> BTreeSet<String> {
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
            .filter(|(name, _)| {
                connections
                    .get(*name)
                    .and_then(|connection| connection.incarnation)
                    .is_some_and(|incarnation| {
                        catalog.incarnation(name) == Some(incarnation)
                            && !catalog.prompt_snapshot_is_current(name, incarnation)
                    })
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// List cold upstreams once each; overlapping callers share one flight and
    /// recheck after acquiring it.
    async fn warm_cold_prompt_snapshots(&self, names: BTreeSet<String>) {
        let mut work = futures::stream::iter(names)
            .map(|name| async move {
                let flight = snapshot_flight(&self.prompt_snapshot_warmup, &name).await;
                let _turn = flight.lock().await;
                let names = BTreeSet::from([name]);
                if self.cold_prompt_snapshots(Some(&names)).await.is_empty() {
                    return;
                }
                let deadline_at =
                    tokio::time::Instant::now() + catalog_listing_timeout(self.request_timeout);
                drop(
                    self.list_upstream_prompts_allowed_until(&[], Some(&names), deadline_at)
                        .await,
                );
            })
            .buffer_unordered(upstream_discovery_concurrency(None));
        while work.next().await.is_some() {}
    }

    /// Start a detached warm-up for never-listed prompt snapshots in `allowed`
    /// and wait at most [`COLD_SNAPSHOT_WAIT`] for it. The task always runs to
    /// completion so a short caller wait never cuts a listing short of
    /// publishing its rows or recording its failure.
    pub async fn ensure_prompt_snapshots_allowed(
        self: &Arc<Self>,
        allowed: Option<&BTreeSet<String>>,
    ) {
        let cold = self.cold_prompt_snapshots(allowed).await;
        if cold.is_empty() {
            return;
        }
        let pool = Arc::clone(self);
        let task = tokio::spawn(async move { pool.warm_cold_prompt_snapshots(cold).await });
        drop(tokio::time::timeout(self.request_timeout.min(COLD_SNAPSHOT_WAIT), task).await);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::Prompt;

    #[tokio::test]
    async fn seeded_but_unconnected_upstreams_are_not_cold() {
        // Only a live connection whose incarnation lacks a settled listing is
        // cold; the warm-up must never try to list an unconnected upstream.
        let pool = Arc::new(UpstreamPool::new());
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("brief", Some("Alpha brief"), None)],
        )
        .await;
        assert!(pool.cold_prompt_snapshots(None).await.is_empty());
        pool.ensure_prompt_snapshots_allowed(None).await;
        let snapshot = pool.published_prompt_catalog().await.unwrap();
        assert_eq!(snapshot.routes().len(), 1);
        assert_eq!(snapshot.routes()[0].upstream_name.as_ref(), "alpha");
    }
}
