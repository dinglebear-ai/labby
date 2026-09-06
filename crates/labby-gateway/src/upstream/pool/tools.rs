//! Tool queries on the upstream pool: healthy-tool listings, candidate/owner
//! lookup, schema and exposure rows, cached summaries, runtime metadata, and
//! tool health. `has_healthy_tools_for_upstream` is `pub(super)` because
//! `ensure.rs` calls it across the module boundary (plan §3.0/§2.1).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use serde_json::Value;

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::{
    ToolExposurePolicy, UpstreamCapability, UpstreamEnrichmentCatalogEntry, UpstreamHealth,
    UpstreamRuntimeMetadata, UpstreamTool, UpstreamToolExposureRow,
};
use super::UpstreamPool;
use super::entries::{
    resolve_request_exposure_policy, resolve_request_resource_exposure_policy, resource_exposed,
};
use super::helpers::{
    SUBJECT_CONN_IDLE_TTL, UpstreamCachedSummary, cached_upstream_tool, max_response_bytes,
};

/// Hard cap on the total number of tools returned by a single `healthy_tools()` call.
///
/// Prevents runaway allocations when a malicious or misconfigured upstream
/// exposes an extremely large catalog.  A truncation warning is emitted when
/// this limit is hit.  Tests can reference this constant to assert bounds behavior.
pub const MAX_UPSTREAM_TOOLS: usize = 1000;

/// Hard cap on the total number of resources returned by `list_upstream_resources()`.
pub(crate) const MAX_UPSTREAM_RESOURCES: usize = 1000;

/// Hard cap on the total number of prompts returned by `collect_upstream_prompts()`.
pub(crate) const MAX_UPSTREAM_PROMPTS: usize = 1000;
const MAX_SUBJECT_SCOPED_UPSTREAMS: usize = 256;
const SUBJECT_SCOPED_ENUMERATION_TIMEOUT: Duration = Duration::from_secs(10);

struct SerializedByteCounter(usize);

struct SubjectScopedToolsResult {
    tools: Vec<(String, Vec<rmcp::model::Tool>)>,
    inspected: usize,
    incomplete: bool,
}

pub(crate) struct BoundedUpstreamToolsResult {
    pub tools: Vec<UpstreamTool>,
    pub inspected: usize,
    pub incomplete: bool,
}

impl Write for SerializedByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn serialized_tool_bytes(tool: &rmcp::model::Tool) -> usize {
    let mut counter = SerializedByteCounter(0);
    serde_json::to_writer(&mut counter, tool).map_or(usize::MAX, |()| counter.0)
}

fn tool_catalog_bytes(tool: &UpstreamTool) -> usize {
    tool.upstream_name
        .len()
        .saturating_add(serialized_tool_bytes(&tool.tool))
}

fn upstream_allowed(allowed: Option<&BTreeSet<String>>, upstream: &str) -> bool {
    allowed.is_none_or(|names| names.contains(upstream))
}

fn insert_bounded_tool_row(
    rows: &mut Vec<UpstreamToolExposureRow>,
    row: UpstreamToolExposureRow,
    limit: usize,
) {
    if limit == 0 {
        return;
    }
    let insert_at = rows
        .binary_search_by(|existing| existing.name.cmp(&row.name))
        .unwrap_or_else(std::convert::identity);
    if insert_at < limit {
        rows.insert(insert_at, row);
        if rows.len() > limit {
            rows.pop();
        }
    }
}

fn insert_bounded_upstream_tool(tools: &mut Vec<UpstreamTool>, tool: UpstreamTool, limit: usize) {
    if limit == 0 {
        return;
    }
    let insert_at = tools
        .binary_search_by(|existing| {
            existing
                .tool
                .name
                .cmp(&tool.tool.name)
                .then_with(|| existing.upstream_name.cmp(&tool.upstream_name))
        })
        .unwrap_or_else(std::convert::identity);
    if insert_at < limit {
        tools.insert(insert_at, tool);
        if tools.len() > limit {
            tools.pop();
        }
    }
}

fn insert_bounded_unambiguous_upstream_tool(
    tools: &mut BTreeMap<String, Option<UpstreamTool>>,
    tool: UpstreamTool,
    limit: usize,
) {
    if limit == 0 {
        return;
    }
    let name = tool.tool.name.to_string();
    if let Some(existing) = tools.get_mut(&name) {
        *existing = None;
        return;
    }
    if tools.len() < limit {
        tools.insert(name, Some(tool));
        return;
    }
    let should_enter = tools
        .last_key_value()
        .is_some_and(|(largest, _)| name < *largest);
    if should_enter {
        tools.insert(name, Some(tool));
        tools.pop_last();
    }
}

fn finish_unambiguous_upstream_tools(
    tools: BTreeMap<String, Option<UpstreamTool>>,
) -> Vec<UpstreamTool> {
    tools.into_values().flatten().collect::<Vec<_>>()
}

fn insert_bounded_name(names: &mut Vec<String>, name: String, limit: usize) {
    if limit == 0 {
        return;
    }
    let insert_at = names
        .binary_search(&name)
        .unwrap_or_else(std::convert::identity);
    if insert_at < limit {
        names.insert(insert_at, name);
        if names.len() > limit {
            names.pop();
        }
    }
}

fn insert_bounded_subject_tool(
    tools: &mut Vec<(String, rmcp::model::Tool)>,
    upstream: String,
    tool: rmcp::model::Tool,
    limit: usize,
) {
    if limit == 0 {
        return;
    }
    let insert_at = tools
        .binary_search_by(|(existing_upstream, existing_tool)| {
            existing_tool
                .name
                .cmp(&tool.name)
                .then_with(|| existing_upstream.cmp(&upstream))
        })
        .unwrap_or_else(std::convert::identity);
    if insert_at < limit {
        tools.insert(insert_at, (upstream, tool));
        if tools.len() > limit {
            tools.pop();
        }
    }
}

impl UpstreamPool {
    /// Get all healthy upstream tools, up to [`MAX_UPSTREAM_TOOLS`] total.
    ///
    /// If the combined catalog across all upstreams exceeds the cap, the excess
    /// is dropped and a `tracing::warn!` is emitted.  This prevents a buggy or
    /// malicious upstream from forcing large allocations.
    pub async fn healthy_tools(&self) -> Vec<UpstreamTool> {
        self.healthy_tools_allowed(None).await
    }

    pub async fn healthy_tools_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        let mut candidates = Vec::new();
        let mut candidate_count = 0usize;
        for tool in catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .filter(|(_, entry)| entry.tool_health.is_routable())
            .flat_map(|(_, entry)| {
                entry
                    .tools
                    .values()
                    .filter(|tool| entry.exposure_policy.matches(tool.tool.name.as_ref()))
            })
        {
            candidate_count = candidate_count.saturating_add(1);
            let insert_at = candidates
                .binary_search_by(|existing: &&UpstreamTool| {
                    existing
                        .tool
                        .name
                        .cmp(&tool.tool.name)
                        .then_with(|| existing.upstream_name.cmp(&tool.upstream_name))
                })
                .unwrap_or_else(std::convert::identity);
            if insert_at < MAX_UPSTREAM_TOOLS {
                candidates.insert(insert_at, tool);
                if candidates.len() > MAX_UPSTREAM_TOOLS {
                    candidates.pop();
                }
            }
        }
        let byte_limit = max_response_bytes();
        let mut candidate_bytes = 0usize;
        let mut tools = Vec::with_capacity(candidates.len());
        for tool in candidates {
            let tool_bytes = tool_catalog_bytes(&tool);
            if candidate_bytes.saturating_add(tool_bytes) > byte_limit {
                tracing::warn!(
                    limit = byte_limit,
                    "upstream tool catalog exceeds serialized byte limit — truncating to cap"
                );
                break;
            }
            candidate_bytes = candidate_bytes.saturating_add(tool_bytes);
            tools.push(tool.clone());
        }
        if candidate_count > MAX_UPSTREAM_TOOLS {
            tracing::warn!(
                limit = MAX_UPSTREAM_TOOLS,
                "upstream tool catalog exceeds limit — truncating to cap"
            );
        }
        tools
    }

    pub async fn healthy_tools_for_upstream(&self, upstream: &str) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        let mut tools = catalog
            .get(upstream)
            .into_iter()
            .filter(|entry| entry.tool_health.is_routable())
            .flat_map(|entry| {
                entry.tools.values().filter_map(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool.tool.name.as_ref())
                        .then(|| tool.clone())
                })
            })
            .collect::<Vec<_>>();
        tools.sort_by(|left, right| left.tool.name.cmp(&right.tool.name));
        tools
    }

    /// Return at most `limit` exposed, routable tools for one upstream in name order.
    ///
    /// Selection happens against borrowed catalog entries so callers serving a
    /// bounded projection do not clone schemas that they will immediately discard.
    pub async fn healthy_tools_for_upstream_bounded(
        &self,
        upstream: &str,
        limit: usize,
    ) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        let Some(entry) = catalog
            .get(upstream)
            .filter(|entry| entry.tool_health.is_routable())
        else {
            return Vec::new();
        };
        let mut selected = Vec::with_capacity(limit.min(entry.tools.len()));
        for tool in entry
            .tools
            .values()
            .filter(|tool| entry.exposure_policy.matches(tool.tool.name.as_ref()))
        {
            let insert_at = selected
                .binary_search_by(|existing: &&UpstreamTool| {
                    existing.tool.name.cmp(&tool.tool.name)
                })
                .unwrap_or_else(std::convert::identity);
            if insert_at < limit {
                selected.insert(insert_at, tool);
                if selected.len() > limit {
                    selected.pop();
                }
            }
        }
        selected.into_iter().cloned().collect()
    }

    /// Inspect at most `inspection_limit` exposed tools and retain the highest
    /// scoring `limit` without cloning discarded schemas.
    pub async fn healthy_tools_for_upstream_ranked_bounded(
        &self,
        upstream: &str,
        limit: usize,
        inspection_limit: usize,
        score: impl Fn(&UpstreamTool) -> u16,
    ) -> (Vec<(UpstreamTool, u16)>, usize, bool) {
        let catalog = self.catalog.read().await;
        let Some(entry) = catalog
            .get(upstream)
            .filter(|entry| entry.tool_health.is_routable())
        else {
            return (Vec::new(), 0, false);
        };
        let mut selected = Vec::<(&UpstreamTool, u16)>::with_capacity(limit.min(entry.tools.len()));
        let mut inspected = 0usize;
        let mut exhausted = false;
        for tool in entry
            .tools
            .values()
            .filter(|tool| entry.exposure_policy.matches(tool.tool.name.as_ref()))
        {
            if inspected == inspection_limit {
                exhausted = true;
                break;
            }
            inspected += 1;
            let candidate_score = score(tool);
            if candidate_score == 0 {
                continue;
            }
            let insert_at = selected
                .binary_search_by(|(existing, existing_score)| {
                    existing_score
                        .cmp(&candidate_score)
                        .reverse()
                        .then_with(|| existing.tool.name.cmp(&tool.tool.name))
                })
                .unwrap_or_else(std::convert::identity);
            if insert_at < limit {
                selected.insert(insert_at, (tool, candidate_score));
                if selected.len() > limit {
                    selected.pop();
                }
            }
        }
        (
            selected
                .into_iter()
                .map(|(tool, score)| (tool.clone(), score))
                .collect(),
            inspected,
            exhausted,
        )
    }

    /// Return one exact exposed, routable tool without cloning its siblings.
    pub async fn healthy_tool_for_upstream(
        &self,
        upstream: &str,
        tool_name: &str,
    ) -> Option<UpstreamTool> {
        let catalog = self.catalog.read().await;
        let entry = catalog
            .get(upstream)
            .filter(|entry| entry.tool_health.is_routable())?;
        let tool = entry.tools.get(tool_name)?;
        entry
            .exposure_policy
            .matches(tool.tool.name.as_ref())
            .then(|| tool.clone())
    }

    pub(super) async fn has_healthy_tools_for_upstream(&self, upstream: &str) -> bool {
        let catalog = self.catalog.read().await;
        catalog.get(upstream).is_some_and(|entry| {
            entry.tool_health.is_routable()
                && entry
                    .tools
                    .values()
                    .any(|tool| entry.exposure_policy.matches(tool.tool.name.as_ref()))
        })
    }

    pub async fn find_tool_candidates(&self, tool_name: &str) -> Vec<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        let mut matches = Vec::new();
        for (upstream_name, entry) in catalog.iter() {
            if !entry.tool_health.is_routable() {
                continue;
            }
            if let Some(tool) = entry.tools.get(tool_name)
                && entry.exposure_policy.matches(tool.tool.name.as_ref())
            {
                matches.push((upstream_name.clone(), tool.clone()));
            }
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        matches
    }

    /// Like [`find_tool_candidates`](Self::find_tool_candidates) but constrained
    /// to the route's allowed upstreams.
    ///
    /// Returns every exposed, routable, route-scope-allowed upstream that exposes
    /// `tool_name`, sorted by upstream name. The Code Mode MCP App callback gate
    /// uses this to detect ambiguity (a tool name exposed by more than one allowed
    /// upstream) and fail closed instead of proxying an arbitrary, hash-order
    /// dependent upstream.
    pub async fn find_exposed_tool_candidates_allowed(
        &self,
        tool_name: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        let mut matches = Vec::new();
        for (upstream_name, entry) in catalog.iter() {
            if !upstream_allowed(allowed, upstream_name) || !entry.tool_health.is_routable() {
                continue;
            }
            if let Some(tool) = entry.tools.get(tool_name)
                && entry.exposure_policy.matches(tool.tool.name.as_ref())
            {
                matches.push((upstream_name.clone(), tool.clone()));
            }
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        matches
    }

    /// Return exposed tools whose upstream also exposes at least one MCP App UI tool.
    ///
    /// Code Mode keeps ordinary raw tools out of `list_tools`, but a rendered MCP
    /// App can only talk back to its server through host `callServerTool`
    /// callbacks. This lookup is the narrow callback allowlist: the requested
    /// tool must still be exposed by its upstream, and that same upstream must
    /// expose an MCP App UI tool.
    pub async fn find_mcp_app_sibling_tool_candidates(
        &self,
        tool_name: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        let mut matches = Vec::new();
        for (upstream_name, entry) in catalog.iter() {
            if !upstream_allowed(allowed, upstream_name)
                || !entry.tool_health.is_routable()
                || !entry.proxy_resources
            {
                continue;
            }
            let Some(tool) = entry.tools.get(tool_name) else {
                continue;
            };
            if !entry.exposure_policy.matches(tool.tool.name.as_ref()) {
                continue;
            }
            let has_ui_sibling = entry.tools.values().any(|candidate| {
                entry.exposure_policy.matches(candidate.tool.name.as_ref())
                    && tool_has_mcp_app_ui_resource(candidate)
                    && mcp_tool_resource_bindings_are_exposed(
                        &candidate.tool,
                        &entry.resource_exposure_policy,
                    )
            });
            if has_ui_sibling {
                matches.push((upstream_name.clone(), tool.clone()));
            }
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        matches
    }

    /// Return tool lists for all OAuth upstreams visible to `subject`.
    ///
    /// P-C1 fix: uses `acquire_or_connect_subject` so the per-(upstream,subject)
    /// connection and tool list are cached — the expensive TLS + initialize +
    /// tools/list is paid at most once per idle-TTL window, not on every call.
    ///
    /// The upstream's `expose_tools` allowlist is enforced here, so exposure is
    /// symmetric with the catalog-backed path (`healthy_tools_allowed` and
    /// friends, which read `UpstreamEntry::exposure_policy`). A subject-scoped
    /// tool list is discovered on a per-`(upstream, subject)` connection and
    /// never lands in `self.catalog`, so there is no `UpstreamEntry` to consult;
    /// the policy is resolved per request from the live `UpstreamConfig` with
    /// the same fail-closed `resolve_exposure_policy` helper the catalog path
    /// uses.
    ///
    /// This covers **discovery** for every subject-scoped consumer: `list_tools`
    /// (`crates/labby/src/mcp/handlers_tools.rs`), the `tools/list_changed`
    /// contract diff (`crates/labby/src/mcp/peer_contract.rs`), and the
    /// owner-resolution scan in `crates/labby/src/mcp/call_tool_upstream.rs`.
    ///
    /// It is **not** the only place exposure has to hold. That same
    /// `call_tool_upstream.rs` has a `pre_resolved_oauth_config` branch that
    /// short-circuits owner resolution and never calls this function, so
    /// "hidden implies uncallable" is enforced independently at the OAuth
    /// execution primitives themselves — `subject_scoped_call_tool*`
    /// (`pool/tools_call.rs`) and the subject-scoped arm of `call_tool_relayed`
    /// (`pool/relay.rs`) — via `subject_scoped_tool_is_exposed`. Do not delete
    /// either guard on the assumption that the other one covers it.
    pub async fn subject_scoped_tools(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
    ) -> Vec<(String, Vec<rmcp::model::Tool>)> {
        self.subject_scoped_tools_inner(
            configs,
            subject,
            None,
            None,
            SUBJECT_SCOPED_ENUMERATION_TIMEOUT,
        )
        .await
        .tools
    }

    /// Return at most `limit` OAuth subject-scoped tools in deterministic
    /// global tool-name/upstream-name order.
    ///
    /// Tool-list surfaces use this bounded form while owner resolution retains
    /// the complete subject-scoped catalog through [`Self::subject_scoped_tools`].
    pub async fn subject_scoped_tools_bounded(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        limit: usize,
    ) -> Vec<(String, Vec<rmcp::model::Tool>)> {
        self.subject_scoped_tools_inner(
            configs,
            subject,
            Some(limit),
            None,
            SUBJECT_SCOPED_ENUMERATION_TIMEOUT,
        )
        .await
        .tools
    }

    async fn subject_scoped_tools_matching_bounded(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        limit: usize,
        predicate: &(dyn Fn(&str, &rmcp::model::Tool) -> bool + Sync),
        deadline: Duration,
    ) -> SubjectScopedToolsResult {
        self.subject_scoped_tools_inner(configs, subject, Some(limit), Some(predicate), deadline)
            .await
    }

    /// Return only already-cached OAuth tools for `subject`.
    ///
    /// Discovery surfaces must use this variant: a cache miss is intentionally
    /// omitted instead of turning `tools/list` into a network/connect path.
    pub async fn cached_subject_scoped_tools_bounded(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        limit: usize,
    ) -> Vec<(String, Vec<rmcp::model::Tool>)> {
        let cache = self.subject_connections.read().await;
        let mut bounded = Vec::new();
        let mut candidate_count = 0usize;
        for config in configs
            .iter()
            .filter(|config| config.enabled && config.oauth.is_some())
        {
            let key = (config.name.clone(), subject.to_string());
            let Some(entry) = cache.get(&key) else {
                continue;
            };
            if entry.last_used.elapsed() >= SUBJECT_CONN_IDLE_TTL {
                continue;
            }
            let exposure_policy =
                resolve_request_exposure_policy(&config.name, config.expose_tools.clone());
            let mut exposed = entry
                .tools
                .iter()
                .filter(|tool| exposure_policy.matches(tool.name.as_ref()))
                .cloned()
                .collect::<Vec<_>>();
            exposed.sort_by(|left, right| left.name.cmp(&right.name));
            candidate_count = candidate_count.saturating_add(exposed.len());
            for tool in exposed {
                insert_bounded_subject_tool(&mut bounded, config.name.clone(), tool, limit);
            }
        }
        drop(cache);
        if candidate_count > limit {
            tracing::warn!(
                limit,
                candidate_count,
                "cached subject-scoped upstream tool list truncated"
            );
        }
        let mut by_upstream = BTreeMap::<String, Vec<rmcp::model::Tool>>::new();
        for (upstream, tool) in bounded {
            by_upstream.entry(upstream).or_default().push(tool);
        }
        by_upstream.into_iter().collect()
    }

    /// Return the complete cached MCP App tool contract for one request,
    /// combining global and subject-scoped OAuth upstreams before resolving
    /// duplicate names.
    ///
    /// The result is cache-only, exposure-aware, resource-backed, and bounded.
    /// A name claimed by more than one eligible upstream is omitted entirely
    /// because MCP `tools/call` has no upstream discriminator and execution
    /// would correctly reject that name as ambiguous.
    pub async fn cached_mcp_app_tools_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
        oauth_configs: &[UpstreamConfig],
        oauth_subject: Option<&str>,
        limit: usize,
    ) -> Vec<UpstreamTool> {
        let mut tools = BTreeMap::<String, Option<UpstreamTool>>::new();
        let mut candidate_count = 0usize;
        {
            let catalog = self.catalog.read().await;
            for (_, entry) in catalog.iter().filter(|(name, entry)| {
                upstream_allowed(allowed, name)
                    && entry.tool_health.is_routable()
                    && entry.proxy_resources
            }) {
                let has_exposed_owner = entry.tools.values().any(|candidate| {
                    entry.exposure_policy.matches(candidate.tool.name.as_ref())
                        && mcp_tool_resource_bindings(&candidate.tool)
                            .into_iter()
                            .any(|uri| uri.is_some())
                        && mcp_tool_resource_bindings_are_exposed(
                            &candidate.tool,
                            &entry.resource_exposure_policy,
                        )
                });
                for tool in entry.tools.values() {
                    if !entry.exposure_policy.matches(tool.tool.name.as_ref())
                        || !mcp_tool_is_mcp_app_host_visible_with_owner(
                            &tool.tool,
                            has_exposed_owner,
                            &entry.resource_exposure_policy,
                        )
                    {
                        continue;
                    }
                    candidate_count = candidate_count.saturating_add(1);
                    insert_bounded_unambiguous_upstream_tool(&mut tools, tool.clone(), limit);
                }
            }
        }

        if let Some(subject) = oauth_subject {
            let cache = self.subject_connections.read().await;
            for config in oauth_configs.iter().filter(|config| {
                config.enabled
                    && config.oauth.is_some()
                    && config.proxy_resources
                    && upstream_allowed(allowed, &config.name)
            }) {
                let key = (config.name.clone(), subject.to_string());
                let Some(entry) = cache.get(&key) else {
                    continue;
                };
                if entry.last_used.elapsed() >= SUBJECT_CONN_IDLE_TTL {
                    continue;
                }
                let tool_policy =
                    resolve_request_exposure_policy(&config.name, config.expose_tools.clone());
                let resource_policy = resolve_request_resource_exposure_policy(
                    &config.name,
                    config.expose_resources.clone(),
                );
                let has_exposed_owner = entry.tools.iter().any(|candidate| {
                    tool_policy.matches(candidate.name.as_ref())
                        && mcp_tool_resource_bindings(candidate)
                            .into_iter()
                            .any(|uri| uri.is_some())
                        && mcp_tool_resource_bindings_are_exposed(candidate, &resource_policy)
                });
                let upstream_name = std::sync::Arc::<str>::from(config.name.as_str());
                for tool in &entry.tools {
                    if !tool_policy.matches(tool.name.as_ref())
                        || !mcp_tool_is_mcp_app_host_visible_with_owner(
                            tool,
                            has_exposed_owner,
                            &resource_policy,
                        )
                    {
                        continue;
                    }
                    candidate_count = candidate_count.saturating_add(1);
                    let (_, routed) = cached_upstream_tool(tool.clone(), &upstream_name);
                    insert_bounded_unambiguous_upstream_tool(&mut tools, routed, limit);
                }
            }
        }

        if candidate_count > limit {
            tracing::warn!(
                limit,
                candidate_count,
                "combined MCP App tool catalog exceeds limit — truncating to cap"
            );
        }
        finish_unambiguous_upstream_tools(tools)
    }

    /// Resolve the cached OAuth owner of one native MCP App `ui://` resource
    /// without crossing subjects or cold-connecting an upstream.
    ///
    /// Tool metadata is authoritative ownership evidence. The URI authority is
    /// retained as the same compatibility fallback used by the global catalog
    /// path for dynamic result UIs that are not present in `tools/list`.
    pub async fn cached_subject_scoped_ui_resource_owner(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        uri: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Result<Option<UpstreamConfig>, String> {
        let authority = mcp_ui_uri_authority(uri);
        let global_metadata_owner = {
            let catalog = self.catalog.read().await;
            catalog
                .iter()
                .filter(|(name, entry)| {
                    upstream_allowed(allowed, name)
                        && entry.tool_health.is_routable()
                        && entry.proxy_resources
                })
                .any(|(_, entry)| {
                    entry.tools.values().any(|tool| {
                        entry.exposure_policy.matches(tool.tool.name.as_ref())
                            && mcp_tool_owns_mcp_app_resource(&tool.tool, uri)
                            && resource_exposed(&entry.resource_exposure_policy, uri)
                    })
                })
        };
        let cache = self.subject_connections.read().await;
        let mut owner: Option<UpstreamConfig> = None;
        for config in configs
            .iter()
            .filter(|config| config.enabled && config.oauth.is_some())
        {
            let authority_owner = authority == Some(config.name.as_str());
            let key = (config.name.clone(), subject.to_string());
            let metadata_owner = cache.get(&key).is_some_and(|entry| {
                if entry.last_used.elapsed() >= SUBJECT_CONN_IDLE_TTL {
                    return false;
                }
                let tool_policy =
                    resolve_request_exposure_policy(&config.name, config.expose_tools.clone());
                entry.tools.iter().any(|tool| {
                    tool_policy.matches(tool.name.as_ref())
                        && mcp_tool_owns_mcp_app_resource(tool, uri)
                })
            });
            if global_metadata_owner && metadata_owner {
                return Err(format!(
                    "native UI resource `{uri}` is claimed by both global and OAuth upstream catalogs"
                ));
            }
            if !metadata_owner && (!authority_owner || global_metadata_owner) {
                continue;
            }
            let resource_policy = resolve_request_resource_exposure_policy(
                &config.name,
                config.expose_resources.clone(),
            );
            if !config.proxy_resources || !resource_exposed(&resource_policy, uri) {
                return Err(format!(
                    "native UI resource `{uri}` is not exposed by OAuth upstream `{}`",
                    config.name
                ));
            }
            if let Some(existing) = owner.as_ref()
                && existing.name != config.name
            {
                return Err(format!(
                    "native UI resource `{uri}` is claimed by multiple OAuth upstreams: `{}` and `{}`",
                    existing.name, config.name
                ));
            }
            owner = Some(config.clone());
        }
        Ok(owner)
    }

    /// Return the OAuth tools visible to one subject in the same routed form
    /// used by the global catalog, without ever publishing them globally.
    pub async fn subject_scoped_upstream_tools_allowed(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<UpstreamTool> {
        self.subject_scoped_upstream_tools_allowed_bounded(
            configs,
            subject,
            allowed,
            MAX_UPSTREAM_TOOLS,
        )
        .await
    }

    pub async fn subject_scoped_upstream_tools_allowed_bounded(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        allowed: Option<&BTreeSet<String>>,
        limit: usize,
    ) -> Vec<UpstreamTool> {
        let configs = configs
            .iter()
            .filter(|config| config.enabled && upstream_allowed(allowed, &config.name))
            .cloned()
            .collect::<Vec<_>>();
        let mut routed = Vec::new();
        for (upstream, tools) in self
            .subject_scoped_tools_bounded(&configs, subject, limit)
            .await
        {
            let upstream_name = std::sync::Arc::<str>::from(upstream);
            for tool in tools {
                let (_, tool) = cached_upstream_tool(tool, &upstream_name);
                insert_bounded_upstream_tool(&mut routed, tool, limit);
            }
        }
        routed
    }

    /// Return a bounded routed projection after filtering the complete visible
    /// subject catalog. All eligible OAuth upstreams share one request deadline
    /// and the pool's catalog-fanout concurrency semaphore.
    pub(crate) async fn subject_scoped_upstream_tools_allowed_matching_bounded(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        allowed: Option<&BTreeSet<String>>,
        limit: usize,
        predicate: &(dyn Fn(&str, &rmcp::model::Tool) -> bool + Sync),
        deadline: Duration,
    ) -> BoundedUpstreamToolsResult {
        let configs = configs
            .iter()
            .filter(|config| config.enabled && upstream_allowed(allowed, &config.name))
            .cloned()
            .collect::<Vec<_>>();
        let mut routed = Vec::new();
        let result = self
            .subject_scoped_tools_matching_bounded(&configs, subject, limit, predicate, deadline)
            .await;
        for (upstream, tools) in result.tools {
            let upstream_name = std::sync::Arc::<str>::from(upstream);
            for tool in tools {
                let (_, tool) = cached_upstream_tool(tool, &upstream_name);
                insert_bounded_upstream_tool(&mut routed, tool, limit);
            }
        }
        BoundedUpstreamToolsResult {
            tools: routed,
            inspected: result.inspected,
            incomplete: result.incomplete,
        }
    }

    /// Resolve one exact OAuth subject-scoped tool before constructing the
    /// schema-bearing routed projection for any of its siblings.
    pub async fn subject_scoped_upstream_tool_allowed(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        tool_name: &str,
    ) -> Option<UpstreamTool> {
        if !config.enabled || config.oauth.is_none() {
            return None;
        }
        let exposure_policy =
            resolve_request_exposure_policy(&config.name, config.expose_tools.clone());
        let result = tokio::time::timeout(SUBJECT_SCOPED_ENUMERATION_TIMEOUT, async {
            let _permit = self
                .acquire_catalog_fanout_permit()
                .await
                .map_err(anyhow::Error::msg)?;
            self.acquire_or_connect_subject_tool(config, subject, tool_name)
                .await
        })
        .await;
        let tool = match result {
            Ok(Ok((_peer, tool))) => tool,
            Ok(Err(error)) => {
                tracing::warn!(
                    upstream = %config.name,
                    error = %error,
                    "subject-scoped upstream exact tool discovery failed"
                );
                return None;
            }
            Err(_) => {
                tracing::warn!(
                    upstream = %config.name,
                    "subject-scoped upstream exact tool discovery timed out"
                );
                return None;
            }
        };
        let tool = tool.filter(|tool| exposure_policy.matches(tool.name.as_ref()))?;
        let upstream_name = std::sync::Arc::<str>::from(config.name.as_str());
        Some(cached_upstream_tool(tool, &upstream_name).1)
    }

    async fn subject_scoped_tools_inner(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        limit: Option<usize>,
        predicate: Option<&(dyn Fn(&str, &rmcp::model::Tool) -> bool + Sync)>,
        deadline_duration: Duration,
    ) -> SubjectScopedToolsResult {
        let eligible_count = configs
            .iter()
            .filter(|config| config.enabled && config.oauth.is_some())
            .count();
        if eligible_count > MAX_SUBJECT_SCOPED_UPSTREAMS {
            tracing::warn!(
                eligible_count,
                limit = MAX_SUBJECT_SCOPED_UPSTREAMS,
                "subject-scoped OAuth upstream registry exceeds limit; failing closed"
            );
            return SubjectScopedToolsResult {
                tools: Vec::new(),
                inspected: 0,
                incomplete: true,
            };
        }
        let deadline = tokio::time::Instant::now() + deadline_duration;
        let mut discovered = Vec::new();
        let mut bounded = Vec::new();
        let mut exposed_count = 0usize;
        let mut exposed_bytes = 0usize;
        let mut remaining_inspections = if predicate.is_some() {
            limit.unwrap_or(usize::MAX)
        } else {
            usize::MAX
        };
        let mut inspected = 0usize;
        let mut incomplete = false;
        let mut cached_names = BTreeSet::new();

        if let Some(predicate) = predicate {
            let cache = self.subject_connections.read().await;
            'cached: for config in configs
                .iter()
                .filter(|config| config.enabled && config.oauth.is_some())
            {
                let key = (config.name.clone(), subject.to_string());
                let Some(entry) = cache
                    .get(&key)
                    .filter(|entry| entry.last_used.elapsed() < SUBJECT_CONN_IDLE_TTL)
                else {
                    continue;
                };
                cached_names.insert(config.name.clone());
                let exposure_policy =
                    resolve_request_exposure_policy(&config.name, config.expose_tools.clone());
                let mut hidden_count = 0usize;
                for tool in &entry.tools {
                    if remaining_inspections == 0 {
                        incomplete = true;
                        break 'cached;
                    }
                    remaining_inspections -= 1;
                    inspected += 1;
                    if !exposure_policy.matches(tool.name.as_ref()) {
                        hidden_count += 1;
                        continue;
                    }
                    if predicate(&config.name, tool) {
                        exposed_count += 1;
                        exposed_bytes = exposed_bytes.saturating_add(serialized_tool_bytes(tool));
                        if exposed_bytes > max_response_bytes() {
                            return SubjectScopedToolsResult {
                                tools: Vec::new(),
                                inspected,
                                incomplete: true,
                            };
                        }
                        insert_bounded_subject_tool(
                            &mut bounded,
                            config.name.clone(),
                            tool.clone(),
                            limit.unwrap_or(usize::MAX),
                        );
                    }
                }
                if hidden_count > 0 {
                    tracing::debug!(
                        upstream = %config.name,
                        hidden_count,
                        "subject-scoped upstream tools hidden by exposure policy"
                    );
                }
            }
        }
        let mut futures = FuturesUnordered::new();
        for config in configs.iter().filter(|config| {
            remaining_inspections > 0
                && config.enabled
                && config.oauth.is_some()
                && !cached_names.contains(&config.name)
        }) {
            let config = config.clone();
            let subject = subject.to_string();
            let pool = self.clone();
            futures.push(async move {
                let exposure_policy =
                    resolve_request_exposure_policy(&config.name, config.expose_tools.clone());
                let _fanout_permit = match pool.acquire_catalog_fanout_permit().await {
                    Ok(permit) => permit,
                    Err(error) => {
                        return (
                            config.name.clone(),
                            exposure_policy,
                            Err(anyhow::anyhow!(error)),
                        );
                    }
                };
                let result = pool.acquire_or_connect_subject(&config, &subject).await;
                (config.name.clone(), exposure_policy, result)
            });
        }

        loop {
            let next = match tokio::time::timeout_at(deadline, futures.next()).await {
                Ok(next) => next,
                Err(_) => {
                    tracing::warn!("subject-scoped OAuth registry enumeration timed out");
                    incomplete = true;
                    break;
                }
            };
            let Some((name, exposure_policy, result)) = next else {
                break;
            };
            match result {
                Ok((_peer, tools)) => {
                    let hidden_count = tools
                        .iter()
                        .filter(|tool| !exposure_policy.matches(tool.name.as_ref()))
                        .count();
                    let mut inspection_budget_exhausted = false;
                    let mut exposed = Vec::new();
                    for tool in tools {
                        if remaining_inspections == 0 {
                            inspection_budget_exhausted = true;
                            break;
                        }
                        remaining_inspections -= 1;
                        inspected += 1;
                        if exposure_policy.matches(tool.name.as_ref())
                            && predicate.is_none_or(|predicate| predicate(&name, &tool))
                        {
                            exposed.push(tool);
                        }
                    }
                    exposed.sort_by(|left, right| left.name.cmp(&right.name));
                    for tool in &exposed {
                        exposed_bytes = exposed_bytes.saturating_add(serialized_tool_bytes(tool));
                        if exposed_bytes > max_response_bytes() {
                            tracing::warn!(
                                candidate_bytes = exposed_bytes,
                                limit = max_response_bytes(),
                                "subject-scoped OAuth registry exceeds byte budget; failing closed"
                            );
                            return SubjectScopedToolsResult {
                                tools: Vec::new(),
                                inspected,
                                incomplete: true,
                            };
                        }
                    }
                    if hidden_count > 0 {
                        tracing::debug!(
                            upstream = %name,
                            hidden_count,
                            exposed_count = exposed.len(),
                            "subject-scoped upstream tools hidden by exposure policy"
                        );
                    }
                    exposed_count = exposed_count.saturating_add(exposed.len());
                    if let Some(limit) = limit {
                        for tool in exposed {
                            insert_bounded_subject_tool(&mut bounded, name.clone(), tool, limit);
                        }
                    } else {
                        if exposed_count > MAX_UPSTREAM_TOOLS {
                            tracing::warn!(
                                candidate_count = exposed_count,
                                limit = MAX_UPSTREAM_TOOLS,
                                "subject-scoped OAuth registry exceeds item budget; failing closed"
                            );
                            return SubjectScopedToolsResult {
                                tools: Vec::new(),
                                inspected,
                                incomplete: true,
                            };
                        }
                        discovered.push((name, exposed));
                    }
                    if inspection_budget_exhausted {
                        incomplete = true;
                        tracing::warn!(
                            inspection_limit = limit.unwrap_or(usize::MAX),
                            "subject-scoped OAuth registry inspection budget exhausted"
                        );
                        break;
                    }
                }
                Err(error) => {
                    incomplete = true;
                    tracing::warn!(
                        upstream = %name,
                        error = %error,
                        "subject-scoped upstream tool discovery failed"
                    );
                }
            }
        }
        if let Some(limit) = limit {
            if exposed_count > limit {
                tracing::warn!(
                    limit,
                    candidate_count = exposed_count,
                    "subject-scoped upstream tool catalog exceeds limit — truncating to cap"
                );
            }
            let mut by_upstream = BTreeMap::<String, Vec<rmcp::model::Tool>>::new();
            for (upstream, tool) in bounded {
                by_upstream.entry(upstream).or_default().push(tool);
            }
            return SubjectScopedToolsResult {
                tools: by_upstream.into_iter().collect(),
                inspected,
                incomplete: incomplete || exposed_count > limit,
            };
        }
        discovered.sort_by(|left, right| left.0.cmp(&right.0));
        SubjectScopedToolsResult {
            tools: discovered,
            inspected,
            incomplete,
        }
    }

    /// Return the names of upstreams currently routable for a capability.
    pub async fn routable_upstream_names(&self, capability: UpstreamCapability) -> Vec<String> {
        let catalog = self.catalog.read().await;
        let mut names: Vec<String> = match capability {
            UpstreamCapability::Resources => {
                let resource_names = self.resource_upstreams.read().await;
                resource_names
                    .iter()
                    .filter(|name| {
                        catalog
                            .get(*name)
                            .is_some_and(|entry| entry.health_for(capability).is_routable())
                    })
                    .cloned()
                    .collect()
            }
            UpstreamCapability::Tools
            | UpstreamCapability::Prompts
            | UpstreamCapability::Skills => catalog
                .iter()
                .filter(|(_, entry)| entry.health_for(capability).is_routable())
                .map(|(name, _)| name.clone())
                .collect(),
        };
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Look up which upstream owns a given tool name.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn find_tool(&self, tool_name: &str) -> Option<(String, UpstreamTool)> {
        self.find_tool_allowed(tool_name, None).await
    }

    #[allow(clippy::significant_drop_tightening)]
    pub async fn find_tool_allowed(
        &self,
        tool_name: &str,
        allowed: Option<&BTreeSet<String>>,
    ) -> Option<(String, UpstreamTool)> {
        let catalog = self.catalog.read().await;
        catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .map(|(_, entry)| entry)
            .filter(|entry| entry.tool_health.is_routable())
            .find_map(|entry| {
                entry.tools.get(tool_name).and_then(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool_name)
                        .then(|| (entry.name.to_string(), tool.clone()))
                })
            })
    }

    /// Get the cached schema for a specific upstream tool.
    #[allow(clippy::significant_drop_tightening)]
    pub async fn tool_schema(&self, tool_name: &str) -> Option<Value> {
        let catalog = self.catalog.read().await;
        catalog.values().find_map(|entry| {
            entry.tools.get(tool_name).and_then(|tool| {
                entry
                    .exposure_policy
                    .matches(tool_name)
                    .then(|| tool.input_schema.clone())
                    .flatten()
            })
        })
    }

    /// Return all discovered tools for one upstream, including hidden tools and exposure metadata.
    pub async fn tool_exposure_rows(&self, upstream_name: &str) -> Vec<UpstreamToolExposureRow> {
        let catalog = self.catalog.read().await;
        let Some(entry) = catalog.get(upstream_name) else {
            return Vec::new();
        };

        let mut rows: Vec<UpstreamToolExposureRow> = entry
            .tools
            .values()
            .map(|tool| {
                let matched_by = entry.exposure_policy.matched_by(tool.tool.name.as_ref());
                UpstreamToolExposureRow {
                    name: tool.tool.name.to_string(),
                    description: tool
                        .tool
                        .description
                        .as_ref()
                        .map(ToString::to_string)
                        .filter(|text| !text.trim().is_empty()),
                    exposed: matched_by.is_some(),
                    matched_by,
                }
            })
            .collect();
        rows.sort_by(|left, right| left.name.cmp(&right.name));
        rows
    }

    /// Return one deterministic cached catalog snapshot for enrichment previews.
    ///
    /// This never connects, probes, reads resources/prompts, or calls upstream
    /// tools. It clones bounded metadata from the in-memory catalog under a
    /// single read lock, allowing callers to filter and cap outside the lock.
    pub async fn cached_enrichment_snapshot(
        &self,
        allowed: Option<&BTreeSet<String>>,
        per_upstream_tool_limit: usize,
    ) -> Vec<UpstreamEnrichmentCatalogEntry> {
        let row_limit = per_upstream_tool_limit.saturating_add(1);
        let catalog = self.catalog.read().await;
        let mut entries = catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .map(|(name, entry)| {
                let mut tool_rows = Vec::new();
                if entry.tool_health.is_routable() {
                    for tool in entry.tools.values() {
                        let matched_by = entry.exposure_policy.matched_by(tool.tool.name.as_ref());
                        let Some(matched_by) = matched_by else {
                            continue;
                        };
                        insert_bounded_tool_row(
                            &mut tool_rows,
                            UpstreamToolExposureRow {
                                name: tool.tool.name.to_string(),
                                description: tool
                                    .tool
                                    .description
                                    .as_ref()
                                    .map(ToString::to_string)
                                    .filter(|text| !text.trim().is_empty()),
                                exposed: true,
                                matched_by: Some(matched_by),
                            },
                            row_limit,
                        );
                    }
                }
                UpstreamEnrichmentCatalogEntry {
                    upstream: name.clone(),
                    tool_rows,
                    resource_count: if entry.resource_health.is_routable() {
                        entry.resource_count
                    } else {
                        0
                    },
                    prompt_count: if entry.prompt_health.is_routable() {
                        entry.prompt_count
                    } else {
                        0
                    },
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.upstream.cmp(&right.upstream));
        entries
    }

    pub async fn cached_upstream_summary(
        &self,
        upstream_name: &str,
    ) -> Option<UpstreamCachedSummary> {
        let catalog = self.catalog.read().await;
        let entry = catalog.get(upstream_name)?;
        let discovered_tool_count = entry.tools.len();
        let exposed_tool_count = entry
            .tools
            .values()
            .filter(|tool| entry.exposure_policy.matches(tool.tool.name.as_ref()))
            .count();
        let discovered_resource_count = entry.resource_count;
        let exposed_resource_count = if entry.resource_health.is_routable() {
            entry.resource_count
        } else {
            0
        };
        let discovered_prompt_count = entry.prompt_count;
        let exposed_prompt_count = if entry.prompt_health.is_routable() {
            entry.prompt_count
        } else {
            0
        };
        let discovered_skill_count = entry.skill_count;
        let exposed_skill_count = if entry.proxy_skills && entry.skill_health.is_routable() {
            entry
                .skill_names
                .iter()
                .filter(|name| entry.skill_exposure_policy.matches(name))
                .count()
        } else {
            0
        };

        Some(UpstreamCachedSummary {
            discovered_tool_count,
            exposed_tool_count,
            discovered_resource_count,
            exposed_resource_count,
            discovered_prompt_count,
            exposed_prompt_count,
            discovered_skill_count,
            exposed_skill_count,
            supports_skills: entry.supports_skills,
        })
    }

    pub async fn upstream_runtime_metadata(
        &self,
        upstream_name: &str,
    ) -> Option<UpstreamRuntimeMetadata> {
        self.connections
            .read()
            .await
            .get(upstream_name)
            .map(|conn| conn.runtime.clone())
    }

    /// Return the current tool health for one upstream.
    pub async fn upstream_tool_health(&self, upstream_name: &str) -> Option<UpstreamHealth> {
        let catalog = self.catalog.read().await;
        catalog.get(upstream_name).map(|entry| entry.tool_health)
    }

    /// Return just the names of all healthy exposed upstream tools.
    ///
    /// Cheaper than `healthy_tools()` for callers that only need tool names
    /// (e.g. `snapshot_catalog` change-detection): avoids deep-cloning every
    /// tool schema just to extract the name field.
    pub async fn healthy_tool_names(&self) -> Vec<String> {
        self.healthy_tool_names_allowed(None).await
    }

    /// Return just the names of healthy exposed upstream tools allowed by a
    /// route scope.
    ///
    /// This is the lightweight counterpart to `healthy_tools_allowed`: it uses
    /// the same exposure, health, and global cap rules without cloning schemas.
    pub async fn healthy_tool_names_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<String> {
        let catalog = self.catalog.read().await;
        let mut names = Vec::new();
        for name in catalog
            .iter()
            .filter(|(name, entry)| {
                upstream_allowed(allowed, name) && entry.tool_health.is_routable()
            })
            .flat_map(|(_, entry)| {
                entry.tools.values().filter_map(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool.tool.name.as_ref())
                        .then(|| tool.tool.name.to_string())
                })
            })
        {
            insert_bounded_name(&mut names, name, MAX_UPSTREAM_TOOLS);
        }
        names
    }
}

fn mcp_ui_uri_authority(uri: &str) -> Option<&str> {
    let rest = uri.strip_prefix("ui://")?;
    let authority = rest.split('/').next()?;
    (!authority.is_empty()).then_some(authority)
}

fn mcp_tool_resource_bindings(tool: &rmcp::model::Tool) -> [Option<&str>; 2] {
    let standard = tool
        .meta
        .as_ref()
        .and_then(|meta| meta.0.get("ui"))
        .and_then(|ui| ui.get("resourceUri"))
        .and_then(Value::as_str)
        .filter(|uri| uri.starts_with("ui://"));
    let openai = tool
        .meta
        .as_ref()
        .and_then(|meta| meta.0.get("openai/outputTemplate"))
        .and_then(Value::as_str)
        .filter(|uri| uri.starts_with("ui://"));
    [standard, openai]
}

pub(super) fn tool_mcp_app_ui_resource_uri(tool: &UpstreamTool) -> Option<&str> {
    mcp_tool_resource_bindings(&tool.tool)
        .into_iter()
        .flatten()
        .next()
}

pub(super) fn mcp_tool_owns_mcp_app_resource(tool: &rmcp::model::Tool, uri: &str) -> bool {
    mcp_tool_resource_bindings(tool)
        .into_iter()
        .flatten()
        .any(|candidate| candidate == uri)
}

fn mcp_tool_resource_bindings_are_exposed(
    tool: &rmcp::model::Tool,
    policy: &ToolExposurePolicy,
) -> bool {
    mcp_tool_resource_bindings(tool)
        .into_iter()
        .flatten()
        .all(|uri| resource_exposed(policy, uri))
}

pub(super) fn tool_has_mcp_app_ui_resource(tool: &UpstreamTool) -> bool {
    mcp_tool_resource_bindings(&tool.tool)
        .into_iter()
        .any(|uri| uri.is_some())
}

fn mcp_tool_is_mcp_app_callback(tool: &rmcp::model::Tool) -> bool {
    let Some(meta) = tool.meta.as_ref() else {
        return false;
    };
    let app_visible = meta
        .0
        .get("ui")
        .and_then(|ui| ui.get("visibility"))
        .and_then(Value::as_array)
        .is_some_and(|visibility| visibility.iter().any(|value| value.as_str() == Some("app")));
    let openai_widget_callback = meta
        .0
        .get("openai/widgetAccessible")
        .and_then(Value::as_bool)
        == Some(true);
    app_visible || openai_widget_callback
}

fn mcp_tool_is_mcp_app_host_visible_with_owner(
    tool: &rmcp::model::Tool,
    has_exposed_owner: bool,
    resource_policy: &ToolExposurePolicy,
) -> bool {
    if mcp_tool_resource_bindings(tool)
        .into_iter()
        .any(|uri| uri.is_some())
    {
        return mcp_tool_resource_bindings_are_exposed(tool, resource_policy);
    }
    has_exposed_owner && mcp_tool_is_mcp_app_callback(tool)
}

fn upstream_has_exposed_mcp_app_owner(
    tools: &[UpstreamTool],
    resource_policy: &ToolExposurePolicy,
) -> bool {
    tools.iter().any(|candidate| {
        mcp_tool_resource_bindings(&candidate.tool)
            .into_iter()
            .any(|uri| uri.is_some())
            && mcp_tool_resource_bindings_are_exposed(&candidate.tool, resource_policy)
    })
}

pub fn upstream_has_mcp_app_ui_owner_for_config(
    tools: &[UpstreamTool],
    config: &UpstreamConfig,
) -> bool {
    if !config.proxy_resources {
        return false;
    }
    let resource_policy =
        resolve_request_resource_exposure_policy(&config.name, config.expose_resources.clone());
    upstream_has_exposed_mcp_app_owner(tools, &resource_policy)
}

pub fn tool_is_mcp_app_host_visible_for_config(
    tool: &UpstreamTool,
    upstream_tools: &[UpstreamTool],
    config: &UpstreamConfig,
) -> bool {
    if !config.proxy_resources {
        return false;
    }
    let resource_policy =
        resolve_request_resource_exposure_policy(&config.name, config.expose_resources.clone());
    if mcp_tool_resource_bindings(&tool.tool)
        .into_iter()
        .any(|uri| uri.is_some())
    {
        return mcp_tool_resource_bindings_are_exposed(&tool.tool, &resource_policy);
    }
    mcp_tool_is_mcp_app_callback(&tool.tool)
        && upstream_has_exposed_mcp_app_owner(upstream_tools, &resource_policy)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;

    use rmcp::model::MetaObject;

    use super::super::super::types::ToolExposurePolicy;
    use super::super::entries::healthy_in_process_entry;
    use super::super::testsupport::*;
    use super::*;

    #[tokio::test]
    async fn empty_pool_has_no_tools() {
        let pool = UpstreamPool::new();
        assert!(pool.healthy_tools().await.is_empty());
        assert_eq!(pool.upstream_count().await, 0);
    }

    #[tokio::test]
    async fn subject_scoped_registry_rejects_upstream_count_before_fanout() {
        let pool = UpstreamPool::new();
        let configs = (0..=MAX_SUBJECT_SCOPED_UPSTREAMS)
            .map(|index| {
                let mut config = named_test_upstream_config(&format!("oauth-{index}"));
                config.oauth = Some(labby_runtime::gateway_config::UpstreamOauthConfig {
                    mode: labby_runtime::gateway_config::UpstreamOauthMode::AuthorizationCodePkce,
                    registration:
                        labby_runtime::gateway_config::UpstreamOauthRegistration::Preregistered {
                            client_id: "client-id".into(),
                            client_secret_env: None,
                        },
                    scopes: None,
                    credential: Default::default(),
                    prefer_client_metadata_document: None,
                });
                config
            })
            .collect::<Vec<_>>();
        let started = tokio::time::Instant::now();

        assert!(
            pool.subject_scoped_tools(&configs, "alice")
                .await
                .is_empty()
        );
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn code_mode_ui_catalog_includes_standard_and_openai_app_callbacks() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("quick-shell");
        let mut tools = test_upstream_tools(
            &upstream_name,
            &[
                "open_quick_shell",
                "standard_app_callback",
                "openai_public_callback",
                "openai_private_callback",
                "openai_render_tool",
                "unrelated_model_tool",
            ],
        );
        tools
            .get_mut("open_quick_shell")
            .expect("UI tool")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([
            (
                "ui".to_string(),
                serde_json::json!({
                    "resourceUri": "ui://quick-shell/mcp-app.html",
                    "visibility": ["model", "app"]
                }),
            ),
            ("openai/visibility".to_string(), serde_json::json!("public")),
        ])));
        tools
            .get_mut("standard_app_callback")
            .expect("standard app callback")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "visibility": ["app"] }),
        )])));
        tools
            .get_mut("openai_public_callback")
            .expect("OpenAI public callback")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([(
            "openai/widgetAccessible".to_string(),
            serde_json::json!(true),
        )])));
        tools
            .get_mut("openai_render_tool")
            .expect("OpenAI render tool")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([(
            "openai/outputTemplate".to_string(),
            serde_json::json!("ui://quick-shell/openai-widget.html"),
        )])));
        tools
            .get_mut("openai_private_callback")
            .expect("OpenAI private callback")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([
            (
                "openai/widgetAccessible".to_string(),
                serde_json::json!(true),
            ),
            (
                "openai/visibility".to_string(),
                serde_json::json!("private"),
            ),
        ])));
        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        pool.catalog
            .write()
            .await
            .insert("quick-shell".to_string(), entry);

        let listed = pool
            .cached_mcp_app_tools_allowed(None, &[], None, MAX_UPSTREAM_TOOLS)
            .await;
        let mut names = listed
            .iter()
            .map(|tool| tool.tool.name.as_ref())
            .collect::<Vec<_>>();
        names.sort_unstable();

        assert_eq!(
            names,
            vec![
                "open_quick_shell",
                "openai_private_callback",
                "openai_public_callback",
                "openai_render_tool",
                "standard_app_callback",
            ],
            "the host needs standard and compatibility app callbacks in tools/list, while unrelated model tools stay hidden"
        );
    }

    #[tokio::test]
    async fn code_mode_ui_catalog_rejects_callback_markers_without_an_app_owner() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("apps");
        let mut tools = test_upstream_tools(&upstream_name, &["callback_only"]);
        tools.get_mut("callback_only").expect("callback").tool.meta =
            Some(MetaObject(serde_json::Map::from_iter([(
                "ui".to_string(),
                serde_json::json!({ "visibility": ["app"] }),
            )])));
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        entry.proxy_resources = true;
        pool.catalog.write().await.insert("apps".to_string(), entry);

        assert!(
            pool.cached_mcp_app_tools_allowed(None, &[], None, MAX_UPSTREAM_TOOLS)
                .await
                .is_empty()
        );
        assert!(
            pool.cached_mcp_app_tools_allowed(None, &[], None, MAX_UPSTREAM_TOOLS)
                .await
                .into_iter()
                .map(|tool| tool.tool.name.to_string())
                .collect::<Vec<_>>()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn code_mode_ui_catalog_respects_resource_exposure_for_bound_widgets() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("apps");
        let mut tools = test_upstream_tools(&upstream_name, &["render_app"]);
        tools.get_mut("render_app").expect("render app").tool.meta =
            Some(MetaObject(serde_json::Map::from_iter([(
                "ui".to_string(),
                serde_json::json!({ "resourceUri": "ui://apps/widget.html" }),
            )])));
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        entry.proxy_resources = true;
        entry.resource_exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["file:///allowed-only".into()])
                .expect("resource policy");
        pool.catalog.write().await.insert("apps".to_string(), entry);

        assert!(
            pool.cached_mcp_app_tools_allowed(None, &[], None, MAX_UPSTREAM_TOOLS)
                .await
                .is_empty()
        );
        assert!(
            pool.cached_mcp_app_tools_allowed(None, &[], None, MAX_UPSTREAM_TOOLS)
                .await
                .into_iter()
                .map(|tool| tool.tool.name.to_string())
                .collect::<Vec<_>>()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn code_mode_ui_catalog_omits_ambiguous_tool_names() {
        let pool = UpstreamPool::new();
        for upstream in ["alpha", "beta"] {
            let upstream_name: Arc<str> = Arc::from(upstream);
            let mut tools = test_upstream_tools(&upstream_name, &["shared_app"]);
            tools.get_mut("shared_app").expect("shared app").tool.meta =
                Some(MetaObject(serde_json::Map::from_iter([(
                    "ui".to_string(),
                    serde_json::json!({ "resourceUri": format!("ui://{upstream}/widget.html") }),
                )])));
            let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
            entry.proxy_resources = true;
            pool.catalog
                .write()
                .await
                .insert(upstream.to_string(), entry);
        }

        assert!(
            pool.cached_mcp_app_tools_allowed(None, &[], None, MAX_UPSTREAM_TOOLS)
                .await
                .is_empty(),
            "an ambiguous app tool name must not advertise an arbitrary upstream descriptor"
        );
        assert!(
            pool.cached_mcp_app_tools_allowed(None, &[], None, MAX_UPSTREAM_TOOLS)
                .await
                .into_iter()
                .map(|tool| tool.tool.name.to_string())
                .collect::<Vec<_>>()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn combined_mcp_app_catalog_omits_global_oauth_name_collisions() {
        let pool = UpstreamPool::new();

        let global_name: Arc<str> = Arc::from("global-apps");
        let mut global_tools = test_upstream_tools(&global_name, &["shared_app"]);
        global_tools
            .get_mut("shared_app")
            .expect("global app")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://global-apps/widget.html" }),
        )])));
        let mut global_entry = healthy_in_process_entry(global_name, global_tools);
        global_entry.proxy_resources = true;
        pool.catalog
            .write()
            .await
            .insert("global-apps".to_string(), global_entry);

        let oauth_name: Arc<str> = Arc::from("oauth-apps");
        let mut oauth_tools = test_upstream_tools(&oauth_name, &["shared_app"]);
        oauth_tools
            .get_mut("shared_app")
            .expect("oauth app")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://oauth-apps/widget.html" }),
        )])));
        let mut oauth = named_test_upstream_config("oauth-apps");
        oauth.proxy_resources = true;
        oauth.oauth = Some(labby_runtime::gateway_config::UpstreamOauthConfig {
            mode: labby_runtime::gateway_config::UpstreamOauthMode::AuthorizationCodePkce,
            registration: labby_runtime::gateway_config::UpstreamOauthRegistration::Preregistered {
                client_id: "client-id".to_string(),
                client_secret_env: None,
            },
            scopes: None,
            credential: Default::default(),
            prefer_client_metadata_document: None,
        });
        pool.install_test_subject_tools_for_upstream(
            &oauth,
            "alice",
            oauth_tools.into_values().map(|tool| tool.tool).collect(),
        )
        .await;

        assert!(
            pool.cached_mcp_app_tools_allowed(
                None,
                std::slice::from_ref(&oauth),
                Some("alice"),
                MAX_UPSTREAM_TOOLS,
            )
            .await
            .is_empty(),
            "a tool name claimed by global and OAuth upstreams must fail closed"
        );
    }

    #[tokio::test]
    async fn oauth_ui_resource_owner_rejects_global_metadata_collision() {
        let pool = UpstreamPool::new();

        let global_name: Arc<str> = Arc::from("global-apps");
        let mut global_tools = test_upstream_tools(&global_name, &["global_render"]);
        global_tools
            .get_mut("global_render")
            .expect("global render")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://oauth-apps/widget.html" }),
        )])));
        let mut global_entry = healthy_in_process_entry(global_name, global_tools);
        global_entry.proxy_resources = true;
        pool.catalog
            .write()
            .await
            .insert("global-apps".to_string(), global_entry);

        let oauth_name: Arc<str> = Arc::from("oauth-apps");
        let mut oauth_tools = test_upstream_tools(&oauth_name, &["oauth_render"]);
        oauth_tools
            .get_mut("oauth_render")
            .expect("oauth render")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://oauth-apps/widget.html" }),
        )])));
        let mut oauth = named_test_upstream_config("oauth-apps");
        oauth.proxy_resources = true;
        oauth.oauth = Some(labby_runtime::gateway_config::UpstreamOauthConfig {
            mode: labby_runtime::gateway_config::UpstreamOauthMode::AuthorizationCodePkce,
            registration: labby_runtime::gateway_config::UpstreamOauthRegistration::Preregistered {
                client_id: "client-id".to_string(),
                client_secret_env: None,
            },
            scopes: None,
            credential: Default::default(),
            prefer_client_metadata_document: None,
        });
        pool.install_test_subject_tools_for_upstream(
            &oauth,
            "alice",
            oauth_tools.into_values().map(|tool| tool.tool).collect(),
        )
        .await;

        assert!(
            pool.cached_subject_scoped_ui_resource_owner(
                std::slice::from_ref(&oauth),
                "alice",
                "ui://oauth-apps/widget.html",
                None,
            )
            .await
            .is_err(),
            "the same native UI URI claimed by global and OAuth metadata must fail closed"
        );
    }

    #[tokio::test]
    async fn oauth_mcp_app_listing_owner_and_native_read_share_subject_scope() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("oauth-apps");
        let mut tools = test_upstream_tools(&upstream_name, &["render_app"]);
        let tool = tools.get_mut("render_app").expect("render app");
        tool.tool.meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://oauth-apps/widget.html" }),
        )])));
        let tool = tool.tool.clone();
        let mut config = named_test_upstream_config("oauth-apps");
        config.proxy_resources = true;
        config.oauth = Some(labby_runtime::gateway_config::UpstreamOauthConfig {
            mode: labby_runtime::gateway_config::UpstreamOauthMode::AuthorizationCodePkce,
            registration: labby_runtime::gateway_config::UpstreamOauthRegistration::Preregistered {
                client_id: "client-id".to_string(),
                client_secret_env: None,
            },
            scopes: None,
            credential: Default::default(),
            prefer_client_metadata_document: None,
        });
        pool.install_test_subject_tools_for_upstream(&config, "alice", vec![tool])
            .await;

        let listed = pool
            .cached_mcp_app_tools_allowed(
                None,
                std::slice::from_ref(&config),
                Some("alice"),
                MAX_UPSTREAM_TOOLS,
            )
            .await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].tool.name.as_ref(), "render_app");
        let owner = pool
            .cached_subject_scoped_ui_resource_owner(
                std::slice::from_ref(&config),
                "alice",
                "ui://oauth-apps/widget.html",
                None,
            )
            .await
            .expect("unambiguous OAuth UI owner")
            .expect("OAuth UI owner");
        assert_eq!(owner.name, "oauth-apps");

        pool.subject_scoped_read_resource_request(
            &config,
            "alice",
            rmcp::model::ReadResourceRequestParams::new("ui://oauth-apps/widget.html"),
        )
        .await
        .expect("native OAuth UI resource should use the subject-scoped connection");

        config.expose_resources = Some(vec!["ui://oauth-apps/allowed-only.html".to_string()]);
        assert!(
            pool.cached_mcp_app_tools_allowed(
                None,
                std::slice::from_ref(&config),
                Some("alice"),
                MAX_UPSTREAM_TOOLS,
            )
            .await
            .is_empty()
        );
        assert!(
            pool.cached_subject_scoped_ui_resource_owner(
                std::slice::from_ref(&config),
                "alice",
                "ui://oauth-apps/widget.html",
                None,
            )
            .await
            .is_err(),
            "an OAuth-owned URI blocked by expose_resources must fail closed instead of falling back to a global connection"
        );
    }

    #[tokio::test]
    async fn hidden_upstream_tools_do_not_appear_in_listings() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let tools = test_upstream_tools(
            &upstream_name,
            &["search_repos", "github_create_issue", "delete_repo"],
        );
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["search_repos".into(), "github_*".into()])
                .expect("policy");

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        let names: Vec<String> = pool
            .healthy_tools()
            .await
            .into_iter()
            .map(|t| t.tool.name.to_string())
            .collect();
        assert!(names.contains(&"search_repos".to_string()));
        assert!(names.contains(&"github_create_issue".to_string()));
        assert!(!names.contains(&"delete_repo".to_string()));
    }

    #[tokio::test]
    async fn healthy_tools_are_sorted_before_the_global_cap_is_applied() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("large");
        let names = (0..(MAX_UPSTREAM_TOOLS + 50))
            .rev()
            .map(|index| format!("tool_{index:04}"))
            .collect::<Vec<_>>();
        let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
        let tools = test_upstream_tools(&upstream_name, &name_refs);
        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        pool.catalog
            .write()
            .await
            .insert("large".to_string(), entry);

        let listed = pool.healthy_tools().await;
        let listed_names = listed
            .iter()
            .map(|tool| tool.tool.name.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(listed_names.len(), MAX_UPSTREAM_TOOLS);
        assert_eq!(listed_names.first().copied(), Some("tool_0000"));
        assert_eq!(listed_names.last().copied(), Some("tool_0999"));
        assert!(listed_names.is_sorted());
    }

    #[tokio::test]
    async fn per_upstream_full_listing_stays_complete_while_bounded_listing_caps() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("large");
        let names = (0..128)
            .rev()
            .map(|index| format!("tool_{index:04}"))
            .collect::<Vec<_>>();
        let name_refs = names.iter().map(String::as_str).collect::<Vec<_>>();
        let tools = test_upstream_tools(&upstream_name, &name_refs);
        pool.catalog.write().await.insert(
            "large".to_string(),
            healthy_in_process_entry(Arc::clone(&upstream_name), tools),
        );

        let full = pool.healthy_tools_for_upstream("large").await;
        let bounded = pool.healthy_tools_for_upstream_bounded("large", 10).await;

        assert_eq!(full.len(), 128);
        assert!(
            full.windows(2)
                .all(|pair| pair[0].tool.name <= pair[1].tool.name)
        );
        assert_eq!(bounded.len(), 10);
        assert_eq!(
            bounded
                .iter()
                .map(|tool| tool.tool.name.as_ref())
                .collect::<Vec<_>>(),
            full[..10]
                .iter()
                .map(|tool| tool.tool.name.as_ref())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn hidden_upstream_tools_cannot_be_called_directly() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("github");
        let tools = test_upstream_tools(&upstream_name, &["search_repos", "delete_repo"]);
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["search_repos".into()]).expect("policy");

        pool.catalog
            .write()
            .await
            .insert("github".to_string(), entry);

        assert!(pool.find_tool("search_repos").await.is_some());
        assert!(pool.find_tool("delete_repo").await.is_none());
    }

    #[tokio::test]
    async fn mcp_app_sibling_lookup_requires_exposed_ui_tool_on_same_upstream() {
        let pool = UpstreamPool::new();

        let apps_name: Arc<str> = Arc::from("apps");
        let mut apps_tools =
            test_upstream_tools(&apps_name, &["youtube_search_ui", "youtube_probe"]);
        let ui_meta = MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://apps/youtube-search.html" }),
        )]));
        apps_tools
            .get_mut("youtube_search_ui")
            .expect("ui tool")
            .tool
            .meta = Some(ui_meta);
        let apps_entry = healthy_in_process_entry(Arc::clone(&apps_name), apps_tools);
        pool.catalog
            .write()
            .await
            .insert("apps".to_string(), apps_entry);

        let plain_name: Arc<str> = Arc::from("plain");
        let plain_tools = test_upstream_tools(&plain_name, &["youtube_probe"]);
        let plain_entry = healthy_in_process_entry(Arc::clone(&plain_name), plain_tools);
        pool.catalog
            .write()
            .await
            .insert("plain".to_string(), plain_entry);

        let candidates = pool
            .find_mcp_app_sibling_tool_candidates("youtube_probe", None)
            .await;
        let upstreams = candidates
            .iter()
            .map(|(upstream, _)| upstream.as_str())
            .collect::<Vec<_>>();

        assert_eq!(upstreams, vec!["apps"]);

        let allowed = BTreeSet::from(["plain".to_string()]);
        assert!(
            pool.find_mcp_app_sibling_tool_candidates("youtube_probe", Some(&allowed))
                .await
                .is_empty(),
            "route scope must still constrain MCP App callback siblings"
        );
    }

    #[tokio::test]
    async fn mcp_app_sibling_lookup_returns_all_candidate_upstreams() {
        // When a hidden tool name is exposed by more than one UI-bearing upstream,
        // the lookup must surface every candidate so the call gate can detect the
        // ambiguity and fail closed (rather than silently picking one).
        let pool = UpstreamPool::new();
        for upstream in ["apps_a", "apps_b"] {
            let name: Arc<str> = Arc::from(upstream);
            let mut tools = test_upstream_tools(&name, &["search_ui", "youtube_probe"]);
            tools.get_mut("search_ui").expect("ui tool").tool.meta =
                Some(MetaObject(serde_json::Map::from_iter([(
                    "ui".to_string(),
                    serde_json::json!({ "resourceUri": format!("ui://{upstream}/s.html") }),
                )])));
            let entry = healthy_in_process_entry(Arc::clone(&name), tools);
            pool.catalog
                .write()
                .await
                .insert(upstream.to_string(), entry);
        }

        let candidates = pool
            .find_mcp_app_sibling_tool_candidates("youtube_probe", None)
            .await;
        let upstreams = candidates
            .iter()
            .map(|(upstream, _)| upstream.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            upstreams,
            vec!["apps_a", "apps_b"],
            "both UI-bearing upstreams must be returned so the gate can detect ambiguity"
        );
    }

    #[tokio::test]
    async fn find_exposed_tool_candidates_allowed_filters_by_scope_and_exposure() {
        let pool = UpstreamPool::new();

        // Upstream "a" exposes `probe`.
        let a: Arc<str> = Arc::from("a");
        let a_tools = test_upstream_tools(&a, &["probe"]);
        pool.catalog_write().await.insert(
            "a".to_string(),
            healthy_in_process_entry(Arc::clone(&a), a_tools),
        );

        // Upstream "b" has `probe` but hides it via exposure policy.
        let b: Arc<str> = Arc::from("b");
        let b_tools = test_upstream_tools(&b, &["probe", "other"]);
        let mut b_entry = healthy_in_process_entry(Arc::clone(&b), b_tools);
        b_entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["other".into()]).expect("policy");
        pool.catalog_write().await.insert("b".to_string(), b_entry);

        // No route scope: only "a" exposes `probe` ("b" hides it).
        let all = pool
            .find_exposed_tool_candidates_allowed("probe", None)
            .await;
        assert_eq!(
            all.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(),
            vec!["a"],
            "exposure policy must hide `probe` on upstream b"
        );

        // Route scope excluding "a" yields nothing.
        let scoped = BTreeSet::from(["b".to_string()]);
        assert!(
            pool.find_exposed_tool_candidates_allowed("probe", Some(&scoped))
                .await
                .is_empty(),
            "route scope must exclude `probe` on a non-allowed upstream"
        );
    }

    #[tokio::test]
    async fn mcp_app_sibling_lookup_respects_exposure_policy() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("apps");
        let mut tools = test_upstream_tools(
            &upstream_name,
            &["youtube_search_ui", "youtube_probe", "internal_delete"],
        );
        tools
            .get_mut("youtube_search_ui")
            .expect("ui tool")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://apps/youtube-search.html" }),
        )])));
        let mut entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        entry.exposure_policy = ToolExposurePolicy::from_patterns(vec![
            "youtube_search_ui".to_string(),
            "youtube_probe".to_string(),
        ])
        .expect("policy");
        pool.catalog_write().await.insert("apps".to_string(), entry);

        assert_eq!(
            pool.find_mcp_app_sibling_tool_candidates("youtube_probe", None)
                .await
                .len(),
            1
        );
        assert!(
            pool.find_mcp_app_sibling_tool_candidates("internal_delete", None)
                .await
                .is_empty(),
            "unexposed sibling tools must remain uncallable"
        );
    }

    // --- lab-tad5: oversized catalog bounds regression tests ---

    /// A gateway pool that receives more than `MAX_UPSTREAM_TOOLS` tools must cap
    /// the result at exactly the limit and not panic or allocate unboundedly.
    #[tokio::test]
    async fn gateway_upstream_tool_cap_truncates_oversized_catalog() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("big-upstream");

        // Build more tools than the cap.
        let tool_names: Vec<String> = (0..MAX_UPSTREAM_TOOLS + 50)
            .map(|i| format!("tool_{i:04}"))
            .collect();
        let tool_name_refs: Vec<&str> = tool_names.iter().map(String::as_str).collect();
        let tools = test_upstream_tools(&upstream_name, &tool_name_refs);

        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        pool.catalog
            .write()
            .await
            .insert("big-upstream".to_string(), entry);

        let result = pool.healthy_tools().await;
        assert_eq!(
            result.len(),
            MAX_UPSTREAM_TOOLS,
            "healthy_tools() must cap at MAX_UPSTREAM_TOOLS={MAX_UPSTREAM_TOOLS}"
        );
    }

    /// A pool with exactly `MAX_UPSTREAM_TOOLS` tools must NOT be truncated.
    #[tokio::test]
    async fn gateway_upstream_tool_cap_allows_exactly_limit_tools() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("exact-upstream");

        let tool_names: Vec<String> = (0..MAX_UPSTREAM_TOOLS)
            .map(|i| format!("tool_{i:04}"))
            .collect();
        let tool_name_refs: Vec<&str> = tool_names.iter().map(String::as_str).collect();
        let tools = test_upstream_tools(&upstream_name, &tool_name_refs);

        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        pool.catalog
            .write()
            .await
            .insert("exact-upstream".to_string(), entry);

        let result = pool.healthy_tools().await;
        assert_eq!(
            result.len(),
            MAX_UPSTREAM_TOOLS,
            "healthy_tools() must not truncate exactly MAX_UPSTREAM_TOOLS tools"
        );
    }

    #[tokio::test]
    async fn gateway_upstream_tool_byte_budget_applies_before_result_insertion() {
        let pool = UpstreamPool::new();
        let upstream_name: Arc<str> = Arc::from("large-schema-upstream");
        let mut tools = test_upstream_tools(&upstream_name, &["first", "second"]);
        tools.get_mut("first").unwrap().tool.description =
            Some("x".repeat(max_response_bytes()).into());
        assert!(
            tool_catalog_bytes(tools.get("first").unwrap()) > max_response_bytes(),
            "oversized fixture must exceed the configured response-byte cap"
        );
        let entry = healthy_in_process_entry(Arc::clone(&upstream_name), tools);
        pool.catalog
            .write()
            .await
            .insert(upstream_name.to_string(), entry);

        assert!(pool.healthy_tools().await.is_empty());
    }

    #[test]
    fn allocation_free_tool_byte_counter_matches_json_encoding() {
        let upstream: Arc<str> = Arc::from("fixture");
        let tools = test_upstream_tools(&upstream, &["search"]);
        let tool = &tools.get("search").expect("fixture tool").tool;
        assert_eq!(
            serialized_tool_bytes(tool),
            serde_json::to_vec(tool).expect("serialize fixture").len()
        );
    }

    #[tokio::test]
    async fn healthy_tool_names_allowed_matches_route_and_exposure_filters() {
        let pool = UpstreamPool::new();
        let allowed_name: Arc<str> = Arc::from("allowed");
        let denied_name: Arc<str> = Arc::from("denied");

        let allowed_tools = test_upstream_tools(&allowed_name, &["visible", "hidden"]);
        let mut allowed_entry = healthy_in_process_entry(Arc::clone(&allowed_name), allowed_tools);
        allowed_entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["visible".to_string()]).expect("policy");
        pool.catalog
            .write()
            .await
            .insert("allowed".to_string(), allowed_entry);

        let denied_tools = test_upstream_tools(&denied_name, &["denied_tool"]);
        pool.catalog_write().await.insert(
            "denied".to_string(),
            healthy_in_process_entry(Arc::clone(&denied_name), denied_tools),
        );

        let allowed = BTreeSet::from(["allowed".to_string()]);
        assert_eq!(
            pool.healthy_tool_names_allowed(Some(&allowed)).await,
            vec!["visible".to_string()]
        );
    }

    /// Regression (tools flapping): under Code Mode the reconcile snapshot uses
    /// `healthy_ui_tool_names_allowed`, which must track ONLY MCP-App UI tools.
    /// Raw upstream churn (an upstream becoming healthy and discovering plain
    /// tools) must be invisible to it, so `tools/list_changed` is not emitted
    /// for a change the Code-Mode client can never see — while `healthy_tools`
    /// (the non-Code-Mode projection) still reflects the raw tool set.
    #[tokio::test]
    async fn healthy_ui_tool_names_hide_raw_tool_churn_under_code_mode() {
        let pool = UpstreamPool::new();

        // An app upstream exposing one UI tool and one plain tool.
        let apps: Arc<str> = Arc::from("apps");
        let mut apps_tools = test_upstream_tools(&apps, &["youtube_search_ui", "youtube_probe"]);
        apps_tools
            .get_mut("youtube_search_ui")
            .expect("ui tool")
            .tool
            .meta = Some(MetaObject(serde_json::Map::from_iter([(
            "ui".to_string(),
            serde_json::json!({ "resourceUri": "ui://apps/youtube-search.html" }),
        )])));
        pool.catalog_write().await.insert(
            "apps".to_string(),
            healthy_in_process_entry(Arc::clone(&apps), apps_tools),
        );

        // Only the UI tool is individually visible under Code Mode.
        assert_eq!(
            pool.cached_mcp_app_tools_allowed(None, &[], None, MAX_UPSTREAM_TOOLS)
                .await
                .into_iter()
                .map(|tool| tool.tool.name.to_string())
                .collect::<Vec<_>>(),
            vec!["youtube_search_ui".to_string()]
        );

        // A brand-new upstream comes online carrying only plain (non-UI) tools —
        // exactly the "late upstream/app hydration" churn from the incident.
        let plain: Arc<str> = Arc::from("plain");
        let plain_tools = test_upstream_tools(&plain, &["search", "download"]);
        pool.catalog_write().await.insert(
            "plain".to_string(),
            healthy_in_process_entry(Arc::clone(&plain), plain_tools),
        );

        // Code-Mode projection is unchanged → reconcile diff stays tools_changed=false.
        assert_eq!(
            pool.cached_mcp_app_tools_allowed(None, &[], None, MAX_UPSTREAM_TOOLS)
                .await
                .into_iter()
                .map(|tool| tool.tool.name.to_string())
                .collect::<Vec<_>>(),
            vec!["youtube_search_ui".to_string()],
            "raw upstream tool churn must not alter the Code-Mode-visible tool set"
        );

        // The raw (non-Code-Mode) projection does grow, confirming the churn is
        // real and only hidden by the Code-Mode filter.
        let raw: BTreeSet<String> = pool
            .healthy_tools()
            .await
            .into_iter()
            .map(|t| t.tool.name.to_string())
            .collect();
        assert!(
            raw.contains("search") && raw.contains("download") && raw.contains("youtube_probe")
        );
    }
}
