//! Shared history and artifact reads; receipt ownership is the authorization source.
use super::dispatch::SnippetDispatchContext;
use super::execution::receipt_owner;
use crate::dispatch::helpers::{lab_home, to_json};
use crate::dispatch::{error::ToolError, gateway::manager::GatewayManager};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryParams {
    name: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactParams {
    execution_id: String,
    path: String,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct SnippetArtifactResponse {
    pub path: String,
    pub sha256: String,
    pub bytes: usize,
    pub content_type: String,
    pub content_base64: String,
}
fn params<T: for<'a> Deserialize<'a>>(value: Value) -> Result<T, ToolError> {
    serde_json::from_value(value).map_err(|_| ToolError::InvalidParam {
        param: "params".into(),
        message: "Invalid snippet history parameters".into(),
    })
}
pub(super) async fn dispatch(
    manager: Option<&GatewayManager>,
    action: &str,
    input: Value,
    context: Option<SnippetDispatchContext>,
) -> Result<Value, ToolError> {
    let context = context.unwrap_or_else(SnippetDispatchContext::trusted_local);
    if !context.is_admin || !context.execution_caller.is_admin() {
        return Err(ToolError::Forbidden {
            message: "Snippet history requires lab:admin".into(),
            required_scopes: vec!["lab:admin".into()],
        });
    }
    let manager =
        manager.ok_or_else(|| ToolError::internal_message("Snippet history requires a gateway"))?;
    if action == "snippets.history" {
        let input: HistoryParams = params(input)?;
        let limit = input.limit.unwrap_or(20);
        if !(1..=50).contains(&limit) {
            return Err(ToolError::InvalidParam {
                param: "limit".into(),
                message: "limit must be between 1 and 50".into(),
            });
        }
        if let Some(name) = &input.name {
            super::store::validate_snippet_name(name)?;
        }
        return to_json(
            manager
                .snippet_history(receipt_owner(&context), input.name, limit, input.cursor)
                .await?,
        );
    }
    let input: ArtifactParams = params(input)?;
    let receipt = manager
        .snippet_receipt(&input.execution_id, receipt_owner(&context))
        .await?;
    let reference = receipt
        .artifacts
        .iter()
        .find(|artifact| artifact.path == input.path)
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "artifact_unavailable".into(),
            message: "Artifact is not available in this execution receipt".into(),
        })?;
    let storage_run_id = reference
        .storage_run_id
        .as_deref()
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "artifact_unavailable".into(),
            message: "This older receipt has no retained artifact location".into(),
        })?;
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        labby_codemode::read_receipted_artifact(
            &lab_home(),
            storage_run_id,
            &reference.path,
            &reference.sha256,
            reference.bytes,
        ),
    )
    .await
    .map_err(|_| ToolError::Sdk {
        sdk_kind: "artifact_unavailable".into(),
        message: "Artifact read exceeded its deadline".into(),
    })??;
    to_json(SnippetArtifactResponse {
        path: reference.path.clone(),
        sha256: reference.sha256.clone(),
        bytes: bytes.len(),
        content_type: reference.content_type.clone(),
        content_base64: STANDARD.encode(bytes),
    })
}
