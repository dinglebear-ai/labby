//! Resource listing and the synthetic gateway documents.
//!
//! `list_upstream_resources` / `subject_scoped_resources` enumerate proxied
//! upstream resources (rewriting URIs to the gateway-prefixed form), while the
//! `gateway_*` methods render the synthetic `lab://gateway/*` documents and
//! resources. `cached_upstream_resource_uris` exposes the cached snapshot.

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
use super::capability_call::{
    CapabilityCallError, bounded_service_error_text, timed_capability_call,
    timed_capability_call_with_timeout,
};
use super::catalog_pagination;
use super::entries::{
    health_str, log_exposure_filter, resolve_exposure_policy,
    resolve_request_resource_exposure_policy, resource_exposed,
};
use super::helpers::{
    bare_upstream_resource_uri, classify_upstream_error, max_response_bytes, rewrite_resource_uri,
    upstream_discovery_timeout, upstream_transport,
};
use super::logging::{
    UpstreamRequestLog, is_capability_unsupported, log_upstream_request_error,
    log_upstream_request_finish, log_upstream_request_start,
};
use super::tools::MAX_UPSTREAM_RESOURCES;

/// Wall-clock cap for one upstream's catalog listing on the connect path.
///
/// Shared by the resource and prompt refreshes so the two budgets cannot drift
/// apart: both run while the lazy-connect mutex and the write-preferring
/// `oauth_invalidation_barrier` read guard are held, so an unbounded listing
/// stalls every queued OAuth writer behind one slow upstream.
const CATALOG_LISTING_TIMEOUT: Duration = Duration::from_secs(10);

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

    /// List resources from all resource-proxy-enabled upstreams.
    ///
    /// Resources are prefixed with `lab://upstream/{name}/` to avoid collisions.
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

    pub async fn list_upstream_resources_with_provenance_allowed(
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
        let mut futures = FuturesUnordered::new();
        let shared_budget = Arc::new(catalog_pagination::SharedCatalogBudget::new(
            MAX_UPSTREAM_RESOURCES,
            max_response_bytes(),
        ));
        for observed in observed_peers {
            let name = observed.upstream().to_string();
            let peer = observed.peer.clone();
            let request_timeout = catalog_listing_timeout(self.request_timeout);
            let shared_budget = Arc::clone(&shared_budget);
            futures.push(async move {
                let started = Instant::now();
                let event = UpstreamRequestLog::resources_list(&name, false);
                log_upstream_request_start(event);
                let result = match catalog_pagination::list_resources_with_budget(
                    &peer,
                    request_timeout,
                    MAX_UPSTREAM_RESOURCES,
                    &shared_budget,
                )
                .await
                {
                    Ok(resources) => {
                        let response_bytes =
                            serde_json::to_vec(&resources).map_or(usize::MAX, |body| body.len());
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
                        log_upstream_request_finish(event, started.elapsed().as_millis(), Some(0));
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
                };
                (observed, result)
            });
        }

        let mut results = Vec::new();
        while let Some(item) = futures.next().await {
            results.push(item);
        }
        results.sort_unstable_by(|a, b| a.0.upstream().cmp(b.0.upstream()));

        let mut resources = Vec::new();
        let mut subscription_refreshes = Vec::new();
        for (observed, result) in results {
            let name = observed.upstream().to_string();
            match result {
                Ok(upstream_resources) => {
                    // The cached snapshot deliberately stays *unfiltered*: it is
                    // what `gateway.discovered_resources` shows the operator who
                    // is editing `expose_resources`, and hiding excluded URIs
                    // there would make the allowlist un-editable. Enforcement
                    // happens on what leaves this function, and on every read.
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
                        if resources.len() >= MAX_UPSTREAM_RESOURCES {
                            tracing::warn!(
                                upstream = %name,
                                limit = MAX_UPSTREAM_RESOURCES,
                                "upstream resource catalog exceeds limit — truncating to cap"
                            );
                            break;
                        }
                        // MCP Apps (mcp-ui) widget resources keep their native
                        // `ui://…` URI: a tool result's `_meta.ui.resourceUri`
                        // references that exact URI, and the host reads it back
                        // verbatim (routed via `read_upstream_ui_resource`).
                        // Rewriting to the `lab://upstream/{name}/…` gateway form
                        // would break that reference, so skip the rewrite here.
                        let native_uri = resource.uri.clone();
                        if !resource.uri.starts_with("ui://") {
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
                let result = catalog_pagination::list_resource_templates_with_budget(
                    &peer,
                    self.request_timeout,
                    MAX_UPSTREAM_RESOURCES,
                    &shared_budget,
                )
                .await;
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
                        pool.record_failure_for(
                            &config.name,
                            UpstreamCapability::Resources,
                            format!("upstream connect failed: {error}"),
                        )
                        .await;
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
                        pool.record_failure_for(
                            &config.name,
                            UpstreamCapability::Resources,
                            error.clone(),
                        )
                        .await;
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
                let timeout_ms = request_timeout.as_millis();
                let result = timed_capability_call_with_timeout(
                    &pool,
                    request_timeout,
                    &config.name,
                    UpstreamCapability::Resources,
                    event,
                    started,
                    async {
                        catalog_pagination::list_resources(
                            &peer,
                            request_timeout,
                            MAX_UPSTREAM_RESOURCES,
                        )
                        .await
                        .map_err(|error| error.into_service_error(&config.name))
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

    #[derive(Clone)]
    struct SchemaToolServer {
        tool_name: &'static str,
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
                _connection: connection,
                peer,
                tools: Vec::new(),
                last_used: Instant::now(),
            },
        );
        let config = oauth_schema_config("linear");

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
    async fn subject_scoped_resources_reuse_the_cached_subject_connection() {
        let pool = catalog_pool_with_server("google-drive", StaticCatalogServer::default()).await;
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
                _connection: connection,
                peer,
                tools: Vec::new(),
                last_used: Instant::now(),
            },
        );
        let mut config = oauth_schema_config("google-drive");
        config.proxy_resources = true;

        let resources = pool.subject_scoped_resources(&[config], "alice").await;
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
