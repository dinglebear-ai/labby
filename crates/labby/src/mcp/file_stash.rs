use std::sync::Arc;

use axum::http::request::Parts;
use base64::Engine as _;
use rmcp::model::{ReadResourceResult, Resource, ResourceContents, ResourceTemplate};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use tokio::io::AsyncReadExt;

use crate::dispatch::error::ToolError;
use crate::dispatch::file_stash::{FileStashService, FileView, parse_stash_uri};
use crate::file_stash::PrincipalId;
use crate::mcp::context::{
    auth_context_from_extensions, propagated_caller_auth, resolve_caller_authorization,
};
use crate::mcp::server::LabMcpServer;

pub(crate) const TEMPLATE_URI: &str = "stash://me/files/{file_id}";
const PRIVATE_IN_PROCESS_TRANSPORT: &str = "in-process";

impl LabMcpServer {
    pub(crate) fn file_stash_caller_bound(&self) -> bool {
        self.registry.dispatch_capability("stash")
            == Some(crate::registry::DispatchCapability::CallerBound)
    }

    pub(crate) async fn dispatch_caller_bound_service(
        &self,
        service: &str,
        action: &str,
        params: serde_json::Value,
        context: &RequestContext<RoleServer>,
        meta: Option<&rmcp::model::RequestMetaObject>,
    ) -> Result<serde_json::Value, ToolError> {
        match service {
            "stash" => {
                let principal = self.file_stash_principal(context, meta).await?;
                crate::dispatch::file_stash::dispatch_for_principal(
                    &self.file_stash_service(),
                    &principal,
                    "mcp",
                    action,
                    params,
                )
                .await
            }
            _ => Err(ToolError::Sdk {
                sdk_kind: "service_unavailable".to_owned(),
                message: "caller-bound service adapter is unavailable".to_owned(),
            }),
        }
    }

    pub(crate) fn file_stash_service(&self) -> FileStashService {
        let page_limit = self.file_stash_runtime.page_limit();
        let max_query_bytes = self.file_stash_runtime.max_query_bytes();
        FileStashService::new(
            Arc::clone(&self.file_stash_runtime),
            Arc::clone(&self.access_runtime),
            page_limit,
            max_query_bytes,
        )
    }

    pub(crate) async fn file_stash_principal(
        &self,
        context: &RequestContext<RoleServer>,
        meta: Option<&rmcp::model::RequestMetaObject>,
    ) -> Result<PrincipalId, ToolError> {
        let caller = resolve_caller_authorization(
            auth_context_from_extensions(&context.extensions),
            self.absent_auth_trust(),
            propagated_caller_auth(meta),
        );
        if !caller.can_read() {
            return Err(forbidden());
        }
        if let Some(parts) = context.extensions.get::<Parts>()
            && let Some(identity) = parts.extensions.get::<labby_auth::VerifiedIdentity>()
        {
            return self
                .access_runtime
                .resolve_file_stash_principal(identity.clone())
                .await
                .map_err(|_| forbidden());
        }
        // Serialized principal IDs are trusted on only the private in-process
        // peer. Network and stdio routes must resolve a VerifiedIdentity.
        if let Some(principal) =
            propagated_file_stash_principal(self.transport_label, propagated_caller_auth(meta))
        {
            return self
                .access_runtime
                .lease_active_file_stash_principal(principal.clone())
                .await
                .map(|_| principal)
                .map_err(|_| forbidden());
        }
        Err(forbidden())
    }

    pub(crate) async fn file_stash_resources(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Vec<Resource> {
        if !self.file_stash_caller_bound()
            || !self.route_scope.allows_service("stash")
            || !self.service_visible_on_mcp("stash").await
        {
            return Vec::new();
        }
        let Ok(principal) = self
            .file_stash_principal(context, Some(&context.meta))
            .await
        else {
            return Vec::new();
        };
        collect_file_stash_resources(
            &self.file_stash_service(),
            &principal,
            self.file_stash_runtime.page_limit(),
        )
        .await
        .unwrap_or_default()
    }

    pub(crate) async fn read_file_stash_resource(
        &self,
        uri: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
        let file_id = parse_stash_uri(uri).map_err(|_| unknown(uri))?;
        let principal = self
            .file_stash_principal(context, Some(&context.meta))
            .await
            .map_err(|_| unknown(uri))?;
        let (_metadata, mut blob) = self
            .file_stash_service()
            .open_download(&principal, &file_id, true)
            .await
            .map_err(|error| match error.kind() {
                "quota_exceeded" => quota_exceeded(uri),
                "not_found" => unknown(uri),
                "busy" => busy(uri),
                _ => unavailable(uri),
            })?;
        let capacity = usize::try_from(blob.size).map_err(|_| quota_exceeded(uri))?;
        let mut bytes = Vec::with_capacity(capacity);
        (&mut blob.file)
            .take(blob.size.saturating_add(1))
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| unavailable(uri))?;
        if bytes.len() != capacity {
            return Err(unavailable(uri));
        }
        let contents = ResourceContents::blob(
            base64::engine::general_purpose::STANDARD.encode(bytes),
            uri.to_owned(),
        )
        .with_mime_type("application/octet-stream");
        Ok(ReadResourceResult::new(vec![contents]).into())
    }
}

async fn collect_file_stash_resources(
    service: &FileStashService,
    principal: &PrincipalId,
    page_limit: usize,
) -> Result<Vec<Resource>, ToolError> {
    let mut cursor = None;
    let mut files = Vec::new();
    while files.len() < crate::mcp::pagination::MCP_RETAINED_LIST_ITEM_CAP {
        let remaining = crate::mcp::pagination::MCP_RETAINED_LIST_ITEM_CAP - files.len();
        let limit = remaining.min(page_limit);
        let page = service
            .list(principal, cursor.as_deref(), Some(limit))
            .await?;
        files.extend(page.files);
        let Some(next) = page.next_cursor else { break };
        if cursor.as_deref() == Some(next.as_str()) {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_cursor".to_owned(),
                message: "File Stash returned a non-advancing cursor".to_owned(),
            });
        }
        cursor = Some(next);
    }
    Ok(files.into_iter().map(resource_for_file).collect())
}

fn propagated_file_stash_principal(
    transport: &str,
    auth: Option<labby_runtime::caller_auth::PropagatedCallerAuth>,
) -> Option<PrincipalId> {
    (transport == PRIVATE_IN_PROCESS_TRANSPORT)
        .then_some(auth?)?
        .access_principal_id
        .and_then(PrincipalId::from_propagated)
}

fn resource_for_file(file: FileView) -> Resource {
    Resource::new(file.uri, file.display_name)
        .with_description(if file.owned {
            "File Stash file owned by the caller"
        } else {
            "File Stash file shared with the caller"
        })
        .with_mime_type("application/octet-stream")
        .with_size(file.size_bytes)
}

pub(crate) fn template() -> ResourceTemplate {
    ResourceTemplate::new(TEMPLATE_URI, "stash/file")
        .with_description("Caller-authorized File Stash object by opaque ULID")
        .with_mime_type("application/octet-stream")
}

fn forbidden() -> ToolError {
    ToolError::Forbidden {
        message: "File Stash requires a verified caller identity".to_owned(),
        required_scopes: vec![
            "lab:read".to_owned(),
            "lab".to_owned(),
            "lab:admin".to_owned(),
        ],
    }
}

fn unknown(uri: &str) -> ErrorData {
    ErrorData::resource_not_found(
        "File Stash resource is unavailable",
        Some(serde_json::json!({"uri": uri})),
    )
}

fn unavailable(uri: &str) -> ErrorData {
    ErrorData::internal_error(
        "File Stash resource could not be read",
        Some(serde_json::json!({"uri": uri, "kind": "service_unavailable"})),
    )
}

fn busy(uri: &str) -> ErrorData {
    ErrorData::internal_error(
        "File Stash resource capacity is busy",
        Some(serde_json::json!({"uri": uri, "kind": "busy"})),
    )
}

fn quota_exceeded(uri: &str) -> ErrorData {
    ErrorData::invalid_request(
        "File Stash resource exceeds the MCP read limit",
        Some(serde_json::json!({"uri": uri, "kind": "quota_exceeded"})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::caller_auth::PropagatedCallerAuth;

    #[test]
    fn propagated_principal_is_honored_only_on_private_in_process_transport() {
        let propagated = PropagatedCallerAuth::scoped(vec!["lab:read".into()], Some("sub".into()))
            .with_access_principal_id("principal-1".into());
        assert_eq!(
            propagated_file_stash_principal("in-process", Some(propagated.clone()))
                .map(|value| value.as_str().to_owned()),
            Some("principal-1".into())
        );
        assert!(propagated_file_stash_principal("http", Some(propagated.clone())).is_none());
        assert!(propagated_file_stash_principal("stdio", Some(propagated)).is_none());
        assert!(propagated_file_stash_principal("in-process", None).is_none());
    }

    #[test]
    fn malformed_and_noncanonical_stash_uris_are_rejected_before_dispatch() {
        for uri in [
            "stash://me/files/not-an-id",
            "stash://me/files/01arz3ndektsv4rrffq69g5fav",
            "stash://other/files/01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV/extra",
        ] {
            assert!(parse_stash_uri(uri).is_err(), "{uri}");
        }
    }

    #[test]
    fn oversized_resource_preserves_quota_exceeded_kind() {
        let error = quota_exceeded("stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(
            error.data.as_ref().and_then(|data| data["kind"].as_str()),
            Some("quota_exceeded")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[tokio::test]
    async fn resource_snapshot_walks_beyond_the_service_page_limit() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("secure temporary directory");
        let mut preferences = crate::config::FileStashPreferences::default();
        preferences.page_size = 2;
        let runtime = Arc::new(
            crate::file_stash::FileStashRuntime::initialize_with_preferences(
                directory.path().join("stash"),
                preferences,
            )
            .await,
        );
        let store = runtime.store().await.expect("ready stash store");
        for index in 0..3 {
            let reservation = store
                .reserve_upload(
                    "principal-1".into(),
                    format!("file-{index}"),
                    format!("file-{index}"),
                    0,
                    i64::MAX,
                    u64::MAX,
                    u64::MAX,
                    10,
                )
                .await
                .expect("reserve");
            store
                .mark_blob_published(reservation.upload_id.clone())
                .await
                .expect("publish");
            store
                .commit_upload(reservation.upload_id)
                .await
                .expect("commit");
        }
        let service = FileStashService::new(
            Arc::clone(&runtime),
            Arc::new(crate::access::AccessRuntime::blocked_unavailable()),
            2,
            64,
        );
        let principal = PrincipalId::from_propagated("principal-1".into()).expect("principal");
        let resources = collect_file_stash_resources(&service, &principal, 2)
            .await
            .expect("resources");
        assert_eq!(resources.len(), 3, "must not freeze the first service page");
        runtime.shutdown().await;
    }
}
