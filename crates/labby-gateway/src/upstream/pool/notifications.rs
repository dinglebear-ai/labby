//! Upstream notification subscriptions and the pool-level event bus.

use std::collections::BTreeSet;
#[cfg(test)]
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{ServerNotification, SubscriptionFilter};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::UpstreamPool;
use super::helpers::bare_upstream_resource_uri;

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

    async fn publish_subscribable_resource_snapshot(&self) {
        let accepted = self.subscription_resources.read().await;
        let mut snapshot = BTreeSet::new();
        for (upstream, uris) in accepted.iter() {
            snapshot.extend(
                uris.iter()
                    .map(|uri| Self::gateway_resource_uri(upstream, uri)),
            );
        }
        self.subscribable_resource_uris.store(Arc::new(snapshot));
    }

    async fn clear_subscription_resources(&self, upstream: &str) {
        self.subscription_resources.write().await.remove(upstream);
        self.publish_subscribable_resource_snapshot().await;
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
        if let Some(previous) = self.subscription_tasks.write().await.remove(upstream) {
            previous.cancel();
        }
        self.clear_subscription_resources(upstream).await;

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

        let cancel = CancellationToken::new();
        self.subscription_tasks
            .write()
            .await
            .insert(upstream.to_string(), cancel.clone());
        let pool = self.clone();
        let upstream = upstream.to_string();
        tokio::spawn(async move {
            loop {
                let established = tokio::select! {
                    () = cancel.cancelled() => break,
                    result = peer.listen(requested.clone()) => result,
                };
                let mut subscription = match established {
                    Ok(subscription) => subscription,
                    Err(error) => {
                        tracing::warn!(
                            upstream = %upstream,
                            error = %error,
                            "failed to establish upstream subscriptions/listen stream"
                        );
                        pool.clear_subscription_resources(&upstream).await;
                        tokio::select! {
                            () = cancel.cancelled() => break,
                            () = tokio::time::sleep(SUBSCRIPTION_RETRY_DELAY) => continue,
                        }
                    }
                };

                let accepted_resources = subscription
                    .acknowledged()
                    .resource_subscriptions
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                pool.subscription_resources
                    .write()
                    .await
                    .insert(upstream.clone(), accepted_resources);
                pool.publish_subscribable_resource_snapshot().await;

                loop {
                    let next = tokio::select! {
                        () = cancel.cancelled() => {
                            drop(subscription.cancel().await);
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

                pool.clear_subscription_resources(&upstream).await;
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = tokio::time::sleep(SUBSCRIPTION_RETRY_DELAY) => {}
                }
            }
            pool.clear_subscription_resources(&upstream).await;
        });
    }

    pub(super) async fn cancel_all_upstream_subscriptions(&self) {
        let tasks = self
            .subscription_tasks
            .write()
            .await
            .drain()
            .map(|(_, token)| token)
            .collect::<Vec<_>>();
        for token in tasks {
            token.cancel();
        }
        self.subscription_resources.write().await.clear();
        self.publish_subscribable_resource_snapshot().await;
    }

    #[cfg(test)]
    pub(super) async fn set_subscription_resources_for_test(
        &self,
        values: HashMap<String, BTreeSet<String>>,
    ) {
        *self.subscription_resources.write().await = values;
        self.publish_subscribable_resource_snapshot().await;
    }
}
