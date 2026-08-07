//! Resource listing and the synthetic gateway documents.
//!
//! `list_upstream_resources` / `subject_scoped_resources` enumerate proxied
//! upstream resources (rewriting URIs to the gateway-prefixed form), while the
//! `gateway_*` methods render the synthetic `lab://gateway/*` documents and
//! resources. `cached_upstream_resource_uris` exposes the cached snapshot.

use std::collections::BTreeSet;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rmcp::model::{Resource, ResourceTemplate};
use serde_json::Value;

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::{ToolExposurePolicy, UpstreamCapability};
use super::UpstreamPool;
use super::capability_call::bounded_service_error_text;
use super::connect::connect_upstream;
use super::discover::routable_upstream_peers;
use super::entries::{
    health_str, log_exposure_filter, resolve_request_resource_exposure_policy, resource_exposed,
};
use super::helpers::{bare_upstream_resource_uri, classify_upstream_error, rewrite_resource_uri};
use super::logging::is_capability_unsupported;
use super::tools::MAX_UPSTREAM_RESOURCES;

fn rewrite_resource_template(template: &mut ResourceTemplate, upstream_name: &str) {
    template.name = format!("{upstream_name}/{}", template.name);
    if !template.uri_template.starts_with("ui://") {
        template.uri_template = format!("lab://upstream/{upstream_name}/{}", template.uri_template);
    }
}

impl UpstreamPool {
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
            .map(|t| {
                serde_json::json!({
                    "name": t.tool.name.as_ref(),
                    "description": t.tool.description.as_ref().map(|s| s.as_ref()),
                    "input_schema": t.input_schema,
                    "meta": t.tool.meta,
                })
            })
            .collect();
        tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Some(serde_json::json!({
            "name": name,
            "tools": tools,
            "health": health_str(entry.tool_health),
            "last_error": entry.tool_last_error,
        }))
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
        let peers = routable_upstream_peers(self, UpstreamCapability::Resources, allowed).await;
        if peers.is_empty() {
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
        for (name, peer) in peers {
            futures.push(async move {
                let result = peer.list_all_resources().await;
                (name, result)
            });
        }

        let mut results: Vec<(String, Result<_, _>)> = Vec::new();
        while let Some(item) = futures.next().await {
            results.push(item);
        }
        results.sort_unstable_by(|a, b| a.0.cmp(&b.0));

        let mut resources = Vec::new();
        let mut subscription_refreshes = Vec::new();
        for (name, result) in results {
            match result {
                Ok(upstream_resources) => {
                    self.record_success_for(&name, UpstreamCapability::Resources)
                        .await;
                    let resource_uris = upstream_resources
                        .iter()
                        .map(|resource| bare_upstream_resource_uri(&resource.uri).to_string())
                        .collect();
                    // The cached snapshot deliberately stays *unfiltered*: it is
                    // what `gateway.discovered_resources` shows the operator who
                    // is editing `expose_resources`, and hiding excluded URIs
                    // there would make the allowlist un-editable. Enforcement
                    // happens on what leaves this function, and on every read.
                    let policy = {
                        let mut catalog = self.catalog.write().await;
                        match catalog.get_mut(&name) {
                            Some(entry) => {
                                entry.resource_count = upstream_resources.len();
                                entry.resource_uris = resource_uris;
                                entry.resource_exposure_policy.clone()
                            }
                            // Unreachable in practice — the peer list came from
                            // the catalog — but an upstream that vanished
                            // mid-listing has no known policy, so hide it.
                            None => ToolExposurePolicy::AllowList(Vec::new()),
                        }
                    };
                    subscription_refreshes.push(name.clone());
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
                        if !resource.uri.starts_with("ui://") {
                            rewrite_resource_uri(&mut resource, &name);
                        }
                        resources.push(resource);
                        exposed_count += 1;
                    }
                    log_exposure_filter(&name, "resources", hidden_count, exposed_count, false);
                }
                Err(e) if is_capability_unsupported(&e) => {
                    // The upstream simply doesn't implement `resources/list`
                    // (JSON-RPC -32601). This is expected capability negotiation,
                    // not a failure: treat it like an empty, successful listing so
                    // the upstream stays routable and accrues no phantom failures.
                    self.record_success_for(&name, UpstreamCapability::Resources)
                        .await;
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.resource_count = 0;
                            entry.resource_uris.clear();
                        }
                    }
                    tracing::debug!(
                        upstream = %name,
                        error = %e,
                        "upstream does not implement resources/list — capability absent"
                    );
                }
                Err(e) => {
                    let error_text = bounded_service_error_text(&e);
                    self.record_failure_for(
                        &name,
                        UpstreamCapability::Resources,
                        format!("failed to list resources from upstream: {error_text}"),
                    )
                    .await;
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.resource_count = 0;
                            entry.resource_uris.clear();
                        }
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

        self.refresh_upstream_subscriptions_concurrently(subscription_refreshes)
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
        let peers = routable_upstream_peers(self, UpstreamCapability::Resources, allowed).await;
        if peers.is_empty() {
            return Vec::new();
        }

        // Deliberate bulkhead exception + partial-result semantics — same
        // contract as `list_upstream_resources_allowed` above.
        let mut futures = FuturesUnordered::new();
        for (name, peer) in peers {
            futures.push(async move {
                let result = peer.list_all_resource_templates().await;
                (name, result)
            });
        }

        let mut results = Vec::new();
        while let Some(item) = futures.next().await {
            results.push(item);
        }
        results.sort_unstable_by(|left, right| left.0.cmp(&right.0));

        let mut templates = Vec::new();
        for (name, result) in results {
            match result {
                Ok(upstream_templates) => {
                    self.record_success_for(&name, UpstreamCapability::Resources)
                        .await;
                    for mut template in upstream_templates {
                        if templates.len() >= MAX_UPSTREAM_RESOURCES {
                            tracing::warn!(
                                upstream = %name,
                                limit = MAX_UPSTREAM_RESOURCES,
                                "upstream resource template catalog exceeds limit — truncating to cap"
                            );
                            break;
                        }
                        rewrite_resource_template(&mut template, &name);
                        templates.push(template);
                    }
                }
                Err(error) if is_capability_unsupported(&error) => {
                    self.record_success_for(&name, UpstreamCapability::Resources)
                        .await;
                    tracing::debug!(
                        upstream = %name,
                        error = %error,
                        "upstream does not implement resources/templates/list — capability absent"
                    );
                }
                Err(error) => {
                    let error_text = bounded_service_error_text(&error);
                    self.record_failure_for(
                        &name,
                        UpstreamCapability::Resources,
                        format!("failed to list resource templates from upstream: {error_text}"),
                    )
                    .await;
                    tracing::warn!(
                        upstream = %name,
                        kind = classify_upstream_error(&error_text),
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
        let oauth_client_cache = self.oauth_client_cache.clone();
        for config in configs
            .iter()
            .filter(|config| config.oauth.is_some() && config.proxy_resources)
        {
            let config = config.clone();
            let subject = subject.to_string();
            let oauth_client_cache = oauth_client_cache.clone();
            futures.push(async move {
                // Subject-scoped resources are discovered over a per-(upstream,
                // subject) connection and never land in `self.catalog`, so
                // there is no `UpstreamEntry::resource_exposure_policy` to
                // read. Resolve the same fail-closed policy from the live
                // config instead — the seam `subject_scoped_tools` uses.
                let policy = resolve_request_resource_exposure_policy(
                    &config.name,
                    config.expose_resources.clone(),
                );
                let result = connect_upstream(
                    &config,
                    Some(subject.as_str()),
                    oauth_client_cache.as_ref(),
                    None,
                    None,
                )
                .await
                .map(|(conn, _)| conn);
                (config.name.clone(), policy, result)
            });
        }

        // Deliberate bulkhead exception + partial-result semantics — same
        // contract as `list_upstream_resources_allowed`. These are ephemeral
        // per-subject connections, so failures are surfaced via the classified
        // `warn!`s below rather than the shared circuit breaker.
        let mut resources = Vec::new();
        while let Some((name, policy, result)) = futures.next().await {
            let conn = match result {
                Ok(conn) => conn,
                Err(error) => {
                    let error_text = error.to_string();
                    tracing::warn!(
                        upstream = %name,
                        kind = classify_upstream_error(&error_text),
                        error = %error_text,
                        "subject-scoped upstream resource connect failed"
                    );
                    continue;
                }
            };
            match conn.peer.list_all_resources().await {
                Ok(upstream_resources) => {
                    let mut hidden_count = 0usize;
                    let mut exposed_count = 0usize;
                    for mut resource in upstream_resources {
                        if !resource_exposed(&policy, bare_upstream_resource_uri(&resource.uri)) {
                            hidden_count += 1;
                            continue;
                        }
                        rewrite_resource_uri(&mut resource, &name);
                        resources.push(resource);
                        exposed_count += 1;
                    }
                    log_exposure_filter(&name, "resources", hidden_count, exposed_count, true);
                }
                Err(error) => {
                    let error_text = bounded_service_error_text(&error);
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
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rmcp::model::{
        ErrorData, ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams,
        ReadResourceResult, ResourceContents, ResourceTemplate, ServerCapabilities, ServerInfo,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler};

    use super::super::super::types::{ToolExposurePolicy, UpstreamTool};
    use super::super::entries::healthy_in_process_entry;
    use super::super::helpers::normalize_resource_result_uri;
    use super::super::testsupport::catalog_pool_with_server;
    use super::*;

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

        let templates = pool.list_upstream_resource_templates_allowed(None).await;
        let rows = templates
            .iter()
            .map(|template| (template.name.as_str(), template.uri_template.as_str()))
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

        let resources = pool.list_upstream_resources().await;
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

    async fn pool_with_empty_upstreams(names: &[&str]) -> UpstreamPool {
        let pool = UpstreamPool::new();
        let mut catalog = pool.catalog.write().await;
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
                tool: rmcp::model::Tool::new(
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
            tool: rmcp::model::Tool::new(name, "desc", Arc::new(serde_json::Map::new())),
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
