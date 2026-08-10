//! Non-blocking subscription reconciliation for resource catalog refreshes.

use std::collections::BTreeSet;
use std::time::Instant;

use super::UpstreamPool;

impl UpstreamPool {
    pub(super) async fn subscription_refresh_required(
        &self,
        upstream: &str,
        resource_uris_changed: bool,
    ) -> bool {
        resource_uris_changed || !self.subscription_tasks.read().await.contains_key(upstream)
    }

    pub(super) async fn schedule_upstream_subscription_refreshes(&self, upstreams: Vec<String>) {
        let requested = upstreams.into_iter().collect::<BTreeSet<_>>();
        let requested_count = requested.len();
        let upstreams = {
            let mut pending = self.subscription_refresh_pending.lock().await;
            requested
                .into_iter()
                .filter(|upstream| pending.insert(upstream.clone()))
                .collect::<BTreeSet<_>>()
        };
        if upstreams.is_empty() {
            tracing::debug!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "subscription.refresh.schedule",
                phase = "coalesced",
                upstream_count = requested_count,
                "upstream subscription refresh already pending"
            );
            return;
        }
        let upstream_count = upstreams.len();
        tracing::info!(
            surface = "dispatch",
            service = "upstream.pool",
            action = "subscription.refresh.schedule",
            phase = "scheduled",
            upstream_count,
            "upstream subscription refresh scheduled"
        );
        let pool = self.clone();
        let cancel = self.subscription_reconcile_cancel.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            tracing::info!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "subscription.refresh.batch",
                phase = "start",
                upstream_count,
                "upstream subscription refresh started"
            );
            let cancelled = tokio::select! {
                biased;
                () = cancel.cancelled() => true,
                () = pool.refresh_upstream_subscriptions_concurrently(
                    upstreams.iter().cloned().collect(),
                ) => false,
            };
            {
                let mut pending = pool.subscription_refresh_pending.lock().await;
                for upstream in &upstreams {
                    pending.remove(upstream);
                }
            }
            tracing::info!(
                surface = "dispatch",
                service = "upstream.pool",
                action = "subscription.refresh.batch",
                phase = if cancelled { "cancelled" } else { "finish" },
                upstream_count,
                elapsed_ms = started.elapsed().as_millis(),
                "upstream subscription refresh finished"
            );
        });
    }
}
