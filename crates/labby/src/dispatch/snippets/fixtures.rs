//! Metadata-only fixture generation; never invokes snippet code or upstream tools.
use super::dispatch::SnippetDispatchContext;
use super::store::{builtin_snippet_dir, merge_snippet_input, resolve_snippet};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::helpers::lab_home;
use labby_codemode::snippet::harness::{FixtureCall, MAX_FIXTURE_BYTES, SnippetFixture};
use labby_codemode::snippet::schemas::{FixtureSchemas, FixtureVariant, generate_response};
use labby_codemode::{CodeModeCatalogKind, CodeModeHost};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureParams {
    name: String,
    tools: Option<Vec<String>>,
    check: Option<SnippetFixture>,
    #[serde(default)]
    params: Value,
    schemas: Option<BTreeMap<String, FixtureSchemas>>,
    #[serde(default)]
    results: BTreeMap<String, Value>,
    #[serde(default)]
    variant: FixtureVariant,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct FixtureDraft {
    name: String,
    ready: bool,
    coverage: String,
    fixture: SnippetFixture,
    warnings: Vec<String>,
    failures: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    changed_tools: Vec<String>,
}

fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: "fixture".into(),
    }
}

/// Publish a complete fixture without replacing an existing destination.
pub(crate) fn write_fixture_output(path: &std::path::Path, fixture: &Value) -> anyhow::Result<()> {
    publish_fixture_output(path, |file| {
        use std::io::Write;
        serde_json::to_writer_pretty(&mut *file, fixture)?;
        file.write_all(b"\n")?;
        Ok(())
    })
}

fn publish_fixture_output(
    path: &std::path::Path,
    write: impl FnOnce(&mut std::fs::File) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("cannot stage fixture output {}", path.display()))?;
    write(staged.as_file_mut())
        .with_context(|| format!("cannot write fixture output {}", path.display()))?;
    staged
        .as_file()
        .sync_all()
        .with_context(|| format!("cannot sync fixture output {}", path.display()))?;
    staged
        .persist_noclobber(path)
        .with_context(|| format!("cannot publish fixture output {}", path.display()))?;
    Ok(())
}

pub(super) async fn generate(
    manager: Option<&GatewayManager>,
    params: Value,
    context: Option<SnippetDispatchContext>,
) -> Result<Value, ToolError> {
    let (params, context, snippet, input, scope, tools) = prepare(params, context)?;
    let contracts = if let Some(schemas) = params.schemas {
        schemas
    } else if tools.is_empty() {
        BTreeMap::new()
    } else {
        let manager = manager.ok_or_else(|| {
            invalid("schema discovery requires a gateway; alternatively supply saved schemas")
        })?;
        let render = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            CodeModeHost::list_tools(
                manager,
                &context.execution_caller,
                context.execution_surface,
                &scope,
                false,
                true,
            ),
        )
        .await
        .map_err(|_| invalid("fixture schema catalog deadline exceeded"))??;
        let mut contracts = BTreeMap::new();
        let mut bytes = 0;
        for id in &tools {
            if let Some(entry) = render.entries.iter().find(|entry| {
                entry.kind == CodeModeCatalogKind::Tool
                    && entry.id == *id
                    && scope.allows(&entry.namespace, &entry.name)
                    && (!scope.is_read_only()
                        || entry.safety.and_then(|s| s.read_only) == Some(true))
            }) {
                let contract = FixtureSchemas {
                    input_schema: entry.schema.clone(),
                    output_schema: entry.output_schema.clone(),
                    ..Default::default()
                };
                bytes += serde_json::to_vec(&contract)
                    .map_err(|_| invalid("invalid tool schema"))?
                    .len();
                if bytes > 128 * 1024 {
                    return Err(invalid("saved tool schemas exceed 128 KiB"));
                }
                contracts.insert(id.clone(), contract);
            }
        }
        contracts
    };
    finish(
        &snippet.name,
        tools,
        input,
        contracts,
        params.results,
        params.variant,
        params.check,
    )
}

pub(crate) async fn generate_remote(
    live: &crate::live_gateway::LiveGateway,
    params: Value,
) -> Result<Value, ToolError> {
    let (params, _, snippet, input, _, tools) = prepare(params, None)?;
    if params.schemas.is_some() {
        return Err(invalid("remote discovery does not accept saved schemas"));
    }
    let contracts = live.fixture_tool_schemas(&tools).await?;
    finish(
        &snippet.name,
        tools,
        input,
        contracts,
        params.results,
        params.variant,
        params.check,
    )
}

type Prepared = (
    FixtureParams,
    SnippetDispatchContext,
    super::store::ResolvedSnippet,
    Value,
    labby_codemode::ToolScope,
    Vec<String>,
);
fn prepare(params: Value, context: Option<SnippetDispatchContext>) -> Result<Prepared, ToolError> {
    if serde_json::to_vec(&params)
        .map_err(|_| invalid("invalid fixture generation parameters"))?
        .len()
        > MAX_FIXTURE_BYTES
    {
        return Err(invalid("fixture generation parameters exceed 512 KiB"));
    }
    let params: FixtureParams =
        serde_json::from_value(params).map_err(|error| invalid(error.to_string()))?;
    let context = context.unwrap_or_else(SnippetDispatchContext::trusted_local);
    super::preview::authorize(&context)?;
    if let Some(fixture) = &params.check {
        validate_check_fixture(fixture)?;
    }
    let snippet = resolve_snippet(&lab_home(), &builtin_snippet_dir(), &params.name)?;
    let input = if params.check.is_some() {
        // Contract comparison neither executes the snippet nor consumes its inputs.
        Value::Null
    } else {
        // Validate execution inputs now, but save caller omissions rather than
        // merged null placeholders: replay distinguishes omitted optional inputs
        // from explicit null on nonnullable declarations.
        merge_snippet_input(&snippet, params.params.clone())?;
        match &params.params {
            Value::Null => Value::Object(Default::default()),
            supplied => supplied.clone(),
        }
    };
    let scope = super::execution::snippet_execution_scope(&snippet, &context.execution_scope);
    let tools = params.tools.clone().or_else(|| snippet.tools.as_ref().map(|tools| tools.as_slice().to_vec()))
        .or_else(|| params.schemas.as_ref().map(|schemas| schemas.keys().cloned().collect()))
        .or_else(|| params.check.as_ref().map(|fixture| fixture.schemas.keys().cloned().collect()))
        .ok_or_else(|| invalid("declare exact snippet tools or supply an explicit schemas map; dynamic calls cannot be inferred"))?;
    if tools.len() > 128
        || params.results.len() > 128
        || params.schemas.as_ref().is_some_and(|s| s.len() > 128)
    {
        return Err(invalid("fixture generation is bounded to 128 tools"));
    }
    let _validated_tools: labby_codemode::snippet::tool_declarations::SnippetToolDeclarations =
        tools.clone().try_into()?;
    if let Some(declared) = &snippet.tools
        && tools.iter().any(|id| !declared.as_slice().contains(id))
    {
        return Err(invalid("fixture tools are outside the snippet declaration"));
    }
    for id in tools
        .iter()
        .chain(params.results.keys())
        .chain(params.schemas.iter().flat_map(|s| s.keys()))
    {
        let (namespace, tool) = id
            .split_once("::")
            .ok_or_else(|| invalid("tools require exact upstream::tool identifiers"))?;
        if !tools.contains(id) || !scope.allows(namespace, tool) {
            return Err(invalid(
                "fixture tool is outside the declared or caller tool scope",
            ));
        }
    }
    Ok((params, context, snippet, input, scope, tools))
}

fn validate_check_fixture(fixture: &SnippetFixture) -> Result<(), ToolError> {
    fixture.validate()?;
    if fixture.schemas.is_empty() {
        return Err(invalid("saved fixture has no contracts to compare"));
    }
    Ok(())
}

fn finish(
    name: &str,
    tools: Vec<String>,
    input: Value,
    contracts: BTreeMap<String, FixtureSchemas>,
    results: BTreeMap<String, Value>,
    variant: FixtureVariant,
    check: Option<SnippetFixture>,
) -> Result<Value, ToolError> {
    let draft = if let Some(fixture) = check {
        validate_check_fixture(&fixture)?;
        for contract in contracts.values() {
            for schema in [&contract.input_schema, &contract.output_schema]
                .into_iter()
                .flatten()
            {
                labby_codemode::snippet::schemas::check_schema(schema)?;
            }
        }
        let changed_tools = fixture
            .schemas
            .keys()
            .chain(contracts.keys())
            .chain(tools.iter())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|id| match (fixture.schemas.get(*id), contracts.get(*id)) {
                (Some(saved), Some(current)) => {
                    saved.contract_fingerprint() != current.contract_fingerprint()
                }
                _ => true,
            })
            .cloned()
            .collect::<Vec<_>>();
        FixtureDraft {
            name: name.into(),
            ready: changed_tools.is_empty(),
            coverage: "schema_contracts_only".into(),
            fixture,
            warnings: vec![
                "Schema check compares saved and current contracts without executing tools.".into(),
            ],
            failures: Vec::new(),
            changed_tools,
        }
    } else {
        build_draft(name, tools, input, contracts, results, variant)?
    };
    serde_json::to_value(draft).map_err(|_| invalid("cannot serialize fixture draft"))
}

fn build_draft(
    name: &str,
    tools: Vec<String>,
    input: Value,
    contracts: BTreeMap<String, FixtureSchemas>,
    results: BTreeMap<String, Value>,
    variant: FixtureVariant,
) -> Result<FixtureDraft, ToolError> {
    let mut fixture = SnippetFixture {
        params: input
            .as_object()
            .ok_or_else(|| invalid("snippet input must be an object"))?
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        ..Default::default()
    };
    let mut failures = Vec::new();
    let mut warnings = vec!["Draft includes one call per selected tool. Edit call order, matching, repetition, branches and assertions before treating it as workflow coverage.".into()];
    for tool in tools {
        let Some(contract) = contracts.get(&tool) else {
            failures.push(format!(
                "{tool}: tool schema unavailable; supply a saved contract and explicit result"
            ));
            continue;
        };
        if contract.input_schema.is_none() {
            warnings.push(format!(
                "{tool}: no input schema; arguments cannot be contract checked"
            ));
        }
        let result = if let Some(result) = results.get(&tool) {
            Ok(result.clone())
        } else if let Some(schema) = &contract.output_schema {
            generate_response(schema, variant)
        } else {
            Err(invalid(
                "no output schema; supply an explicit synthetic result",
            ))
        };
        match result {
            Ok(result) => {
                if contract.output_schema.is_none() {
                    warnings.push(format!("{tool}: explicit response has no output contract"));
                }
                let mut contract = contract.clone();
                contract.fingerprint = Some(contract.contract_fingerprint());
                fixture.schemas.insert(tool.clone(), contract);
                fixture.calls.push(FixtureCall {
                    tool,
                    r#match: None,
                    result,
                    error: None,
                    times: 1,
                });
            }
            Err(error) => failures.push(format!("{tool}: {error}")),
        }
    }
    fixture.budgets.tool_calls = fixture.calls.len();
    fixture.validate()?;
    Ok(FixtureDraft {
        name: name.into(),
        ready: failures.is_empty(),
        coverage: "selected_tools_only".into(),
        fixture,
        warnings,
        failures,
        changed_tools: Vec::new(),
    })
}

#[cfg(test)]
mod tests;
