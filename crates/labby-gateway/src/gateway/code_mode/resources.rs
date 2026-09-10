//! Resource discovery reuses the native MCP listing and URI rewriting paths.

use std::collections::BTreeSet;

use labby_codemode::{CodeModeCaller, CodeModeSurface, ToolScope};
use labby_runtime::error::ToolError;
use serde_json::{Value, json};

use crate::gateway::manager::GatewayManager;

use super::code_mode_host::{oauth_subject, runtime_owner};

pub(super) fn validate_read_uri(uri: &str) -> Result<(), ToolError> {
    if uri.starts_with("ui://")
        || uri.strip_prefix("lab://upstream/").is_some_and(|rest| {
            rest.split_once('/')
                .is_some_and(|(name, resource)| !name.is_empty() && !resource.is_empty())
        })
    {
        return Ok(());
    }
    Err(ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: "readResource requires a resource URI, not an upstream::tool identifier. \
            Use codemode.listResources(upstream) and pass a returned resources[].uri unchanged. \
            Proxied resource URIs use lab://upstream/<upstream>/<resource-uri>; native ui:// URIs are also supported."
            .to_string(),
    })
}

pub(super) async fn list_resources(
    manager: &GatewayManager,
    upstream: &str,
    caller: &CodeModeCaller,
    surface: CodeModeSurface,
    scope: &ToolScope,
) -> Result<Value, ToolError> {
    // Check scope before inspecting configuration or establishing a connection.
    if scope
        .allowed_namespaces()
        .is_some_and(|allowed| !allowed.contains(upstream))
    {
        return Err(ToolError::Sdk {
            sdk_kind: "forbidden".to_string(),
            message: "resource upstream is outside this Code Mode scope".to_string(),
        });
    }
    let config = manager
        .upstream_config(upstream)
        .await
        .filter(|config| config.enabled && config.proxy_resources)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: "upstream is not available for resource discovery".to_string(),
        })?;
    let subject = if config.oauth.is_some() {
        Some(
            oauth_subject(caller)
                .filter(|subject| !subject.is_empty())
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "forbidden".to_string(),
                    message: "OAuth resource discovery requires a caller subject".to_string(),
                })?,
        )
    } else {
        None
    };
    let owner = runtime_owner(caller, surface);
    manager
        .ensure_upstream_tool_runtime_ready(upstream, Some(&owner), subject)
        .await?;
    let pool = manager.current_pool().await.ok_or_else(|| ToolError::Sdk {
        sdk_kind: "provider_unavailable".to_string(),
        message: "gateway upstream pool is unavailable".to_string(),
    })?;
    let resources = if let Some(subject) = subject {
        pool.subject_scoped_resources(&[config], subject).await
    } else {
        let allowed = BTreeSet::from([upstream.to_string()]);
        pool.list_upstream_resources_allowed(Some(&allowed)).await
    };
    Ok(json!({ "resources": resources }))
}
