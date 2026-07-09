//! Palette-facing launcher catalog and execution helpers.
//!
//! This module owns the gateway portion of the desktop launcher contract. It
//! projects already-discovered upstream MCP tools without cold-connecting, then
//! re-resolves the live tool at execution time before validating parameters and
//! dispatching through the same upstream call helper used by Code Mode.

use std::collections::BTreeSet;
use std::time::Instant;

use labby_codemode::{
    CodeModeCaller, CodeModeCallerCapabilities, CodeModeSurface, ToolCallOutcome, ToolScope,
    destructive_permitted,
};
use labby_runtime::error::ToolError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::gateway::SHARED_GATEWAY_OAUTH_SUBJECT;
use crate::gateway::code_mode::validate_code_mode_params_against_schema;
use crate::gateway::manager::GatewayManager;
use crate::gateway::projection::sanitize_tool_text;
use crate::upstream::types::{UpstreamRuntimeOwner, UpstreamTool};

const MAX_SCHEMA_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LauncherCatalogView {
    pub fingerprint: String,
    pub entries: Vec<LauncherEntryView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum LauncherEntryView {
    LabbyAction(LabbyActionLauncherEntry),
    McpTool(McpToolLauncherEntry),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LabbyActionLauncherEntry {
    pub id: String,
    pub label: String,
    pub description: String,
    pub source: String,
    pub destructive: bool,
    pub input_schema: Option<Value>,
    pub schema_fingerprint: Option<String>,
    pub service: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolLauncherEntry {
    pub id: String,
    pub label: String,
    pub description: String,
    pub source: String,
    pub destructive: bool,
    pub input_schema: Option<Value>,
    pub schema_fingerprint: Option<String>,
    pub upstream: String,
    pub tool: String,
}

#[derive(Debug, Clone)]
pub struct PaletteCaller {
    pub caller: CodeModeCaller,
    pub scope: ToolScope,
    pub owner: UpstreamRuntimeOwner,
    pub oauth_subject: String,
}

impl PaletteCaller {
    #[must_use]
    pub fn admin(subject: Option<&str>, request_id: Option<&str>) -> Self {
        let owner = crate::gateway::shared::make_api_runtime_owner(subject, request_id);
        let subject = subject.map(ToOwned::to_owned);
        Self {
            caller: CodeModeCaller::Scoped {
                capabilities: CodeModeCallerCapabilities {
                    can_execute: true,
                    can_use_snippets: false,
                    is_admin: true,
                },
                sub: subject,
            },
            scope: ToolScope::default(),
            owner,
            oauth_subject: SHARED_GATEWAY_OAUTH_SUBJECT.to_string(),
        }
    }

    #[must_use]
    pub fn scoped_read_only(
        subject: Option<&str>,
        request_id: Option<&str>,
        allowed_upstreams: Vec<String>,
    ) -> Self {
        let owner = crate::gateway::shared::make_api_runtime_owner(subject, request_id);
        let subject = subject.map(ToOwned::to_owned);
        Self {
            caller: CodeModeCaller::Scoped {
                capabilities: CodeModeCallerCapabilities {
                    can_execute: false,
                    can_use_snippets: false,
                    is_admin: false,
                },
                sub: subject.clone(),
            },
            scope: ToolScope::scoped_namespaces(allowed_upstreams, Vec::new()),
            owner,
            oauth_subject: subject.unwrap_or_else(|| SHARED_GATEWAY_OAUTH_SUBJECT.to_string()),
        }
    }

    fn allowed_upstreams(&self) -> Option<&BTreeSet<String>> {
        self.scope.allowed_namespaces()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaletteExecuteRequest {
    pub id: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub confirm_destructive: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PaletteExecuteResponse {
    pub id: String,
    pub result: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui: Option<Value>,
}

impl GatewayManager {
    pub async fn palette_catalog(
        &self,
        caller: &PaletteCaller,
    ) -> Result<LauncherCatalogView, ToolError> {
        let start = Instant::now();
        let mut entries = Vec::new();
        let mut schema_bytes = 0usize;

        let cfg = self.config.read().await.clone();
        if let Some(pool) = self.current_pool().await {
            for upstream in cfg.upstream.iter().filter(|upstream| {
                upstream.enabled
                    && upstream.priority > 0.0
                    && caller
                        .allowed_upstreams()
                        .is_none_or(|allowed| allowed.contains(&upstream.name))
            }) {
                let mut tools = pool.healthy_tools_for_upstream(&upstream.name).await;
                tools.sort_by(|a, b| a.tool.name.cmp(&b.tool.name));
                for tool in tools {
                    let entry = mcp_entry(&upstream.name, tool);
                    schema_bytes += entry
                        .input_schema
                        .as_ref()
                        .map(|schema| schema.to_string().len())
                        .unwrap_or(0);
                    entries.push(LauncherEntryView::McpTool(entry));
                }
            }
        }

        entries.sort_by(|a, b| entry_id(a).cmp(entry_id(b)));
        let fingerprint = catalog_fingerprint(&entries);
        tracing::info!(
            surface = "api",
            service = "palette",
            action = "palette.catalog",
            entry_count = entries.len(),
            schema_bytes,
            fingerprint,
            cache_hit = false,
            elapsed_ms = start.elapsed().as_millis(),
            "palette launcher catalog built"
        );
        Ok(LauncherCatalogView {
            fingerprint,
            entries,
        })
    }

    pub async fn palette_execute(
        &self,
        caller: &PaletteCaller,
        request: PaletteExecuteRequest,
    ) -> Result<PaletteExecuteResponse, ToolError> {
        let (upstream, tool) = parse_mcp_launcher_id(&request.id)?;
        if caller
            .allowed_upstreams()
            .is_some_and(|allowed| !allowed.contains(upstream))
        {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: format!("upstream `{upstream}` is outside the caller scope"),
            });
        }
        if !caller.caller.can_execute() {
            return Err(ToolError::Sdk {
                sdk_kind: "forbidden".to_string(),
                message: "palette execution requires execute permission".to_string(),
            });
        }

        let upstream_tool = self
            .resolve_code_mode_upstream_tool(
                upstream,
                tool,
                Some(&caller.owner),
                Some(&caller.oauth_subject),
            )
            .await
            .map_err(map_unknown_tool_to_not_found)?;

        if upstream_tool.destructive {
            if !destructive_permitted(CodeModeSurface::Mcp, &caller.caller) {
                return Err(ToolError::Sdk {
                    sdk_kind: "forbidden".to_string(),
                    message: format!("tool `{upstream}::{tool}` requires execute permission"),
                });
            }
            if !request.confirm_destructive {
                return Err(ToolError::Sdk {
                    sdk_kind: "confirmation_required".to_string(),
                    message: format!("tool `{upstream}::{tool}` is destructive"),
                });
            }
        }

        validate_code_mode_params_against_schema(
            &request.params,
            upstream_tool.input_schema.as_ref(),
        )?;
        let outcome = self
            .execute_upstream_tool(upstream, tool, request.params)
            .await?;
        Ok(execution_response(request.id, outcome))
    }
}

fn mcp_entry(upstream: &str, tool: UpstreamTool) -> McpToolLauncherEntry {
    let name = tool.tool.name.to_string();
    let input_schema = project_palette_schema(tool.input_schema);
    let schema_fingerprint = input_schema.as_ref().map(stable_json_fingerprint);
    McpToolLauncherEntry {
        id: format!("mcp:{upstream}::{name}"),
        label: name.clone(),
        description: sanitize_tool_text(
            tool.tool
                .description
                .as_ref()
                .map(|value| value.as_ref())
                .unwrap_or(""),
            512,
        ),
        source: upstream.to_string(),
        destructive: tool.destructive,
        input_schema,
        schema_fingerprint,
        upstream: upstream.to_string(),
        tool: name,
    }
}

fn execution_response(id: String, outcome: ToolCallOutcome) -> PaletteExecuteResponse {
    PaletteExecuteResponse {
        id,
        result: outcome.value,
        ui: outcome.ui.map(|ui| ui.ui_meta),
    }
}

fn parse_mcp_launcher_id(id: &str) -> Result<(&str, &str), ToolError> {
    let rest = id.strip_prefix("mcp:").ok_or_else(|| ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: format!("launcher entry `{id}` was not found"),
    })?;
    let Some((upstream, tool)) = rest.split_once("::") else {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("launcher entry `{id}` was not found"),
        });
    };
    if upstream.is_empty() || tool.is_empty() || tool.contains("::") {
        return Err(ToolError::Sdk {
            sdk_kind: "not_found".to_string(),
            message: format!("launcher entry `{id}` was not found"),
        });
    }
    Ok((upstream, tool))
}

fn map_unknown_tool_to_not_found(error: ToolError) -> ToolError {
    match error {
        ToolError::Sdk { sdk_kind, message }
            if sdk_kind == "unknown_tool"
                || sdk_kind == "unknown_upstream"
                || sdk_kind == "invalid_code_mode_id" =>
        {
            ToolError::Sdk {
                sdk_kind: "not_found".to_string(),
                message,
            }
        }
        other => other,
    }
}

fn project_palette_schema(schema: Option<Value>) -> Option<Value> {
    let mut schema = schema?;
    redact_schema_value(&mut schema);
    if schema.to_string().len() > MAX_SCHEMA_BYTES {
        return None;
    }
    Some(schema)
}

fn redact_schema_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("default");
            map.remove("examples");
            map.remove("example");
            for (key, child) in map.iter_mut() {
                if secret_key(key) {
                    *child = Value::String("[REDACTED]".to_string());
                } else {
                    redact_schema_value(child);
                }
            }
        }
        Value::Array(values) => {
            values.retain(|value| !secret_enum_value(value));
            for child in values {
                redact_schema_value(child);
            }
        }
        Value::String(text) => {
            *text = sanitize_tool_text(text, 512);
        }
        _ => {}
    }
}

fn secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("token")
        || key.contains("secret")
        || key.contains("password")
        || key.contains("apikey")
        || key.contains("api_key")
        || key.contains("authorization")
}

fn secret_enum_value(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| secret_key(text) || text.starts_with("sk-") || text.starts_with("ghp_"))
}

fn entry_id(entry: &LauncherEntryView) -> &str {
    match entry {
        LauncherEntryView::LabbyAction(entry) => &entry.id,
        LauncherEntryView::McpTool(entry) => &entry.id,
    }
}

fn catalog_fingerprint(entries: &[LauncherEntryView]) -> String {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update(entry_id(entry).as_bytes());
        hasher.update([0]);
        match entry {
            LauncherEntryView::LabbyAction(entry) => {
                if let Some(fp) = &entry.schema_fingerprint {
                    hasher.update(fp.as_bytes());
                }
            }
            LauncherEntryView::McpTool(entry) => {
                if let Some(fp) = &entry.schema_fingerprint {
                    hasher.update(fp.as_bytes());
                }
            }
        }
        hasher.update([0xff]);
    }
    hex_digest(hasher.finalize().as_slice())
}

fn stable_json_fingerprint(value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.to_string().as_bytes());
    hex_digest(hasher.finalize().as_slice())
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn palette_schema_projection_redacts_defaults_examples_and_secret_enums() {
        let projected = project_palette_schema(Some(json!({
            "type": "object",
            "default": { "token": "sk-secret" },
            "examples": [{ "token": "sk-secret" }],
            "properties": {
                "apiKey": {
                    "type": "string",
                    "enum": ["public", "sk-secretsecretsecretsecret"]
                },
                "name": { "type": "string" }
            }
        })))
        .expect("schema remains");

        assert!(projected.get("default").is_none());
        assert!(projected.get("examples").is_none());
        assert_eq!(
            projected.pointer("/properties/apiKey"),
            Some(&Value::String("[REDACTED]".to_string()))
        );
    }
}
