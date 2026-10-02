//! Metadata-only preview and optimistic execution guards. Never evaluates JavaScript.
use super::dispatch::SnippetDispatchContext;
use super::execution::{digest, receipt_owner, snippet_execution_scope, value_digest};
use super::store::{
    ResolvedSnippet, builtin_snippet_dir, code_for_snippet, merge_snippet_input, resolve_snippet,
};
use crate::dispatch::error::ToolError;
use crate::dispatch::gateway::manager::GatewayManager;
use crate::dispatch::helpers::lab_home;
use labby_codemode::{
    CatalogDescriptor, CodeModeCatalogKind, CodeModeHost, CodeModeToolSafety, ToolScope,
};
use labby_gateway::codemode_journal::receipts::SnippetExecutionReceipt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
mod fingerprint;

const MAX_DESCRIPTORS: usize = 128;
const MAX_DESCRIPTOR_BYTES: usize = 128 * 1024;
const MAX_RECEIPT_SCHEMA_BYTES: usize = 24 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreviewParams {
    pub name: Option<String>,
    pub execution_id: Option<String>,
    #[serde(default)]
    pub params: Value,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReplayParams {
    pub execution_id: String,
    pub params: Value,
    pub expected_preview_fingerprint: String,
    pub acknowledged_drift: Vec<String>,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PreviewFingerprints {
    pub snippet_digest: String,
    pub input_digest: String,
    pub effective_scope_fingerprint: String,
    pub runtime_version: String,
    pub tool_schema_digest: String,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct PreviewTool {
    pub id: String,
    pub status: String,
    pub annotations: Option<CodeModeToolSafety>,
    pub schema_digest: Option<String>,
    pub parameters: String,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PreviewDrift {
    pub field: String,
    pub status: String,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct PreviewInputSummary {
    pub keys: Vec<String>,
    pub provided_keys: Vec<String>,
    pub defaulted_keys: Vec<String>,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct SnippetPreview {
    pub name: String,
    pub execution_id: Option<String>,
    pub mode: String,
    pub dynamic_unknown: bool,
    pub coverage: String,
    pub can_execute: bool,
    pub input_summary: PreviewInputSummary,
    pub declared_tools: Vec<PreviewTool>,
    pub fingerprints: PreviewFingerprints,
    pub preview_fingerprint: String,
    pub drift: Vec<PreviewDrift>,
    pub warnings: Vec<String>,
}
pub(super) struct PreparedSnippet {
    pub snippet: ResolvedSnippet,
    pub code: String,
    pub input: Value,
    pub scope: ToolScope,
    pub config: labby_runtime::CodeModeConfig,
    pub schema_digests: Option<BTreeMap<String, Option<String>>>,
    pub preview: SnippetPreview,
}

pub(super) fn authorize(context: &SnippetDispatchContext) -> Result<(), ToolError> {
    if !context.is_admin || !context.execution_caller.is_admin() {
        return Err(ToolError::Forbidden {
            message: "snippet preview and replay require lab:admin".into(),
            required_scopes: vec!["lab:admin".into()],
        });
    }
    Ok(())
}

fn descriptor_digest(entry: &CatalogDescriptor, remaining: &mut usize) -> Option<String> {
    // Never return schemas or defaults: only their digests and safe annotations.
    #[derive(Serialize)]
    struct Evidence<'a> {
        input: &'a Option<Value>,
        output: &'a Option<Value>,
        safety: Option<CodeModeToolSafety>,
    }
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit: *remaining,
    };
    serde_json::to_writer(
        &mut writer,
        &Evidence {
            input: &entry.schema,
            output: &entry.output_schema,
            safety: entry.safety,
        },
    )
    .ok()?;
    *remaining -= writer.bytes.len();
    Some(digest(&writer.bytes))
}

fn descriptor_access_allowed(scope: &ToolScope, entry: &CatalogDescriptor) -> bool {
    scope.allows(&entry.namespace, &entry.name)
        && (!scope.is_read_only() || entry.safety.and_then(|safety| safety.read_only) == Some(true))
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
}
impl std::io::Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("descriptor evidence budget exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) async fn prepare(
    manager: &GatewayManager,
    name: &str,
    supplied: Value,
    context: &SnippetDispatchContext,
    previous: Option<&SnippetExecutionReceipt>,
) -> Result<PreparedSnippet, ToolError> {
    authorize(context)?;
    let snippet = resolve_snippet(&lab_home(), &builtin_snippet_dir(), name)?;
    let code = code_for_snippet(&snippet)?;
    let provided_keys: Vec<String> = supplied
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let input = merge_snippet_input(&snippet, supplied)?;
    let keys: Vec<String> = input
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let defaulted_keys = keys
        .iter()
        .filter(|key| !provided_keys.contains(key))
        .cloned()
        .collect();
    let scope = snippet_execution_scope(&snippet, &context.execution_scope);
    let config = manager.code_mode_config().await;
    // Match the runtime's combined source/input ceiling without evaluating code.
    super::store::wrap_snippet_with_input_bounded(
        &code,
        &input,
        config
            .max_source_bytes
            .min(labby_codemode::MAX_SOURCE_BYTES),
    )?;
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
    .map_err(|_| ToolError::Sdk {
        sdk_kind: "timeout".into(),
        message: "snippet metadata catalog deadline exceeded".into(),
    })??;
    let mut remaining = MAX_DESCRIPTOR_BYTES;
    let mut declared_tools = Vec::new();
    let mut map = BTreeMap::new();
    let mut warnings = vec!["Metadata preview does not execute or simulate JavaScript. Dynamic calls, branches, nested snippets, local providers and tool parameters remain unresolved; invocation authorization is rechecked at runtime.".into()];
    let mut schema_digests = if let Some(declaration) = &snippet.tools {
        debug_assert!(declaration.as_slice().len() <= MAX_DESCRIPTORS);
        for id in declaration.as_slice() {
            let (namespace, tool) = id.split_once("::").expect("validated exact identifier");
            let allowed = scope.allows(namespace, tool);
            let entry = allowed
                .then(|| {
                    render
                        .entries
                        .iter()
                        .find(|entry| entry.kind == CodeModeCatalogKind::Tool && entry.id == *id)
                })
                .flatten();
            let schema_digest = entry.and_then(|entry| descriptor_digest(entry, &mut remaining));
            let access_allowed = entry.is_none_or(|entry| descriptor_access_allowed(&scope, entry));
            map.insert(id.clone(), schema_digest.clone());
            declared_tools.push(PreviewTool {
                id: id.clone(),
                status: if !allowed || !access_allowed {
                    "denied"
                } else if entry.is_none() || schema_digest.is_none() {
                    "unavailable"
                } else {
                    "allowed"
                }
                .into(),
                annotations: entry.and_then(|entry| entry.safety),
                schema_digest,
                parameters: "unresolved".into(),
            });
        }
        Some(map)
    } else {
        warnings.push("No exact tool declaration: dynamic tool-schema coverage cannot be proven; the catalog fingerprint guards the visible snapshot only.".into());
        None
    };
    let tool_schema_digest = value_digest(
        &json!({"descriptors":schema_digests,"catalog":if snippet.tools.is_none() {Some(digest(render.catalog_json.as_bytes()))} else {None}}),
    );
    if schema_digests.as_ref().is_some_and(|map| {
        serde_json::to_vec(map).is_ok_and(|bytes| bytes.len() > MAX_RECEIPT_SCHEMA_BYTES)
    }) {
        schema_digests = None;
        warnings.push("Descriptor digest evidence exceeds the 24 KiB receipt allowance; replay schema comparison will remain unverifiable. The current preview fingerprint still guards the full bounded snapshot.".into());
    }
    let fingerprints = PreviewFingerprints {
        snippet_digest: digest(snippet.body.as_bytes()),
        input_digest: value_digest(&input),
        effective_scope_fingerprint: scope.fingerprint(),
        runtime_version: format!("labby/{} (javy/quickjs)", env!("CARGO_PKG_VERSION")),
        tool_schema_digest,
    };
    let drift = previous
        .map(|old| drift(old, &fingerprints, schema_digests.as_ref()))
        .unwrap_or_default();
    let preview_fingerprint =
        fingerprint::assemble(name, &fingerprints, &config, context, previous, &drift);
    let can_execute = config.enabled
        && context.execution_caller.can_use_snippets()
        && (context.execution_caller.can_execute()
            || scope.is_read_only() && context.execution_caller.can_read())
        && declared_tools.iter().all(|tool| tool.status == "allowed");
    if schema_digests
        .as_ref()
        .is_some_and(|map| map.values().any(Option::is_none))
    {
        warnings.push("Some declared descriptors are unavailable, filtered by authority, or exceed the bounded metadata budget. Their schemas are unverifiable.".into());
    }
    let preview = SnippetPreview {
        name: name.into(),
        execution_id: previous.map(|r| r.execution_id.clone()),
        mode: "metadata".into(),
        dynamic_unknown: true,
        coverage: if snippet.tools.is_some() {
            "declared_tools_only"
        } else {
            "unrestricted_dynamic"
        }
        .into(),
        can_execute,
        input_summary: PreviewInputSummary {
            keys,
            provided_keys,
            defaulted_keys,
        },
        declared_tools,
        fingerprints,
        preview_fingerprint,
        drift,
        warnings,
    };
    Ok(PreparedSnippet {
        snippet,
        code,
        input,
        scope,
        config,
        schema_digests,
        preview,
    })
}

fn drift(
    old: &SnippetExecutionReceipt,
    now: &PreviewFingerprints,
    schemas: Option<&BTreeMap<String, Option<String>>>,
) -> Vec<PreviewDrift> {
    let mut result = Vec::new();
    for (field, before, after) in [
        ("snippet", &old.snippet_digest, &now.snippet_digest),
        ("input", &old.input_digest, &now.input_digest),
        (
            "effective_scope",
            &old.effective_scope_fingerprint,
            &now.effective_scope_fingerprint,
        ),
        ("runtime", &old.runtime_version, &now.runtime_version),
    ] {
        if before != after {
            result.push(PreviewDrift {
                field: field.into(),
                status: "changed".into(),
            });
        }
    }
    let status = match (&old.tool_schema_digests, schemas) {
        (Some(before), Some(after))
            if before.values().all(Option::is_some) && after.values().all(Option::is_some) =>
        {
            (before != after).then_some("changed")
        }
        _ => Some("unverifiable"),
    };
    if let Some(status) = status {
        result.push(PreviewDrift {
            field: "tool_schema".into(),
            status: status.into(),
        });
    }
    result
}

pub(super) fn check_guard(prepared: &PreparedSnippet, expected: &str) -> Result<(), ToolError> {
    if expected != prepared.preview.preview_fingerprint {
        return Err(ToolError::Sdk {
            sdk_kind: "preview_stale".into(),
            message: "snippet execution preview changed; request a new preview before executing"
                .into(),
        });
    }
    if !prepared.preview.can_execute {
        return Err(ToolError::Sdk {
            sdk_kind: "preview_unavailable".into(),
            message:
                "current metadata cannot verify all declared tools under this caller authority"
                    .into(),
        });
    }
    Ok(())
}

pub(super) fn check_replay(
    prepared: &PreparedSnippet,
    expected: &str,
    acknowledgements: &[String],
) -> Result<(), ToolError> {
    check_guard(prepared, expected)?;
    let mut acknowledged = acknowledgements.to_vec();
    acknowledged.sort();
    let mut required: Vec<_> = prepared
        .preview
        .drift
        .iter()
        .map(|d| d.field.clone())
        .collect();
    required.sort();
    if acknowledged != required {
        return Err(ToolError::Sdk {sdk_kind:"confirmation_required".into(),message:"replay requires explicit acknowledgement of every current changed or unverifiable preview field".into()});
    }
    Ok(())
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ToolError> {
    serde_json::from_value(params).map_err(|_| ToolError::InvalidParam {
        message: "invalid snippet preview or replay request".into(),
        param: "params".into(),
    })
}

fn outcome_json(outcome: super::dispatch::SnippetExecutionOutcome) -> Result<Value, ToolError> {
    let mut response = crate::dispatch::helpers::to_json(outcome.display_response)?;
    response["receipt_status"] = json!(outcome.receipt_status);
    Ok(response)
}

pub(super) async fn guarded_exec(
    manager: Option<&GatewayManager>,
    name: &str,
    input: Value,
    context: Option<SnippetDispatchContext>,
    expected: &str,
) -> Result<Value, ToolError> {
    let owned;
    let manager = if let Some(manager) = manager {
        manager
    } else {
        owned = crate::dispatch::gateway::require_gateway_manager()?;
        owned.as_ref()
    };
    let context = context.unwrap_or_else(SnippetDispatchContext::trusted_local);
    let prepared = prepare(manager, name, input, &context, None).await?;
    check_guard(&prepared, expected)?;
    outcome_json(super::execution::execute_prepared_outcome(manager, prepared, &context).await?)
}

pub(super) async fn dispatch(
    manager: Option<&GatewayManager>,
    action: &str,
    params: Value,
    context: Option<SnippetDispatchContext>,
) -> Result<Value, ToolError> {
    let context = context.unwrap_or_else(SnippetDispatchContext::trusted_local);
    authorize(&context)?;
    let owned;
    let manager = if let Some(manager) = manager {
        manager
    } else {
        owned = crate::dispatch::gateway::require_gateway_manager()?;
        owned.as_ref()
    };
    if action == "snippets.preview" {
        let params: PreviewParams = parse(params)?;
        let previous = match (&params.name, &params.execution_id) {
            (Some(_), None) => None,
            (None, Some(id)) => Some(manager.snippet_receipt(id, receipt_owner(&context)).await?),
            _ => {
                return Err(ToolError::InvalidParam {
                    message: "preview requires exactly one name or execution_id".into(),
                    param: "name".into(),
                });
            }
        };
        let name = params
            .name
            .as_deref()
            .or_else(|| previous.as_ref().map(|r| r.snippet_name.as_str()))
            .expect("validated name");
        let prepared = prepare(manager, name, params.params, &context, previous.as_ref()).await?;
        return crate::dispatch::helpers::to_json(prepared.preview);
    }
    let params: ReplayParams = parse(params)?;
    if !params.params.is_object() {
        return Err(ToolError::InvalidParam {
            message: "replay requires a caller-supplied input object".into(),
            param: "params".into(),
        });
    }
    let previous = manager
        .snippet_receipt(&params.execution_id, receipt_owner(&context))
        .await?;
    let prepared = prepare(
        manager,
        &previous.snippet_name,
        params.params,
        &context,
        Some(&previous),
    )
    .await?;
    check_replay(
        &prepared,
        &params.expected_preview_fingerprint,
        &params.acknowledged_drift,
    )?;
    outcome_json(super::execution::execute_prepared_outcome(manager, prepared, &context).await?)
}

#[cfg(test)]
mod tests;
