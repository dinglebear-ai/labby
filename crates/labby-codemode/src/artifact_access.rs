//! Owner-bound retrieval of persisted Code Mode outputs. Legacy files are not enrolled.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use ulid::Ulid;

use crate::artifacts::CodeModeArtifactReceipt;
use crate::error::ToolError;
use crate::{CodeModeCaller, ToolScope};
use labby_runtime::path_safety::reject_existing_symlink_ancestors;

pub(crate) const METADATA_DIR: &str = ".labby-artifact-metadata";

#[cfg(test)]
#[path = "artifact_access_tests.rs"]
mod tests;

#[derive(Serialize, Deserialize)]
struct Record {
    owner: String,
    receipt: CodeModeArtifactReceipt,
}

fn unavailable() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "not_found".into(),
        message: "artifact is unavailable or not owned by this caller".into(),
    }
}

pub(crate) fn owner(caller: &CodeModeCaller, scope: &ToolScope) -> Result<String, ToolError> {
    // Team/route identity is not part of the kernel caller contract. Fail closed
    // rather than inventing ownership from transient request credentials.
    if !caller.is_admin()
        || !caller.can_use_snippets()
        || scope.allowed_namespaces().is_some()
        || scope.allowed_tools().is_some()
    {
        return Err(ToolError::Forbidden {
            message: "artifact retrieval requires an unscoped admin or trusted-local caller".into(),
            required_scopes: vec!["lab:admin".into()],
        });
    }
    let identity = match caller.without_authority() {
        CodeModeCaller::TrustedLocal => "trusted-local".to_owned(),
        _ => format!(
            "subject:{}",
            caller
                .subject()
                .filter(|s| !s.is_empty())
                .ok_or_else(unavailable)?
        ),
    };
    Ok(hex::encode(Sha256::digest(identity.as_bytes())))
}

pub(crate) async fn enroll(
    root: &Path,
    receipt: &mut CodeModeArtifactReceipt,
    caller: &CodeModeCaller,
    scope: &ToolScope,
) -> Result<(), ToolError> {
    let Ok(owner) = owner(caller, scope) else {
        // Preserve existing restricted write behavior without granting retrieval.
        return Ok(());
    };
    let run = root
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(unavailable)?;
    let id = Ulid::new().to_string();
    let metadata = root.join(METADATA_DIR);
    reject_existing_symlink_ancestors(root, &metadata)?;
    tokio::fs::create_dir_all(&metadata)
        .await
        .map_err(|_| unavailable())?;
    receipt.artifact_id = Some(format!("{run}:{id}"));
    let record = Record {
        owner,
        receipt: receipt.clone(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|_| unavailable())?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(metadata.join(format!("{id}.json")))
        .await
        .map_err(|_| unavailable())?;
    file.write_all(&bytes).await.map_err(|_| unavailable())?;
    file.flush().await.map_err(|_| unavailable())?;
    Ok(())
}

fn location(store: &Path, id: &str) -> Result<(PathBuf, PathBuf), ToolError> {
    let (run, artifact) = id.split_once(':').ok_or_else(unavailable)?;
    Ulid::from_string(run).map_err(|_| unavailable())?;
    Ulid::from_string(artifact).map_err(|_| unavailable())?;
    // ULID parsing is case insensitive; require canonical spellings for path lookup.
    if id.len() != 53
        || !id
            .bytes()
            .all(|b| b == b':' || b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(unavailable());
    }
    let root = store.join(run);
    let metadata = root.join(METADATA_DIR).join(format!("{artifact}.json"));
    reject_existing_symlink_ancestors(store, &metadata)?;
    Ok((root, metadata))
}

async fn record(
    store: &Path,
    id: &str,
    expected_owner: &str,
) -> Result<(PathBuf, Record), ToolError> {
    let (root, metadata) = location(store, id)?;
    let size = tokio::fs::metadata(&metadata)
        .await
        .map_err(|_| unavailable())?
        .len();
    if size > 16 * 1024 {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    tokio::fs::File::open(metadata)
        .await
        .map_err(|_| unavailable())?
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| unavailable())?;
    if bytes.len() > 16 * 1024 {
        return Err(unavailable());
    }
    let record: Record = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if record.owner != expected_owner || record.receipt.artifact_id.as_deref() != Some(id) {
        return Err(unavailable());
    }
    Ok((root, record))
}

pub(crate) async fn dispatch(
    operation: &str,
    params: &Value,
    caller: &CodeModeCaller,
    scope: &ToolScope,
) -> Result<Value, ToolError> {
    dispatch_at(
        &crate::util::lab_home().join("code-mode-artifacts"),
        operation,
        params,
        caller,
        scope,
    )
    .await
}

async fn dispatch_at(
    store: &Path,
    operation: &str,
    params: &Value,
    caller: &CodeModeCaller,
    scope: &ToolScope,
) -> Result<Value, ToolError> {
    let owner = owner(caller, scope)?;
    if operation == "list_artifacts" {
        return list(store, &owner, params).await;
    }
    let id = params
        .get("artifact_id")
        .and_then(Value::as_str)
        .ok_or_else(unavailable)?;
    let (root, record) = record(store, id, &owner).await?;
    let mut metadata = serde_json::to_value(&record.receipt).map_err(|_| unavailable())?;
    metadata
        .as_object_mut()
        .ok_or_else(unavailable)?
        .remove("absolute_path");
    metadata["retention"] = json!({
        "runs":crate::artifacts::artifact_retention_runs(),
        "store_bytes":crate::artifacts::artifact_max_store_bytes(),
        "permanent":false,
    });
    if operation == "artifact_info" {
        return Ok(metadata);
    }
    labby_runtime::path_safety::reject_path_traversal(&record.receipt.path)?;
    let file = root.join(&record.receipt.path);
    reject_existing_symlink_ancestors(store, &file)?;
    let max = crate::artifacts::artifact_max_bytes();
    if record.receipt.bytes > max
        || tokio::fs::metadata(&file)
            .await
            .map_err(|_| unavailable())?
            .len()
            > max as u64
    {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    tokio::fs::File::open(file)
        .await
        .map_err(|_| unavailable())?
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| unavailable())?;
    if bytes.len() != record.receipt.bytes
        || hex::encode(Sha256::digest(&bytes)) != record.receipt.sha256
    {
        return Err(unavailable());
    }
    let content = String::from_utf8(bytes).map_err(|_| unavailable())?;
    let offset = params.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let length = params
        .get("length")
        .and_then(Value::as_u64)
        .unwrap_or(1024 * 1024)
        .clamp(1, 1024 * 1024) as usize;
    if offset > content.len() || !content.is_char_boundary(offset) {
        return Err(ToolError::InvalidParam {
            message: "artifact offset must be a UTF-8 byte boundary within the file".into(),
            param: "offset".into(),
        });
    }
    let mut end = offset.saturating_add(length).min(content.len());
    while end > offset && !content.is_char_boundary(end) {
        end -= 1;
    }
    if end == offset && end < content.len() {
        return Err(ToolError::InvalidParam {
            message: "artifact length must fit the next UTF-8 character".into(),
            param: "length".into(),
        });
    }
    Ok(
        json!({"metadata": metadata, "content": &content[offset..end], "offset":offset,
        "next_offset": if end < content.len() {Some(end)} else {None},"done":end == content.len()}),
    )
}

async fn list(store: &Path, owner: &str, params: &Value) -> Result<Value, ToolError> {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(25)
        .clamp(1, 100) as usize;
    let cursor = params.get("cursor").and_then(Value::as_str).unwrap_or("");
    let mut ids = Vec::new();
    let mut incomplete = false;
    let mut dirs = match tokio::fs::read_dir(store).await {
        Ok(dirs) => dirs,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"artifacts":[],"next_cursor":null,"incomplete":false}));
        }
        Err(_) => return Err(unavailable()),
    };
    let mut scanned = 0;
    while let Some(dir) = dirs.next_entry().await.map_err(|_| unavailable())? {
        scanned += 1;
        if scanned > 1000 {
            incomplete = true;
            break;
        }
        let Some(run) = dir.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if Ulid::from_string(&run).is_err() {
            continue;
        }
        let path = dir.path().join(METADATA_DIR);
        if reject_existing_symlink_ancestors(store, &path).is_err() {
            continue;
        }
        let Ok(mut entries) = tokio::fs::read_dir(path).await else {
            continue;
        };
        while let Some(entry) = entries.next_entry().await.map_err(|_| unavailable())? {
            if ids.len() >= 1000 {
                incomplete = true;
                break;
            }
            if let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|s| s.strip_suffix(".json"))
            {
                ids.push(format!("{run}:{id}"));
            }
        }
        if incomplete {
            break;
        }
    }
    ids.sort();
    let mut rows = Vec::new();
    for id in ids.iter().filter(|id| id.as_str() > cursor) {
        if let Ok((_, record)) = record(store, id, owner).await {
            let mut row = serde_json::to_value(record.receipt).map_err(|_| unavailable())?;
            row.as_object_mut()
                .ok_or_else(unavailable)?
                .remove("absolute_path");
            rows.push(row);
            if rows.len() > limit {
                break;
            }
        }
    }
    let next = if rows.len() > limit {
        rows.truncate(limit);
        rows.last().and_then(|r| r.get("artifact_id")).cloned()
    } else {
        None
    };
    Ok(json!({"artifacts": rows, "next_cursor": next, "incomplete": incomplete}))
}
