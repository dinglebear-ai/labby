//! Marketplace artifact fork lifecycle stubs.
//!
//! Full fork lifecycle behavior belongs to `lab-iut1.3`. This module wires the
//! action surface with stable signatures and structured placeholder errors.

use serde_json::Value;

use crate::dispatch::error::ToolError;
use crate::dispatch::marketplace::params::{
    ArtifactListParams, ArtifactResetParams, ForkParams, UnforkParams,
};

pub(super) async fn artifact_fork(params: ForkParams) -> Result<Value, ToolError> {
    crate::dispatch::marketplace::stash_bridge::fork_artifacts(&params.plugin_id, params.artifacts)
        .await
}

pub(super) async fn artifact_list(params: ArtifactListParams) -> Result<Value, ToolError> {
    crate::dispatch::marketplace::stash_bridge::list_forks(params.plugin_id).await
}

pub(super) async fn artifact_unfork(params: UnforkParams) -> Result<Value, ToolError> {
    tracing::info!(
        surface = "dispatch",
        service = "marketplace",
        action = "artifact.unfork",
        plugin_id = %params.plugin_id,
        "destructive action intent: removing fork metadata"
    );
    Err(not_implemented_error(
        "artifact.unfork",
        format!(
            "fork lifecycle removal is not implemented yet for `{}`",
            params.plugin_id
        ),
    ))
}

pub(super) async fn artifact_reset(params: ArtifactResetParams) -> Result<Value, ToolError> {
    tracing::info!(
        surface = "dispatch",
        service = "marketplace",
        action = "artifact.reset",
        plugin_id = %params.plugin_id,
        "destructive action intent: resetting forked artifacts"
    );
    Err(not_implemented_error(
        "artifact.reset",
        format!(
            "fork reset is not implemented yet for `{}`",
            params.plugin_id
        ),
    ))
}

fn not_implemented_error(action: &'static str, detail: String) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_implemented".to_string(),
        message: format!("{action}: {detail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn artifact_list_empty_when_no_forks_exist() {
        let result = artifact_list(ArtifactListParams {
            plugin_id: None,
            instance: None,
        })
        .await
        .unwrap();
        let rows = result.as_array().unwrap();
        assert!(rows.is_empty());
    }
}
