use labby_runtime::error::ToolError;

use super::ExecutionLoadoutError;

impl std::fmt::Display for ExecutionLoadoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_json::to_string(self).unwrap_or_else(|_| "execution loadout error".into())
        )
    }
}

impl std::error::Error for ExecutionLoadoutError {}

impl From<ExecutionLoadoutError> for ToolError {
    fn from(error: ExecutionLoadoutError) -> Self {
        let kind = match error {
            ExecutionLoadoutError::NotFound { .. } => "not_found",
            ExecutionLoadoutError::StaleRevision { .. } => "revision_conflict",
            ExecutionLoadoutError::Unresolved { .. } => "loadout_unresolved",
            ExecutionLoadoutError::Storage { .. } => "storage_error",
            ExecutionLoadoutError::Durability { .. } => "durability_unconfirmed",
            _ => "invalid_params",
        };
        ToolError::Sdk {
            sdk_kind: kind.into(),
            message: error.to_string(),
        }
    }
}
