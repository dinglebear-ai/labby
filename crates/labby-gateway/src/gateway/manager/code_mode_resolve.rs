//! Code Mode tool resolution: mapping `<upstream>::<tool>` selectors onto live
//! upstream catalog entries for `execute`/`callTool` and the raw tool proxy.

use std::collections::{BTreeSet, HashMap};

use crate::gateway::code_mode::split_namespaced_id;
use crate::upstream::pool::{tool_has_mcp_app_ui_resource, tool_is_mcp_app_host_visible};
use crate::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};
use labby_runtime::error::ToolError;

use super::GatewayManager;

async fn routed_tools_for_upstream(
    pool: &crate::upstream::pool::UpstreamPool,
    config: &labby_runtime::gateway_config::UpstreamConfig,
    oauth_subject: Option<&str>,
) -> Vec<UpstreamTool> {
    if config.oauth.is_some() {
        let Some(subject) = oauth_subject else {
            return Vec::new();
        };
        return pool
            .subject_scoped_upstream_tools_allowed(std::slice::from_ref(config), subject, None)
            .await;
    }
    pool.healthy_tools_for_upstream(&config.name).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackToolLookup {
    LegacyAnyExposed,
    DirectMcpApp,
    SiblingOfMcpApp,
}

impl GatewayManager {
    pub async fn resolve_widget_callback_tool_candidates_scoped(
        &self,
        tool: &str,
        allowed_upstreams: Option<&BTreeSet<String>>,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
        lookup: CallbackToolLookup,
    ) -> Result<Vec<(String, UpstreamTool)>, ToolError> {
        let (cfg, pool) = self.published_config_and_pool().await;
        let Some(pool) = pool else {
            return Ok(Vec::new());
        };

        let mut matches = Vec::new();
        for upstream in cfg.upstream.iter().filter(|upstream| {
            upstream.enabled
                && is_routable(upstream.priority)
                && allowed_upstreams.is_none_or(|allowed| allowed.contains(&upstream.name))
        }) {
            if upstream.oauth.is_some() && oauth_subject.is_none() {
                continue;
            }
            // Ordinary callback gating is intentionally cache-only: a missing
            // allowed tool must remain `not_found`, not turn into a cold-connect
            // transport error. OAuth is the exception because its catalog is
            // never global; ensure only that subject-scoped cache here.
            if upstream.oauth.is_some() {
                self.ensure_upstream_tool_runtime_ready(&upstream.name, owner, oauth_subject)
                    .await?;
            }
            let upstream_tools = routed_tools_for_upstream(&pool, upstream, oauth_subject).await;
            let Some(candidate) = upstream_tools
                .iter()
                .find(|candidate| candidate.tool.name.as_ref() == tool)
            else {
                continue;
            };

            let matched = match lookup {
                CallbackToolLookup::LegacyAnyExposed => true,
                CallbackToolLookup::DirectMcpApp => tool_is_mcp_app_host_visible(candidate),
                CallbackToolLookup::SiblingOfMcpApp => {
                    upstream_tools.iter().any(tool_has_mcp_app_ui_resource)
                }
            };
            if matched {
                matches.push((upstream.name.clone(), candidate.clone()));
            }
        }
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(matches)
    }

    pub async fn resolve_code_mode_upstream_tool(
        &self,
        upstream: &str,
        tool: &str,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<UpstreamTool, ToolError> {
        let cfg = self.config.read().await;
        // The gateway Code Mode surface is gated by the single `code_mode.enabled`
        // toggle, which also exposes `codemode`. callTool resolution is reachable
        // only from that surface, so reject when it is off.
        if !cfg.code_mode.enabled {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "the gateway codemode surface is not enabled; \
                    set [code_mode] enabled = true in config"
                    .to_string(),
            });
        }
        let upstream_config = cfg
            .upstream
            .iter()
            .find(|candidate| candidate.name == upstream)
            .cloned();
        drop(cfg);

        let priority = upstream_config.as_ref().map(|c| c.priority).unwrap_or(1.0);
        if !is_routable(priority) {
            tracing::warn!(
                surface = "dispatch",
                service = "gateway",
                action = "code_mode.resolve_tool",
                upstream = %upstream,
                tool = %tool,
                priority = priority,
                "skipping tool resolution: upstream priority is non-positive (disabled)"
            );
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("upstream tool `{upstream}::{tool}` was not found"),
            });
        }
        if upstream_config
            .as_ref()
            .is_some_and(|config| config.oauth.is_some())
            && oauth_subject.is_none()
        {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("upstream tool `{upstream}::{tool}` was not found"),
            });
        }

        self.ensure_upstream_tool_runtime_ready(upstream, owner, oauth_subject)
            .await?;
        // Runtime readiness can race a reload. Re-snapshot both components so
        // execution never combines the old config with the newly published
        // pool (or vice versa).
        let (published_cfg, pool) = self.published_config_and_pool().await;
        if !published_cfg.code_mode.enabled {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "the gateway codemode surface is not enabled; set [code_mode] enabled = true in config".to_string(),
            });
        }
        let pool = pool.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "unknown_tool".to_string(),
            message: format!("upstream tool `{upstream}::{tool}` was not found"),
        })?;

        let Some(upstream_config) = published_cfg
            .upstream
            .iter()
            .find(|candidate| candidate.name == upstream && candidate.enabled)
        else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("upstream tool `{upstream}::{tool}` was not found"),
            });
        };
        if !is_routable(upstream_config.priority) {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("upstream tool `{upstream}::{tool}` was not found"),
            });
        }
        routed_tools_for_upstream(&pool, upstream_config, oauth_subject)
            .await
            .into_iter()
            .find(|candidate| candidate.tool.name.as_ref() == tool)
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("upstream tool `{upstream}::{tool}` was not found"),
            })
    }

    pub async fn resolve_raw_upstream_tool(
        &self,
        tool: &str,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(String, UpstreamTool), ToolError> {
        self.resolve_raw_upstream_tool_allowed(tool, None, owner, oauth_subject)
            .await
    }

    pub async fn resolve_raw_upstream_tool_scoped(
        &self,
        tool: &str,
        allowed_upstreams: Option<&BTreeSet<String>>,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(String, UpstreamTool), ToolError> {
        self.resolve_raw_upstream_tool_allowed(tool, allowed_upstreams, owner, oauth_subject)
            .await
    }

    async fn resolve_raw_upstream_tool_allowed(
        &self,
        tool: &str,
        allowed_upstreams: Option<&BTreeSet<String>>,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<(String, UpstreamTool), ToolError> {
        let selector = ToolExecuteSelector::parse(tool, None)?;
        let (cfg, pool) = self.published_config_and_pool().await;
        let priority_by_upstream: HashMap<String, f32> = cfg
            .upstream
            .iter()
            .map(|upstream| (upstream.name.clone(), upstream.priority))
            .collect();
        let Some(pool) = pool else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown tool `{}`", selector.display_name()),
            });
        };

        if let Some(upstream_name) = selector.upstream.as_deref() {
            if allowed_upstreams.is_some_and(|allowed| !allowed.contains(upstream_name)) {
                return Err(ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
            }
            let Some(upstream_config) = cfg
                .upstream
                .iter()
                .find(|upstream| upstream.name == upstream_name && upstream.enabled)
            else {
                return Err(ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
            };
            let priority = upstream_config.priority;
            if !is_routable(priority) {
                tracing::warn!(
                    surface = "dispatch",
                    service = "gateway",
                    action = "tool_execute.resolve_tool",
                    upstream = %upstream_name,
                    tool = %selector.tool_name,
                    priority = priority,
                    "skipping tool resolution: upstream priority is non-positive (disabled)"
                );
                return Err(ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
            }
            if upstream_config.oauth.is_some() && oauth_subject.is_none() {
                return Err(ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
            }
            self.ensure_upstream_tool_runtime_ready(upstream_name, owner, oauth_subject)
                .await?;
            return routed_tools_for_upstream(&pool, upstream_config, oauth_subject)
                .await
                .into_iter()
                .find(|candidate| candidate.tool.name.as_ref() == selector.tool_name)
                .map(|tool| (upstream_name.to_string(), tool))
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
        }

        let mut matches = pool
            .find_exposed_tool_candidates_allowed(&selector.tool_name, allowed_upstreams)
            .await;
        matches.retain(|(upstream, _)| {
            is_routable(priority_by_upstream.get(upstream).copied().unwrap_or(1.0))
        });
        for upstream in cfg
            .upstream
            .iter()
            .filter(|upstream| upstream.enabled)
            .filter(|upstream| {
                allowed_upstreams.is_none_or(|allowed| allowed.contains(&upstream.name))
            })
            .filter(|upstream| is_routable(upstream.priority))
        {
            if upstream.oauth.is_some() && oauth_subject.is_none() {
                continue;
            }
            if upstream.oauth.is_none()
                && matches
                    .iter()
                    .any(|(candidate, _)| candidate == &upstream.name)
            {
                continue;
            }
            self.ensure_upstream_tool_runtime_ready(&upstream.name, owner, oauth_subject)
                .await?;
            matches.extend(
                routed_tools_for_upstream(&pool, upstream, oauth_subject)
                    .await
                    .into_iter()
                    .filter(|candidate| candidate.tool.name.as_ref() == selector.tool_name)
                    .map(|tool| (upstream.name.clone(), tool)),
            );
        }

        matches.sort_by(|left, right| left.0.cmp(&right.0));
        matches.dedup_by(|left, right| left.0 == right.0 && left.1.tool.name == right.1.tool.name);

        if matches.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown tool `{}`", selector.display_name()),
            });
        }
        if matches.len() > 1 {
            let valid = matches
                .iter()
                .map(|(upstream, tool)| format!("{upstream}::{}", tool.tool.name))
                .collect::<Vec<_>>();
            return Err(ToolError::AmbiguousTool {
                message: format!(
                    "tool `{}` matched multiple upstream tools",
                    selector.tool_name
                ),
                valid,
            });
        }
        Ok(matches.into_iter().next().expect("checked len"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolExecuteSelector {
    upstream: Option<String>,
    tool_name: String,
}

impl ToolExecuteSelector {
    /// Parse a tool selector of the form `[<upstream>::]<tool>` or a bare tool
    /// name. When an explicit `upstream` hint is provided it takes precedence
    /// over an embedded `<upstream>::` prefix in `name`.
    ///
    /// The `<upstream>::<tool>` splitting is delegated to
    /// [`split_namespaced_id`] (from `labby-codemode`) so the two callers
    /// share one implementation.
    fn parse(name: &str, upstream: Option<&str>) -> Result<Self, ToolError> {
        let explicit_upstream = upstream.map(str::trim).filter(|value| !value.is_empty());
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: "tool name must not be empty".to_string(),
            });
        }

        if let Some(upstream_name) = explicit_upstream {
            let tool_name = trimmed_name
                .strip_prefix(upstream_name)
                .and_then(|rest| rest.strip_prefix("::"))
                .unwrap_or(trimmed_name)
                .trim();
            if tool_name.is_empty() {
                return Err(ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message: "tool name must not be empty".to_string(),
                });
            }
            return Ok(Self {
                upstream: Some(upstream_name.to_string()),
                tool_name: tool_name.to_string(),
            });
        }

        // Use the shared `<upstream>::<tool>` splitter from `code_mode::types`
        // instead of an inline `split_once("..")` so the two implementations
        // stay in sync (e.g. both reject `a::b::c` and empty segments).
        if trimmed_name.contains("::") {
            return match split_namespaced_id(trimmed_name) {
                Some((upstream_name, tool_name)) => Ok(Self {
                    upstream: Some(upstream_name.to_string()),
                    tool_name: tool_name.to_string(),
                }),
                None => Err(ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message: "qualified tool names must use `<upstream>::<tool>`".to_string(),
                }),
            };
        }

        Ok(Self {
            upstream: None,
            tool_name: trimmed_name.to_string(),
        })
    }

    fn display_name(&self) -> String {
        match &self.upstream {
            Some(upstream) => format!("{upstream}::{}", self.tool_name),
            None => self.tool_name.clone(),
        }
    }
}

/// Returns `true` when `priority` makes an upstream eligible for tool
/// resolution.
///
/// A non-positive priority (`<= 0.0`) is the conventional way to disable an
/// upstream without removing it from the config. The named predicate makes the
/// intent explicit at every check site and avoids the subtle risk of a
/// misread `> 0.0` / `<= 0.0` comparison.
#[inline]
fn is_routable(priority: f32) -> bool {
    priority > 0.0
}
