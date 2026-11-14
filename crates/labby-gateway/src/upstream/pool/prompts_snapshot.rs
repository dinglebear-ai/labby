//! Cache-backed prompt metadata for callers that must not fan out live.
//!
//! Code Mode builds its discovery catalog on every run and every sandbox
//! `search()`. A live `prompts/list` fan-out there puts the slowest upstream's
//! prompt listing on every run's latency floor, and one upstream that stalls on
//! `prompts/list` while serving tools normally can time out every run. These
//! helpers serve the published prompt snapshot instead and give never-listed
//! upstreams a bounded, single-flight warm-up, matching the resource contract
//! (`ensure_resource_snapshots_allowed`).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use rmcp::model::Prompt;

use super::UpstreamPool;
use super::resources_list::catalog_listing_timeout;

/// Longest a caller waits for never-listed prompt snapshots before serving
/// what is published. The warm-up keeps running and publishes for later runs.
const COLD_PROMPT_SNAPSHOT_WAIT: Duration = Duration::from_millis(250);

/// One published, exposed prompt and its owning upstream.
#[derive(Debug, Clone)]
pub struct CachedUpstreamPrompt {
    pub upstream_name: Arc<str>,
    pub native_name: Arc<str>,
    pub prompt: Prompt,
}

impl UpstreamPool {
    /// Connected, prompt-routable upstreams in `allowed` with no settled prompt
    /// listing for their current connection.
    pub(super) async fn cold_prompt_snapshots(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> BTreeSet<String> {
        let observed = self.observe_routable_prompt_connections(allowed).await;
        let catalog = self.catalog.read().await;
        observed
            .into_iter()
            .filter(|entry| {
                !catalog.prompt_snapshot_is_current(entry.upstream(), entry.incarnation())
            })
            .map(|entry| entry.upstream().to_string())
            .collect()
    }

    /// List cold upstreams once each; overlapping callers share one flight and
    /// recheck after acquiring it.
    async fn warm_cold_prompt_snapshots(&self, names: BTreeSet<String>) {
        let mut work = futures::stream::iter(names)
            .map(|name| async move {
                let flight = {
                    let mut flights = self.prompt_snapshot_warmup.lock().await;
                    flights.retain(|_, value| value.strong_count() > 0);
                    flights
                        .get(&name)
                        .and_then(std::sync::Weak::upgrade)
                        .unwrap_or_else(|| {
                            let flight = Arc::new(tokio::sync::Mutex::new(()));
                            flights.insert(name.clone(), Arc::downgrade(&flight));
                            flight
                        })
                };
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
            .buffer_unordered(super::helpers::upstream_discovery_concurrency(None));
        while work.next().await.is_some() {}
    }

    /// Start a detached warm-up for never-listed prompt snapshots in `allowed`
    /// and wait at most [`COLD_PROMPT_SNAPSHOT_WAIT`] for it. The task always
    /// runs to completion so a short caller wait never cuts a listing short of
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
        drop(tokio::time::timeout(self.request_timeout.min(COLD_PROMPT_SNAPSHOT_WAIT), task).await);
    }

    /// Exposed prompts from the published regular-upstream snapshot, filtered
    /// to `allowed`. Issues no upstream RPC. OAuth/subject-scoped prompts are
    /// not part of this snapshot.
    pub async fn cached_upstream_prompts_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<CachedUpstreamPrompt> {
        let snapshot = match self.published_prompt_catalog().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(
                    surface = "dispatch",
                    service = "upstream.pool",
                    action = "prompts.cached",
                    error = ?error,
                    "published prompt catalog is unavailable; serving no cached prompts"
                );
                return Vec::new();
            }
        };
        snapshot
            .routes()
            .iter()
            .filter(|route| {
                allowed.is_none_or(|allowed| allowed.contains(route.upstream_name.as_ref()))
            })
            .map(|route| CachedUpstreamPrompt {
                upstream_name: Arc::clone(&route.upstream_name),
                native_name: Arc::clone(&route.native_name),
                prompt: route.prompt.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cached_prompts_come_from_the_published_snapshot_and_respect_allowed() {
        let pool = UpstreamPool::new();
        pool.insert_prompt_routes_for_tests(
            "alpha",
            vec![Prompt::new("brief", Some("Alpha brief"), None)],
        )
        .await;
        pool.insert_prompt_routes_for_tests(
            "beta",
            vec![Prompt::new("brief", Some("Beta brief"), None)],
        )
        .await;

        let all = pool.cached_upstream_prompts_allowed(None).await;
        let mut owners = all
            .iter()
            .map(|prompt| {
                (
                    prompt.upstream_name.to_string(),
                    prompt.native_name.to_string(),
                )
            })
            .collect::<Vec<_>>();
        owners.sort();
        assert_eq!(
            owners,
            [
                ("alpha".into(), "brief".into()),
                ("beta".into(), "brief".into())
            ]
        );

        let allowed = BTreeSet::from(["beta".to_string()]);
        let scoped = pool.cached_upstream_prompts_allowed(Some(&allowed)).await;
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].upstream_name.as_ref(), "beta");
    }
}
