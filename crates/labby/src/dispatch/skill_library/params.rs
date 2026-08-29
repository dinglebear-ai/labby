#![allow(dead_code, reason = "consumed by the concurrent Wave 3 dispatcher")]

use serde::Deserialize;

use super::types::{CreateVisibility, LogicalFileInput};

pub(crate) const DEFAULT_PAGE_LIMIT: usize = 50;
pub(crate) const MAX_PAGE_LIMIT: usize = 100;
pub(crate) const MAX_CURSOR_BYTES: usize = 512;
pub(crate) const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;
pub(crate) const MAX_PREVIEW_BYTES: usize = 256 * 1024;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageParams {
    pub(crate) cursor: Option<String>,
    pub(crate) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ArtifactParams {
    pub(crate) artifact_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReadRevisionParams {
    pub(crate) artifact_id: String,
    pub(crate) revision_id: String,
    pub(crate) path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiffRevisionParams {
    pub(crate) artifact_id: String,
    pub(crate) from_revision_id: String,
    pub(crate) to_revision_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidateParams {
    pub(crate) name: String,
    pub(crate) files: Vec<LogicalFileInput>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateParams {
    pub(crate) name: String,
    pub(crate) files: Vec<LogicalFileInput>,
    pub(crate) expected_library_version: u64,
    pub(crate) idempotency_key: String,
    /// Defaults to private for compatibility with pre-library App clients.
    #[serde(default)]
    pub(crate) visibility: CreateVisibility,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionMutationParams {
    pub(crate) artifact_id: String,
    pub(crate) expected_revision_id: String,
    pub(crate) expected_library_version: u64,
    pub(crate) idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SaveParams {
    pub(crate) artifact_id: String,
    pub(crate) expected_revision_id: String,
    pub(crate) files: Vec<LogicalFileInput>,
    pub(crate) expected_library_version: u64,
    pub(crate) idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LibraryMutationParams {
    pub(crate) artifact_id: String,
    pub(crate) expected_library_version: u64,
    pub(crate) idempotency_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportParams {
    pub(crate) source: SourceSelector,
    pub(crate) expected_library_version: u64,
    pub(crate) idempotency_key: String,
}

/// Public, non-secret selector for one exact object on a server-configured connection.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SourceSelector {
    Depot {
        connection_id: String,
        artifact_id: String,
        revision_id: String,
    },
    Repository {
        connection_id: String,
        artifact_id: String,
        revision_id: String,
    },
}

pub(crate) fn page_limit(value: Option<usize>) -> Result<usize, &'static str> {
    let value = value.unwrap_or(DEFAULT_PAGE_LIMIT);
    (1..=MAX_PAGE_LIMIT)
        .contains(&value)
        .then_some(value)
        .ok_or("limit")
}

pub(crate) fn validate_cursor(value: Option<String>) -> Result<Option<String>, &'static str> {
    match value {
        Some(value) if value.is_empty() || value.len() > MAX_CURSOR_BYTES => Err("cursor"),
        value => Ok(value),
    }
}

pub(crate) fn validate_idempotency_key(value: &str) -> Result<(), &'static str> {
    if value.is_empty()
        || value.len() > MAX_IDEMPOTENCY_KEY_BYTES
        || value.chars().any(char::is_control)
    {
        Err("idempotency_key")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pagination_and_idempotency_are_bounded() {
        assert_eq!(page_limit(None), Ok(DEFAULT_PAGE_LIMIT));
        assert!(page_limit(Some(0)).is_err());
        assert!(page_limit(Some(MAX_PAGE_LIMIT + 1)).is_err());
        assert!(validate_cursor(Some("x".repeat(MAX_CURSOR_BYTES + 1))).is_err());
        assert!(validate_idempotency_key(&"x".repeat(MAX_IDEMPOTENCY_KEY_BYTES + 1)).is_err());
    }

    #[test]
    fn import_wire_accepts_only_server_connection_and_exact_selector() {
        let unknown = serde_json::json!({
            "source": { "kind": "depot", "connection_id": "primary", "artifact_id": "skill", "revision_id": "sha256:exact", "server_path": "/etc" },
            "expected_library_version": 0,
            "idempotency_key": "request-1"
        });
        assert!(serde_json::from_value::<ImportParams>(unknown).is_err());
        let bytes = serde_json::json!({
            "acquisition": { "interchange": {}, "files": [] },
            "expected_library_version": 0,
            "idempotency_key": "request-1"
        });
        assert!(serde_json::from_value::<ImportParams>(bytes).is_err());
    }
}
