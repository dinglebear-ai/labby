//! Owner-bound retrieval of persisted Code Mode outputs. Legacy files are not enrolled.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use ulid::Ulid;

use crate::artifacts::CodeModeArtifactReceipt;
use crate::error::ToolError;
use crate::{CodeModeCaller, ToolScope};
use labby_runtime::path_safety::reject_existing_symlink_ancestors;

#[path = "artifact_access_cache.rs"]
mod cache;
pub(crate) use cache::FileIdentity;

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

/// Keep restricted writes separate from retrievable runs of the same subject.
/// An anonymous execution can reclaim only files from that execution.
pub(crate) fn retention_owner(caller: &CodeModeCaller, scope: &ToolScope, root: &Path) -> String {
    owner(caller, scope).unwrap_or_else(|_| {
        let identity = caller.subject().filter(|s| !s.is_empty()).map_or_else(
            || format!("anonymous:{}", root.display()),
            |s| format!("restricted:{s}"),
        );
        hex::encode(Sha256::digest(identity.as_bytes()))
    })
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
    crate::artifacts::publication::publish(&metadata.join(format!("{id}.json")), &bytes)
        .await
        .map_err(|_| unavailable())?;
    if let Some(store) = root.parent() {
        cache::invalidate_listing(store);
    }
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
    let (root, record) = read_record(store, id).await?;
    if record.owner != expected_owner {
        return Err(unavailable());
    }
    Ok((root, record))
}

async fn read_record(store: &Path, id: &str) -> Result<(PathBuf, Record), ToolError> {
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
    if record.receipt.artifact_id.as_deref() != Some(id) {
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
    let max = record.receipt.bytes;
    if max > crate::artifacts::MAX_DOWNLOAD_BYTES {
        return Err(ToolError::Sdk {
            sdk_kind: "response_too_large".into(),
            message: "Artifact exceeds the download limit".into(),
        });
    }
    let mut opened = tokio::fs::File::open(&file)
        .await
        .map_err(|_| unavailable())?;
    let before = opened.metadata().await.map_err(|_| unavailable())?;
    if before.len() != record.receipt.bytes as u64 {
        return Err(unavailable());
    }
    let identity = FileIdentity::from_metadata(&before);
    let cached = identity
        .as_ref()
        .and_then(|identity| cache::content(&file, &record.receipt.sha256, identity));
    let content = if let Some(content) = cached {
        content
    } else {
        #[cfg(test)]
        cache::record_verification(&file);
        let mut bytes = Vec::new();
        (&mut opened)
            .take(max as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| unavailable())?;
        if bytes.len() != record.receipt.bytes
            || hex::encode(Sha256::digest(&bytes)) != record.receipt.sha256
        {
            return Err(unavailable());
        }
        let after = opened.metadata().await.map_err(|_| unavailable())?;
        if identity != FileIdentity::from_metadata(&after) || before.len() != after.len() {
            return Err(unavailable());
        }
        let content: Arc<str> = String::from_utf8(bytes).map_err(|_| unavailable())?.into();
        if let Some(identity) = identity.clone() {
            cache::remember_content(
                file.clone(),
                record.receipt.sha256.clone(),
                identity,
                Arc::clone(&content),
            );
        }
        content
    };
    // Recheck the path as well as the open handle so cached bytes cannot conceal
    // replacement, truncation, or a symlink introduced during this request.
    reject_existing_symlink_ancestors(store, &file)?;
    let current = tokio::fs::metadata(&file)
        .await
        .map_err(|_| unavailable())?;
    if current.len() != before.len() || identity != FileIdentity::from_metadata(&current) {
        return Err(unavailable());
    }
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

async fn scan_listing(store: &Path) -> Result<cache::Listing, ToolError> {
    let root = store.to_owned();
    let (mut owners, legacy, incomplete) = tokio::task::spawn_blocking(move || {
        let mut runs = std::collections::BTreeSet::new();
        let mut owners = std::collections::BTreeMap::<String, Vec<String>>::new();
        let mut legacy = Vec::new();
        let mut incomplete = false;
        let dirs = match std::fs::read_dir(&root) {
            Ok(dirs) => dirs,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((owners, legacy, false));
            }
            Err(_) => return Err(unavailable()),
        };
        for dir in dirs {
            let dir = dir.map_err(|_| unavailable())?;
            if let Some(run) = dir
                .file_name()
                .to_str()
                .and_then(|s| Ulid::from_string(s).ok())
            {
                runs.insert(run);
                if runs.len() > 1000 {
                    runs.pop_first();
                    incomplete = true;
                }
            }
        }
        let mut remaining = 1000usize;
        for run in runs.into_iter().rev() {
            let path = root.join(run.to_string()).join(METADATA_DIR);
            if reject_existing_symlink_ancestors(&root, &path).is_err() {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(path) else {
                continue;
            };
            let mut ids = Vec::new();
            let mut owner_marker = None;
            for entry in entries {
                let entry = entry.map_err(|_| unavailable())?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if let Some(artifact) = name.strip_suffix(".json") {
                    if remaining == 0 {
                        incomplete = true;
                        break;
                    }
                    remaining -= 1;
                    ids.push(format!("{run}:{artifact}"));
                } else if name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    owner_marker = Some(name.to_owned());
                }
            }
            if let Some(owner) = owner_marker {
                // The marker accelerates discovery only. list() verifies each returned receipt.
                owners.entry(owner).or_default().extend(ids);
            } else {
                legacy.extend(ids);
            }
        }
        Ok((owners, legacy, incomplete))
    })
    .await
    .map_err(|_| unavailable())??;
    for id in legacy {
        if let Ok((_, record)) = read_record(store, &id).await {
            owners.entry(record.owner).or_default().push(id);
        }
    }
    for ids in owners.values_mut() {
        ids.sort_unstable();
    }
    Ok(cache::Listing { owners, incomplete })
}

async fn list(store: &Path, owner: &str, params: &Value) -> Result<Value, ToolError> {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(25)
        .clamp(1, 100) as usize;
    let cursor = params.get("cursor").and_then(Value::as_str).unwrap_or("");
    let cell = cache::listing(store);
    let listing = cell.get_or_try_init(|| scan_listing(store)).await?;
    let ids = listing
        .owners
        .get(owner)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let incomplete = listing.incomplete;
    let mut rows = Vec::new();
    let start = ids.partition_point(|id| id.as_str() <= cursor);
    for id in &ids[start..] {
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
