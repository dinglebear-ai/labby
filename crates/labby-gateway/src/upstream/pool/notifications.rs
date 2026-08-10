//! Upstream notification subscriptions and the pool-level event bus.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rmcp::model::{ServerNotification, SubscriptionFilter};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::helpers::{DISCOVERY_TIMEOUT, bare_upstream_resource_uri, cached_upstream_tool};

const NOTIFICATION_EVENT_CAPACITY: usize = 1024;
const SUBSCRIPTION_RETRY_DELAY: Duration = Duration::from_secs(2);

/// A notification observed on an upstream MCP connection and normalized for
/// downstream gateway consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UpstreamNotificationEvent {
    ToolListChanged { upstream: String },
    PromptListChanged { upstream: String },
    ResourceListChanged { upstream: String },
    ResourceUpdated { upstream: String, uri: String },
}

impl UpstreamPool {
    pub(super) fn notification_channel() -> (
        broadcast::Sender<UpstreamNotificationEvent>,
        broadcast::Receiver<UpstreamNotificationEvent>,
    ) {
        broadcast::channel(NOTIFICATION_EVENT_CAPACITY)
    }

    /// Subscribe to normalized upstream notifications. Receivers are cheap and
    /// independent; a slow consumer cannot block upstream MCP connections.
    pub fn subscribe_notifications(&self) -> broadcast::Receiver<UpstreamNotificationEvent> {
        self.notification_tx.subscribe()
    }

    /// Re-list one exact upstream after it reports `tools/list_changed` and
    /// atomically replace only that upstream's cached tools.
    ///
    /// The notification event bus has two producers: the shared
    /// subscriptions/listen connection and request-scoped relay connections.
    /// Its consumer calls this method before evaluating downstream catalog
    /// contracts so both paths observe the updated tools without bypassing the
    /// visible-contract suppression invariant.
    pub async fn refresh_tools_after_list_changed(&self, upstream: &str) -> bool {
        let upstream_name = {
            let catalog = self.catalog.read().await;
            catalog.get(upstream).map(|entry| Arc::clone(&entry.name))
        };
        let Some(upstream_name) = upstream_name else {
            tracing::warn!(
                upstream,
                "cannot refresh tools after list-changed signal: catalog entry is missing"
            );
            return false;
        };

        let peer = {
            let connections = self.connections.read().await;
            connections
                .get(upstream)
                .map(|connection| connection.peer.clone())
        };
        let Some(peer) = peer else {
            let error = "cannot refresh tools after list-changed signal: connection is missing";
            self.record_failure_for(upstream, UpstreamCapability::Tools, error)
                .await;
            tracing::warn!(upstream, error, "upstream tool-list refresh failed");
            return false;
        };

        let listed = tokio::time::timeout(DISCOVERY_TIMEOUT, peer.list_all_tools()).await;
        let tools = match listed {
            Ok(Ok(tools)) => tools,
            Ok(Err(error)) => {
                let error = format!("tool-list refresh after list-changed signal failed: {error}");
                self.record_failure_for(upstream, UpstreamCapability::Tools, error.clone())
                    .await;
                tracing::warn!(upstream, error = %error, "upstream tool-list refresh failed");
                return false;
            }
            Err(_) => {
                let error = format!(
                    "tool-list refresh after list-changed signal timed out after {}s",
                    DISCOVERY_TIMEOUT.as_secs()
                );
                self.record_failure_for(upstream, UpstreamCapability::Tools, error.clone())
                    .await;
                tracing::warn!(upstream, error = %error, "upstream tool-list refresh failed");
                return false;
            }
        };
        let tool_count = tools.len();
        let tools = tools
            .into_iter()
            .map(|tool| cached_upstream_tool(tool, &upstream_name))
            .collect::<HashMap<_, _>>();

        let replaced = {
            let mut catalog = self.catalog.write().await;
            if let Some(entry) = catalog.get_mut(upstream) {
                entry.tools = tools;
                true
            } else {
                false
            }
        };
        if !replaced {
            tracing::warn!(
                upstream,
                "discarding refreshed tools because the catalog entry was removed"
            );
            return false;
        }

        self.record_success_for(upstream, UpstreamCapability::Tools)
            .await;
        tracing::debug!(
            upstream,
            tool_count,
            "refreshed upstream tools after list-changed signal"
        );
        true
    }

    /// Snapshot the gateway-facing resource URIs for which an upstream has
    /// actually acknowledged notification delivery.
    pub fn subscribable_resource_uris_snapshot(&self) -> Arc<BTreeSet<String>> {
        self.subscribable_resource_uris.load_full()
    }

    pub(super) fn gateway_resource_uri(upstream: &str, native_uri: &str) -> String {
        if native_uri.starts_with("ui://") {
            native_uri.to_string()
        } else {
            format!(
                "lab://upstream/{upstream}/{}",
                bare_upstream_resource_uri(native_uri)
            )
        }
    }

    fn store_subscribable_resource_snapshot(&self, accepted: &HashMap<String, BTreeSet<String>>) {
        let mut snapshot = BTreeSet::new();
        for (upstream, uris) in accepted {
            snapshot.extend(
                uris.iter()
                    .map(|uri| Self::gateway_resource_uri(upstream, uri)),
            );
        }
        self.subscribable_resource_uris.store(Arc::new(snapshot));
    }

    /// Atomically replace an upstream's active subscription generation and
    /// retire the prior generation's published acknowledgement.
    pub(super) async fn begin_subscription_generation(
        &self,
        upstream: &str,
    ) -> Arc<CancellationToken> {
        let generation = Arc::new(CancellationToken::new());
        let mut generations = self.subscription_tasks.write().await;
        if let Some(previous) = generations.insert(upstream.to_string(), generation.clone()) {
            previous.cancel();
        }
        let mut resources = self.subscription_resources.write().await;
        resources.remove(upstream);
        self.store_subscribable_resource_snapshot(&resources);
        generation
    }

    /// Publish an acknowledgement only while `generation` still owns this
    /// upstream. Holding the generation read lock through the snapshot store
    /// makes ownership validation and publication one atomic state transition.
    pub(super) async fn record_subscription_resources_if_current(
        &self,
        upstream: &str,
        generation: &Arc<CancellationToken>,
        accepted: &SubscriptionFilter,
    ) -> bool {
        let generations = self.subscription_tasks.read().await;
        if !generations
            .get(upstream)
            .is_some_and(|current| Arc::ptr_eq(current, generation))
        {
            return false;
        }
        let accepted_resources = accepted
            .resource_subscriptions
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut resources = self.subscription_resources.write().await;
        resources.insert(upstream.to_string(), accepted_resources);
        self.store_subscribable_resource_snapshot(&resources);
        true
    }

    /// Clear an acknowledgement only while `generation` still owns this
    /// upstream, so teardown from an older task cannot erase a newer ack.
    pub(super) async fn clear_subscription_resources_if_current(
        &self,
        upstream: &str,
        generation: &Arc<CancellationToken>,
    ) -> bool {
        let generations = self.subscription_tasks.read().await;
        if !generations
            .get(upstream)
            .is_some_and(|current| Arc::ptr_eq(current, generation))
        {
            return false;
        }
        let mut resources = self.subscription_resources.write().await;
        resources.remove(upstream);
        self.store_subscribable_resource_snapshot(&resources);
        true
    }

    fn subscription_filter_is_empty(filter: &SubscriptionFilter) -> bool {
        filter.tools_list_changed != Some(true)
            && filter.prompts_list_changed != Some(true)
            && filter.resources_list_changed != Some(true)
            && filter
                .resource_subscriptions
                .as_ref()
                .is_none_or(Vec::is_empty)
    }

    /// Start or replace the long-lived subscriptions/listen consumer for one
    /// shared upstream connection. The requested filter is rebuilt from the
    /// current catalog each time so newly discovered resource URIs become
    /// deliverable without restarting Labby.
    pub(super) async fn refresh_upstream_subscription(&self, upstream: &str) {
        let generation = self.begin_subscription_generation(upstream).await;

        let peer = {
            let connections = self.connections.read().await;
            connections
                .get(upstream)
                .map(|connection| connection.peer.clone())
        };
        let Some(peer) = peer else {
            return;
        };
        let Some(server_info) = peer.peer_info() else {
            return;
        };

        let resource_uris = {
            let catalog = self.catalog.read().await;
            catalog
                .get(upstream)
                .filter(|entry| entry.proxy_resources)
                .map(|entry| entry.resource_uris.clone())
                .unwrap_or_default()
        };
        let requested = SubscriptionFilter::builder()
            .tools_list_changed()
            .prompts_list_changed()
            .resources_list_changed()
            .resource_subscriptions(resource_uris)
            .build()
            .supported_by(&server_info.capabilities);
        if Self::subscription_filter_is_empty(&requested) {
            return;
        }

        // Publish the initial acknowledgement before discovery returns. An
        // outer Labby may immediately open its own subscription after listing
        // resources; deferring this handshake to the background creates a
        // multi-hop race where the middle gateway temporarily rejects an exact
        // resource URI that its leaf has already accepted.
        let initial_established = tokio::select! {
            biased;
            () = generation.cancelled() => return,
            result = tokio::time::timeout(
                DISCOVERY_TIMEOUT,
                peer.listen(requested.clone()),
            ) => result,
        };
        let initial_subscription = match initial_established {
            Ok(Ok(mut subscription)) => {
                if !self
                    .record_subscription_resources_if_current(
                        upstream,
                        &generation,
                        subscription.acknowledged(),
                    )
                    .await
                {
                    drop(subscription.cancel().await);
                    return;
                }
                Some(subscription)
            }
            Ok(Err(error)) => {
                tracing::warn!(
                    upstream,
                    error = %error,
                    "failed to establish initial upstream subscriptions/listen stream"
                );
                None
            }
            Err(_) => {
                tracing::warn!(
                    upstream,
                    timeout_secs = DISCOVERY_TIMEOUT.as_secs(),
                    "timed out establishing initial upstream subscriptions/listen stream"
                );
                None
            }
        };

        let pool = self.clone();
        let upstream = upstream.to_string();
        tokio::spawn(async move {
            let mut initial_subscription = initial_subscription;
            loop {
                let (mut subscription, publish_acknowledgement) = match initial_subscription.take()
                {
                    Some(subscription) => (subscription, false),
                    None => {
                        let established = tokio::select! {
                            biased;
                            () = generation.cancelled() => break,
                            result = peer.listen(requested.clone()) => result,
                        };
                        let subscription = match established {
                            Ok(subscription) => subscription,
                            Err(error) => {
                                tracing::warn!(
                                    upstream = %upstream,
                                    error = %error,
                                    "failed to establish upstream subscriptions/listen stream"
                                );
                                if !pool
                                    .clear_subscription_resources_if_current(&upstream, &generation)
                                    .await
                                {
                                    return;
                                }
                                tokio::select! {
                                    biased;
                                    () = generation.cancelled() => break,
                                    () = tokio::time::sleep(SUBSCRIPTION_RETRY_DELAY) => continue,
                                }
                            }
                        };
                        (subscription, true)
                    }
                };

                if publish_acknowledgement {
                    if !pool
                        .record_subscription_resources_if_current(
                            &upstream,
                            &generation,
                            subscription.acknowledged(),
                        )
                        .await
                    {
                        drop(subscription.cancel().await);
                        return;
                    }
                }

                loop {
                    let next = tokio::select! {
                        biased;
                        () = generation.cancelled() => {
                            drop(subscription.cancel().await);
                            let _ = pool.clear_subscription_resources_if_current(
                                &upstream,
                                &generation,
                            ).await;
                            return;
                        }
                        result = subscription.next() => result,
                    };
                    match next {
                        Ok(Some(ServerNotification::ToolListChangedNotification(_))) => {
                            drop(pool.notification_tx.send(
                                UpstreamNotificationEvent::ToolListChanged {
                                    upstream: upstream.clone(),
                                },
                            ));
                        }
                        Ok(Some(ServerNotification::PromptListChangedNotification(_))) => {
                            drop(pool.notification_tx.send(
                                UpstreamNotificationEvent::PromptListChanged {
                                    upstream: upstream.clone(),
                                },
                            ));
                        }
                        Ok(Some(ServerNotification::ResourceListChangedNotification(_))) => {
                            drop(pool.notification_tx.send(
                                UpstreamNotificationEvent::ResourceListChanged {
                                    upstream: upstream.clone(),
                                },
                            ));
                        }
                        Ok(Some(ServerNotification::ResourceUpdatedNotification(notification))) => {
                            let uri =
                                Self::gateway_resource_uri(&upstream, &notification.params.uri);
                            drop(pool.notification_tx.send(
                                UpstreamNotificationEvent::ResourceUpdated {
                                    upstream: upstream.clone(),
                                    uri,
                                },
                            ));
                        }
                        Ok(Some(other)) => {
                            tracing::debug!(
                                upstream = %upstream,
                                notification = ?other,
                                "ignoring non-subscription notification on listen stream"
                            );
                        }
                        Ok(None) => break,
                        Err(error) => {
                            tracing::warn!(
                                upstream = %upstream,
                                error = %error,
                                "upstream notification subscription ended with error"
                            );
                            break;
                        }
                    }
                }

                if !pool
                    .clear_subscription_resources_if_current(&upstream, &generation)
                    .await
                {
                    return;
                }
                tokio::select! {
                    biased;
                    () = generation.cancelled() => break,
                    () = tokio::time::sleep(SUBSCRIPTION_RETRY_DELAY) => {}
                }
            }
            let _ = pool
                .clear_subscription_resources_if_current(&upstream, &generation)
                .await;
        });
    }

    /// Refresh multiple upstream acknowledgements in parallel. Each refresh
    /// still completes its initial handshake before this batch returns, while
    /// independent 15-second deadlines no longer accumulate serially.
    pub(super) async fn refresh_upstream_subscriptions_concurrently(&self, upstreams: Vec<String>) {
        let mut refreshes = FuturesUnordered::new();
        for upstream in upstreams.into_iter().collect::<BTreeSet<_>>() {
            refreshes.push(async move {
                self.refresh_upstream_subscription(&upstream).await;
            });
        }
        while refreshes.next().await.is_some() {}
    }

    pub(super) async fn cancel_all_upstream_subscriptions(&self) {
        self.subscription_reconcile_cancel.cancel();
        let tasks = {
            let mut generations = self.subscription_tasks.write().await;
            let tasks = generations
                .drain()
                .map(|(_, token)| token)
                .collect::<Vec<_>>();
            let mut resources = self.subscription_resources.write().await;
            resources.clear();
            self.store_subscribable_resource_snapshot(&resources);
            tasks
        };
        for token in tasks {
            token.cancel();
        }
    }

    #[cfg(test)]
    pub(super) async fn set_subscription_resources_for_test(
        &self,
        values: HashMap<String, BTreeSet<String>>,
    ) {
        let mut resources = self.subscription_resources.write().await;
        *resources = values;
        self.store_subscribable_resource_snapshot(&resources);
    }
}
