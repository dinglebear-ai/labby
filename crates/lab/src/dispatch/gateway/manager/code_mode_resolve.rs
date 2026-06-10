//! Code Mode tool resolution: mapping `<upstream>::<tool>` selectors onto live
//! upstream catalog entries for `execute`/`callTool` and the raw tool proxy.

use std::collections::HashMap;

use crate::dispatch::error::ToolError;
use crate::dispatch::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};

use super::GatewayManager;

impl GatewayManager {
    pub async fn resolve_code_mode_upstream_tool(
        &self,
        upstream: &str,
        tool: &str,
        owner: Option<&UpstreamRuntimeOwner>,
        oauth_subject: Option<&str>,
    ) -> Result<UpstreamTool, ToolError> {
        let cfg = self.config.read().await;
        // The gateway search/execute surface is gated by the single `code_mode.enabled`
        // toggle, which also exposes the tools. `execute` is only reachable when the
        // surface is exposed, so reject when it is off. This is the single-surface
        // (Cloudflare-parity) model: when search + execute are on, callTool resolution works.
        if !cfg.code_mode.enabled {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: "the gateway search/execute surface is not enabled; \
                    set [code_mode] enabled = true in config"
                    .to_string(),
            });
        }
        let priority = cfg
            .upstream
            .iter()
            .find(|candidate| candidate.name == upstream)
            .map(|candidate| candidate.priority.max(0.0))
            .unwrap_or(1.0);
        drop(cfg);

        if priority <= 0.0 {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("upstream tool `{upstream}::{tool}` was not found"),
            });
        }

        self.ensure_upstream_tool_runtime_ready(upstream, owner, oauth_subject)
            .await?;
        let pool = self.current_pool().await.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "unknown_tool".to_string(),
            message: format!("upstream tool `{upstream}::{tool}` was not found"),
        })?;

        pool.healthy_tools_for_upstream(upstream)
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
        let selector = ToolExecuteSelector::parse(tool, None)?;
        let cfg = self.config.read().await.clone();
        let priority_by_upstream: HashMap<String, f32> = cfg
            .upstream
            .iter()
            .map(|upstream| (upstream.name.clone(), upstream.priority.max(0.0)))
            .collect();

        let Some(pool) = self.current_pool().await else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_tool".to_string(),
                message: format!("unknown tool `{}`", selector.display_name()),
            });
        };

        if let Some(upstream_name) = selector.upstream.as_deref() {
            if priority_by_upstream
                .get(upstream_name)
                .copied()
                .unwrap_or(1.0)
                <= 0.0
            {
                return Err(ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
            }
            self.ensure_upstream_tool_runtime_ready(upstream_name, owner, oauth_subject)
                .await?;
            return pool
                .healthy_tools_for_upstream(upstream_name)
                .await
                .into_iter()
                .find(|candidate| candidate.tool.name.as_ref() == selector.tool_name)
                .map(|tool| (upstream_name.to_string(), tool))
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "unknown_tool".to_string(),
                    message: format!("unknown tool `{}`", selector.display_name()),
                });
        }

        if let Some((upstream, tool)) = pool.find_tool(&selector.tool_name).await
            && priority_by_upstream.get(&upstream).copied().unwrap_or(1.0) > 0.0
        {
            return Ok((upstream, tool));
        }

        let mut matches = Vec::new();
        for upstream in cfg
            .upstream
            .iter()
            .filter(|upstream| upstream.enabled && upstream.priority.max(0.0) > 0.0)
        {
            self.ensure_upstream_tool_runtime_ready(&upstream.name, owner, oauth_subject)
                .await?;
            matches.extend(
                pool.healthy_tools_for_upstream(&upstream.name)
                    .await
                    .into_iter()
                    .filter(|candidate| candidate.tool.name.as_ref() == selector.tool_name)
                    .map(|tool| (upstream.name.clone(), tool)),
            );
        }

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

        if let Some((upstream_name, tool_name)) = trimmed_name.split_once("::") {
            let upstream_name = upstream_name.trim();
            let tool_name = tool_name.trim();
            if upstream_name.is_empty() || tool_name.is_empty() {
                return Err(ToolError::Sdk {
                    sdk_kind: "invalid_param".to_string(),
                    message: "qualified tool names must use `<upstream>::<tool>`".to_string(),
                });
            }
            return Ok(Self {
                upstream: Some(upstream_name.to_string()),
                tool_name: tool_name.to_string(),
            });
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
