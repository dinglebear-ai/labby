//! Binary-owned bounded first-tool verification. Metadata never selects a call.
use super::readiness::{self, Check};
use crate::dispatch::{error::ToolError, helpers::require_str};
use labby_gateway::gateway::manager::GatewayManager;
use rmcp::model::{CallToolRequestParams, CallToolResponse};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::time::Duration;

fn invalid(message: &str) -> ToolError {
    ToolError::InvalidParam {
        param: "verification".into(),
        message: message.into(),
    }
}
fn unavailable() -> ToolError {
    ToolError::Sdk { sdk_kind: "mcp_verification_unavailable".into(), message: "This server cannot complete the supported first-tool check. Review its runtime, exposure and connection in Gateway.".into() }
}

fn validate_keys(params: &Value, call: bool) -> Result<(), ToolError> {
    let object = params
        .as_object()
        .ok_or_else(|| invalid("Expected structured verification inputs"))?;
    if object.keys().any(|key| {
        !matches!(key.as_str(), "name" | "expected_url")
            && !(call
                && matches!(
                    key.as_str(),
                    "tool" | "approved" | "arguments" | "expected_fingerprint"
                ))
    }) {
        return Err(invalid(
            "Verification accepts only the reviewed server, endpoint, selected tool and approval",
        ));
    }
    Ok(())
}

async fn reviewed_server(manager: &GatewayManager, params: &Value) -> Result<String, ToolError> {
    let name = require_str(params, "name")?;
    let expected = require_str(params, "expected_url")?;
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || name.is_empty()
        || name.len() > 64
        || expected.len() > 2048
    {
        return Err(invalid("Server identity exceeds its supported bounds"));
    }
    let view = manager.get(name).await?;
    let url = url::Url::parse(expected).map_err(|_| invalid("Review a valid HTTPS endpoint"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || view.config.url.as_deref() != Some(expected)
        || view.config.oauth_enabled
        || view.config.command.is_some()
        || !view.config.enabled
    {
        return Err(invalid(
            "The server connection changed or requires an unsupported authorization flow. Review it again in Gateway.",
        ));
    }
    Ok(name.into())
}

// Present only bounded scalar forms; reject assertions the form cannot explain.
fn supported_input(schema: &serde_json::Map<String, Value>) -> bool {
    if schema.get("type").and_then(Value::as_str) != Some("object")
        || schema.keys().any(|key| {
            !matches!(
                key.as_str(),
                "type"
                    | "properties"
                    | "required"
                    | "additionalProperties"
                    | "description"
                    | "title"
                    | "$schema"
            )
        })
        || schema
            .get("additionalProperties")
            .is_some_and(|v| !v.is_boolean())
    {
        return false;
    }
    let empty = serde_json::Map::new();
    let Some(properties) = schema
        .get("properties")
        .map(Value::as_object)
        .unwrap_or(Some(&empty))
    else {
        return false;
    };
    if properties.len() > 16 {
        return false;
    }
    if let Some(required) = schema.get("required") {
        let Some(required) = required.as_array() else {
            return false;
        };
        if required
            .iter()
            .any(|v| v.as_str().is_none_or(|name| !properties.contains_key(name)))
        {
            return false;
        }
    }
    properties.iter().all(|(name, raw)| {
        let Some(p) = raw.as_object() else {
            return false;
        };
        name.len() <= 128
            && !name.is_empty()
            && matches!(
                p.get("type").and_then(Value::as_str),
                Some("string" | "boolean" | "integer" | "number")
            )
            && p.keys().all(|k| {
                matches!(
                    k.as_str(),
                    "type"
                        | "description"
                        | "title"
                        | "enum"
                        | "default"
                        | "minimum"
                        | "maximum"
                        | "minLength"
                        | "maxLength"
                )
            })
            && p.get("enum").is_none_or(|v| {
                v.as_array().is_some_and(|v| {
                    !v.is_empty()
                        && v.len() <= 100
                        && v.iter()
                            .all(|v| v.is_string() || v.is_number() || v.is_boolean())
                })
            })
    }) && serde_json::to_vec(schema).is_ok_and(|v| v.len() <= 16 * 1024)
}
fn supported(tool: &labby_gateway::upstream::types::UpstreamTool) -> bool {
    !tool.destructive
        && tool.tool.annotations.as_ref().is_some_and(|annotations| {
            annotations.read_only_hint == Some(true) && annotations.destructive_hint != Some(true)
        })
        && supported_input(tool.tool.input_schema.as_ref())
}

fn review_fingerprint(
    tool: &labby_gateway::upstream::types::UpstreamTool,
    publication: &str,
) -> Result<String, ToolError> {
    let bytes = serde_json::to_vec(&json!({"publication":publication,"upstream":tool.upstream_name,"tool":tool.tool,"destructive":tool.destructive})).map_err(|_| unavailable())?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub(crate) async fn tools(manager: &GatewayManager, params: &Value) -> Result<Value, ToolError> {
    validate_keys(params, false)?;
    let name = reviewed_server(manager, params).await?;
    let generation = manager.current_pool_publication_generation();
    let pool = manager.current_pool().await.ok_or_else(unavailable)?;
    let catalog = pool
        .published_tool_catalog()
        .await
        .map_err(|_| unavailable())?;
    let publication = format!("{generation:?}:{:?}", catalog.generation());
    let tools = catalog.routes().iter().filter(|route| route.upstream_name.as_ref() == name && supported(&route.tool)).take(20).map(|route| {
        let tool = &route.tool;
        Ok(json!({"name":tool.tool.name,"inputSchema":tool.tool.input_schema,"description":tool.tool.description.as_deref().unwrap_or("").chars().take(2048).collect::<String>(),"reviewFingerprint":review_fingerprint(tool, &publication)?}))
    }).collect::<Result<Vec<_>, ToolError>>()?;
    Ok(
        json!({"tools":tools,"supported_scope":"exposed_read_only_scalar_inputs","max_call_seconds":10}),
    )
}

pub(crate) async fn call(
    manager: &GatewayManager,
    store: crate::access::AccessStore,
    principal_id: String,
    params: Value,
    authority: crate::access::GatewayActionAuthorization,
) -> Result<Value, ToolError> {
    validate_keys(&params, true)?;
    if params.get("approved").and_then(Value::as_bool) != Some(true) {
        return Err(invalid(
            "Review and explicitly approve the selected tool before calling it",
        ));
    }
    let tool_name = require_str(&params, "tool")?.to_owned();
    if tool_name.is_empty() || tool_name.len() > 256 {
        return Err(invalid("Select a supported tool"));
    }
    let capture_store = store.clone();
    let subject = principal_id.clone();
    let fingerprint = tokio::task::spawn_blocking(move || {
        readiness::begin_verification_for_store(&capture_store, &subject, Check::McpToolCall)
    })
    .await
    .map_err(|_| unavailable())??;
    let name = reviewed_server(manager, &params).await?;
    let generation = manager.current_pool_publication_generation();
    let pool = manager.current_pool().await.ok_or_else(unavailable)?;
    let catalog = pool
        .published_tool_catalog()
        .await
        .map_err(|_| unavailable())?;
    let route = catalog
        .routes()
        .iter()
        .find(|route| {
            route.upstream_name.as_ref() == name
                && route.tool_name.as_ref() == tool_name
                && supported(&route.tool)
        })
        .ok_or_else(|| {
            invalid("The selected tool is no longer exposed or supported. Reload the tool list.")
        })?;
    let expected = require_str(&params, "expected_fingerprint")?;
    let publication = format!("{generation:?}:{:?}", catalog.generation());
    if expected != review_fingerprint(&route.tool, &publication)? {
        return Err(invalid(
            "The reviewed tool or runtime publication changed. Reload the tool list and review it again.",
        ));
    }
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let object = arguments
        .as_object()
        .ok_or_else(|| invalid("Tool inputs must be an object"))?;
    let schema = Value::Object(route.tool.tool.input_schema.as_ref().clone());
    if serde_json::to_vec(&arguments)
        .map_err(|_| unavailable())?
        .len()
        > 8192
    {
        return Err(invalid("Tool inputs exceed the 8 KiB verification limit"));
    }
    // Never pass fields that were not presented, even when upstream permits extras.
    if object.keys().any(|key| {
        schema
            .get("properties")
            .and_then(Value::as_object)
            .is_none_or(|p| !p.contains_key(key))
    }) {
        return Err(invalid("Tool inputs include an unsupported field"));
    }
    labby_gateway::gateway::code_mode::validate_code_mode_params_against_schema(
        &arguments,
        Some(&schema),
    )?;
    authority.validate_before_external_effect().await?;
    let response = tokio::time::timeout(Duration::from_secs(10), manager.execute_published_tool_exact(generation, catalog.generation(), &name, &tool_name, CallToolRequestParams::new(tool_name.clone()).with_arguments(object.clone())))
        .await.map_err(|_| ToolError::Sdk { sdk_kind: "mcp_verification_timeout".into(), message: "The tool check exceeded 10 seconds. No readiness was recorded; retry only after reviewing the server state.".into() })?
        .map_err(|_| unavailable())?;
    let CallToolResponse::Complete(result) = response else {
        return Err(invalid(
            "The tool requires additional interaction; use Gateway to complete it",
        ));
    };
    if result.is_error == Some(true) {
        return Err(ToolError::Sdk {
            sdk_kind: "mcp_tool_error".into(),
            message: "The MCP server returned a tool error. No readiness was recorded.".into(),
        });
    }
    let bytes = serde_json::to_vec(&result)
        .map_err(|_| unavailable())?
        .len();
    if bytes > 16 * 1024 {
        return Err(invalid(
            "The tool response exceeds the 16 KiB first-use verification limit",
        ));
    }
    authority.validate_before_external_effect().await?;
    if manager.current_pool_publication_generation() != generation {
        return Err(invalid(
            "The gateway runtime changed during the tool check. Review it and retry.",
        ));
    }
    let current_catalog = pool
        .published_tool_catalog()
        .await
        .map_err(|_| unavailable())?;
    if current_catalog.generation() != catalog.generation() {
        return Err(invalid(
            "The tool catalog changed during verification. Reload the tool list and review it again.",
        ));
    }
    reviewed_server(manager, &params).await?;
    let resource = format!("{name}::{tool_name}");
    tokio::task::spawn_blocking(move || {
        readiness::record_verified_if_unchanged_for_store(
            &store,
            &principal_id,
            Check::McpToolCall,
            &resource,
            fingerprint,
        )
    })
    .await
    .map_err(|_| unavailable())??;
    Ok(json!({"verified":true,"server":name,"tool":tool_name,"response_bytes":bytes}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[allow(clippy::disallowed_methods)]
    fn first_use_requires_explicit_read_only_and_non_destructive_capability() {
        let schema = std::sync::Arc::new(
            json!({"type":"object","properties":{}})
                .as_object()
                .unwrap()
                .clone(),
        );
        let mut candidate = labby_gateway::upstream::types::UpstreamTool {
            tool: rmcp::model::Tool::new("version", "Read server version", schema),
            input_schema: None,
            output_schema: None,
            upstream_name: std::sync::Arc::from("server"),
            destructive: false,
        };
        assert!(!supported(&candidate));
        let mut annotations = rmcp::model::ToolAnnotations::default();
        annotations.read_only_hint = Some(true);
        annotations.destructive_hint = Some(false);
        candidate.tool.annotations = Some(annotations);
        assert!(supported(&candidate));
        candidate.destructive = true;
        assert!(!supported(&candidate));
        candidate.destructive = false;
        candidate
            .tool
            .annotations
            .as_mut()
            .unwrap()
            .destructive_hint = Some(true);
        assert!(!supported(&candidate));
    }

    #[test]
    fn scalar_input_schema_is_strict_and_fail_closed() {
        for schema in [
            json!({"type":"object"}),
            json!({"type":"object","properties":{"timezone":{"type":"string","enum":["UTC","America/New_York"]}},"required":["timezone"]}),
            json!({"type":"object","properties":{},"required":[],"additionalProperties":false}),
        ] {
            assert!(supported_input(schema.as_object().unwrap()));
        }
        for schema in [
            json!({"type":"object","required":["path"]}),
            json!({"type":"object","minProperties":1}),
            json!({"type":"object","allOf":[]}),
            json!({"type":"array"}),
        ] {
            assert!(!supported_input(schema.as_object().unwrap()));
        }
    }
    #[test]
    #[allow(clippy::disallowed_methods)]
    fn reviewed_fingerprint_changes_with_schema_description_and_publication() {
        let schema = std::sync::Arc::new(json!({"type":"object"}).as_object().unwrap().clone());
        let mut tool = labby_gateway::upstream::types::UpstreamTool {
            tool: rmcp::model::Tool::new("time", "Read time", schema),
            input_schema: None,
            output_schema: None,
            upstream_name: std::sync::Arc::from("server"),
            destructive: false,
        };
        let reviewed = review_fingerprint(&tool, "pool1:catalog1").unwrap();
        assert_ne!(
            reviewed,
            review_fingerprint(&tool, "pool2:catalog1").unwrap()
        );
        tool.tool.description = Some("Different meaning".into());
        assert_ne!(
            reviewed,
            review_fingerprint(&tool, "pool1:catalog1").unwrap()
        );
        tool.tool.description = Some("Read time".into());
        tool.tool.input_schema = std::sync::Arc::new(
            json!({"type":"object","properties":{"zone":{"type":"string"}}})
                .as_object()
                .unwrap()
                .clone(),
        );
        assert_ne!(
            reviewed,
            review_fingerprint(&tool, "pool1:catalog1").unwrap()
        );
    }
}
