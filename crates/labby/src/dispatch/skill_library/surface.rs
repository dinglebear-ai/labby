//! Surface-neutral Skill Library action routing after transport authentication.
//!
//! HTTP and MCP adapters project transport-specific identity/correlation facts
//! into this boundary, then share one implementation for remote controls,
//! imports, idempotency validation, and library dispatch/error mapping.

use std::sync::Arc;

use serde_json::Value;

use crate::access::AccessRuntime;
use crate::dispatch::error::ToolError;

use super::ProcessSkillLibrary;
use super::audit::SkillLibraryCorrelationId;
use super::auth::SkillLibraryCaller;
use super::import::ImportCoordinator;
use super::params::{ImportBatchParams, ImportParams, validate_idempotency_key};

pub(crate) async fn dispatch_authorized_action(
    service: &Arc<ProcessSkillLibrary>,
    imports: Option<Arc<ImportCoordinator>>,
    access_runtime: &AccessRuntime,
    caller: SkillLibraryCaller,
    project_id: &str,
    action: &str,
    params: Value,
    correlation: &SkillLibraryCorrelationId,
) -> Result<Value, ToolError> {
    if crate::dispatch::remote_control::REMOTE_ARTIFACT_ACTIONS
        .iter()
        .any(|candidate| candidate.name == action)
    {
        let operation = crate::dispatch::remote_control::operation("artifacts", action)
            .ok_or_else(|| ToolError::UnknownAction {
                message: format!("Unknown action: {action}"),
                valid: Vec::new(),
                hint: None,
            })?;
        let permission = crate::dispatch::artifact_control::operation_permission(operation);
        let authority = crate::dispatch::artifact_control::authorize_authority_context(
            access_runtime,
            caller.identity().clone(),
            project_id,
            caller.selected_team_id(),
            permission,
        )
        .await?;
        return crate::dispatch::remote_control::dispatch_with_context(
            "artifacts",
            action,
            params,
            Some(&authority),
        )
        .await;
    }

    if action == "artifacts.import" {
        let import_params: ImportParams =
            parse_params(params, "Skill Library import parameters are invalid")?;
        validate_idempotency_key(&import_params.idempotency_key).map_err(|_| {
            ToolError::InvalidParam {
                message: "Skill Library idempotency key is invalid".to_owned(),
                param: "idempotency_key".to_owned(),
            }
        })?;
        let imports = imports.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Skill import sources are not configured".to_owned(),
        })?;
        return imports
            .import_selected(
                service,
                access_runtime,
                caller,
                project_id,
                import_params.source,
                import_params.expected_library_version,
                import_params.idempotency_key,
                correlation,
            )
            .await
            .map_err(super::map_import_error);
    }

    if action == "artifacts.import_batch" {
        let import_params: ImportBatchParams =
            parse_params(params, "Artifact batch import parameters are invalid")?;
        validate_idempotency_key(&import_params.idempotency_key).map_err(|_| {
            ToolError::InvalidParam {
                message: "Artifact batch idempotency key is invalid".to_owned(),
                param: "idempotency_key".to_owned(),
            }
        })?;
        let imports = imports.ok_or_else(|| ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Artifact import sources are not configured".to_owned(),
        })?;
        return imports
            .import_batch_selected(
                service,
                access_runtime,
                caller,
                project_id,
                import_params.sources,
                import_params.expected_library_version,
                import_params.idempotency_key,
                correlation,
            )
            .await
            .map_err(super::map_import_error);
    }

    service
        .dispatch(
            access_runtime,
            caller,
            project_id,
            action,
            params,
            correlation,
        )
        .await
        .map_err(super::map_dispatch_error)
}

fn parse_params<T: serde::de::DeserializeOwned>(
    params: Value,
    message: &'static str,
) -> Result<T, ToolError> {
    serde_json::from_value(params).map_err(|_| ToolError::InvalidParam {
        message: message.to_owned(),
        param: "params".to_owned(),
    })
}
