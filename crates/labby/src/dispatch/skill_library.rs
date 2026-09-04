//! Shared Skill Library policy boundary.

pub(crate) mod audit;
pub(crate) mod auth;
pub(crate) mod blocking;
pub(crate) mod catalog;
pub(crate) mod client;
pub(crate) mod depot;
pub(crate) mod dispatch;
pub(crate) mod import;
pub(crate) mod params;
pub(crate) mod types;

use std::sync::{Arc, OnceLock};

use labby_runtime::artifacts::ArtifactError;
use labby_runtime::error::ToolError;

use crate::skills::registry::FirstPartyGeneration;

type ProcessSkillLibrary = dispatch::SkillLibraryService<FirstPartyGeneration>;

pub(crate) struct ProcessSkillLibraryRuntime {
    pub(crate) service: Arc<ProcessSkillLibrary>,
    pub(crate) imports: Arc<import::ImportCoordinator>,
    pub(crate) controls: Arc<crate::dispatch::artifact_control::ArtifactControlPlane>,
}

static PROCESS_SKILL_LIBRARY_RUNTIME: OnceLock<Arc<ProcessSkillLibraryRuntime>> = OnceLock::new();

pub(crate) fn install_process_runtime(
    runtime: Arc<ProcessSkillLibraryRuntime>,
) -> Result<(), Arc<ProcessSkillLibraryRuntime>> {
    PROCESS_SKILL_LIBRARY_RUNTIME.set(runtime)
}

pub(crate) fn process_service() -> Option<Arc<ProcessSkillLibrary>> {
    PROCESS_SKILL_LIBRARY_RUNTIME
        .get()
        .map(|runtime| Arc::clone(&runtime.service))
}

pub(crate) fn process_imports() -> Option<Arc<import::ImportCoordinator>> {
    PROCESS_SKILL_LIBRARY_RUNTIME
        .get()
        .map(|runtime| Arc::clone(&runtime.imports))
}

pub(crate) fn process_controls()
-> Option<Arc<crate::dispatch::artifact_control::ArtifactControlPlane>> {
    PROCESS_SKILL_LIBRARY_RUNTIME
        .get()
        .map(|runtime| Arc::clone(&runtime.controls))
}

/// Closed, redacted projection of internal management failures to the shared surface contract.
pub(crate) fn map_dispatch_error(error: dispatch::SkillLibraryDispatchError) -> ToolError {
    use auth::SkillLibraryAuthorizationError;
    use dispatch::SkillLibraryDispatchError;

    match error {
        SkillLibraryDispatchError::Authorization(SkillLibraryAuthorizationError::Denied) => {
            ToolError::Forbidden {
                message: "Skill Library access denied".to_owned(),
                required_scopes: Vec::new(),
            }
        }
        SkillLibraryDispatchError::Authorization(SkillLibraryAuthorizationError::Unavailable) => {
            ToolError::Sdk {
                sdk_kind: "service_unavailable".to_owned(),
                message: "Skill Library authorization is unavailable".to_owned(),
            }
        }
        SkillLibraryDispatchError::Artifact(ArtifactError::InvalidField { field, .. }) => {
            ToolError::InvalidParam {
                message: "Skill Library parameter is invalid".to_owned(),
                param: field.to_owned(),
            }
        }
        SkillLibraryDispatchError::Artifact(
            ArtifactError::UnsupportedSchema
            | ArtifactError::UnsafePath(_)
            | ArtifactError::SecretMaterialDetected { .. }
            | ArtifactError::SkillVerification
            | ArtifactError::LogicalSkillFile { .. },
        )
        | SkillLibraryDispatchError::InvalidParams => ToolError::InvalidParam {
            message: "Skill Library parameters are invalid".to_owned(),
            param: "params".to_owned(),
        },
        SkillLibraryDispatchError::Artifact(ArtifactError::LimitExceeded { .. }) => {
            ToolError::Sdk {
                sdk_kind: "budget_exceeded".to_owned(),
                message: "Skill Library request exceeds a safety budget".to_owned(),
            }
        }
        SkillLibraryDispatchError::Artifact(ArtifactError::NotFound(_)) => ToolError::Sdk {
            sdk_kind: "not_found".to_owned(),
            message: "Skill Library item was not found".to_owned(),
        },
        SkillLibraryDispatchError::Artifact(ArtifactError::Conflict(reason)) => {
            ToolError::Conflict {
                message: if reason == "library_version_changed" {
                    "Skill Library version is stale; re-list and retry"
                } else {
                    "Skill Library state conflicts with this request"
                }
                .to_owned(),
                existing_id: "skill_library".to_owned(),
            }
        }
        SkillLibraryDispatchError::Artifact(ArtifactError::Busy) => ToolError::Sdk {
            sdk_kind: "queue_saturated".to_owned(),
            message: "Skill Library is busy; retry later".to_owned(),
        },
        SkillLibraryDispatchError::Artifact(ArtifactError::CommittedPending {
            committed_version,
        }) => ToolError::contract(
            "service_unavailable",
            "Skill Library commit requires reconciliation",
            serde_json::Map::from_iter([
                ("committed_version".to_owned(), committed_version.into()),
                (
                    "recovery".to_owned(),
                    "retry_with_same_idempotency_key".into(),
                ),
            ]),
            None,
            None,
            None,
        ),
        SkillLibraryDispatchError::Artifact(
            ArtifactError::LibraryCorrupt(_) | ArtifactError::Io(_) | ArtifactError::Json(_),
        )
        | SkillLibraryDispatchError::Serialization
        | SkillLibraryDispatchError::InjectedFault(_) => ToolError::Sdk {
            sdk_kind: "internal_error".to_owned(),
            message: "Skill Library operation failed".to_owned(),
        },
        SkillLibraryDispatchError::BlockingIndeterminate { operation } => ToolError::contract(
            "service_unavailable",
            "Skill Library outcome is indeterminate; retry the identical request with the same idempotency key",
            serde_json::Map::from_iter([
                ("operation".to_owned(), operation.into()),
                ("outcome".to_owned(), "indeterminate".into()),
                (
                    "recovery".to_owned(),
                    "retry_with_same_idempotency_key".into(),
                ),
            ]),
            None,
            None,
            None,
        ),
        SkillLibraryDispatchError::BlockingTimeout { .. } => ToolError::Sdk {
            sdk_kind: "timeout".to_owned(),
            message: "Skill Library work did not complete in time".to_owned(),
        },
        SkillLibraryDispatchError::BlockingWorkerFailed { .. } => ToolError::Sdk {
            sdk_kind: "internal_error".to_owned(),
            message: "Skill Library worker failed".to_owned(),
        },
        SkillLibraryDispatchError::UnknownAction => ToolError::UnknownAction {
            message: "unknown Skill Library action".to_owned(),
            valid: catalog::ACTIONS
                .iter()
                .map(|spec| spec.name.to_owned())
                .collect(),
            hint: Some("use skills schema to inspect supported actions".to_owned()),
        },
    }
}

pub(crate) fn map_import_error(error: import::ImportAdapterError) -> ToolError {
    match error {
        import::ImportAdapterError::SourceUnavailable => ToolError::Sdk {
            sdk_kind: "source_unavailable".to_owned(),
            message: "Requested Skill import source is not configured".to_owned(),
        },
        import::ImportAdapterError::Artifact(ArtifactError::Conflict("provider_timeout")) => {
            ToolError::Sdk {
                sdk_kind: "timeout".to_owned(),
                message: "Skill import source timed out".to_owned(),
            }
        }
        import::ImportAdapterError::Artifact(ArtifactError::Conflict(
            "source_authorization_expired",
        )) => ToolError::Forbidden {
            message: "Skill import source authorization failed".to_owned(),
            required_scopes: Vec::new(),
        },
        import::ImportAdapterError::Artifact(error) => {
            map_dispatch_error(dispatch::SkillLibraryDispatchError::Artifact(error))
        }
        import::ImportAdapterError::Dispatch(error) => map_dispatch_error(error),
    }
}
