//! Metadata-only schema reads through the selected authenticated MCP connection.
use super::*;
use labby_codemode::snippet::schemas::FixtureSchemas;
use rmcp::model::CallToolRequestParams;
use serde_json::json;
use std::collections::BTreeMap;

const SCHEMA_DEADLINE: Duration = Duration::from_secs(20);
const MAX_SCHEMA_BYTES: usize = 128 * 1024;

impl LiveGateway {
    /// Fetch exact caller-visible contracts without invoking an upstream tool.
    /// The snippet body and fixture stay local; discovery never falls back locally.
    pub async fn fixture_tool_schemas(
        &self,
        tools: &[String],
    ) -> Result<BTreeMap<String, FixtureSchemas>, ToolError> {
        let _validated: labby_codemode::snippet::tool_declarations::SnippetToolDeclarations =
            tools.to_vec().try_into()?;
        if tools.is_empty() {
            return Ok(BTreeMap::new());
        }
        let service = self.connect_service_bounded(()).await?;
        let peer = service.peer().clone();
        let requests =
            tools
                .iter()
                .try_fold(BTreeMap::<String, Vec<String>>::new(), |mut groups, id| {
                    let (upstream, name) = id
                        .split_once("::")
                        .ok_or_else(|| schema_error("invalid tool identifier"))?;
                    groups.entry(upstream.into()).or_default().push(name.into());
                    Ok::<_, ToolError>(groups)
                })?;
        let (result, cleanup) = bounded_codemode_call_and_cleanup(
            async {
                Ok::<_, std::convert::Infallible>(
                    async {
                        let mut contracts = BTreeMap::new();
                        for (upstream, names) in requests {
                            let code = schema_read_code(&upstream, &names)?;
                            let response = peer
                                .call_tool(
                                    CallToolRequestParams::new("codemode_read").with_arguments(
                                        json!({"code":code})
                                            .as_object()
                                            .cloned()
                                            .unwrap_or_default(),
                                    ),
                                )
                                .await
                                .map_err(|error| {
                                    schema_transport_error("schema read", &upstream, &error)
                                })?;
                            let payload = codemode_result_value(response)?;
                            let selected = decode_schema_trace(payload, &upstream, &names)?;
                            contracts.extend(selected);
                            if serde_json::to_vec(&contracts)
                                .map_err(|_| schema_error("invalid tool schemas"))?
                                .len()
                                > MAX_SCHEMA_BYTES
                            {
                                return Err(schema_error("saved tool schemas exceed 128 KiB"));
                            }
                        }
                        supplement_native_outputs(&peer, &mut contracts).await?;
                        if serde_json::to_vec(&contracts)
                            .map_err(|_| schema_error("invalid tool schemas"))?
                            .len()
                            > MAX_SCHEMA_BYTES
                        {
                            return Err(schema_error("saved tool schemas exceed 128 KiB"));
                        }
                        Ok::<_, ToolError>(contracts)
                    }
                    .await,
                )
            },
            SCHEMA_DEADLINE,
            MCP_CLEANUP_TIMEOUT,
            service.cancel(),
        )
        .await;
        if cleanup.is_err() {
            tracing::warn!(
                surface = "cli",
                service = "gateway",
                action = "snippets.fixture",
                "schema discovery MCP cleanup failed"
            );
        }
        result?
    }
}

/// Older schema resources omitted outputs. Recover native wire contracts by exact
/// qualified ID, or by a read-only bare name verified to resolve to that exact ID.
/// Both paths require matching input contracts to avoid borrowing another output.
async fn supplement_native_outputs(
    peer: &rmcp::service::Peer<RoleClient>,
    contracts: &mut BTreeMap<String, FixtureSchemas>,
) -> Result<(), ToolError> {
    if contracts
        .values()
        .all(|contract| contract.output_schema.is_some())
    {
        return Ok(());
    }
    let mut cursor: Option<String> = None;
    let mut seen = BTreeSet::new();
    let mut native = BTreeMap::new();
    let mut bytes = 0;
    let mut count = 0;
    for page_index in 0..32 {
        let page = peer
            .list_tools(cursor.clone().map(|cursor| {
                rmcp::model::PaginatedRequestParams::default().with_cursor(Some(cursor))
            }))
            .await
            .map_err(|error| {
                schema_transport_error("native tool listing", "selected gateway", &error)
            })?;
        bytes += serde_json::to_vec(&page)
            .map_err(|_| schema_error("invalid native tool catalog"))?
            .len();
        count += page.tools.len();
        if bytes > 8 * 1024 * 1024 || count > 4096 {
            return Err(schema_error("native MCP schema catalog exceeds bounds"));
        }
        for tool in page.tools {
            if native.insert(tool.name.to_string(), tool).is_some() {
                return Err(schema_error("duplicate native tool identity"));
            }
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
        if !seen.insert(cursor.clone()) || page_index == 31 {
            return Err(schema_error("native MCP schema pagination incomplete"));
        }
    }
    let reserved_services = crate::registry::build_docs_registry();
    for (id, contract) in contracts
        .iter_mut()
        .filter(|(_, contract)| contract.output_schema.is_none())
    {
        let (_, name) = id
            .split_once("::")
            .ok_or_else(|| schema_error("invalid tool identifier"))?;
        let Some(tool) = native.get(id).or_else(|| native.get(name)) else {
            continue;
        };
        let Some(output) = tool.output_schema.as_ref() else {
            continue;
        };
        if contract.input_schema.as_ref() != Some(&Value::Object((*tool.input_schema).clone())) {
            continue;
        }
        if tool.name.as_ref() != id.as_str() {
            // Labby service identities are reserved on the native wire and may
            // shadow upstream names. Do not treat them as upstream aliases.
            if reserved_services.service(name).is_some()
                || crate::mcp::permanent_tools::is_reserved_non_upstream_tool_name(name)
                || tool.annotations.as_ref().and_then(|a| a.read_only_hint) != Some(true)
            {
                continue;
            }
            let literal =
                serde_json::to_string(name).map_err(|_| schema_error("invalid tool name"))?;
            let code = format!(
                "async () => {{ const d = await codemode.describe({literal}); return {{id:d.id,schema_status:d.schema_status}}; }}"
            );
            let response = peer
                .call_tool(
                    CallToolRequestParams::new("codemode_read").with_arguments(
                        json!({"code":code})
                            .as_object()
                            .cloned()
                            .unwrap_or_default(),
                    ),
                )
                .await
                .map_err(|error| {
                    schema_transport_error("native identity verification", id, &error)
                })?;
            let trace = codemode_result_value(response)?;
            if trace.pointer("/result/id").and_then(Value::as_str) != Some(id.as_str())
                || trace
                    .pointer("/result/schema_status")
                    .and_then(Value::as_str)
                    != Some("complete")
            {
                continue;
            }
        }
        contract.output_schema = Some(Value::Object((**output).clone()));
    }
    Ok(())
}

fn schema_error(message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "schema_unavailable".into(),
        message: message.into(),
    }
}

fn schema_transport_error(operation: &str, target: &str, error: &rmcp::ServiceError) -> ToolError {
    // MCP error data may echo request payloads; retain only the code and message.
    let cause = match error {
        rmcp::ServiceError::McpError(error) => format!("MCP {}: {}", error.code.0, error.message),
        rmcp::ServiceError::Cancelled { .. } => "request cancelled".into(),
        other => other.to_string(),
    };
    let message = labby_runtime::redact::redact_secret_like_segments(&format!(
        "{operation} for {target} failed: {cause}"
    ));
    schema_error(&message.chars().take(1024).collect::<String>())
}

fn schema_read_code(upstream: &str, names: &[String]) -> Result<String, ToolError> {
    // Serialize all caller values as literals; no identifier/source interpolation.
    let uri = serde_json::to_string(&format!("lab://gateway/{upstream}/schema"))
        .map_err(|_| schema_error("invalid schema resource"))?;
    let names = serde_json::to_string(names).map_err(|_| schema_error("invalid tool names"))?;
    Ok(format!(
        "async () => {{ const r = await codemode.readResource({uri}); const c = r.contents.find(x => typeof x.text === 'string'); if (!c) throw new Error('schema resource missing text'); const d = JSON.parse(c.text); const names = {names}; return {{name:d.name, health:d.health, tools:d.tools.filter(t => names.includes(t.name)).map(t => ({{name:t.name,input_schema:t.input_schema,output_schema:t.output_schema}}))}}; }}"
    ))
}

fn decode_schema_trace(
    trace: Value,
    upstream: &str,
    names: &[String],
) -> Result<BTreeMap<String, FixtureSchemas>, ToolError> {
    if trace.get("error").is_some()
        || trace.get("error_kind").is_some()
        || trace.pointer("/result_shape/truncated") == Some(&json!(true))
        || trace.pointer("/result_shaping/truncated") == Some(&json!(true))
    {
        return Err(schema_error(
            "live schema discovery failed or was truncated; no local fallback",
        ));
    }
    let doc = trace
        .get("result")
        .ok_or_else(|| schema_error("schema discovery returned no result"))?;
    if doc["name"] != upstream || doc["health"] != "healthy" {
        return Err(schema_error(
            "schema discovery returned an unavailable or mismatched upstream",
        ));
    }
    let rows = doc["tools"]
        .as_array()
        .ok_or_else(|| schema_error("schema discovery returned invalid tools"))?;
    let mut result = BTreeMap::new();
    for row in rows {
        let name = row["name"]
            .as_str()
            .filter(|name| names.iter().any(|n| n == name))
            .ok_or_else(|| schema_error("schema discovery returned an unrequested tool"))?;
        let contract: FixtureSchemas = serde_json::from_value(json!({
            "input_schema":row.get("input_schema").cloned().unwrap_or(Value::Null),
            "output_schema":row.get("output_schema").cloned().unwrap_or(Value::Null)
        }))
        .map_err(|_| schema_error("schema discovery returned invalid contracts"))?;
        if result
            .insert(format!("{upstream}::{name}"), contract)
            .is_some()
        {
            return Err(schema_error("schema discovery returned duplicate tools"));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
