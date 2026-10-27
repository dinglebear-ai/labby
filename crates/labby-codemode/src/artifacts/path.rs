//! One lexical path representation for artifact guards, publication and receipts.

use std::path::{Component, Path};

use crate::error::ToolError;
use labby_runtime::path_safety::reject_path_traversal;

pub(super) fn normalize_artifact_path(raw: &str) -> Result<String, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid_path(
            "artifact path must be a non-empty relative path",
        ));
    }
    // Convert Windows separators before validation, including on Unix where a
    // backslash would otherwise be an ordinary filename byte.
    let separated = trimmed.replace('\\', "/");
    if is_rooted_or_drive_absolute(&separated) {
        return Err(invalid_path("artifact path must be a relative path"));
    }
    reject_path_traversal(&separated)?;
    // CurDir and redundant separators resolve to the same filesystem location.
    // Drop them before the caller checks reserved names or joins the run root.
    let normalized = Path::new(&separated)
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    if normalized.is_empty() {
        return Err(invalid_path("artifact path must name a file"));
    }
    // Removing an initial dot can expose a Windows drive-relative prefix.
    if is_rooted_or_drive_absolute(&normalized) {
        return Err(invalid_path("artifact path must be a relative path"));
    }
    Ok(normalized)
}

fn invalid_path(message: &str) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: "path".into(),
    }
}

/// Reject rooted, drive-absolute and drive-relative paths on every platform.
fn is_rooted_or_drive_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || Path::new(path).is_absolute()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_preserves_traversal_and_drive_guards() {
        for path in [
            "",
            ".",
            "././",
            "a/../b",
            r"a\..\b",
            "/etc/evil",
            "C:evil",
            "C:/evil",
            "./C:evil",
            "./C:/evil",
        ] {
            assert!(normalize_artifact_path(path).is_err(), "{path}");
        }
    }
}
