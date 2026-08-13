//! Tool queries on the upstream pool: healthy-tool listings, candidate/owner
//! lookup, schema and exposure rows, cached summaries, runtime metadata, and
//! tool health. `has_healthy_tools_for_upstream` is `pub(super)` because
//! `ensure.rs` calls it across the module boundary (plan §3.0/§2.1).

use std::collections::{BTreeMap, BTreeSet};

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use serde_json::Value;

use labby_runtime::gateway_config::UpstreamConfig;

use super::super::types::{
    UpstreamCapability, UpstreamEnrichmentCatalogEntry, UpstreamHealth, UpstreamRuntimeMetadata,
    UpstreamTool, UpstreamToolExposureRow,
};
use super::UpstreamPool;
use super::entries::resolve_request_exposure_policy;
use super::helpers::UpstreamCachedSummary;

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
        let mut tools = Vec::new();
        let mut candidate_count = 0usize;
        for tool in catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .filter(|(_, entry)| entry.tool_health.is_routable())
            .flat_map(|(_, entry)| {
                entry.tools.values().filter_map(|tool| {
                    entry
                        .exposure_policy
                        .matches(tool.tool.name.as_ref())
                        .then(|| tool.clone())
                })
            })
        {
            candidate_count = candidate_count.saturating_add(1);
            insert_bounded_upstream_tool(&mut tools, tool, MAX_UPSTREAM_TOOLS);
        }
        if candidate_count > MAX_UPSTREAM_TOOLS {
            tracing::warn!(
                limit = MAX_UPSTREAM_TOOLS,
                "upstream tool catalog exceeds limit — truncating to cap"
            );
        }
        tools
    }

    /// Return healthy tools that an MCP App host must advertise.
    ///
    /// This includes both tools that own a UI resource and private/app-visible
    /// callbacks invoked by that resource. Hosts such as Codex reject an app's
    /// `tools/call` before it reaches Labby when the callback is absent from
    /// `tools/list`, even though the callback remains hidden from the model.
    pub async fn healthy_ui_tools_allowed(
        &self,
        allowed: Option<&BTreeSet<String>>,
    ) -> Vec<UpstreamTool> {
        let catalog = self.catalog.read().await;
        let mut tools = Vec::new();
        let mut candidate_count = 0usize;
        for tool in catalog
            .iter()
            .filter(|(name, _)| upstream_allowed(allowed, name))
            .filter(|(_, entry)| entry.tool_health.is_routable())
            .filter(|(_, entry)| entry.proxy_resources)
            .flat_map(|(_, entry)| {
                entry.tools.values().filter_map(|tool| {
                    (entry.exposure_policy.matches(tool.tool.name.as_ref())
                        && tool_is_mcp_app_host_visible(tool))
                    .then(|| tool.clone())
                })
            })
        {
            candidate_count = candidate_count.saturating_add(1);
            insert_bounded_upstream_tool(&mut tools, tool, MAX_UPSTREAM_TOOLS);
        }
        if candidate_count > MAX_UPSTREAM_TOOLS {
            tracing::warn!(
                limit = MAX_UPSTREAM_TOOLS,
                "upstream MCP App tool catalog exceeds limit — truncating to cap"
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
            if !upstream_allowed(allowed, upstream_name) || !entry.tool_health.is_routable() {
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
        self.subject_scoped_tools_inner(configs, subject, None)
            .await
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
        self.subject_scoped_tools_inner(configs, subject, Some(limit))
            .await
    }

    async fn subject_scoped_tools_inner(
        &self,
        configs: &[UpstreamConfig],
        subject: &str,
        limit: Option<usize>,
    ) -> Vec<(String, Vec<rmcp::model::Tool>)> {
        let mut futures = FuturesUnordered::new();
        for config in configs.iter().filter(|config| config.oauth.is_some()) {
            let config = config.clone();
            let subject = subject.to_string();
            let pool = self.clone();
            futures.push(async move {
                let exposure_policy =
                    resolve_request_exposure_policy(&config.name, config.expose_tools.clone());
                let result = pool.acquire_or_connect_subject(&config, &subject).await;
                (config.name.clone(), exposure_policy, result)
            });
        }

        let mut discovered = Vec::new();
        let mut bounded = Vec::new();
        let mut exposed_count = 0usize;
        while let Some((name, exposure_policy, result)) = futures.next().await {
            match result {
                Ok((_peer, tools)) => {
                    let discovered_count = tools.len();
                    let mut exposed: Vec<rmcp::model::Tool> = tools
                        .into_iter()
                        .filter(|tool| exposure_policy.matches(tool.name.as_ref()))
                        .collect();
                    exposed.sort_by(|left, right| left.name.cmp(&right.name));
                    let hidden_count = discovered_count - exposed.len();
                    if hidden_count > 0 {
                        tracing::debug!(
                            upstream = %name,
                            hidden_count,
                            exposed_count = exposed.len(),
                            "subject-scoped upstream tools hidden by exposure policy"
                        );
                    }
                    if let Some(limit) = limit {
                        exposed_count = exposed_count.saturating_add(exposed.len());
                        for tool in exposed {
                            insert_bounded_subject_tool(&mut bounded, name.clone(), tool, limit);
                        }
                    } else {
                        discovered.push((name, exposed));
                    }
                }
                Err(error) => {
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
            return by_upstream.into_iter().collect();
        }
        discovered.sort_by(|left, right| left.0.cmp(&right.0));
        discovered
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

        Some(UpstreamCachedSummary {
            discovered_tool_count,
            exposed_tool_count,
            discovered_resource_count,
            exposed_resource_count,
            discovered_prompt_count,
            exposed_prompt_count,
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

    /// Return just the names of healthy MCP App host-visible tools allowed by a route scope.
    ///
    /// Mirrors `healthy_ui_tools_allowed` without cloning tool schemas, for
    /// downstream `tools/list_changed` snapshot comparisons.
    pub async fn healthy_ui_tool_names_allowed(
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
            .filter(|(_, entry)| entry.proxy_resources)
            .flat_map(|(_, entry)| {
                entry.tools.values().filter_map(|tool| {
                    (entry.exposure_policy.matches(tool.tool.name.as_ref())
                        && tool_is_mcp_app_host_visible(tool))
                    .then(|| tool.tool.name.to_string())
                })
            })
        {
            insert_bounded_name(&mut names, name, MAX_UPSTREAM_TOOLS);
        }
        names
    }
}

pub(super) fn tool_mcp_app_ui_resource_uri(tool: &UpstreamTool) -> Option<&str> {
    tool.tool
        .meta
        .as_ref()
        .and_then(|meta| meta.0.get("ui"))
        .and_then(|ui| ui.get("resourceUri"))
        .and_then(Value::as_str)
        .filter(|uri| uri.starts_with("ui://"))
}

pub fn tool_has_mcp_app_ui_resource(tool: &UpstreamTool) -> bool {
    tool_mcp_app_ui_resource_uri(tool).is_some()
}

pub fn tool_is_mcp_app_host_visible(tool: &UpstreamTool) -> bool {
    if tool_has_mcp_app_ui_resource(tool) {
        return true;
    }
    let Some(meta) = tool.tool.meta.as_ref() else {
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

        let listed = pool.healthy_ui_tools_allowed(None).await;
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
                "standard_app_callback",
            ],
            "the host needs standard and compatibility app callbacks in tools/list, while unrelated model tools stay hidden"
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
        pool.catalog.write().await.insert(
            "a".to_string(),
            healthy_in_process_entry(Arc::clone(&a), a_tools),
        );

        // Upstream "b" has `probe` but hides it via exposure policy.
        let b: Arc<str> = Arc::from("b");
        let b_tools = test_upstream_tools(&b, &["probe", "other"]);
        let mut b_entry = healthy_in_process_entry(Arc::clone(&b), b_tools);
        b_entry.exposure_policy =
            ToolExposurePolicy::from_patterns(vec!["other".into()]).expect("policy");
        pool.catalog.write().await.insert("b".to_string(), b_entry);

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
        pool.catalog.write().await.insert("apps".to_string(), entry);

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
        pool.catalog.write().await.insert(
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
        pool.catalog.write().await.insert(
            "apps".to_string(),
            healthy_in_process_entry(Arc::clone(&apps), apps_tools),
        );

        // Only the UI tool is individually visible under Code Mode.
        assert_eq!(
            pool.healthy_ui_tool_names_allowed(None).await,
            vec!["youtube_search_ui".to_string()]
        );

        // A brand-new upstream comes online carrying only plain (non-UI) tools —
        // exactly the "late upstream/app hydration" churn from the incident.
        let plain: Arc<str> = Arc::from("plain");
        let plain_tools = test_upstream_tools(&plain, &["search", "download"]);
        pool.catalog.write().await.insert(
            "plain".to_string(),
            healthy_in_process_entry(Arc::clone(&plain), plain_tools),
        );

        // Code-Mode projection is unchanged → reconcile diff stays tools_changed=false.
        assert_eq!(
            pool.healthy_ui_tool_names_allowed(None).await,
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
