//! Small Code Mode helpers: error constructors, catalog sizing, and the
//! self-contained filesystem/env helpers the kernel needs (artifact root,
//! snippet dir, size caps). These are pure stdlib so the crate stays free of a
//! dependency on the host binary's helper module.

pub(crate) use labby_runtime::{env_non_empty, lab_home};

use crate::error::ToolError;
use crate::lab_action_unknown_tool_hint;

use super::types::CatalogDescriptor;

/// Replace the user's home-directory prefix with literal `~` so paths embedded
/// in logs/responses don't leak the OS username. Self-contained copy of the
/// host helper.
#[must_use]
pub(crate) fn redact_home(path: &str) -> String {
    let Some(home) = std::env::var_os("HOME") else {
        return path.to_string();
    };
    if home.is_empty() {
        return path.to_string();
    }
    match std::path::Path::new(path).strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.to_string(),
    }
}

pub(crate) fn invalid_code_mode_id(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_code_mode_id".to_string(),
        message: message.into(),
    }
}

pub(crate) fn lab_action_unknown_tool() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "unknown_tool".to_string(),
        message: format!(
            "lab:: IDs are not supported by Code Mode. {}",
            lab_action_unknown_tool_hint()
        ),
    }
}

/// Serialized byte size of a catalog (host render-size accounting).
pub fn serialized_catalog_size(entries: &[CatalogDescriptor]) -> Result<usize, ToolError> {
    serde_json::to_vec(entries)
        .map(|bytes| bytes.len())
        .map_err(|err| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: format!("failed to serialize Code Mode catalog: {err}"),
        })
}
