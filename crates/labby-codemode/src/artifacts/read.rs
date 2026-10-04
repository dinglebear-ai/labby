//! Read only artifact bytes selected by an authorized, integrity-checked receipt.
use crate::error::ToolError;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_DOWNLOAD_BYTES: usize = 8 * 1024 * 1024;
fn unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "artifact_unavailable".into(),
        message: "Artifact is no longer retained, changed, or cannot be read safely".into(),
    }
}

/// The caller must first authorize the execution receipt and select one of its
/// artifact references. No arbitrary host path is accepted. Payloads are bounded
/// and checked against the receipt's exact size and digest before release.
pub async fn read_receipted_artifact(
    home: &Path,
    execution_id: &str,
    path: &str,
    expected_sha256: &str,
    expected_bytes: usize,
) -> Result<Vec<u8>, ToolError> {
    if execution_id.is_empty()
        || execution_id.len() > 128
        || !execution_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(unavailable());
    }
    let path = super::normalize_artifact_path(path)?;
    if expected_bytes > MAX_DOWNLOAD_BYTES {
        return Err(ToolError::InvalidParam {
            param: "path".into(),
            message: "Artifact exceeds the 8 MiB download limit".into(),
        });
    }
    let home = home.to_owned();
    let execution_id = execution_id.to_owned();
    let expected_sha256 = expected_sha256.to_owned();
    tokio::task::spawn_blocking(move || {
        let home = std::fs::canonicalize(home).map_err(|_| unavailable())?;
        let relative = PathBuf::from("code-mode-artifacts")
            .join(execution_id)
            .join(path);
        let mut file = open_contained(&home, &relative)?;
        let metadata = file.metadata().map_err(|_| unavailable())?;
        if !metadata.is_file() || metadata.len() != expected_bytes as u64 {
            return Err(unavailable());
        }
        let mut bytes = Vec::with_capacity(expected_bytes);
        file.by_ref()
            .take((MAX_DOWNLOAD_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        if bytes.len() != expected_bytes || hex::encode(Sha256::digest(&bytes)) != expected_sha256 {
            return Err(unavailable());
        }
        Ok(bytes)
    })
    .await
    .map_err(|_| unavailable())?
}

#[cfg(unix)]
fn open_contained(root: &Path, relative: &Path) -> Result<std::fs::File, ToolError> {
    use nix::fcntl::{OFlag, open, openat};
    use nix::sys::stat::Mode;
    // Every untrusted component is opened relative to a held directory FD.
    // NOFOLLOW and NONBLOCK reject symlink/FIFO races without unsafe code.
    let directory_flags =
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let mut directory = open(root, directory_flags, Mode::empty()).map_err(|_| unavailable())?;
    let components = relative.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        let std::path::Component::Normal(name) = component else {
            return Err(unavailable());
        };
        let flags = if index + 1 == components.len() {
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK
        } else {
            directory_flags
        };
        let descriptor =
            openat(&directory, Path::new(name), flags, Mode::empty()).map_err(|_| unavailable())?;
        if index + 1 == components.len() {
            return Ok(std::fs::File::from(descriptor));
        }
        directory = descriptor;
    }
    Err(unavailable())
}

#[cfg(not(unix))]
fn open_contained(root: &Path, relative: &Path) -> Result<std::fs::File, ToolError> {
    let destination = root.join(relative);
    labby_runtime::path_safety::reject_existing_symlink_ancestors(root, &destination)?;
    let file = std::fs::File::open(&destination).map_err(|_| unavailable())?;
    labby_runtime::path_safety::reject_existing_symlink_ancestors(root, &destination)?;
    // The opened handle's bytes still must match the authorized receipt exactly.
    Ok(file)
}

#[cfg(test)]
mod tests;
