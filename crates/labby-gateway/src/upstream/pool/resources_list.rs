//! Resource listing and the synthetic gateway documents.
//!
//! Three entry points share one contract:
//!
//! - `list_upstream_resources*` is the live fan-out. It re-lists every routable
//!   regular upstream and republishes each one's snapshot.
//! - `cached_upstream_resources_with_provenance_allowed` is the cache-only
//!   projection the MCP `resources/list` handler and Code Mode read. It never
//!   performs peer I/O.
//! - `spawn_resource_snapshot_warmup` / `warm_cold_resource_snapshots_allowed`
//!   re-list only the upstreams whose snapshot is missing or older than
//!   `RESOURCE_SNAPSHOT_MAX_AGE` without a push channel.
//!
//! Freshness owner: a snapshot is replaced on connect, reconnect, gateway
//! reload, an upstream `resources/list_changed` (`refresh_resources_after_
//! list_changed`), and by the warm-up above. `subject_scoped_resources` lists
//! OAuth upstreams over the per-subject connection and caches the result on
//! that connection under the same freshness bound. The `gateway_*` methods
//! render the synthetic `lab://gateway/*` documents.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rmcp::model::{Resource, ResourceTemplate};
use serde_json::Value;

use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::{ToolExposurePolicy, UpstreamCapability};
use super::UpstreamPool;
use super::capability::peer_declares_resources;
use super::capability_call::{
    CapabilityCallError, bounded_service_error_text, timed_capability_call,
    timed_capability_call_with_timeout,
};
use super::catalog_pagination;
use super::catalog_publication::{ResourceSnapshotColdSet, is_ui_resource_uri};
use super::entries::{
    health_str, log_exposure_filter, resolve_exposure_policy,
    resolve_request_resource_exposure_policy, resource_exposed,
};
use super::helpers::{
    RESOURCE_SNAPSHOT_MAX_AGE, SUBJECT_CONN_IDLE_TTL, bare_upstream_resource_uri,
    classify_upstream_error, max_response_bytes, rewrite_resource_uri, upstream_discovery_timeout,
    upstream_transport,
};
use super::logging::{
    UpstreamRequestLog, is_capability_unsupported, log_upstream_capability_skipped,
    log_upstream_request_error, log_upstream_request_finish, log_upstream_request_start,
};
use super::tools::MAX_UPSTREAM_RESOURCES;

/// Wall-clock cap for one upstream's catalog listing on the connect path.
///
/// Shared by the resource and prompt refreshes so the two budgets cannot drift
/// apart: both run while the lazy-connect mutex and the write-preferring
/// `oauth_invalidation_barrier` read guard are held, so an unbounded listing
/// stalls every queued OAuth writer behind one slow upstream.
const CATALOG_LISTING_TIMEOUT: Duration = Duration::from_secs(10);

/// A started resource snapshot warm-up. See
/// `UpstreamPool::spawn_resource_snapshot_warmup`.
pub struct ResourceSnapshotWarmup {
    /// What the warm-up covers.
    pub cold: ResourceSnapshotColdSet,
    /// The running fan-out, `None` when nothing was cold. Dropping the handle
    /// detaches the task; it keeps running to completion.
    pub task: Option<tokio::task::JoinHandle<()>>,
}

/// One regular upstream Resource with its exact pre-rewrite provenance.
///
/// This is observational listing metadata, not read authority or a grant.
#[derive(Clone, Debug)]
pub struct ListedUpstreamResource {
    pub upstream_name: String,
    pub native_uri: String,
    pub resource: Resource,
}

/// One template returned by the regular non-OAuth listing path with exact
/// pre-rewrite provenance. Native UI templates may be present. This is
/// observational metadata, not read authority or a grant.
#[derive(Clone, Debug)]
pub struct ListedUpstreamResourceTemplate {
    pub upstream_name: String,
    pub native_uri_template: String,
    pub template: ResourceTemplate,
}

pub(super) fn catalog_listing_timeout(request_timeout: Duration) -> Duration {
    request_timeout.min(CATALOG_LISTING_TIMEOUT)
}

fn rewrite_resource_template(template: &mut ResourceTemplate, upstream_name: &str) {
    template.name = format!("{upstream_name}/{}", template.name);
    if !template.uri_template.starts_with("ui://") {
        template.uri_template = format!("lab://upstream/{upstream_name}/{}", template.uri_template);
    }
}

impl UpstreamPool {
    async fn apply_observed_resource_template_success(
        &self,
        observed: &super::incarnation::ObservedConnectionCatalogEntry,
        templates: &[ResourceTemplate],
    ) -> bool {
        let name = observed.upstream();
        self.apply_to_observed_catalog(observed, |catalog| {
            let entry = catalog.get_mut(name).expect("observed entry validated");
            super::health::record_success_on_entry(name, entry, UpstreamCapability::Resources);
            catalog.set_resource_template_source(name, observed.incarnation(), templates);
        })
        .await
        .is_some()
    }

    async fn apply_observed_resource_template_failure(
        &self,
        observed: &super::incarnation::ObservedConnectionCatalogEntry,
        error_text: &str,
    ) -> bool {
        let name = observed.upstream();
        self.apply_to_observed_catalog(observed, |catalog| {
            let entry = catalog.get_mut(name).expect("observed entry validated");
            super::health::record_failure_on_entry(
                name,
                entry,
                UpstreamCapability::Resources,
                format!("failed to list resource templates from upstream: {error_text}"),
            );
            catalog.remove_resource_template_source(name);
        })
        .await
        .is_some()
    }

    async fn apply_observed_resource_list_success(
        &self,
        observed: &super::incarnation::ObservedConnectionCatalogEntry,
        resources: &[Resource],
    ) -> Option<(ToolExposurePolicy, bool)> {
        let name = observed.upstream();
        let resource_uris = resources
            .iter()
            .map(|resource| bare_upstream_resource_uri(&resource.uri).to_string())
            .collect::<Vec<_>>();
        self.apply_to_observed_catalog(observed, |catalog| {
            let entry = catalog.get_mut(name).expect("observed entry validated");
            super::health::record_success_on_entry(name, entry, UpstreamCapability::Resources);
            let changed = entry.resource_uris != resource_uris;
            entry.resource_count = resources.len();
            entry.resource_uris = resource_uris;
            let policy = entry.resource_exposure_policy.clone();
            catalog.set_resource_source(name, observed.incarnation(), resources);
            (policy, changed)
        })
        .await
    }

    async fn apply_observed_resource_list_failure(
        &self,
        observed: &super::incarnation::ObservedConnectionCatalogEntry,
        error_text: &str,
    ) -> bool {
        let name = observed.upstream();
        self.apply_to_observed_catalog(observed, |catalog| {
            let entry = catalog.get_mut(name).expect("observed entry validated");
            super::health::record_failure_on_entry(
                name,
                entry,
                UpstreamCapability::Resources,
                format!("failed to list resources from upstream: {error_text}"),
            );
            entry.resource_count = 0;
            entry.resource_uris.clear();
            catalog.remove_resource_source(name);
        })
        .await
        .is_some()
    }

    /// Return cached resource URIs keyed by upstream name (used in catalog snapshots).
    pub async fn cached_upstream_resource_uris(&self) -> Vec<(String, Vec<String>)> {
        let catalog = self.catalog.read().await;
        catalog
            .iter()
            .filter(|(_, entry)| !entry.resource_uris.is_empty())
            .map(|(name, entry)| (name.clone(), entry.resource_uris.clone()))
            .collect()
    }

    /// Render the synthetic `lab://gateway/servers` document.
    ///
    /// Lists every registered upstream (regardless of health) with the
    /// tool count an agent would see in the corresponding schema document.
    pub async fn gateway_servers_doc(&self) -> Value {
        self.gateway_servers_doc_allowed(None).await
    }

    pub async fn gateway_servers_doc_allowed(&self, allowed: Option<&BTreeSet<String>>) -> Value {
        let catalog = self.catalog.read().await;
        let mut servers: Vec<Value> = catalog
            .iter()
            .filter(|(name, _)| allowed.is_none_or(|allowed| allowed.contains(*name)))
            .map(|(name, e)| {
                let tool_count = e
                    .tools
                    .values()
                    .filter(|t| e.exposure_policy.matches(&t.tool.name))
                    .count();
                serde_json::json!({
                    "name": name,
                    "tool_count": tool_count,
                    "prompt_count": e.prompt_count,
                    "resource_count": e.resource_count,
                    "tool_health": health_str(e.tool_health),
                    "tool_last_error": e.tool_last_error,
                })
            })
            .collect();
        servers.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        serde_json::json!({ "servers": servers })
    }

    /// Render the synthetic `lab://gateway/<name>/schema` document.
    ///
    /// Returns `None` when the upstream is not registered. Tools hidden by
    /// the upstream's `ToolExposurePolicy` are omitted. `input_schema` and
    /// `meta` are passed through verbatim from the cached tool definition.
    pub async fn gateway_server_schema(&self, name: &str) -> Option<Value> {
        self.gateway_server_schema_allowed(name, None).await
    }

    pub async fn gateway_server_schema_allowed(
        &self,
        name: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Option<Value> {
        if allowed.is_some_and(|allowed| !allowed.contains(name)) {
            return None;
        }
        let catalog = self.catalog.read().await;
        let entry = catalog.get(name)?;
        let mut tools: Vec<Value> = entry
            .tools
            .values()
            .filter(|t| entry.exposure_policy.matches(&t.tool.name))
            .map(|t| render_gateway_tool_row(&t.tool, t.input_schema.clone()))
            .collect();
        tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Some(serde_json::json!({
            "name": name,
            "tools": tools,
            "health": health_str(entry.tool_health),
            "last_error": entry.tool_last_error,
            "catalog_source": "shared_cache",
        }))
    }

    /// Render a gateway schema from a request-scoped OAuth discovery result.
    ///
    /// OAuth upstreams intentionally do not populate the subject-less catalog.
    /// This path discovers the named upstream with the caller's isolated
    /// subject connection and returns the same schema shape as the cached path.
    pub async fn subject_scoped_gateway_server_schema(
        &self,
        config: &UpstreamConfig,
        subject: &str,
    ) -> Result<Value, ToolError> {
        let started = Instant::now();
        let connect_timeout = upstream_discovery_timeout(config, self.request_timeout);
        let (peer, _) = tokio::time::timeout(
            connect_timeout,
            self.acquire_or_connect_subject(config, subject),
        )
        .await
        .map_err(|_| ToolError::Sdk {
            sdk_kind: "timeout".to_string(),
            message: format!(
                "subject-scoped discovery for upstream `{}` timed out after {}s",
                config.name,
                connect_timeout.as_secs()
            ),
        })?
        .map_err(|error| classified_schema_error(&config.name, &error.to_string()))?;

        let event = UpstreamRequestLog::tools_list(&config.name, true)
            .with_transport(upstream_transport(config));
        log_upstream_request_start(event);
        let tools = timed_capability_call(
            self,
            &config.name,
            UpstreamCapability::Tools,
            event,
            started,
            async {
                catalog_pagination::list_tools(
                    &peer,
                    self.request_timeout,
                    super::tools::MAX_UPSTREAM_TOOLS,
                )
                .await
                .map_err(|error| error.into_service_error(&config.name))
            },
            |tools| serde_json::to_vec(tools).map_or(usize::MAX, |body| body.len()),
            Some(subject),
            |error| format!("upstream `{}` tools/list failed: {error}", config.name),
            format!("upstream `{}` tools/list timed out", config.name),
        )
        .await
        .map_err(|error| capability_schema_error(&config.name, &error))?;
        let exposure_policy = resolve_exposure_policy(&config.name, config.expose_tools.clone());
        Ok(render_subject_scoped_gateway_schema(
            &config.name,
            &tools,
            &exposure_policy,
        ))
    }

    /// Synthetic gateway resources to emit from `list_resources`.
    ///
    /// Returns one entry for `lab://gateway/servers` plus one
    /// `lab://gateway/<name>/schema` entry per registered upstream.
    pub async fn gateway_synthetic_resources(&self) -> Vec<Resource> {
        self.gateway_synthetic_resources_allowed(None).await
    }

    pub async fn gateway_synthetic_resources_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<Resource> {
        let mut out = vec![
            Resource::new("lab://gateway/servers", "gateway/servers")
                .with_description("Index of upstream MCP servers registered with the gateway")
                .with_mime_type("application/json"),
        ];
        let catalog = self.catalog.read().await;
        let mut names: Vec<&String> = catalog.keys().collect();
        if let Some(allowed) = allowed {
            names.retain(|name| allowed.contains(*name));
        }
        names.sort();
        for name in names {
            out.push(
                Resource::new(
                    format!("lab://gateway/{name}/schema"),
                    format!("gateway/{name}/schema"),
                )
                .with_description(format!("Tool schemas for upstream `{name}`"))
                .with_mime_type("application/json"),
            );
        }
        out
    }

    /// Live fan-out: list resources from all resource-proxy-enabled upstreams
    /// and republish their snapshots.
    ///
    /// Resources are prefixed with `lab://upstream/{name}/` to avoid collisions.
    /// Discovery surfaces should read
    /// `cached_upstream_resources_with_provenance_allowed` instead.
    pub async fn list_upstream_resources(&self) -> Vec<Resource> {
        self.list_upstream_resources_allowed(None).await
    }

    pub async fn list_upstream_resources_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<Resource> {
        self.list_upstream_resources_with_provenance_allowed(allowed)
            .await
            .into_iter()
            .map(|listed| listed.resource)
            .collect()
    }

    /// Return the already-discovered resource catalog of regular (non-OAuth)
    /// upstreams, including their MCP Apps `ui://` rows, bounded exactly like
    /// the live listing.
    ///
    /// This is intentionally cache-only: callers that are merely enumerating
    /// resources must not turn an MCP resources/list request into fleet-wide I/O.
    pub async fn cached_upstream_resources_with_provenance_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<ListedUpstreamResource> {
        let mut listed = Vec::new();
        for (upstream_name, mut resource) in self.cached_upstream_resources_allowed(allowed).await {
            let native_uri = resource.uri.clone();
            if !is_ui_resource_uri(&resource.uri) {
                rewrite_resource_uri(&mut resource, &upstream_name);
            }
            listed.push(ListedUpstreamResource {
                upstream_name,
                native_uri,
                resource,
            });
        }
        self.bound_listed_resources(&mut listed);
        listed
    }

    /// Re-list the regular upstreams in `allowed` (all when `None`) and
    /// republish their snapshots without building the merged listing envelope.
    /// This is the refresh primitive behind connect, list_changed, and warm-up.
    pub(super) async fn refresh_resource_snapshots_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) {
        drop(self.fan_out_upstream_resources_allowed(allowed).await);
    }

    /// Re-list the upstreams whose snapshot is missing or stale and wait for
    /// the fan-out to finish. Single-flight: concurrent callers queue behind
    /// one fan-out and then find nothing left to warm. Returns what was cold
    /// when this caller took its turn.
    pub async fn warm_cold_resource_snapshots_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> ResourceSnapshotColdSet {
        let _turn = self.resource_snapshot_warmup.lock().await;
        let cold = self.cold_resource_snapshots(allowed).await;
        if !cold.is_empty() {
            self.refresh_resource_snapshots_allowed(Some(&cold.names()))
                .await;
        }
        cold
    }

    /// Start a warm-up for the cold snapshots in `allowed` and report what it
    /// covers. The task always runs to completion so a caller deadline can
    /// never cut a fan-out short of publishing its rows, recording its
    /// failures, or scheduling its subscription refreshes; a caller that stops
    /// waiting simply serves the snapshot it has.
    pub async fn spawn_resource_snapshot_warmup(
        self: &Arc<Self>,
        allowed: Option<&BTreeSet<String>>,
    ) -> ResourceSnapshotWarmup {
        let cold = self.cold_resource_snapshots(allowed).await;
        if cold.is_empty() {
            return ResourceSnapshotWarmup { cold, task: None };
        }
        let pool = Arc::clone(self);
        let names = cold.names();
        let task = tokio::spawn(async move {
            drop(
                pool.warm_cold_resource_snapshots_allowed(Some(&names))
                    .await,
            );
        });
        ResourceSnapshotWarmup {
            cold,
            task: Some(task),
        }
    }

    /// Make the cached listing usable: wait for never-listed upstreams to be
    /// warmed, and let stale ones refresh in the background.
    pub async fn ensure_resource_snapshots_allowed(
        self: &Arc<Self>,
        allowed: Option<&BTreeSet<String>>,
    ) {
        let warmup = self.spawn_resource_snapshot_warmup(allowed).await;
        if let Some(task) = warmup.task
            && !warmup.cold.missing.is_empty()
        {
            drop(task.await);
        }
    }

    /// Sort the merged listing, apply the response byte envelope, and cap the
    /// item count. Shared by the live and cached listings so both surfaces
    /// honour one bound contract.
    fn bound_listed_resources(&self, resources: &mut Vec<ListedUpstreamResource>) {
        // One pass in (upstream, URI) order keeps the result independent of
        // completion order and measures each resource exactly once.
        resources.sort_by(|left, right| {
            (&left.upstream_name, &left.native_uri).cmp(&(&right.upstream_name, &right.native_uri))
        });
        let max_bytes = max_response_bytes();
        let mut bytes = 2usize;
        resources.retain(|item| {
            #[cfg(test)]
            self.merged_resource_measurements
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            bytes = bytes.saturating_add(
                serde_json::to_vec(&item.resource).map_or(usize::MAX, |body| body.len() + 1),
            );
            bytes <= max_bytes
        });
        resources.truncate(MAX_UPSTREAM_RESOURCES);
    }

    pub async fn list_upstream_resources_with_provenance_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<ListedUpstreamResource> {
        let mut resources = self.fan_out_upstream_resources_allowed(allowed).await;
        // Bound only the merged envelope, after every server's complete,
        // independently validated snapshot has been published.
        self.bound_listed_resources(&mut resources);
        resources
    }

    /// Issue resources/list to every routable regular upstream in `allowed`,
    /// publish each snapshot, and return the exposed rows unbounded.
    async fn fan_out_upstream_resources_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<ListedUpstreamResource> {
        let observed_peers = self.observe_routable_resource_connections(allowed).await;
        if observed_peers.is_empty() {
            return Vec::new();
        }

        // Deliberate bulkhead exception: fan-out aggregation over every
        // routable upstream (catalog listing/refresh), not a caller-attributed
        // RPC, so it skips the per-upstream `timed_capability_call` permit.
        // Per-upstream failures deliberately degrade the merged result to
        // partial data — the MCP `resources/list` wire shape carries no
        // per-upstream error field (do not invent one). Failure visibility
        // lives in the circuit breaker + `resource_last_error` recorded below
        // (surfaced via `gateway.status`) and the classified `warn!` per
        // failing upstream.
        //
        // Issue RPCs in parallel, then sort by upstream name for deterministic order.
        // Each peer gets its own bounded walk. A fleet envelope limit must
        // never become a failure in another server's capability circuit.
        let mut futures = futures::stream::iter(observed_peers)
            .map(|observed| {
                let name = observed.upstream().to_string();
                let peer = observed.peer.clone();
                let request_timeout = catalog_listing_timeout(self.request_timeout);
                async move {
                    let event = UpstreamRequestLog::resources_list(&name, false);
                    let result = if !peer_declares_resources(&peer) {
                        log_upstream_capability_skipped(event);
                        Ok(Vec::new())
                    } else {
                        let started = Instant::now();
                        log_upstream_request_start(event);
                        match catalog_pagination::list_resources(
                            &peer,
                            request_timeout,
                            MAX_UPSTREAM_RESOURCES,
                        )
                        .await
                        {
                            Ok(resources) => {
                                let response_bytes = serde_json::to_vec(&resources)
                                    .map_or(usize::MAX, |body| body.len());
                                log_upstream_request_finish(
                                    event,
                                    started.elapsed().as_millis(),
                                    Some(response_bytes),
                                );
                                Ok(resources)
                            }
                            Err(catalog_pagination::CatalogPaginationError::Service(error))
                                if is_capability_unsupported(&error) =>
                            {
                                log_upstream_request_finish(
                                    event,
                                    started.elapsed().as_millis(),
                                    Some(0),
                                );
                                tracing::debug!(
                                    upstream = %name,
                                    "upstream does not implement resources/list — capability absent"
                                );
                                Ok(Vec::new())
                            }
                            Err(error) => {
                                let error_text = error.bounded_text();
                                log_upstream_request_error(
                                    event,
                                    started.elapsed().as_millis(),
                                    error.kind(),
                                    Some(&error_text),
                                    None,
                                    None,
                                );
                                Err(error_text)
                            }
                        }
                    };
                    (observed, result)
                }
            })
            .buffer_unordered(super::helpers::upstream_discovery_concurrency(None));

        let mut resources = Vec::new();
        let mut subscription_refreshes = Vec::new();
        while let Some((observed, result)) = futures.next().await {
            let name = observed.upstream().to_string();
            match result {
                Ok(upstream_resources) => {
                    // The cached snapshot deliberately stays *unfiltered*: it is
                    // what `gateway.discovered_resources` shows the operator who
                    // is editing `expose_resources`, and hiding excluded URIs
                    // there would make the allowlist un-editable. Enforcement
                    // happens on what leaves this function, on the cache-only
                    // projection (`cached_upstream_resources_allowed`), and on
                    // every read.
                    let Some((policy, resource_uris_changed)) = self
                        .apply_observed_resource_list_success(&observed, &upstream_resources)
                        .await
                    else {
                        tracing::debug!(upstream = %name, "discarding stale resources/list success");
                        continue;
                    };
                    if self
                        .subscription_refresh_required(&name, resource_uris_changed)
                        .await
                    {
                        subscription_refreshes.push(observed);
                    }
                    let mut hidden_count = 0usize;
                    let mut exposed_count = 0usize;
                    for mut resource in upstream_resources {
                        if !resource_exposed(&policy, bare_upstream_resource_uri(&resource.uri)) {
                            hidden_count += 1;
                            continue;
                        }
                        // MCP Apps (mcp-ui) widget resources keep their native
                        // `ui://…` URI: a tool result's `_meta.ui.resourceUri`
                        // references that exact URI, and the host reads it back
                        // verbatim (routed via `read_upstream_ui_resource`).
                        // Rewriting to the `lab://upstream/{name}/…` gateway form
                        // would break that reference, so skip the rewrite here.
                        let native_uri = resource.uri.clone();
                        if !is_ui_resource_uri(&resource.uri) {
                            rewrite_resource_uri(&mut resource, &name);
                        }
                        resources.push(ListedUpstreamResource {
                            upstream_name: name.clone(),
                            native_uri,
                            resource,
                        });
                        exposed_count += 1;
                    }
                    log_exposure_filter(&name, "resources", hidden_count, exposed_count, false);
                }
                Err(error_text) => {
                    if !self
                        .apply_observed_resource_list_failure(&observed, &error_text)
                        .await
                    {
                        tracing::debug!(upstream = %name, "discarding stale resources/list failure");
                        continue;
                    }
                    tracing::warn!(
                        upstream = %name,
                        kind = classify_upstream_error(&error_text),
                        error = %error_text,
                        "failed to list resources from upstream"
                    );
                }
            }
        }

        self.schedule_observed_upstream_subscription_refreshes(subscription_refreshes)
            .await;

        resources
    }

    /// List every resource template from all visible resource-proxy upstreams.
    /// Names and non-UI URI templates are namespaced to avoid cross-upstream
    /// collisions while preserving all other template metadata verbatim.
    pub async fn list_upstream_resource_templates_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<ResourceTemplate> {
        self.list_upstream_resource_templates_with_provenance_allowed(allowed)
            .await
            .into_iter()
            .map(|listed| listed.template)
            .collect()
    }

    pub async fn list_upstream_resource_templates_with_provenance_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<ListedUpstreamResourceTemplate> {
        let observed_peers = self.observe_routable_resource_connections(allowed).await;
        if observed_peers.is_empty() {
            return Vec::new();
        }

        // Deliberate bulkhead exception + partial-result semantics — same
        // contract as `list_upstream_resources_allowed` above.
        let mut futures = FuturesUnordered::new();
        let shared_budget = Arc::new(catalog_pagination::SharedCatalogBudget::new(
            MAX_UPSTREAM_RESOURCES,
            max_response_bytes(),
        ));
        for observed in observed_peers {
            let peer = observed.peer.clone();
            let shared_budget = Arc::clone(&shared_budget);
            futures.push(async move {
                let result = if peer_declares_resources(&peer) {
                    catalog_pagination::list_resource_templates_with_budget(
                        &peer,
                        self.request_timeout,
                        MAX_UPSTREAM_RESOURCES,
                        &shared_budget,
                    )
                    .await
                } else {
                    tracing::debug!(
                        upstream = %observed.upstream(),
                        "initialize did not advertise resources; skipping resources/templates/list"
                    );
                    Ok(Vec::new())
                };
                (observed, result)
            });
        }

        let mut results = Vec::new();
        while let Some(item) = futures.next().await {
            results.push(item);
        }
        results.sort_unstable_by(|left, right| left.0.upstream().cmp(right.0.upstream()));

        let mut templates = Vec::new();
        for (observed, result) in results {
            let name = observed.upstream().to_string();
            match result {
                Ok(upstream_templates) => {
                    if !self
                        .apply_observed_resource_template_success(&observed, &upstream_templates)
                        .await
                    {
                        tracing::debug!(upstream = %name, "discarding stale resources/templates/list success");
                        continue;
                    }
                    for mut template in upstream_templates {
                        if templates.len() >= MAX_UPSTREAM_RESOURCES {
                            tracing::warn!(
                                upstream = %name,
                                limit = MAX_UPSTREAM_RESOURCES,
                                "upstream resource template catalog exceeds limit — truncating to cap"
                            );
                            break;
                        }
                        let native_uri_template = template.uri_template.clone();
                        rewrite_resource_template(&mut template, &name);
                        templates.push(ListedUpstreamResourceTemplate {
                            upstream_name: name.clone(),
                            native_uri_template,
                            template,
                        });
                    }
                }
                Err(catalog_pagination::CatalogPaginationError::Service(error))
                    if is_capability_unsupported(&error) =>
                {
                    if !self
                        .apply_observed_resource_template_success(&observed, &[])
                        .await
                    {
                        tracing::debug!(upstream = %name, "discarding stale resources/templates/list unsupported result");
                        continue;
                    }
                    tracing::debug!(
                        upstream = %name,
                        error = %bounded_service_error_text(&error),
                        "upstream does not implement resources/templates/list — capability absent"
                    );
                }
                Err(error) => {
                    let error_text = error.bounded_text();
                    if !self
                        .apply_observed_resource_template_failure(&observed, &error_text)
                        .await
                    {
                        tracing::debug!(upstream = %name, "discarding stale resources/templates/list failure");
                        continue;
                    }
                    tracing::warn!(
                        upstream = %name,
                        kind = error.kind(),
                        error = %error_text,
                        "failed to list resource templates from upstream"
                    );
                }
            }
        }

        templates
    }

    pub async fn subject_scoped_resources(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
    ) -> Vec<Resource> {
        let mut futures = FuturesUnordered::new();
        for config in configs
            .iter()
            .filter(|config| config.oauth.is_some() && config.proxy_resources)
        {
            let config = config.clone();
            let subject = subject.to_string();
            let pool = self.clone();
            futures.push(async move {
                let started = Instant::now();
                let request_timeout = catalog_listing_timeout(pool.request_timeout);
                let key = (config.name.clone(), subject.clone());
                let cached = {
                    let cache = pool.subject_connections.read().await;
                    cache.get(&key).and_then(|entry| {
                        if entry.peer.is_transport_closed()
                            || entry.last_used.elapsed() >= SUBJECT_CONN_IDLE_TTL
                        {
                            return None;
                        }
                        let listed_at = entry.optional_catalogs.resources_listed_at?;
                        if listed_at.elapsed() >= RESOURCE_SNAPSHOT_MAX_AGE {
                            return None;
                        }
                        entry.optional_catalogs.resources.clone()
                    })
                };
                // A warm subject catalog is reused for `RESOURCE_SNAPSHOT_MAX_AGE`
                // after it was listed, or until the subject connection is
                // dropped or `refresh_resources_after_list_changed` clears it.
                // Subject connections have no push channel of their own, so the
                // age bound is what keeps every downstream resources/list from
                // becoming another OAuth upstream resources/list RPC without
                // freezing the catalog for the life of the connection.
                if let Some(resources) = cached {
                    let policy = resolve_request_resource_exposure_policy(
                        &config.name,
                        config.expose_resources.clone(),
                    );
                    return (config.name, policy, Ok(resources.to_vec()));
                }
                // Subject-scoped resources are discovered over a per-(upstream,
                // subject) connection and never land in `self.catalog`, so
                // there is no `UpstreamEntry::resource_exposure_policy` to
                // read. Resolve the same fail-closed policy from the live
                // config instead — the seam `subject_scoped_tools` uses.
                let policy = resolve_request_resource_exposure_policy(
                    &config.name,
                    config.expose_resources.clone(),
                );
                let event = UpstreamRequestLog::resources_list(&config.name, true)
                    .with_transport(upstream_transport(&config));
                log_upstream_request_start(event);
                let _fanout_permit = match tokio::time::timeout(
                    request_timeout,
                    pool.acquire_catalog_fanout_permit(),
                )
                .await
                {
                    Ok(Ok(permit)) => permit,
                    Ok(Err(error)) => return (config.name, policy, Err(error)),
                    Err(_) => {
                        return (
                            config.name,
                            policy,
                            Err("subject catalog concurrency wait timed out".to_string()),
                        );
                    }
                };
                let peer = match tokio::time::timeout(
                    request_timeout,
                    pool.acquire_or_connect_subject(&config, &subject),
                )
                .await
                {
                    Ok(Ok((peer, _tools))) => peer,
                    Ok(Err(error)) => {
                        log_upstream_request_error(
                            event,
                            started.elapsed().as_millis(),
                            "upstream_connect_error",
                            Some(&error),
                            None,
                            None,
                        );
                        return (config.name, policy, Err(error.to_string()));
                    }
                    Err(_) => {
                        let error = format!(
                            "subject-scoped upstream connection timed out after {}ms",
                            request_timeout.as_millis()
                        );
                        log_upstream_request_error(
                            event,
                            started.elapsed().as_millis(),
                            "timeout",
                            None,
                            None,
                            None,
                        );
                        return (config.name, policy, Err(error));
                    }
                };
                if !peer_declares_resources(&peer) {
                    pool.record_subject_optional_catalog(
                        &config.name,
                        &subject,
                        &peer,
                        Some(Vec::new()),
                        None,
                    )
                    .await;
                    log_upstream_capability_skipped(event);
                    return (config.name, policy, Ok(Vec::new()));
                }
                let timeout_ms = request_timeout.as_millis();
                let result = timed_capability_call_with_timeout(
                    &pool,
                    request_timeout,
                    &config.name,
                    UpstreamCapability::Resources,
                    event,
                    started,
                    async {
                        match catalog_pagination::list_resources(
                            &peer,
                            request_timeout,
                            MAX_UPSTREAM_RESOURCES,
                        )
                        .await
                        {
                            Ok(resources) => Ok(resources),
                            Err(catalog_pagination::CatalogPaginationError::Service(error))
                                if is_capability_unsupported(&error) =>
                            {
                                tracing::debug!(
                                    upstream = %config.name,
                                    "subject-scoped upstream does not implement resources/list — capability absent"
                                );
                                Ok(Vec::new())
                            }
                            Err(error) => Err(error.into_service_error(&config.name)),
                        }
                    },
                    |resources| serde_json::to_vec(resources).map_or(usize::MAX, |body| body.len()),
                    Some(&subject),
                    |error| format!("subject-scoped upstream resource discovery failed: {error}"),
                    format!(
                        "subject-scoped upstream resource listing timed out after {timeout_ms}ms"
                    ),
                    // Discovery fan-out: no downstream request to withdraw from.
                    None,
                )
                .await
                .map_err(|error| error.to_string());
                if let Ok(resources) = &result {
                    pool.record_subject_optional_catalog(
                        &config.name,
                        &subject,
                        &peer,
                        Some(resources.clone()),
                        None,
                    )
                    .await;
                }
                (config.name.clone(), policy, result)
            });
        }

        // Subject-scoped resource lists reuse the per-(upstream, subject)
        // connection cache and execute concurrently under the ordinary request
        // budget. One slow or broken OAuth upstream therefore degrades to a
        // partial catalog without delaying every other upstream.
        let mut results = Vec::new();
        while let Some((name, policy, result)) = futures.next().await {
            results.push((name, policy, result));
        }
        results.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        aggregate_subject_resources(results, MAX_UPSTREAM_RESOURCES, max_response_bytes())
    }
}

type SubjectResourceResult = (String, ToolExposurePolicy, Result<Vec<Resource>, String>);

fn aggregate_subject_resources(
    results: Vec<SubjectResourceResult>,
    item_limit: usize,
    byte_limit: usize,
) -> Vec<Resource> {
    let mut resources = Vec::new();
    let mut serialized_bytes = 0usize;
    for (name, policy, result) in results {
        match result {
            Ok(upstream_resources) => {
                let mut hidden_count = 0usize;
                let mut exposed_count = 0usize;
                for mut resource in upstream_resources {
                    if !resource_exposed(&policy, bare_upstream_resource_uri(&resource.uri)) {
                        hidden_count += 1;
                        continue;
                    }
                    if resources.len() >= item_limit {
                        tracing::warn!(
                            limit = item_limit,
                            accepted_items = resources.len(),
                            "subject-scoped resource catalog exceeds global item limit"
                        );
                        return resources;
                    }
                    rewrite_resource_uri(&mut resource, &name);
                    let resource_bytes =
                        serde_json::to_vec(&resource).map_or(usize::MAX, |body| body.len());
                    if serialized_bytes.saturating_add(resource_bytes) > byte_limit {
                        tracing::warn!(
                            limit = byte_limit,
                            accepted_bytes = serialized_bytes,
                            "subject-scoped resource catalog exceeds global byte limit"
                        );
                        return resources;
                    }
                    serialized_bytes = serialized_bytes.saturating_add(resource_bytes);
                    resources.push(resource);
                    exposed_count += 1;
                }
                log_exposure_filter(&name, "resources", hidden_count, exposed_count, true);
            }
            Err(error_text) => {
                tracing::warn!(
                    upstream = %name,
                    kind = classify_upstream_error(&error_text),
                    error = %error_text,
                    "subject-scoped upstream resource discovery failed"
                );
            }
        }
    }
    resources
}

fn render_subject_scoped_gateway_schema(
    name: &str,
    tools: &[rmcp::model::Tool],
    exposure_policy: &ToolExposurePolicy,
) -> Value {
    let mut tool_rows: Vec<Value> = tools
        .iter()
        .filter(|tool| exposure_policy.matches(tool.name.as_ref()))
        .map(|tool| {
            let input_schema = (!tool.input_schema.is_empty())
                .then(|| Value::Object((*tool.input_schema).clone()));
            render_gateway_tool_row(tool, input_schema)
        })
        .collect();
    tool_rows.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    serde_json::json!({
        "name": name,
        "tools": tool_rows,
        "health": "healthy",
        "last_error": Value::Null,
        "catalog_source": "subject_scoped_live",
    })
}

fn render_gateway_tool_row(tool: &rmcp::model::Tool, input_schema: Option<Value>) -> Value {
    serde_json::json!({
        "name": tool.name.as_ref(),
        "description": tool.description.as_ref().map(|description| description.as_ref()),
        "input_schema": input_schema,
        "meta": tool.meta,
    })
}

fn classified_schema_error(upstream: &str, message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: classify_upstream_error(message).to_string(),
        message: format!("subject-scoped discovery for upstream `{upstream}` failed: {message}"),
    }
}

fn capability_schema_error(upstream: &str, error: &CapabilityCallError) -> ToolError {
    let sdk_kind = match error {
        CapabilityCallError::Timeout { .. } => "timeout",
        CapabilityCallError::QueueSaturated { .. } => "queue_saturated",
        CapabilityCallError::ResponseTooLarge { .. } => "response_too_large",
        CapabilityCallError::Protocol { .. } => "decode_error",
        CapabilityCallError::Cancelled { .. } => "cancelled",
        CapabilityCallError::InputRequiredRoundsExceeded { .. } => "confirmation_required",
        CapabilityCallError::Mcp { .. } | CapabilityCallError::Other { .. } => {
            match classify_upstream_error(&error.to_string()) {
                kind @ ("auth_failed" | "auth_required") => kind,
                _ => "upstream_error",
            }
        }
        CapabilityCallError::Transport { .. } => classify_upstream_error(&error.to_string()),
    };
    ToolError::Sdk {
        sdk_kind: sdk_kind.to_string(),
        message: format!("subject-scoped discovery for upstream `{upstream}` failed: {error}"),
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures construct upstream Tool values directly
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use rmcp::model::{
        ErrorData, ListResourceTemplatesResult, ListResourcesResult, ListToolsResult,
        PaginatedRequestParams, ReadResourceResult, ResourceContents, ResourceTemplate,
        ServerCapabilities, ServerInfo, Tool,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};

    use labby_runtime::gateway_config::{
        UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
    };

    use super::super::super::types::{ToolExposurePolicy, UpstreamTool};
    use super::super::SubjectScopedConnection;
    use super::super::entries::healthy_in_process_entry;
    use super::super::helpers::normalize_resource_result_uri;
    use super::super::testsupport::{StaticCatalogServer, catalog_pool_with_server};
    use super::*;

    #[test]
    fn subject_resource_aggregation_enforces_global_item_cap() {
        let policy = ToolExposurePolicy::All;
        let results = vec![
            (
                "alpha".to_string(),
                policy.clone(),
                Ok((0..700)
                    .map(|index| Resource::new(format!("file:///alpha/{index}"), "resource"))
                    .collect()),
            ),
            (
                "beta".to_string(),
                policy,
                Ok((0..700)
                    .map(|index| Resource::new(format!("file:///beta/{index}"), "resource"))
                    .collect()),
            ),
        ];

        let resources = aggregate_subject_resources(results, MAX_UPSTREAM_RESOURCES, usize::MAX);

        assert_eq!(resources.len(), MAX_UPSTREAM_RESOURCES);
    }

    #[test]
    fn subject_resource_aggregation_enforces_global_byte_cap() {
        let policy = ToolExposurePolicy::All;
        let results = vec![(
            "alpha".to_string(),
            policy,
            Ok(vec![
                Resource::new("file:///alpha/one", "x".repeat(128)),
                Resource::new("file:///alpha/two", "y".repeat(128)),
            ]),
        )];

        let resources = aggregate_subject_resources(results, MAX_UPSTREAM_RESOURCES, 300);

        assert_eq!(resources.len(), 1);
    }

    #[tokio::test]
    async fn subject_catalog_fanout_gate_is_global_and_bounded() {
        let pool = UpstreamPool::new();
        let permit_count = pool.catalog_fanout_semaphore.available_permits() as u32;
        let held = Arc::clone(&pool.catalog_fanout_semaphore)
            .acquire_many_owned(permit_count)
            .await
            .expect("hold every permit");

        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                pool.acquire_catalog_fanout_permit()
            )
            .await
            .is_err(),
            "a second catalog job must wait behind the fleet-wide gate"
        );

        drop(held);
        drop(
            tokio::time::timeout(
                Duration::from_millis(100),
                pool.acquire_catalog_fanout_permit(),
            )
            .await
            .expect("permit becomes available")
            .expect("gate remains open"),
        );
    }

    /// Every upstream publishes 600 resources. Bounding the merged envelope
    /// must measure each resource once, not re-sort and re-serialize the whole
    /// merged list every time one more upstream completes.
    #[derive(Clone, Default)]
    struct ManyResourcesServer;

    impl ServerHandler for ManyResourcesServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult::with_all_items(
                (0..600)
                    .map(|index| Resource::new(format!("test://{index}"), format!("r{index}")))
                    .collect(),
            ))
        }
    }

    #[tokio::test]
    async fn merged_resource_cap_serializes_each_resource_once() {
        let pool = catalog_pool_with_server("many-0", ManyResourcesServer).await;
        for index in 1..5 {
            let name = format!("many-{index}");
            let other = catalog_pool_with_server(&name, ManyResourcesServer).await;
            let (connection, entry) = other.remove_connection_catalog_entry(&name).await;
            pool.install_connection_catalog_entry(
                name.clone(),
                connection.expect("fixture connection"),
                entry.expect("fixture entry"),
            )
            .await
            .expect("connection identity");
            pool.resource_upstreams.write().await.push(name);
        }
        pool.merged_resource_measurements.store(0, Ordering::SeqCst);

        let resources = pool.list_upstream_resources_allowed(None).await;

        assert_eq!(resources.len(), 3000.min(MAX_UPSTREAM_RESOURCES));
        let measurements = pool.merged_resource_measurements.load(Ordering::SeqCst);
        assert!(
            measurements <= 3000,
            "each resource must be measured once while bounding the merged envelope; measured {measurements} times"
        );
    }

    #[derive(Clone)]
    struct SchemaToolServer {
        tool_name: &'static str,
    }

    #[derive(Clone)]
    struct ToolsOnlyResourceProbeServer {
        resource_calls: Arc<AtomicUsize>,
    }

    impl ServerHandler for ToolsOnlyResourceProbeServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            Ok(ListToolsResult::with_all_items(vec![Tool::new(
                "example",
                "subject-specific tool",
                Arc::new(serde_json::Map::new()),
            )]))
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            self.resource_calls.fetch_add(1, Ordering::SeqCst);
            Err(ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                "resources/list should not be called",
                None,
            ))
        }

        async fn read_resource(
            &self,
            _request: rmcp::model::ReadResourceRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
            self.resource_calls.fetch_add(1, Ordering::SeqCst);
            Err(ErrorData::new(
                rmcp::model::ErrorCode::METHOD_NOT_FOUND,
                "resources/read should not be called",
                None,
            ))
        }
    }

    struct SlowResourceListServer;

    impl ServerHandler for SlowResourceListServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            // Long enough that the caller can only return by honoring its own
            // request budget, never by outlasting this fixture.
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok(ListResourcesResult::with_all_items(vec![Resource::new(
                "file:///slow",
                "slow",
            )]))
        }
    }

    impl ServerHandler for SchemaToolServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            Ok(ListToolsResult::with_all_items(vec![Tool::new(
                self.tool_name,
                "subject-specific tool",
                Arc::new(serde_json::Map::new()),
            )]))
        }
    }

    #[test]
    fn resource_catalog_timeout_caps_the_general_upstream_budget() {
        assert_eq!(
            catalog_listing_timeout(Duration::from_mins(1)),
            Duration::from_secs(10)
        );
        assert_eq!(
            catalog_listing_timeout(Duration::from_millis(25)),
            Duration::from_millis(25)
        );
    }

    fn oauth_schema_config(name: &str) -> UpstreamConfig {
        UpstreamConfig {
            display_name: None,
            lifecycle: None,
            enabled: true,
            name: name.to_string(),
            url: Some("http://127.0.0.1:1/mcp".to_string()),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: None,
            args: Vec::new(),
            env: Default::default(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: Some(UpstreamOauthConfig {
                mode: UpstreamOauthMode::AuthorizationCodePkce,
                registration: UpstreamOauthRegistration::Preregistered {
                    client_id: "test-client".to_string(),
                    client_secret_env: None,
                },
                scopes: None,
                credential: Default::default(),
                additional_endpoint_origins: vec![],
                prefer_client_metadata_document: None,
            }),
            imported_from: None,
            priority: 1.0,
        }
    }

    #[derive(Clone, Default)]
    struct PaginatedResourceTemplateServer {
        calls: Arc<AtomicUsize>,
    }

    impl ServerHandler for PaginatedResourceTemplateServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
        }

        async fn list_resource_templates(
            &self,
            request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourceTemplatesResult, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let cursor = request.and_then(|request| request.cursor);
            let mut result = match cursor.as_deref() {
                None => ListResourceTemplatesResult::with_all_items(vec![ResourceTemplate::new(
                    "file:///{path}",
                    "first",
                )]),
                Some("page-2") => {
                    ListResourceTemplatesResult::with_all_items(vec![ResourceTemplate::new(
                        "https://example.com/{id}",
                        "second",
                    )])
                }
                Some(other) => {
                    return Err(ErrorData::invalid_params(
                        format!("unexpected cursor: {other}"),
                        None,
                    ));
                }
            };
            if cursor.is_none() {
                result.next_cursor = Some("page-2".to_string());
            }
            Ok(result)
        }
    }

    #[test]
    fn resource_template_rewrite_preserves_nested_gateway_namespace() {
        let mut template =
            ResourceTemplate::new("lab://upstream/leaf/fixture://template/{value}", "nested");

        rewrite_resource_template(&mut template, "middle");

        assert_eq!(template.name, "middle/nested");
        assert_eq!(
            template.uri_template,
            "lab://upstream/middle/lab://upstream/leaf/fixture://template/{value}"
        );
    }

    #[tokio::test]
    async fn resource_template_catalog_traverses_and_namespaces_all_pages() {
        let server = PaginatedResourceTemplateServer::default();
        let calls = Arc::clone(&server.calls);
        let pool = catalog_pool_with_server("paged", server).await;

        let templates = pool
            .list_upstream_resource_templates_with_provenance_allowed(None)
            .await;
        assert_eq!(templates[0].upstream_name, "paged");
        assert_eq!(templates[0].native_uri_template, "file:///{path}");
        assert_eq!(
            templates[0].template.uri_template,
            "lab://upstream/paged/file:///{path}"
        );
        let rows = templates
            .iter()
            .map(|listed| {
                (
                    listed.template.name.as_str(),
                    listed.template.uri_template.as_str(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rows,
            vec![
                ("paged/first", "lab://upstream/paged/file:///{path}"),
                (
                    "paged/second",
                    "lab://upstream/paged/https://example.com/{id}",
                ),
            ]
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let published = pool
            .published_resource_template_catalog()
            .await
            .expect("published templates");
        assert_eq!(published.routes().len(), 2);
        assert_eq!(published.routes()[0].upstream_name.as_ref(), "paged");
        assert_eq!(
            published.routes()[0].native_uri_template.as_ref(),
            "file:///{path}"
        );
        assert_eq!(published.routes()[0].template.name, "first");
    }

    #[derive(Clone, Default)]
    struct PaginatedResourceServer {
        calls: Arc<AtomicUsize>,
    }

    impl ServerHandler for PaginatedResourceServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
        }

        async fn list_resources(
            &self,
            request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let cursor = request.and_then(|request| request.cursor);
            let mut result = match cursor.as_deref() {
                None => ListResourcesResult::with_all_items(vec![Resource::new(
                    "file:///first",
                    "first",
                )]),
                Some("page-2") => ListResourcesResult::with_all_items(vec![Resource::new(
                    "file:///second",
                    "second",
                )]),
                Some(other) => {
                    return Err(ErrorData::invalid_params(
                        format!("unexpected cursor: {other}"),
                        None,
                    ));
                }
            };
            if cursor.is_none() {
                result.next_cursor = Some("page-2".to_string());
            }
            Ok(result)
        }
    }

    #[tokio::test]
    async fn resource_catalog_traverses_all_upstream_pages() {
        let server = PaginatedResourceServer::default();
        let calls = Arc::clone(&server.calls);
        let pool = catalog_pool_with_server("paged", server).await;

        let listed = pool
            .list_upstream_resources_with_provenance_allowed(None)
            .await;
        assert_eq!(listed[0].upstream_name, "paged");
        assert_eq!(listed[0].native_uri, "file:///first");
        assert_eq!(listed[0].resource.uri, "lab://upstream/paged/file:///first");
        let resources = listed
            .into_iter()
            .map(|listed| listed.resource)
            .collect::<Vec<_>>();
        let uris = resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            uris,
            vec![
                "lab://upstream/paged/file:///first",
                "lab://upstream/paged/file:///second",
            ]
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let published = pool
            .published_resource_catalog()
            .await
            .expect("published resource catalog");
        assert_eq!(
            published
                .routes()
                .iter()
                .map(|route| route.native_uri.as_ref())
                .collect::<Vec<_>>(),
            vec!["file:///first", "file:///second"]
        );
        assert_eq!(published.routes()[0].resource.name, "first");
        assert_eq!(
            pool.catalog
                .read()
                .await
                .get("paged")
                .expect("paged catalog entry")
                .resource_count,
            2
        );
    }

    #[tokio::test]
    async fn resource_attribution_rejects_success_and_failure_after_same_object_aba() {
        let pool = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        let stale = pool
            .observe_routable_resource_connections(None)
            .await
            .pop()
            .expect("initial routable observation");
        let (connection_a, entry_a) = pool.remove_connection_catalog_entry("alpha").await;

        let replacement = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        let (connection_b, entry_b) = replacement.remove_connection_catalog_entry("alpha").await;
        pool.install_connection_catalog_entry(
            "alpha".to_string(),
            connection_b.expect("B connection"),
            entry_b.expect("B entry"),
        )
        .await
        .expect("install B");
        drop(pool.remove_connection_catalog_entry("alpha").await);
        pool.install_connection_catalog_entry(
            "alpha".to_string(),
            connection_a.expect("A connection"),
            entry_a.expect("A entry"),
        )
        .await
        .expect("reinstall A");

        let current = pool
            .observe_connection_catalog_entry("alpha")
            .await
            .expect("current observation");
        let current_rows = vec![Resource::new("file:///current", "current")];
        assert!(
            pool.apply_observed_resource_list_success(&current, &current_rows)
                .await
                .is_some()
        );
        let before = pool.catalog.read().await["alpha"].clone();

        let stale_rows = vec![Resource::new("file:///stale", "stale")];
        assert!(
            pool.apply_observed_resource_list_success(&stale, &stale_rows)
                .await
                .is_none()
        );
        assert!(
            !pool
                .apply_observed_resource_list_failure(&stale, "old A failed")
                .await
        );
        let after = &pool.catalog.read().await["alpha"];
        assert_eq!(after.resource_count, before.resource_count);
        assert_eq!(after.resource_uris, before.resource_uris);
        assert_eq!(after.resource_health, before.resource_health);
        assert_eq!(after.resource_last_error, before.resource_last_error);
    }

    #[derive(Clone)]
    struct DelayedResourceServer {
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    impl ServerHandler for DelayedResourceServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(ListResourcesResult::with_all_items(vec![Resource::new(
                "file:///old-a",
                "old A",
            )]))
        }
    }

    #[tokio::test]
    async fn live_resource_fanout_discards_delayed_result_after_replacement() {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let pool = catalog_pool_with_server(
            "alpha",
            DelayedResourceServer {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
            },
        )
        .await;
        let listing_pool = Arc::clone(&pool);
        let listing = tokio::spawn(async move { listing_pool.list_upstream_resources().await });
        started.notified().await;

        let replacement = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        let (connection, mut entry) = replacement.remove_connection_catalog_entry("alpha").await;
        let entry = entry.as_mut().expect("replacement entry");
        entry.resource_count = 7;
        entry.resource_uris = vec!["file:///replacement".to_string()];
        entry.resource_last_error = Some("replacement sentinel".to_string());
        let previous_a = pool
            .install_connection_catalog_entry(
                "alpha".to_string(),
                connection.expect("replacement connection"),
                entry.clone(),
            )
            .await
            .expect("install replacement")
            .expect("previous A connection remains alive");

        release.notify_one();
        assert!(listing.await.expect("listing task").is_empty());
        previous_a
            .shutdown("alpha", "test.resource-list.stale")
            .await;
        let current = &pool.catalog.read().await["alpha"];
        assert_eq!(current.resource_count, 7);
        assert_eq!(current.resource_uris, ["file:///replacement"]);
        assert_eq!(
            current.resource_last_error.as_deref(),
            Some("replacement sentinel")
        );
    }

    #[derive(Clone)]
    struct DelayedResourceTemplateServer {
        started: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        fail: bool,
    }

    impl ServerHandler for DelayedResourceTemplateServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
        }

        async fn list_resource_templates(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourceTemplatesResult, ErrorData> {
            self.started.notify_one();
            self.release.notified().await;
            if self.fail {
                Err(ErrorData::internal_error("old A failed", None))
            } else {
                Ok(ListResourceTemplatesResult::with_all_items(vec![
                    ResourceTemplate::new("file:///{path}", "old-a"),
                ]))
            }
        }
    }

    async fn assert_delayed_resource_template_result_is_discarded(fail: bool) {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let pool = catalog_pool_with_server(
            "alpha",
            DelayedResourceTemplateServer {
                started: Arc::clone(&started),
                release: Arc::clone(&release),
                fail,
            },
        )
        .await;
        let listing_pool = Arc::clone(&pool);
        let listing = tokio::spawn(async move {
            listing_pool
                .list_upstream_resource_templates_allowed(None)
                .await
        });
        started.notified().await;
        let mut original_entry = pool.catalog.read().await["alpha"].clone();

        let replacement = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        let (connection, mut entry) = replacement.remove_connection_catalog_entry("alpha").await;
        let entry = entry.as_mut().expect("replacement entry");
        entry.resource_last_error = Some("replacement sentinel".to_string());
        let previous_a = pool
            .install_connection_catalog_entry(
                "alpha".to_string(),
                connection.expect("replacement connection"),
                entry.clone(),
            )
            .await
            .expect("install replacement")
            .expect("previous A connection remains alive");

        // Reinstall the exact same A connection object after B. Its fresh
        // install incarnation must still invalidate the in-flight old-A result.
        let (replacement_b, _) = pool.remove_connection_catalog_entry("alpha").await;
        original_entry.resource_last_error = Some("reinstalled A sentinel".to_string());
        let replacement_health = original_entry.resource_health;
        let reinstalled_previous = pool
            .install_connection_catalog_entry("alpha".to_string(), previous_a, original_entry)
            .await
            .expect("reinstall same A object");
        assert!(reinstalled_previous.is_none());

        release.notify_one();
        assert!(listing.await.expect("listing task").is_empty());
        if let Some(replacement_b) = replacement_b {
            replacement_b
                .shutdown("alpha", "test.resource-template-list.replaced")
                .await;
        }
        let current = &pool.catalog.read().await["alpha"];
        assert_eq!(current.resource_health, replacement_health);
        assert_eq!(
            current.resource_last_error.as_deref(),
            Some("reinstalled A sentinel")
        );
    }

    #[tokio::test]
    async fn live_resource_template_fanout_discards_delayed_success_after_replacement() {
        assert_delayed_resource_template_result_is_discarded(false).await;
    }

    #[tokio::test]
    async fn live_resource_template_fanout_discards_delayed_failure_after_replacement() {
        assert_delayed_resource_template_result_is_discarded(true).await;
    }

    #[tokio::test]
    async fn current_resource_template_unsupported_result_records_success() {
        let pool = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        pool.catalog_write()
            .await
            .get_mut("alpha")
            .expect("alpha entry")
            .resource_last_error = Some("sentinel".into());
        assert!(
            pool.list_upstream_resource_templates_allowed(None)
                .await
                .is_empty()
        );
        assert!(
            pool.catalog.read().await["alpha"]
                .resource_last_error
                .is_none()
        );
    }

    #[tokio::test]
    async fn observed_resource_routing_honors_membership_health_and_allowlist() {
        let pool = catalog_pool_with_server("alpha", StaticCatalogServer::default()).await;
        let allowed = BTreeSet::from(["alpha".to_string()]);
        assert_eq!(
            pool.observe_routable_resource_connections(Some(&allowed))
                .await
                .len(),
            1
        );
        assert!(
            pool.observe_routable_resource_connections(Some(&BTreeSet::new()))
                .await
                .is_empty()
        );
        pool.record_failure_for("alpha", UpstreamCapability::Resources, "one")
            .await;
        pool.record_failure_for("alpha", UpstreamCapability::Resources, "two")
            .await;
        pool.record_failure_for("alpha", UpstreamCapability::Resources, "three")
            .await;
        assert!(
            pool.observe_routable_resource_connections(None)
                .await
                .is_empty()
        );
        pool.record_success_for("alpha", UpstreamCapability::Resources)
            .await;
        pool.resource_upstreams.write().await.clear();
        assert!(
            pool.observe_routable_resource_connections(None)
                .await
                .is_empty()
        );
    }

    async fn pool_with_empty_upstreams(names: &[&str]) -> UpstreamPool {
        let pool = UpstreamPool::new();
        let mut catalog = pool.catalog_write().await;
        for name in names {
            let entry = healthy_in_process_entry(Arc::from(*name), HashMap::new());
            catalog.insert((*name).to_string(), entry);
        }
        drop(catalog);
        pool
    }

    #[test]
    fn normalize_resource_result_uri_rewrites_all_contents() {
        let result = ReadResourceResult::new(vec![
            ResourceContents::text("hello", "http://upstream/resource"),
            ResourceContents::blob("YWJj", "file:///tmp/upstream"),
        ]);

        let normalized =
            normalize_resource_result_uri(result, "lab://upstream/demo/http://upstream/resource");

        let uris: Vec<_> = normalized
            .contents
            .iter()
            .map(|content| match content {
                ResourceContents::TextResourceContents { uri, .. }
                | ResourceContents::BlobResourceContents { uri, .. } => uri.as_str(),
                _ => "",
            })
            .collect();

        assert_eq!(
            uris,
            vec![
                "lab://upstream/demo/http://upstream/resource",
                "lab://upstream/demo/http://upstream/resource",
            ]
        );
    }

    #[tokio::test]
    async fn gateway_servers_doc_lists_one_healthy_upstream() {
        let pool = UpstreamPool::new();
        let mut tools = HashMap::new();
        tools.insert(
            "search".to_string(),
            UpstreamTool {
                tool: Tool::new(
                    "search",
                    "search the index",
                    Arc::new(serde_json::Map::new()),
                ),
                input_schema: Some(serde_json::json!({"type": "object"})),
                output_schema: None,
                upstream_name: Arc::from("alpha"),
                destructive: false,
            },
        );
        let entry = healthy_in_process_entry(Arc::from("alpha"), tools);
        pool.catalog
            .write()
            .await
            .insert("alpha".to_string(), entry);

        let doc = pool.gateway_servers_doc().await;
        let servers = doc
            .get("servers")
            .and_then(|v| v.as_array())
            .expect("servers array");
        assert_eq!(servers.len(), 1);
        let s = &servers[0];
        assert_eq!(s["name"], "alpha");
        assert_eq!(s["tool_count"], 1);
        assert_eq!(s["tool_health"], "healthy");
        assert!(s["tool_last_error"].is_null());
        assert_eq!(s["prompt_count"], 0);
        assert_eq!(s["resource_count"], 0);
    }

    #[tokio::test]
    async fn gateway_server_schema_respects_exposure_policy() {
        let make_tool = |name: &'static str| UpstreamTool {
            tool: Tool::new(name, "desc", Arc::new(serde_json::Map::new())),
            input_schema: Some(serde_json::json!({"type": "object"})),
            output_schema: None,
            upstream_name: Arc::from("alpha"),
            destructive: false,
        };

        let mut tools = HashMap::new();
        tools.insert("github_create".into(), make_tool("github_create"));
        tools.insert("delete_repo".into(), make_tool("delete_repo"));

        let mut entry = healthy_in_process_entry(Arc::from("alpha"), tools);
        entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["github_*".into()]).expect("policy");

        let pool = UpstreamPool::new();
        pool.catalog
            .write()
            .await
            .insert("alpha".to_string(), entry);

        let doc = pool.gateway_server_schema("alpha").await.expect("doc");
        let names: Vec<&str> = doc["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, vec!["github_create"]);
        assert_eq!(doc["health"], "healthy");
        assert!(doc["last_error"].is_null());
        assert_eq!(doc["name"], "alpha");
    }

    #[test]
    fn subject_scoped_gateway_schema_renders_and_filters_tools() {
        let tools = vec![
            Tool::new("hidden_tool", "hidden", Arc::new(serde_json::Map::new())),
            Tool::new(
                "fill_linear_dev_handoff",
                "prepare a Linear handoff",
                Arc::new(serde_json::Map::new()),
            ),
        ];
        let policy = ToolExposurePolicy::from_patterns(vec!["fill_*".to_string()]).expect("policy");

        let doc =
            render_subject_scoped_gateway_schema("notification_worker_linear", &tools, &policy);

        assert_eq!(doc["name"], "notification_worker_linear");
        assert_eq!(doc["health"], "healthy");
        assert!(doc["last_error"].is_null());
        assert_eq!(doc["catalog_source"], "subject_scoped_live");
        assert_eq!(doc["tools"][0]["name"], "fill_linear_dev_handoff");
        assert!(doc["tools"][0]["input_schema"].is_null());
        assert_eq!(doc["tools"].as_array().expect("tools array").len(), 1);
    }

    #[tokio::test]
    async fn subject_scoped_gateway_schema_uses_only_the_requested_subject_connection() {
        let pool = catalog_pool_with_server(
            "linear",
            SchemaToolServer {
                tool_name: "alice_tool",
            },
        )
        .await;
        let peer = pool
            .connections
            .read()
            .await
            .get("linear")
            .expect("linear connection")
            .peer
            .clone();
        let connection = pool
            .connections
            .write()
            .await
            .remove("linear")
            .expect("move connection into subject cache");
        pool.subject_connections.write().await.insert(
            ("linear".to_string(), "alice".to_string()),
            SubjectScopedConnection {
                optional_catalogs: Default::default(),
                _connection: connection,
                peer,
                tools: Vec::new(),
                last_used: Instant::now(),
            },
        );
        let config = oauth_schema_config("linear");
        pool.register_upstream_config_for_tests(&config);

        let alice = pool
            .subject_scoped_gateway_server_schema(&config, "alice")
            .await
            .expect("alice uses her cached peer");
        assert_eq!(alice["tools"][0]["name"], "alice_tool");

        let bob = pool
            .subject_scoped_gateway_server_schema(&config, "bob")
            .await
            .expect_err("bob must not reuse alice's subject connection");
        assert!(
            matches!(bob, ToolError::Sdk { .. }),
            "failure should stay classified: {bob:?}"
        );
    }

    #[tokio::test]
    async fn warming_cold_snapshots_lists_only_cold_upstreams_once() {
        let server = StaticCatalogServer::default();
        let resource_calls = Arc::clone(&server.list_resources_count);
        let pool = catalog_pool_with_server("cold", server).await;
        assert!(
            pool.cached_upstream_resources_allowed(None)
                .await
                .is_empty(),
            "a connected peer that was never listed has no cached resources"
        );
        let cold = pool.cold_resource_snapshots(None).await;
        assert_eq!(cold.missing, BTreeSet::from(["cold".to_string()]));
        assert!(cold.stale.is_empty());

        let warmed = pool.warm_cold_resource_snapshots_allowed(None).await;
        assert_eq!(warmed.missing, BTreeSet::from(["cold".to_string()]));
        assert_eq!(resource_calls.load(Ordering::SeqCst), 1);
        let cached = pool.cached_upstream_resources_allowed(None).await;
        assert_eq!(
            cached.len(),
            2,
            "warming publishes the snapshot: {cached:?}"
        );
        assert!(pool.cold_resource_snapshots(None).await.is_empty());

        let warmed = pool.warm_cold_resource_snapshots_allowed(None).await;
        assert!(warmed.is_empty());
        assert_eq!(
            resource_calls.load(Ordering::SeqCst),
            1,
            "a warm snapshot must not be listed again"
        );

        let other = BTreeSet::from(["other".to_string()]);
        pool.warm_cold_resource_snapshots_allowed(Some(&other))
            .await;
        assert_eq!(resource_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_warmups_share_one_fan_out() {
        let server = StaticCatalogServer::default();
        let resource_calls = Arc::clone(&server.list_resources_count);
        let pool = catalog_pool_with_server("cold", server).await;
        let warmups = (0..8)
            .map(|_| {
                let pool = Arc::clone(&pool);
                tokio::spawn(async move { pool.warm_cold_resource_snapshots_allowed(None).await })
            })
            .collect::<Vec<_>>();
        let mut listed = 0usize;
        for warmup in warmups {
            if !warmup.await.expect("warm-up task").is_empty() {
                listed += 1;
            }
        }
        assert_eq!(listed, 1, "exactly one caller found the snapshot cold");
        assert_eq!(
            resource_calls.load(Ordering::SeqCst),
            1,
            "queued callers must not repeat the fan-out"
        );
    }

    #[tokio::test]
    async fn stale_snapshot_without_push_channel_is_relisted_in_the_background() {
        let server = StaticCatalogServer::default();
        let resource_calls = Arc::clone(&server.list_resources_count);
        let pool = catalog_pool_with_server("legacy", server).await;
        pool.warm_cold_resource_snapshots_allowed(None).await;
        assert_eq!(resource_calls.load(Ordering::SeqCst), 1);

        pool.age_resource_snapshot_for_tests("legacy", RESOURCE_SNAPSHOT_MAX_AGE)
            .await;
        let cold = pool.cold_resource_snapshots(None).await;
        assert!(cold.missing.is_empty(), "an aged snapshot is still served");
        assert_eq!(cold.stale, BTreeSet::from(["legacy".to_string()]));

        let warmup = pool.spawn_resource_snapshot_warmup(None).await;
        assert!(warmup.cold.missing.is_empty());
        assert_eq!(
            pool.cached_upstream_resources_allowed(None).await.len(),
            2,
            "the current rows stay listable while the refresh runs"
        );
        warmup
            .task
            .expect("a stale snapshot starts a warm-up")
            .await
            .expect("warm-up task");
        assert_eq!(resource_calls.load(Ordering::SeqCst), 2);
        assert!(pool.cold_resource_snapshots(None).await.is_empty());

        // An upstream with an acknowledged subscriptions/listen stream
        // announces its own changes, so age alone does not make it stale.
        pool.age_resource_snapshot_for_tests("legacy", RESOURCE_SNAPSHOT_MAX_AGE)
            .await;
        pool.set_subscription_resources_for_test(HashMap::from([(
            "legacy".to_string(),
            BTreeSet::new(),
        )]))
        .await;
        assert!(pool.cold_resource_snapshots(None).await.is_empty());
        let warmup = pool.spawn_resource_snapshot_warmup(None).await;
        assert!(warmup.task.is_none());
        assert_eq!(resource_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn rejected_snapshot_is_omitted_and_retried_only_after_it_ages() {
        let server = RejectedCatalogServer::default();
        let resource_calls = Arc::clone(&server.list_resources_count);
        let pool = catalog_pool_with_server("dupes", server).await;
        pool.warm_cold_resource_snapshots_allowed(None).await;
        assert_eq!(resource_calls.load(Ordering::SeqCst), 1);
        assert!(
            pool.cached_upstream_resources_allowed(None)
                .await
                .is_empty(),
            "rows that were not retained cannot be listed"
        );
        assert!(
            pool.cold_resource_snapshots(None).await.is_empty(),
            "a rejected listing is settled for this incarnation"
        );
        pool.warm_cold_resource_snapshots_allowed(None).await;
        assert_eq!(resource_calls.load(Ordering::SeqCst), 1);

        pool.age_resource_snapshot_for_tests("dupes", RESOURCE_SNAPSHOT_MAX_AGE)
            .await;
        assert_eq!(
            pool.cold_resource_snapshots(None).await.stale,
            BTreeSet::from(["dupes".to_string()])
        );
        pool.warm_cold_resource_snapshots_allowed(None).await;
        assert_eq!(resource_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn oversized_ui_row_is_rejected_like_a_regular_row() {
        let pool = catalog_pool_with_server("apps", OversizedUiCatalogServer).await;
        pool.list_upstream_resources().await;
        assert!(
            pool.cached_upstream_resources_allowed(None)
                .await
                .is_empty(),
            "ui rows share the per-row byte cap"
        );
    }

    #[tokio::test]
    async fn cached_listing_applies_exposure_policy_to_ui_rows() {
        let pool = catalog_pool_with_server("apps", UiCatalogServer).await;
        pool.list_upstream_resources().await;
        async fn set_policy(pool: &UpstreamPool, patterns: &[&str]) {
            let mut catalog = pool.catalog_write().await;
            catalog
                .get_mut("apps")
                .expect("apps entry")
                .resource_exposure_policy = ToolExposurePolicy::from_patterns(
                patterns
                    .iter()
                    .map(|pattern| (*pattern).to_string())
                    .collect(),
            )
            .expect("valid allowlist");
        }
        let uris = |pool: Arc<UpstreamPool>| async move {
            pool.cached_upstream_resources_allowed(None)
                .await
                .into_iter()
                .map(|(_, resource)| resource.uri.to_string())
                .collect::<Vec<_>>()
        };

        set_policy(&pool, &["file:///tmp/regular"]).await;
        assert_eq!(uris(Arc::clone(&pool)).await, ["file:///tmp/regular"]);
        set_policy(&pool, &["ui://*"]).await;
        assert_eq!(uris(Arc::clone(&pool)).await, ["ui://apps/widget.html"]);
        set_policy(&pool, &["*"]).await;
        assert_eq!(
            uris(Arc::clone(&pool)).await,
            ["file:///tmp/regular", "ui://apps/widget.html"],
            "the snapshot stays unfiltered, so a policy change needs no re-list"
        );
    }

    #[derive(Clone, Default)]
    struct RejectedCatalogServer {
        list_resources_count: Arc<AtomicUsize>,
    }

    impl ServerHandler for RejectedCatalogServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            self.list_resources_count.fetch_add(1, Ordering::SeqCst);
            Ok(ListResourcesResult::with_all_items(vec![
                Resource::new("file:///tmp/twice", "first"),
                Resource::new("file:///tmp/twice", "second"),
            ]))
        }
    }

    #[derive(Clone)]
    struct OversizedUiCatalogServer;

    impl ServerHandler for OversizedUiCatalogServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult::with_all_items(vec![
                Resource::new("ui://apps/widget.html", "widget")
                    .with_description("x".repeat(2 * 1024 * 1024)),
            ]))
        }
    }

    #[tokio::test]
    async fn cached_listing_keeps_ui_rows_with_their_native_uri() {
        let pool = catalog_pool_with_server("apps", UiCatalogServer).await;
        pool.list_upstream_resources().await;
        let cached = pool
            .cached_upstream_resources_with_provenance_allowed(None)
            .await;
        let uris = cached
            .iter()
            .map(|listed| (listed.native_uri.as_str(), listed.resource.uri.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            uris,
            [
                (
                    "file:///tmp/regular",
                    "lab://upstream/apps/file:///tmp/regular"
                ),
                ("ui://apps/widget.html", "ui://apps/widget.html"),
            ]
        );
        let published = pool
            .published_resource_catalog()
            .await
            .expect("published resource catalog");
        assert_eq!(
            published
                .routes()
                .iter()
                .map(|route| route.native_uri.as_ref())
                .collect::<Vec<_>>(),
            vec!["file:///tmp/regular"],
            "ui rows never become published routes"
        );
    }

    #[derive(Clone)]
    struct UiCatalogServer;

    impl ServerHandler for UiCatalogServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
        }

        async fn list_resources(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListResourcesResult, ErrorData> {
            Ok(ListResourcesResult::with_all_items(vec![
                Resource::new("ui://apps/widget.html", "widget"),
                Resource::new("file:///tmp/regular", "regular"),
            ]))
        }
    }

    #[tokio::test]
    async fn subject_scoped_resources_reuse_the_cached_subject_connection() {
        let server = StaticCatalogServer::default();
        let resource_calls = Arc::clone(&server.list_resources_count);
        let pool = catalog_pool_with_server("google-drive", server).await;
        let peer = pool
            .connections
            .read()
            .await
            .get("google-drive")
            .expect("fixture connection")
            .peer
            .clone();
        let connection = pool
            .connections
            .write()
            .await
            .remove("google-drive")
            .expect("move fixture connection into subject cache");
        pool.subject_connections.write().await.insert(
            ("google-drive".to_string(), "alice".to_string()),
            SubjectScopedConnection {
                optional_catalogs: Default::default(),
                _connection: connection,
                peer,
                tools: Vec::new(),
                last_used: Instant::now(),
            },
        );
        let mut config = oauth_schema_config("google-drive");
        config.proxy_resources = true;
        pool.register_upstream_config_for_tests(&config);

        let baseline_calls = resource_calls.load(Ordering::SeqCst);
        let resources = pool
            .subject_scoped_resources(std::slice::from_ref(&config), "alice")
            .await;
        let uris = resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            uris,
            vec![
                "lab://upstream/google-drive/file:///tmp/upstream-one",
                "lab://upstream/google-drive/lab://upstream/old-name/file:///tmp/upstream-two",
            ]
        );

        assert_eq!(
            resource_calls.load(Ordering::SeqCst),
            baseline_calls + 1,
            "the cold subject catalog should perform one resources/list RPC"
        );

        let cached = pool
            .subject_scoped_resources(std::slice::from_ref(&config), "alice")
            .await;
        let cached_uris = cached
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>();
        assert_eq!(cached_uris, uris);
        assert_eq!(
            resource_calls.load(Ordering::SeqCst),
            baseline_calls + 1,
            "the warm subject catalog must not repeat resources/list"
        );

        // Subject connections have no push channel: the cached catalog is
        // re-fetched once it is older than the freshness bound.
        pool.age_subject_resource_catalog_for_tests(
            "google-drive",
            "alice",
            RESOURCE_SNAPSHOT_MAX_AGE,
        )
        .await;
        let refreshed = pool
            .subject_scoped_resources(std::slice::from_ref(&config), "alice")
            .await;
        assert_eq!(refreshed.len(), uris.len());
        assert_eq!(
            resource_calls.load(Ordering::SeqCst),
            baseline_calls + 2,
            "an aged subject catalog is listed again"
        );

        // An upstream list_changed drops every subject's cached catalog for
        // that upstream without touching the connections themselves.
        assert!(
            !pool
                .refresh_resources_after_list_changed("google-drive")
                .await
        );
        assert!(
            pool.connections.read().await.get("google-drive").is_none(),
            "the connection moved to the subject cache and stays there"
        );
        pool.subject_scoped_resources(std::slice::from_ref(&config), "alice")
            .await;
        assert_eq!(
            resource_calls.load(Ordering::SeqCst),
            baseline_calls + 3,
            "list_changed invalidates the subject catalog"
        );
        pool.subject_scoped_resources(std::slice::from_ref(&config), "alice")
            .await;
        assert_eq!(resource_calls.load(Ordering::SeqCst), baseline_calls + 3);
    }

    #[tokio::test]
    async fn subject_scoped_resources_honor_initialize_and_skip_unadvertised_capability() {
        let resource_calls = Arc::new(AtomicUsize::new(0));
        let pool = catalog_pool_with_server(
            "tools-only",
            ToolsOnlyResourceProbeServer {
                resource_calls: Arc::clone(&resource_calls),
            },
        )
        .await;
        let peer = pool
            .connections
            .read()
            .await
            .get("tools-only")
            .expect("fixture connection")
            .peer
            .clone();
        let connection = pool
            .connections
            .write()
            .await
            .remove("tools-only")
            .expect("move fixture connection into subject cache");
        pool.subject_connections.write().await.insert(
            ("tools-only".to_string(), "alice".to_string()),
            SubjectScopedConnection {
                optional_catalogs: Default::default(),
                _connection: connection,
                peer,
                tools: Vec::new(),
                last_used: Instant::now(),
            },
        );
        let mut config = oauth_schema_config("tools-only");
        config.proxy_resources = true;
        pool.register_upstream_config_for_tests(&config);

        let resources = pool
            .subject_scoped_resources(std::slice::from_ref(&config), "alice")
            .await;

        assert!(resources.is_empty());
        assert_eq!(
            resource_calls.load(Ordering::SeqCst),
            0,
            "resources/list must not be sent when initialize omits resources"
        );
        let connections = pool.subject_connections.read().await;
        let subject = connections
            .get(&("tools-only".to_string(), "alice".to_string()))
            .expect("subject connection remains cached");
        let empty: &[Resource] = &[];
        assert_eq!(subject.optional_catalogs.resources.as_deref(), Some(empty));
        drop(connections);

        let error = pool
            .subject_scoped_read_resource_request(
                &config,
                "alice",
                rmcp::model::ReadResourceRequestParams::new(
                    "lab://upstream/tools-only/file:///missing".to_string(),
                ),
            )
            .await
            .expect_err("resources/read must fail before RPC when resources are not advertised");
        assert!(error.contains("does not advertise the MCP resources capability"));
        assert_eq!(
            resource_calls.load(Ordering::SeqCst),
            0,
            "resources/read must not be sent when initialize omits resources"
        );
    }

    /// Ceiling for the "did the caller give up on its own budget?" assertions
    /// below.
    ///
    /// Those tests pair a 25ms request budget with a fixture that stalls for
    /// 30 seconds. The ceiling only has to prove the caller returned on its own
    /// budget instead of waiting for the upstream, so anything far below the
    /// stall does the job. It used to sit at 100ms against a 200ms stall, close
    /// enough to the budget that scheduler jitter under parallel test load
    /// pushed a correct run over it.
    const STALLED_UPSTREAM_CEILING: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn subject_scoped_resources_bound_a_stalled_upstream() {
        let pool = catalog_pool_with_server("slow", SlowResourceListServer).await;
        let peer = pool
            .connections
            .read()
            .await
            .get("slow")
            .expect("fixture connection")
            .peer
            .clone();
        let connection = pool
            .connections
            .write()
            .await
            .remove("slow")
            .expect("move fixture connection into subject cache");
        pool.subject_connections.write().await.insert(
            ("slow".to_string(), "alice".to_string()),
            SubjectScopedConnection {
                optional_catalogs: Default::default(),
                _connection: connection,
                peer,
                tools: Vec::new(),
                last_used: Instant::now(),
            },
        );
        let mut pool = Arc::try_unwrap(pool)
            .ok()
            .expect("fixture pool has one owner");
        pool.request_timeout = Duration::from_millis(25);
        let mut config = oauth_schema_config("slow");
        config.proxy_resources = true;
        pool.register_upstream_config_for_tests(&config);

        let started = Instant::now();
        let resources = pool.subject_scoped_resources(&[config], "alice").await;

        assert!(
            resources.is_empty(),
            "a timed-out upstream yields partial data"
        );
        assert!(
            started.elapsed() < STALLED_UPSTREAM_CEILING,
            "a stalled upstream exceeded the request budget: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn subject_scoped_resources_bound_connection_acquisition() {
        let mut pool = UpstreamPool::new();
        pool.request_timeout = Duration::from_millis(25);
        let connect_lock = Arc::new(tokio::sync::Mutex::new(()));
        pool.subject_connect_locks.write().await.insert(
            ("slow-connect".to_string(), "alice".to_string()),
            Arc::clone(&connect_lock),
        );
        let guard = connect_lock.lock_owned().await;
        tokio::spawn(async move {
            // As above: the caller must give up on its own budget rather than
            // wait for this lock to free.
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(guard);
        });
        let mut config = oauth_schema_config("slow-connect");
        config.proxy_resources = true;
        pool.register_upstream_config_for_tests(&config);

        let started = Instant::now();
        let resources = pool.subject_scoped_resources(&[config], "alice").await;

        assert!(
            resources.is_empty(),
            "a timed-out connect yields partial data"
        );
        assert!(
            started.elapsed() < STALLED_UPSTREAM_CEILING,
            "connection acquisition exceeded the request budget: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn shared_resources_bound_a_stalled_upstream() {
        let pool = catalog_pool_with_server("slow", SlowResourceListServer).await;
        let mut pool = Arc::try_unwrap(pool)
            .ok()
            .expect("fixture pool has one owner");
        pool.request_timeout = Duration::from_millis(25);

        let started = Instant::now();
        let resources = pool.list_upstream_resources().await;

        assert!(
            resources.is_empty(),
            "a timed-out upstream yields partial data"
        );
        assert!(
            started.elapsed() < STALLED_UPSTREAM_CEILING,
            "a stalled upstream exceeded the request budget: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn gateway_server_schema_unknown_upstream_returns_none() {
        let pool = UpstreamPool::new();
        assert!(pool.gateway_server_schema("nope").await.is_none());
    }

    #[tokio::test]
    async fn gateway_synthetic_resources_lists_index_and_per_upstream() {
        let pool = pool_with_empty_upstreams(&["alpha", "beta"]).await;

        let resources = pool.gateway_synthetic_resources().await;
        let uris: Vec<String> = resources.iter().map(|r| r.uri.clone()).collect();
        assert!(uris.iter().any(|u| u == "lab://gateway/servers"));
        assert!(uris.iter().any(|u| u == "lab://gateway/alpha/schema"));
        assert!(uris.iter().any(|u| u == "lab://gateway/beta/schema"));
        assert_eq!(uris.len(), 3);
    }

    #[tokio::test]
    async fn gateway_synthetic_resources_respect_allowed_upstreams() {
        let pool = pool_with_empty_upstreams(&["alpha", "beta"]).await;
        let allowed = BTreeSet::from(["alpha".to_string()]);

        let resources = pool
            .gateway_synthetic_resources_allowed(Some(&allowed))
            .await;
        let uris: Vec<String> = resources.iter().map(|r| r.uri.clone()).collect();
        assert!(uris.iter().any(|u| u == "lab://gateway/servers"));
        assert!(uris.iter().any(|u| u == "lab://gateway/alpha/schema"));
        assert!(!uris.iter().any(|u| u == "lab://gateway/beta/schema"));
        assert_eq!(uris.len(), 2);

        let doc = pool.gateway_servers_doc_allowed(Some(&allowed)).await;
        let servers = doc
            .get("servers")
            .and_then(|v| v.as_array())
            .expect("servers array");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], "alpha");

        assert!(
            pool.gateway_server_schema_allowed("beta", Some(&allowed))
                .await
                .is_none()
        );
    }
}
