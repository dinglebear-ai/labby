//! Resource discovery reuses the native MCP listing and URI rewriting paths.

use std::collections::BTreeSet;

use labby_codemode::{CodeModeCaller, CodeModeSurface, ToolScope};
use labby_runtime::error::ToolError;
use serde_json::{Value, json};

use crate::gateway::manager::GatewayManager;

use super::code_mode_host::{oauth_subject, runtime_owner};

impl GatewayManager {
    /// Read a Labby-owned operator resource through the same authorization as Code Mode.
    pub async fn read_code_mode_operator_resource(
        &self,
        uri: &str,
        caller: &CodeModeCaller,
        scope: &ToolScope,
    ) -> Result<Value, ToolError> {
        validate_read_uri(uri)?;
        read_local_resource(self, uri, caller, scope).await
    }
}

pub(super) fn validate_read_uri(uri: &str) -> Result<(), ToolError> {
    if matches!(
        uri,
        "lab://gateway/servers"
            | "lab://gateway/status"
            | "lab://gateway/limits"
            | "lab://capabilities"
    ) || uri.strip_prefix("lab://gateway/").is_some_and(|s| {
        s.strip_suffix("/schema")
            .is_some_and(|name| !name.is_empty() && !name.contains('/'))
    }) || uri.starts_with("ui://")
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

pub(super) async fn read_local_resource(
    manager: &GatewayManager,
    uri: &str,
    caller: &CodeModeCaller,
    scope: &ToolScope,
) -> Result<Value, ToolError> {
    // These are gateway operator views. A tool/route subset must not expose
    // aggregate metadata about capabilities outside that subset.
    if !caller.is_admin()
        || !caller.can_use_snippets()
        || scope.allowed_namespaces().is_some()
        || scope.allowed_tools().is_some()
    {
        return Err(ToolError::Forbidden {
            message: "Labby operator resources require an unscoped admin or trusted-local caller"
                .into(),
            required_scopes: vec!["lab:admin".into()],
        });
    }
    let enrichment = crate::gateway::GatewayEnrichmentScope {
        route_visible_upstreams: None,
        oauth_subject: caller.subject().map(str::to_owned),
    };
    let data = match uri {
        "lab://gateway/limits" => {
            let config = manager.code_mode_config().await;
            json!({"timeout_ms":config.timeout_ms,"max_source_bytes":config.max_source_bytes,
                "max_response_bytes":config.max_response_bytes,"max_response_tokens":config.max_response_tokens,
                "token_estimate_divisor":config.token_estimate_divisor,
                "storage":labby_codemode::effective_storage_limits()})
        }
        "lab://gateway/status" => {
            serde_json::to_value(manager.status_scoped(None, &enrichment).await?).map_err(|_| {
                ToolError::Sdk {
                    sdk_kind: "internal_error".into(),
                    message: "cannot serialize gateway status".into(),
                }
            })?
        }
        "lab://gateway/servers" => manager.gateway_servers_doc_scoped(&enrichment).await?,
        "lab://capabilities" => {
            let servers = manager.gateway_servers_doc_scoped(&enrichment).await?;
            json!({"servers":servers,"discovery":{"tools":"codemode.search({query, kinds: ['tool']})",
                "skills":"codemode.listSkills()","artifacts":"codemode.listArtifacts()",
                "resources":"codemode.listResources(upstream)","kinds":["tool","skill","command","prompt","subagent","snippet"]}})
        }
        _ => {
            let name = uri
                .strip_prefix("lab://gateway/")
                .and_then(|s| s.strip_suffix("/schema"))
                .ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "not_found".into(),
                    message: "unknown Labby resource".into(),
                })?;
            manager
                .gateway_server_schema_scoped(name, &enrichment)
                .await?
        }
    };
    Ok(json!({"contents":[{"uri":uri,"mimeType":"application/json","text":data.to_string()}]}))
}

pub(super) async fn list_resources(
    manager: &GatewayManager,
    upstream: &str,
    caller: &CodeModeCaller,
    surface: CodeModeSurface,
    scope: &ToolScope,
) -> Result<Value, ToolError> {
    if upstream == "labby" {
        if !caller.is_admin()
            || !caller.can_use_snippets()
            || scope.allowed_namespaces().is_some()
            || scope.allowed_tools().is_some()
        {
            return Err(ToolError::Forbidden {
                message: "Labby operator resources require unscoped admin access".into(),
                required_scopes: vec!["lab:admin".into()],
            });
        }
        return Ok(json!({"resources":[
            {"uri":"lab://gateway/servers","name":"Gateway servers","mimeType":"application/json"},
            {"uri":"lab://gateway/status","name":"Gateway status","mimeType":"application/json"},
            {"uri":"lab://gateway/limits","name":"Code Mode limits","mimeType":"application/json"},
            {"uri":"lab://capabilities","name":"Capability overview","mimeType":"application/json"}
        ]}));
    }
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
