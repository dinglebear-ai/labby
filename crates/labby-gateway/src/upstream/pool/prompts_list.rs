//! Prompt listing and ownership lookup.
//!
//! `collect_upstream_prompts` is the single shared RPC pass; `list_upstream_prompts`,
//! `prompt_ownership_map`, and `find_prompt_owner` are built on it, plus the cached
//! name/owner snapshot helpers. `cached_prompt_owner` is private and stays
//! co-located with its only caller (`find_prompt_owner`) — no `pub(super)` needed
//! (plan §2.1 drop note).

use std::collections::BTreeSet;
use std::collections::HashMap;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rmcp::model::Prompt;

use super::super::types::UpstreamCapability;
use super::UpstreamPool;
use super::catalog_pagination;
use super::discover::routable_upstream_peers;
use super::helpers::merge_upstream_prompts;
use super::logging::is_capability_unsupported;
use super::tools::MAX_UPSTREAM_PROMPTS;

impl UpstreamPool {
    /// Fetch prompts from all healthy upstreams and merge them, returning both the
    /// deduplicated prompt list and the ownership map (prompt_name -> upstream_name).
    ///
    /// This is the single RPC pass shared by all prompt-related queries.
    async fn collect_upstream_prompts(
        &self,
        builtin_names: &[&str],
        allowed: Option<&BTreeSet<String>>,
        deadline_at: tokio::time::Instant,
    ) -> (Vec<Prompt>, HashMap<String, String>) {
        let peers = routable_upstream_peers(self, UpstreamCapability::Prompts, allowed).await;

        // Deliberate bulkhead exception: this is a fan-out aggregation pass
        // over every routable upstream (catalog listing/refresh), not a
        // caller-attributed RPC, so it does not take the per-upstream
        // `timed_capability_call` permit. Per-upstream failures deliberately
        // degrade the merged result to partial data — the MCP `prompts/list`
        // wire shape carries no per-upstream error field (do not invent one).
        // Failure visibility instead lives in the circuit breaker +
        // `prompt_last_error` recorded below (surfaced via `gateway.status`)
        // and the classified `warn!` per failing upstream.
        //
        // Issue RPCs in parallel. merge_upstream_prompts sorts internally,
        // so completion order does not affect the final result.
        let mut futures = FuturesUnordered::new();
        for (name, peer) in peers {
            futures.push(async move {
                let remaining = deadline_at.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return (
                        name,
                        Err(catalog_pagination::CatalogPaginationError::Deadline {
                            deadline_ms: 0,
                        }),
                    );
                }
                let result =
                    catalog_pagination::list_prompts(&peer, remaining, MAX_UPSTREAM_PROMPTS).await;
                (name, result)
            });
        }

        let mut upstream_prompts = Vec::new();
        let mut prompt_name_updates: HashMap<String, Vec<String>> = HashMap::new();
        while let Some((name, result)) = futures.next().await {
            match result {
                Ok(prompts) => {
                    self.record_success_for(&name, UpstreamCapability::Prompts)
                        .await;
                    prompt_name_updates.insert(name.clone(), Vec::new());
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.prompt_count = prompts.len();
                        }
                    }
                    upstream_prompts.push((name, prompts));
                }
                Err(catalog_pagination::CatalogPaginationError::Service(e))
                    if is_capability_unsupported(&e) =>
                {
                    // The upstream simply doesn't implement `prompts/list`
                    // (JSON-RPC -32601). This is expected capability negotiation,
                    // not a failure: treat it like an empty, successful listing so
                    // the upstream stays routable and accrues no phantom failures.
                    self.record_success_for(&name, UpstreamCapability::Prompts)
                        .await;
                    prompt_name_updates.insert(name.clone(), Vec::new());
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.prompt_count = 0;
                        }
                    }
                    tracing::debug!(
                        upstream = %name,
                        error = %e,
                        "upstream does not implement prompts/list — capability absent"
                    );
                }
                Err(e) => {
                    let error_text = e.bounded_text();
                    if matches!(
                        e,
                        catalog_pagination::CatalogPaginationError::Deadline { .. }
                    ) {
                        tracing::warn!(
                            upstream = %name,
                            kind = "timeout",
                            phase = "raw_pagination",
                            partial_result = true,
                            "prompt catalog upstream exceeded shared request deadline"
                        );
                    }
                    self.record_failure_for(
                        &name,
                        UpstreamCapability::Prompts,
                        format!("failed to list prompts from upstream: {error_text}"),
                    )
                    .await;
                    {
                        let mut catalog = self.catalog.write().await;
                        if let Some(entry) = catalog.get_mut(&name) {
                            entry.prompt_count = 0;
                        }
                    }
                    tracing::warn!(
                        upstream = %name,
                        kind = e.kind(),
                        error = %error_text,
                        "failed to list prompts from upstream"
                    );
                }
            }
        }

        let (mut prompts, owners) = merge_upstream_prompts(builtin_names, upstream_prompts);
        if prompts.len() > MAX_UPSTREAM_PROMPTS {
            prompts.truncate(MAX_UPSTREAM_PROMPTS);
            tracing::warn!(
                limit = MAX_UPSTREAM_PROMPTS,
                "upstream prompt catalog exceeds limit — truncating to cap"
            );
        }
        if !prompt_name_updates.is_empty() {
            for prompt in &prompts {
                if let Some(upstream_name) = owners.get(prompt.name.as_str())
                    && let Some(names) = prompt_name_updates.get_mut(upstream_name)
                {
                    names.push(prompt.name.to_string());
                }
            }
            let mut catalog = self.catalog.write().await;
            for (upstream_name, names) in prompt_name_updates {
                if let Some(entry) = catalog.get_mut(&upstream_name) {
                    entry.prompt_names = names;
                }
            }
        }

        // Filter *after* the cache write, not before: the cached `prompt_names`
        // snapshot deliberately stays unfiltered because it is what
        // `gateway.discovered_prompts` shows the operator who is editing
        // `expose_prompts`, and hiding excluded prompts there would make the
        // allowlist un-editable. The cache is only an ownership hint — routing a
        // hidden prompt through it still fails, because `get_prompt` re-checks
        // the policy before forwarding.
        let prompts = self.retain_exposed_prompts(prompts, &owners).await;

        (prompts, owners)
    }

    /// List prompts from all healthy upstreams, filtering built-in and cross-upstream collisions.
    pub async fn list_upstream_prompts(&self, builtin_names: &[&str]) -> Vec<Prompt> {
        let deadline_at = tokio::time::Instant::now() + self.request_timeout;
        let (prompts, _) = self
            .collect_upstream_prompts(builtin_names, None, deadline_at)
            .await;
        prompts
    }

    /// Return cached prompt names from all upstreams, excluding any that clash with builtins.
    pub async fn cached_upstream_prompt_names(&self, builtins: &[&str]) -> Vec<String> {
        let catalog = self.catalog.read().await;
        catalog
            .values()
            .flat_map(|entry| entry.prompt_names.iter().cloned())
            .filter(|name| !builtins.contains(&name.as_str()))
            .collect()
    }

    /// Return cached prompt names keyed by upstream name.
    ///
    /// Used by inspection actions (e.g. `gateway.discovered_prompts`) to answer
    /// from already-populated catalog data without issuing live RPCs to all upstreams.
    /// Entries whose `prompt_health` is not routable are excluded.
    pub async fn cached_upstream_prompt_names_by_upstream(&self) -> Vec<(String, Vec<String>)> {
        let catalog = self.catalog.read().await;
        catalog
            .iter()
            .filter(|(_, entry)| {
                entry.prompt_health.is_routable() && !entry.prompt_names.is_empty()
            })
            .map(|(name, entry)| (name.clone(), entry.prompt_names.clone()))
            .collect()
    }

    async fn cached_prompt_owner(
        &self,
        prompt_name: &str,
        require_routable: bool,
    ) -> Option<String> {
        let catalog = self.catalog.read().await;
        let mut entries = catalog.values().collect::<Vec<_>>();
        entries.sort_unstable_by(|left, right| left.name.cmp(&right.name));
        entries.into_iter().find_map(|entry| {
            if require_routable && !entry.prompt_health.is_routable() {
                return None;
            }
            entry
                .prompt_names
                .iter()
                .any(|name| name == prompt_name)
                .then(|| entry.name.to_string())
        })
    }

    /// Build prompt ownership map: prompt_name -> upstream_name.
    ///
    /// Makes M RPCs (one per healthy upstream), not M*N. Use this when you need
    /// to look up ownership for multiple prompts.
    pub async fn prompt_ownership_map(&self, builtin_names: &[&str]) -> HashMap<String, String> {
        let deadline_at = tokio::time::Instant::now() + self.request_timeout;
        let (_, owners) = self
            .collect_upstream_prompts(builtin_names, None, deadline_at)
            .await;
        owners
    }

    /// Return the cached prompt ownership map (prompt_name -> upstream_name)
    /// built from already-populated catalog data, without issuing any live RPCs.
    ///
    /// P-M8: used by `gateway.status` to avoid a live `prompts/list` fan-out on
    /// every poll. The cache is populated whenever `list_upstream_prompts` or
    /// `prompt_ownership_map` is called (e.g. on catalog reload). Falls back to
    /// an empty map when the cache is cold, which is the same behavior the
    /// previous live-RPC path would return when no upstreams are routable.
    pub async fn cached_prompt_ownership_map(&self) -> HashMap<String, String> {
        let catalog = self.catalog.read().await;
        let mut owners = HashMap::new();
        let mut entries: Vec<_> = catalog.values().collect();
        // Sort by name for deterministic winner when two upstreams have the
        // same prompt name — consistent with collect_upstream_prompts ordering.
        entries.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        for entry in entries {
            if !entry.prompt_health.is_routable() {
                continue;
            }
            for prompt_name in &entry.prompt_names {
                owners
                    .entry(prompt_name.clone())
                    .or_insert_with(|| entry.name.to_string());
            }
        }
        owners
    }

    /// Resolve which upstream owns a given prompt name.
    ///
    /// Prefer `prompt_ownership_map()` when resolving ownership for multiple
    /// prompts to avoid an N+1 RPC pattern.
    pub async fn find_prompt_owner(&self, prompt_name: &str) -> Option<String> {
        if let Some(owner) = self.cached_prompt_owner(prompt_name, true).await {
            return Some(owner);
        }

        let deadline_at = tokio::time::Instant::now() + self.request_timeout;
        let (_, owners) = self.collect_upstream_prompts(&[], None, deadline_at).await;
        if let Some(owner) = owners.get(prompt_name) {
            return Some(owner.clone());
        }

        self.cached_prompt_owner(prompt_name, false).await
    }

    pub async fn list_upstream_prompts_allowed(
        &self,
        builtin_name_refs: &[&str],
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<Prompt> {
        let deadline_at = tokio::time::Instant::now() + self.request_timeout;
        self.list_upstream_prompts_allowed_until(builtin_name_refs, allowed, deadline_at)
            .await
    }

    /// List prompts using a caller-owned absolute deadline. This lets the MCP
    /// adapter share one budget across raw and OAuth subject-scoped passes.
    pub async fn list_upstream_prompts_allowed_until(
        &self,
        builtin_name_refs: &[&str],
        allowed: Option<&BTreeSet<String>>,
        deadline_at: tokio::time::Instant,
    ) -> Vec<Prompt> {
        let (prompts, _) = self
            .collect_upstream_prompts(builtin_name_refs, allowed, deadline_at)
            .await;
        prompts
    }

    pub async fn find_prompt_owner_allowed(
        &self,
        prompt_name: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Option<String> {
        let owner = self.find_prompt_owner(prompt_name).await?;
        if allowed.is_some_and(|names| !names.contains(&owner)) {
            return None;
        }
        Some(owner)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rmcp::model::{
        ErrorData, GetPromptRequestParams, ListPromptsResult, PaginatedRequestParams, Prompt,
        ServerCapabilities, ServerInfo,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler, ServiceExt};

    use super::super::super::types;
    use super::super::helpers::merge_upstream_prompts;
    use super::super::testsupport::*;
    use super::*;

    #[derive(Clone, Default)]
    struct PaginatedPromptServer {
        calls: Arc<AtomicUsize>,
    }

    #[derive(Clone, Default)]
    struct StalledPromptServer;

    impl ServerHandler for StalledPromptServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_prompts().build())
        }

        async fn list_prompts(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListPromptsResult, ErrorData> {
            std::future::pending().await
        }
    }

    async fn attach_prompt_server<S>(pool: &UpstreamPool, upstream_name: &str, server: S)
    where
        S: ServerHandler,
    {
        let (server_transport, client_transport) =
            tokio::io::duplex(super::super::helpers::IN_PROCESS_PEER_BUFFER_BYTES);
        let server_task = tokio::spawn(async move {
            let running = server
                .serve(server_transport)
                .await
                .expect("prompt server starts");
            running.waiting().await.expect("prompt server runs");
        });
        let client_service: rmcp::service::RunningService<rmcp::RoleClient, ()> = ()
            .serve(client_transport)
            .await
            .expect("prompt client starts");
        let peer = client_service.peer().clone();
        let entry_name = Arc::<str>::from(upstream_name);
        pool.catalog.write().await.insert(
            upstream_name.to_string(),
            super::super::entries::healthy_in_process_entry(entry_name, HashMap::new()),
        );
        pool.connections.write().await.insert(
            upstream_name.to_string(),
            super::super::UpstreamConnection {
                _client_service: client_service.into(),
                _server_task: Some(server_task),
                peer,
                runtime: super::super::UpstreamRuntimeMetadata::default(),
            },
        );
    }

    #[tokio::test]
    async fn prompt_catalog_obeys_caller_absolute_deadline() {
        let pool = catalog_pool_with_server("stalled", StalledPromptServer).await;
        let started = tokio::time::Instant::now();
        let deadline_at = started + std::time::Duration::from_millis(30);

        let prompts = pool
            .list_upstream_prompts_allowed_until(&[], None, deadline_at)
            .await;

        assert!(prompts.is_empty());
        assert!(
            started.elapsed() < std::time::Duration::from_millis(150),
            "the caller deadline must bound the entire prompt aggregation"
        );
    }

    #[tokio::test]
    async fn prompt_catalog_returns_completed_upstreams_when_one_stalls() {
        let pool = catalog_pool_with_server("quick", PaginatedPromptServer::default()).await;
        attach_prompt_server(pool.as_ref(), "stalled", StalledPromptServer).await;
        let started = tokio::time::Instant::now();
        let deadline_at = started + std::time::Duration::from_millis(40);

        let prompts = pool
            .list_upstream_prompts_allowed_until(&[], None, deadline_at)
            .await;
        let names = prompts
            .iter()
            .map(|prompt| prompt.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["quick/first", "quick/second"]);
        assert!(started.elapsed() < std::time::Duration::from_millis(160));
    }

    impl ServerHandler for PaginatedPromptServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_prompts().build())
        }

        async fn list_prompts(
            &self,
            request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListPromptsResult, ErrorData> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let cursor = request.and_then(|request| request.cursor);
            let mut result = match cursor.as_deref() {
                None => ListPromptsResult::with_all_items(vec![Prompt::new(
                    "first",
                    Some("first page"),
                    None,
                )]),
                Some("page-2") => ListPromptsResult::with_all_items(vec![Prompt::new(
                    "second",
                    Some("second page"),
                    None,
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
    async fn prompt_catalog_traverses_all_upstream_pages() {
        let server = PaginatedPromptServer::default();
        let calls = Arc::clone(&server.calls);
        let pool = catalog_pool_with_server("paged", server).await;

        let prompts = pool.list_upstream_prompts(&[]).await;
        let names = prompts
            .iter()
            .map(|prompt| prompt.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(names, vec!["paged/first", "paged/second"]);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            pool.catalog
                .read()
                .await
                .get("paged")
                .expect("paged catalog entry")
                .prompt_count,
            2
        );
    }

    #[test]
    fn merge_upstream_prompts_is_deterministic() {
        let left = Prompt::new("shared", Some("left"), None);
        let right = Prompt::new("shared", Some("right"), None);
        let left_only = Prompt::new("left-only", Some("left-only"), None);
        let right_only = Prompt::new("right-only", Some("right-only"), None);

        let (prompts, owners) = merge_upstream_prompts(
            &["builtin"],
            vec![
                ("zeta".into(), vec![right.clone(), right_only]),
                ("alpha".into(), vec![left.clone(), left_only]),
            ],
        );

        // Every prompt is namespaced by its owning upstream, so the two `shared`
        // prompts no longer collide — both survive with distinct names. Ordering
        // is deterministic: upstreams sorted (alpha < zeta), prompts in order.
        let names: Vec<_> = prompts.iter().map(|prompt| prompt.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "alpha/shared",
                "alpha/left-only",
                "zeta/shared",
                "zeta/right-only",
            ]
        );
        assert_eq!(
            owners.get("alpha/shared").map(String::as_str),
            Some("alpha")
        );
        assert_eq!(owners.get("zeta/shared").map(String::as_str), Some("zeta"));
        assert_eq!(
            owners.get("alpha/left-only").map(String::as_str),
            Some("alpha")
        );
        assert_eq!(
            owners.get("zeta/right-only").map(String::as_str),
            Some("zeta")
        );
    }

    // The `LabMcpServer::snapshot_catalog` projection of these cached prompt names
    // is asserted by the `lab` crate's `gateway_schema_resources` integration test;
    // here we cover only the pool-level listing + cache, which is all the upstream
    // pool owns.
    #[tokio::test]
    async fn successful_prompt_listing_populates_snapshot_cache() {
        let pool = static_catalog_pool("static").await;

        let prompts = pool.list_upstream_prompts(&[]).await;
        let prompt_names: Vec<&str> = prompts.iter().map(|prompt| prompt.name.as_str()).collect();
        // Prompt names are namespaced by their owning upstream.
        assert_eq!(
            prompt_names,
            vec!["static/upstream.prompt.one", "static/upstream.prompt.two"]
        );
        assert_eq!(
            pool.cached_upstream_prompt_names(&[]).await,
            vec![
                "static/upstream.prompt.one".to_string(),
                "static/upstream.prompt.two".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn prompt_owner_lookup_uses_cache_without_listing_upstreams() {
        let server = StaticCatalogServer::default();
        let list_prompts_count = Arc::clone(&server.list_prompts_count);
        let get_prompt_count = Arc::clone(&server.get_prompt_count);
        let pool = static_catalog_pool_with_server("static", server).await;

        let prompts = pool.list_upstream_prompts(&[]).await;
        assert_eq!(prompts.len(), 2);
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);

        let owner = pool.find_prompt_owner("static/upstream.prompt.one").await;
        assert_eq!(owner.as_deref(), Some("static"));
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);

        // The gateway-facing name is namespaced; `get_prompt` strips the
        // `{upstream}/` prefix before forwarding the bare name to the upstream.
        let result = pool
            .get_prompt(
                "static",
                GetPromptRequestParams::new("static/upstream.prompt.one"),
            )
            .await
            .expect("upstream remains connected")
            .expect("prompt get succeeds");
        assert_eq!(result.messages.len(), 1);
        assert_eq!(get_prompt_count.load(Ordering::SeqCst), 1);
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn prompt_owner_lookup_falls_back_to_stale_cache_after_listing_miss() {
        let server = StaticCatalogServer::default();
        let list_prompts_count = Arc::clone(&server.list_prompts_count);
        let fail_list_prompts = Arc::clone(&server.fail_list_prompts);
        let pool = static_catalog_pool_with_server("static", server).await;

        let prompts = pool.list_upstream_prompts(&[]).await;
        assert_eq!(prompts.len(), 2);
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);

        for _ in 0..types::CIRCUIT_BREAKER_THRESHOLD {
            pool.record_failure_for(
                "static",
                UpstreamCapability::Prompts,
                "prompt listing failed for test",
            )
            .await;
        }
        fail_list_prompts.store(true, Ordering::SeqCst);

        let owner = pool.find_prompt_owner("static/upstream.prompt.one").await;
        assert_eq!(owner.as_deref(), Some("static"));
        assert_eq!(list_prompts_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            pool.cached_upstream_prompt_names(&[]).await,
            vec![
                "static/upstream.prompt.one".to_string(),
                "static/upstream.prompt.two".to_string()
            ]
        );
    }
}
