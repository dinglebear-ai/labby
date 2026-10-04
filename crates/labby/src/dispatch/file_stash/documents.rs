//! Passive context documents and virtual folders over the canonical Stash store.

use super::*;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt as _;

pub(crate) const MAX_DOCUMENT_BYTES: usize = 512 * 1024;
const TEXT_CHUNK_BYTES: usize = 16 * 1024;

pub(crate) const FOLDER_PARAM: ParamSpec = ParamSpec {
    name: "folder",
    ty: "string",
    required: false,
    description: "Virtual folder, such as dinglebear-ai/labby; empty selects Unfiled",
};
pub(crate) const SAVE_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "filename",
        ty: "string",
        required: true,
        description: "Display filename; existing names in the same folder conflict",
    },
    ParamSpec {
        name: "content",
        ty: "string",
        required: true,
        description: "Exact UTF-8 document, at most 512 KiB",
    },
    ParamSpec {
        name: "format",
        ty: "markdown|text",
        required: false,
        description: "Document format; defaults to markdown",
    },
    FOLDER_PARAM,
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];
pub(crate) const READ_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "uri",
        ty: "string",
        required: true,
        description: "Stable stash://me/files/{id} resource URI",
    },
    ParamSpec {
        name: "cursor",
        ty: "string",
        required: false,
        description: "Continuation from the preceding text read",
    },
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];
pub(crate) const MOVE_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "file_id",
        ty: "string",
        required: true,
        description: "Opaque file ID; moving preserves its resource URI",
    },
    ParamSpec {
        name: "folder",
        ty: "string",
        required: true,
        description: "Destination virtual folder; empty moves to Unfiled",
    },
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];
pub(crate) const FOLDERS_PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "cursor",
        ty: "string",
        required: false,
        description: "Folder continuation from the preceding folder page",
    },
    ParamSpec {
        name: "limit",
        ty: "integer",
        required: false,
        description: "Page size, at most 200",
    },
    OWNER_KIND_PARAM,
    OWNER_ID_PARAM,
];

#[derive(Serialize)]
pub(crate) struct FolderView {
    pub folder: String,
    pub file_count: u64,
}
#[derive(Serialize)]
pub(crate) struct FolderPage {
    pub folders: Vec<FolderView>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Serialize)]
pub(crate) struct TextPage {
    pub file: FileView,
    pub content: String,
    pub next_cursor: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TextCursor {
    file_id: String,
    offset: usize,
}

/// Folder paths are display metadata, never filesystem paths or URI segments.
pub(crate) fn normalize_folder(raw: &str) -> Result<String, ToolError> {
    let folder: String = raw.nfc().collect();
    if folder.len() > 1024
        || folder.chars().any(|c| c.is_control() || c == '\\')
        || (!folder.is_empty()
            && folder
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == ".." || part.len() > 255))
    {
        return Err(invalid("folder"));
    }
    Ok(folder)
}

impl FileStashService {
    pub(crate) async fn reserve_document_upload(
        &self,
        principal: &PrincipalId,
        name: &str,
        bytes: u64,
        folder: &str,
        content_type: &str,
    ) -> Result<(UploadReservation, UploadAdmission), ToolError> {
        let folder = normalize_folder(folder)?;
        let (name, key) = normalize_name(name)?;
        let key = if folder.is_empty() {
            key
        } else {
            format!("{folder}/{key}")
        };
        let (store, blobs) = self.stores().await?;
        let (reservation, admission) = blobs
            .reserve(principal, name, key, bytes)
            .await
            .map_err(map_error)?;
        if let Err(error) = store
            .set_upload_metadata(
                reservation.upload_id.clone(),
                folder,
                content_type.to_owned(),
            )
            .await
        {
            // No bytes have been published, and the admission guard still owns
            // this reservation. A failed cancellation remains recoverable.
            let _ = store.cancel_upload(reservation.upload_id.clone()).await;
            return Err(map_error(error));
        }
        Ok((reservation, admission))
    }

    pub(crate) async fn save_text(
        &self,
        principal: &PrincipalId,
        filename: &str,
        content: &str,
        format: Option<&str>,
        folder: Option<&str>,
    ) -> Result<FileView, ToolError> {
        if content.len() > MAX_DOCUMENT_BYTES {
            return Err(service_error(
                "quota_exceeded",
                "Context documents are limited to 512 KiB",
            ));
        }
        let content_type = match format.unwrap_or("markdown") {
            "markdown" => "text/markdown",
            "text" => "text/plain",
            _ => return Err(invalid("format")),
        };
        let (reservation, admission) = self
            .reserve_document_upload(
                principal,
                filename,
                content.len() as u64,
                folder.unwrap_or_default(),
                content_type,
            )
            .await?;
        let file_id = self
            .finalize_upload(
                reservation,
                admission,
                content.as_bytes(),
                CancellationToken::new(),
            )
            .await?;
        self.metadata(principal, &file_id).await
    }

    pub(crate) async fn read_text(
        &self,
        principal: &PrincipalId,
        uri: &str,
        cursor: Option<&str>,
    ) -> Result<TextPage, ToolError> {
        let file_id = parse_stash_uri(uri)?;
        let offset = match cursor {
            None => 0,
            Some(value) => {
                if value.len() > 256 {
                    return Err(invalid("cursor"));
                }
                let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(value)
                    .map_err(|_| invalid("cursor"))?;
                let decoded: TextCursor =
                    serde_json::from_slice(&bytes).map_err(|_| invalid("cursor"))?;
                if decoded.file_id != file_id {
                    return Err(invalid("cursor"));
                }
                decoded.offset
            }
        };
        let (file, mut blob) = self.open_download(principal, &file_id, true).await?;
        if !matches!(file.content_type.as_str(), "text/markdown" | "text/plain") {
            return Err(invalid("uri"));
        }
        if file.size_bytes > MAX_DOCUMENT_BYTES as u64 {
            return Err(service_error(
                "quota_exceeded",
                "Context document exceeds the text limit",
            ));
        }
        let mut bytes = Vec::with_capacity(file.size_bytes as usize);
        (&mut blob)
            .take(file.size_bytes + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| {
                service_error("service_unavailable", "Context document could not be read")
            })?;
        if bytes.len() != file.size_bytes as usize {
            return Err(service_error(
                "integrity_error",
                "Context document size mismatch",
            ));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| service_error("integrity_error", "Context document is not UTF-8"))?;
        if !text.is_char_boundary(offset) {
            return Err(invalid("cursor"));
        }
        let mut end = offset.saturating_add(TEXT_CHUNK_BYTES).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let next_cursor = if end < text.len() {
            let encoded = serde_json::to_vec(&TextCursor {
                file_id,
                offset: end,
            })
            .map_err(|_| service_error("internal_error", "Context cursor failed"))?;
            Some(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(encoded))
        } else {
            None
        };
        Ok(TextPage {
            file,
            content: text[offset..end].to_owned(),
            next_cursor,
        })
    }

    pub(crate) async fn folders(
        &self,
        principal: &PrincipalId,
        cursor: Option<&str>,
        limit: Option<usize>,
    ) -> Result<FolderPage, ToolError> {
        let limit = validated_limit(limit, self.page_limit)?;
        let after = cursor.map(normalize_folder).transpose()?;
        let (store, _) = self.stores().await?;
        let mut rows = store
            .list_folders(principal.as_str().to_owned(), after, limit + 1)
            .await
            .map_err(map_error)?;
        let next_cursor = if rows.len() > limit {
            rows.truncate(limit);
            rows.last().map(|row| row.0.clone())
        } else {
            None
        };
        Ok(FolderPage {
            folders: rows
                .into_iter()
                .map(|(folder, file_count)| FolderView { folder, file_count })
                .collect(),
            next_cursor,
        })
    }

    pub(crate) async fn move_file(
        &self,
        principal: &PrincipalId,
        file_id: &str,
        folder: &str,
    ) -> Result<FileView, ToolError> {
        validate_id(file_id, "file_id")?;
        let folder = normalize_folder(folder)?;
        let (store, _) = self.stores().await?;
        store
            .move_file(principal.as_str().to_owned(), file_id.to_owned(), folder)
            .await
            .map(Into::into)
            .map_err(map_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_folder_paths_are_normalized_and_bounded() {
        assert_eq!(
            normalize_folder("dinglebear-ai/labby").unwrap(),
            "dinglebear-ai/labby"
        );
        assert_eq!(normalize_folder("cafe\u{301}/notes").unwrap(), "café/notes");
        assert_eq!(normalize_folder("").unwrap(), "");
        for folder in ["/absolute", "a/../b", "a/./b", "a//b", "a/", "a\\b", "a\0b"] {
            assert!(normalize_folder(folder).is_err(), "{folder:?}");
        }
        assert!(normalize_folder(&"x".repeat(256)).is_err());
    }

    #[cfg(target_os = "linux")]
    async fn fixture() -> (FileStashService, Arc<FileStashRuntime>, tempfile::TempDir) {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::tempdir().unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let runtime = Arc::new(FileStashRuntime::initialize(temp.path().join("stash")).await);
        runtime.wait_for_recovery().await;
        let service = FileStashService::new(
            runtime.clone(),
            Arc::new(AccessRuntime::blocked_unavailable()),
            50,
            128,
        );
        (service, runtime, temp)
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn documents_persist_with_private_folders_and_stable_uris() {
        let (service, runtime, temp) = fixture().await;
        let owner = PrincipalId::for_test("owner");
        let stranger = PrincipalId::for_test("stranger");
        let content = "# Handoff\nKeep café and 🦀 exactly.\n";
        let first = service
            .save_text(
                &owner,
                "plan.md",
                content,
                None,
                Some("dinglebear-ai/labby"),
            )
            .await
            .unwrap();
        let second = service
            .save_text(
                &owner,
                "plan.md",
                "another repo",
                None,
                Some("dinglebear-ai/cortex"),
            )
            .await
            .unwrap();
        assert_eq!(first.folder, "dinglebear-ai/labby");
        assert_eq!(first.content_type, "text/markdown");
        assert_eq!(
            service
                .save_text(
                    &owner,
                    "PLAN.md",
                    "collision",
                    None,
                    Some("dinglebear-ai/labby")
                )
                .await
                .unwrap_err()
                .kind(),
            "conflict"
        );
        assert!(
            service
                .list_in_folder(&stranger, None, None, None, Some("dinglebear-ai/labby"))
                .await
                .unwrap()
                .files
                .is_empty()
        );
        assert!(
            service
                .folders(&stranger, None, None)
                .await
                .unwrap()
                .folders
                .is_empty()
        );
        assert_eq!(
            service
                .read_text(&stranger, &first.uri, None)
                .await
                .unwrap_err()
                .kind(),
            "not_found"
        );
        let found = service
            .list_in_folder(&owner, None, None, Some(1), Some("dinglebear-ai/labby"))
            .await
            .unwrap();
        assert_eq!(found.files.len(), 1);
        assert_eq!(found.files[0].file_id, first.file_id);
        assert!(found.next_cursor.is_none());
        assert_eq!(
            service
                .move_file(&owner, &first.file_id, "dinglebear-ai/cortex")
                .await
                .unwrap_err()
                .kind(),
            "conflict"
        );
        service.delete(&owner, &second.file_id).await.unwrap();
        let moved = service
            .move_file(&owner, &first.file_id, "dinglebear-ai/cortex")
            .await
            .unwrap();
        assert_eq!(moved.uri, first.uri);
        let renamed = service
            .rename(&owner, &first.file_id, "handoff.md")
            .await
            .unwrap();
        assert_eq!(renamed.folder, "dinglebear-ai/cortex");
        assert_eq!(renamed.uri, first.uri);
        assert_eq!(
            service
                .read_text(&owner, &first.uri, None)
                .await
                .unwrap()
                .content,
            content
        );
        runtime.shutdown().await;
        drop(service);
        drop(runtime);
        let runtime = Arc::new(FileStashRuntime::initialize(temp.path().join("stash")).await);
        runtime.wait_for_recovery().await;
        let service = FileStashService::new(
            runtime.clone(),
            Arc::new(AccessRuntime::blocked_unavailable()),
            50,
            128,
        );
        let read = service.read_text(&owner, &first.uri, None).await.unwrap();
        assert_eq!(read.content, content);
        assert_eq!(read.file.folder, "dinglebear-ai/cortex");
        runtime.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn unfiled_pagination_and_unicode_moves_preserve_name_claims() {
        let (service, runtime, _temp) = fixture().await;
        let owner = PrincipalId::for_test("owner");
        service
            .save_text(&owner, "root.md", "root", None, None)
            .await
            .unwrap();
        let file = service
            .save_text(&owner, "CAFÉ.md", "text", None, Some("café/notes"))
            .await
            .unwrap();
        let first = service.folders(&owner, None, Some(1)).await.unwrap();
        assert_eq!(first.folders[0].folder, "");
        assert_eq!(first.next_cursor.as_deref(), Some(""));
        let second = service
            .folders(&owner, first.next_cursor.as_deref(), Some(1))
            .await
            .unwrap();
        assert_eq!(second.folders[0].folder, "café/notes");
        assert!(second.next_cursor.is_none());
        service.move_file(&owner, &file.file_id, "").await.unwrap();
        assert_eq!(
            service
                .save_text(&owner, "café.md", "collision", None, None)
                .await
                .unwrap_err()
                .kind(),
            "conflict"
        );
        service
            .move_file(&owner, &file.file_id, "🦀/context")
            .await
            .unwrap();
        assert_eq!(
            service
                .save_text(&owner, "café.md", "collision", None, Some("🦀/context"))
                .await
                .unwrap_err()
                .kind(),
            "conflict"
        );
        assert_eq!(
            service
                .list_in_folder(&owner, None, None, None, Some(""))
                .await
                .unwrap()
                .files
                .len(),
            1
        );
        assert_eq!(
            service
                .read_text(&owner, &file.uri, None)
                .await
                .unwrap()
                .content,
            "text"
        );
        runtime.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn text_continuation_preserves_unicode_and_revalidates_access() {
        let (service, runtime, _temp) = fixture().await;
        let owner = PrincipalId::for_test("owner");
        let reader = PrincipalId::for_test("reader");
        let content = format!(
            "{}🦀{}",
            "a".repeat(TEXT_CHUNK_BYTES - 1),
            "café".repeat(5000)
        );
        let saved = service
            .save_text(&owner, "large.md", &content, None, None)
            .await
            .unwrap();
        let other = service
            .save_text(&owner, "other.md", "other", None, None)
            .await
            .unwrap();
        let grant = service
            .create_grant_validated(&owner, &saved.file_id, &reader)
            .await
            .unwrap();
        let mut page = service.read_text(&reader, &saved.uri, None).await.unwrap();
        assert!(page.content.len() <= TEXT_CHUNK_BYTES);
        let cursor = page.next_cursor.clone().unwrap();
        assert_eq!(
            service
                .read_text(&owner, &other.uri, Some(&cursor))
                .await
                .unwrap_err()
                .kind(),
            "invalid_param"
        );
        let mut assembled = page.content;
        while let Some(cursor) = page.next_cursor {
            page = service
                .read_text(&reader, &saved.uri, Some(&cursor))
                .await
                .unwrap();
            assembled.push_str(&page.content);
        }
        assert_eq!(assembled, content);
        service
            .revoke_grant(&owner, &saved.file_id, &grant.grant_id)
            .await
            .unwrap();
        assert_eq!(
            service
                .read_text(&reader, &saved.uri, Some(&cursor))
                .await
                .unwrap_err()
                .kind(),
            "not_found"
        );
        let invalid_cursor = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&TextCursor {
                file_id: saved.file_id,
                offset: TEXT_CHUNK_BYTES,
            })
            .unwrap(),
        );
        assert_eq!(
            service
                .read_text(&owner, &saved.uri, Some(&invalid_cursor))
                .await
                .unwrap_err()
                .kind(),
            "invalid_param"
        );
        runtime.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    async fn assert_rejected_metadata_mutation_is_rolled_back(action: &str) {
        let (service, runtime, _temp) = fixture().await;
        let owner = PrincipalId::for_test("owner");
        let reader = PrincipalId::for_test("reader");
        let new_reader = PrincipalId::for_test("new-reader");
        let saved = service
            .save_text(&owner, "private.md", "private", None, Some("original"))
            .await
            .unwrap();
        let grant = service
            .create_grant_validated(&owner, &saved.file_id, &reader)
            .await
            .unwrap();
        let params = serde_json::json!({"file_id":saved.file_id,"folder":"moved","display_name":"renamed.md","grant_id":grant.grant_id});
        let error = dispatch_with_final_check(
            &service,
            &owner,
            "mcp",
            action,
            params,
            Some(&new_reader),
            async { Err::<(), _>(service_error("not_found", "authority revoked")) },
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), "not_found");
        let current = service.metadata(&owner, &saved.file_id).await.unwrap();
        assert_eq!(current.folder, "original", "{action}");
        assert_eq!(current.display_name, "private.md", "{action}");
        assert_eq!(
            service
                .read_text(&owner, &saved.uri, None)
                .await
                .unwrap()
                .content,
            "private"
        );
        assert_eq!(
            service
                .read_text(&reader, &saved.uri, None)
                .await
                .unwrap()
                .content,
            "private"
        );
        assert!(
            service
                .read_text(&new_reader, &saved.uri, None)
                .await
                .is_err()
        );
        assert_eq!(
            service
                .grants(&owner, &saved.file_id, None, None)
                .await
                .unwrap()
                .grants
                .len(),
            1
        );
        assert_eq!(
            service.stats(&owner).await.unwrap().owned_committed_bytes,
            7
        );
        assert_eq!(
            service
                .save_text(&owner, "PRIVATE.md", "collision", None, Some("original"))
                .await
                .unwrap_err()
                .kind(),
            "conflict"
        );
        runtime.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn rejected_move_authority_preserves_metadata_and_name_claims() {
        assert_rejected_metadata_mutation_is_rolled_back("stash.move").await;
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn rejected_rename_authority_preserves_metadata_and_name_claims() {
        assert_rejected_metadata_mutation_is_rolled_back("stash.rename").await;
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn rejected_delete_authority_preserves_file_bytes_and_quota() {
        assert_rejected_metadata_mutation_is_rolled_back("stash.delete").await;
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn rejected_grant_creation_authority_preserves_private_access() {
        assert_rejected_metadata_mutation_is_rolled_back("stash.grants.create").await;
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn rejected_grant_revocation_authority_preserves_existing_access() {
        assert_rejected_metadata_mutation_is_rolled_back("stash.grants.revoke").await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn canceled_metadata_request_retains_rollback_until_authority_gate_finishes() {
        let (service, runtime, _temp) = fixture().await;
        let owner = PrincipalId::for_test("owner");
        let saved = service
            .save_text(&owner, "private.md", "private", None, Some("original"))
            .await
            .unwrap();
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let finished = Arc::new(tokio::sync::Notify::new());
        let task_service = service.clone();
        let task_owner = owner.clone();
        let file_id = saved.file_id.clone();
        let gate_entered = entered.clone();
        let gate_release = release.clone();
        let gate_finished = finished.clone();
        let request = tokio::spawn(async move {
            dispatch_with_final_check(
                &task_service,
                &task_owner,
                "api",
                "stash.move",
                serde_json::json!({"file_id":file_id,"folder":"moved"}),
                None,
                async move {
                    gate_entered.notify_one();
                    gate_release.notified().await;
                    gate_finished.notify_one();
                    Err::<(), _>(service_error("not_found", "authority revoked"))
                },
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        // A parallel caller cannot observe a staged folder change.
        assert_eq!(
            service
                .metadata(&owner, &saved.file_id)
                .await
                .unwrap()
                .folder,
            "original"
        );
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(5), finished.notified())
            .await
            .unwrap();
        // This writer is a barrier behind the denied transaction; renaming to
        // the same name does not repair or hide a mistakenly committed folder.
        let after = service
            .rename(&owner, &saved.file_id, &saved.display_name)
            .await
            .unwrap();
        assert_eq!(after.folder, "original");
        assert_eq!(
            service
                .read_text(&owner, &saved.uri, None)
                .await
                .unwrap()
                .content,
            "private"
        );
        runtime.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn timed_out_metadata_authority_rolls_back_and_releases_writer() {
        let (service, runtime, _temp) = fixture().await;
        let owner = PrincipalId::for_test("owner");
        let saved = service
            .save_text(&owner, "private.md", "private", None, Some("original"))
            .await
            .unwrap();
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            dispatch_with_final_check(
                &service,
                &owner,
                "api",
                "stash.move",
                serde_json::json!({"file_id":saved.file_id,"folder":"moved"}),
                None,
                std::future::pending::<Result<(), ToolError>>(),
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.kind(), "busy");
        let after = service
            .rename(&owner, &saved.file_id, &saved.display_name)
            .await
            .unwrap();
        assert_eq!(after.folder, "original");
        assert_eq!(
            service.stats(&owner).await.unwrap().owned_committed_bytes,
            7
        );
        runtime.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn folder_counts_include_active_shared_files_and_exclude_private_or_revoked_files() {
        let (service, runtime, _temp) = fixture().await;
        let owner = PrincipalId::for_test("owner");
        let stranger = PrincipalId::for_test("stranger");
        service
            .save_text(&owner, "owned.md", "owned", None, Some("repo"))
            .await
            .unwrap();
        service
            .save_text(&owner, "root.md", "root", None, None)
            .await
            .unwrap();
        let shared = service
            .save_text(&stranger, "shared.md", "shared", None, Some("repo"))
            .await
            .unwrap();
        service
            .create_grant_validated(&stranger, &shared.file_id, &owner)
            .await
            .unwrap();
        service
            .save_text(&stranger, "private.md", "private", None, Some("secret"))
            .await
            .unwrap();
        let revoked = service
            .save_text(&stranger, "revoked.md", "revoked", None, Some("hidden"))
            .await
            .unwrap();
        let grant = service
            .create_grant_validated(&stranger, &revoked.file_id, &owner)
            .await
            .unwrap();
        service
            .revoke_grant(&stranger, &revoked.file_id, &grant.grant_id)
            .await
            .unwrap();
        let first = service.folders(&owner, None, Some(1)).await.unwrap();
        assert_eq!(first.folders.len(), 1);
        assert_eq!(first.folders[0].folder, "");
        assert_eq!(first.folders[0].file_count, 1);
        assert_eq!(first.next_cursor.as_deref(), Some(""));
        let second = service
            .folders(&owner, first.next_cursor.as_deref(), Some(1))
            .await
            .unwrap();
        assert_eq!(second.folders.len(), 1);
        assert_eq!(second.folders[0].folder, "repo");
        assert_eq!(second.folders[0].file_count, 2);
        assert!(second.next_cursor.is_none());
        runtime.shutdown().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn rejected_final_authority_removes_new_document_and_releases_quota() {
        let (service, runtime, _temp) = fixture().await;
        let owner = PrincipalId::for_test("owner");
        let result = dispatch_with_final_check(
            &service,
            &owner,
            "mcp",
            "stash.save_text",
            serde_json::json!({"filename":"private.md","content":"private","folder":"labby"}),
            None,
            async { Err::<(), _>(service_error("not_found", "authority revoked")) },
        )
        .await;
        assert_eq!(result.unwrap_err().kind(), "not_found");
        assert_eq!(
            service.stats(&owner).await.unwrap().owned_committed_bytes,
            0
        );
        assert!(
            service
                .list(&owner, None, None)
                .await
                .unwrap()
                .files
                .is_empty()
        );
        assert_eq!(
            service
                .save_text(
                    &owner,
                    "large.md",
                    &"x".repeat(MAX_DOCUMENT_BYTES + 1),
                    None,
                    None
                )
                .await
                .unwrap_err()
                .kind(),
            "quota_exceeded"
        );
        assert_eq!(
            service
                .save_text(&owner, "a.md", "a", Some("html"), None)
                .await
                .unwrap_err()
                .kind(),
            "invalid_param"
        );
        let (reservation, admission) = service
            .reserve_upload(&owner, "binary.md", 2)
            .await
            .unwrap();
        let id = service
            .finalize_upload(
                reservation,
                admission,
                &[0xff, 0x00][..],
                CancellationToken::new(),
            )
            .await
            .unwrap();
        let binary = service.metadata(&owner, &id).await.unwrap();
        assert_eq!(binary.content_type, "application/octet-stream");
        assert_eq!(
            service
                .read_text(&owner, &binary.uri, None)
                .await
                .unwrap_err()
                .kind(),
            "invalid_param"
        );
        runtime.shutdown().await;
    }
}
