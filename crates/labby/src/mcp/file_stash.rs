use std::sync::Arc;

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
    verified_identity_from_extensions,
};
use crate::mcp::server::LabMcpServer;

pub(crate) const TEMPLATE_URI: &str = "stash://me/files/{file_id}";
/// Request-scoped Stash owner selection. This value chooses a scope; it grants
/// no authority and is always re-evaluated against the verified caller.
pub(crate) const OWNER_META_KEY: &str = "ai.dinglebear.labby/stashOwner";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StashOwnerSelection {
    kind: String,
    #[serde(default)]
    id: Option<String>,
}

impl LabMcpServer {
    fn route_stash_owner<'a>(
        &'a self,
        kind: Option<&'a str>,
        id: Option<&'a str>,
    ) -> Result<(Option<&'a str>, Option<&'a str>), ToolError> {
        let Some(team) = self.route_scope.bound_team_id() else {
            return Ok((kind, id));
        };
        if kind.is_some_and(|kind| kind != "team") || id.is_some_and(|id| id != team) {
            return Err(ToolError::InvalidParam {
                param: "owner_id".into(),
                message: "File Stash owner does not match the route's Team".into(),
            });
        }
        Ok((Some("team"), Some(team)))
    }

    pub(crate) fn file_stash_caller_bound(&self) -> bool {
        self.registry.dispatch_capability("stash")
            == Some(crate::registry::DispatchCapability::CallerBound)
    }

    pub(super) async fn file_stash_resources_visible(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> bool {
        #[cfg(feature = "gateway")]
        if !matches!(
            super::bound_access::project_execution_binding(
                &context.extensions,
                std::time::SystemTime::now()
            ),
            super::bound_access::ProjectExecutionBinding::Legacy
        ) {
            return false;
        }
        #[cfg(not(feature = "gateway"))]
        let _ = context;
        self.file_stash_caller_bound()
            && self.route_scope.exposes_resources()
            && self.service_visible_on_mcp("stash").await
            && self
                .action_allowed_on_mcp("stash", "stash.resources.read")
                .await
    }

    /// Transport authority ceiling for the caller. Absent auth is trusted as
    /// local operator authority only on transports that genuinely imply local
    /// stdio (see [`LabMcpServer::absent_auth_trust`]); everywhere else an
    /// absent context yields no ceiling and the caller is denied.
    pub(crate) fn caller_ceiling(
        &self,
        auth: Option<&labby_auth::auth_context::AuthContext>,
    ) -> Option<crate::access::AuthorityCeiling> {
        match (auth, self.absent_auth_trust()) {
            (Some(auth), _) => Some(crate::access::AuthorityCeiling::from_auth_context(auth)),
            (None, crate::mcp::context::AbsentAuth::TrustedLocal) => {
                Some(crate::access::AuthorityCeiling::trusted_local())
            }
            (None, crate::mcp::context::AbsentAuth::Untrusted) => None,
        }
    }

    fn authority_ceiling(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<crate::access::AuthorityCeiling, ToolError> {
        self.caller_ceiling(auth_context_from_extensions(&context.extensions))
            .ok_or_else(forbidden)
    }

    pub(crate) async fn dispatch_caller_bound_service(
        &self,
        service: &str,
        action: &str,
        params: serde_json::Value,
        context: &RequestContext<RoleServer>,
    ) -> Result<serde_json::Value, ToolError> {
        match service {
            "agents" | "tasks" | "projects" => {
                let identity = verified_identity_from_extensions(&context.extensions)
                    .cloned()
                    .ok_or_else(forbidden)?;
                let mut ceiling = self.authority_ceiling(context)?;
                ceiling.bound_team = self.route_scope.bound_team_id().map(str::to_owned);
                // Store lifecycle failures are typed outages, never denials.
                let store = self.access_runtime.store().await.map_err(|error| {
                    crate::dispatch::access_errors::map_runtime_error(
                        match service {
                            "agents" => "agents",
                            "tasks" => "tasks",
                            _ => "projects",
                        },
                        error,
                    )
                })?;
                match service {
                    "agents" => {
                        crate::dispatch::agents::dispatch(
                            crate::dispatch::agents::AgentDispatchContext {
                                store,
                                identity,
                                ceiling,
                            },
                            action,
                            params,
                        )
                        .await
                    }
                    "tasks" => {
                        crate::dispatch::tasks::dispatch(
                            crate::dispatch::tasks::TaskDispatchContext {
                                store,
                                identity,
                                ceiling,
                            },
                            action,
                            params,
                        )
                        .await
                    }
                    _ => {
                        crate::dispatch::projects::dispatch(
                            crate::dispatch::projects::ProjectDispatchContext {
                                store,
                                identity,
                                ceiling,
                            },
                            action,
                            params,
                        )
                        .await
                    }
                }
            }
            "stash" => {
                let recipient = if action == "stash.grants.create" {
                    Some(
                        params
                            .get("grantee_principal_id")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| ToolError::InvalidParam {
                                param: "grantee_principal_id".into(),
                                message: "invalid File Stash parameter".into(),
                            })?
                            .to_owned(),
                    )
                } else {
                    None
                };
                let identity = verified_identity_from_extensions(&context.extensions)
                    .cloned()
                    .ok_or_else(forbidden)?;
                let (kind, id) = self.route_stash_owner(
                    params.get("owner_kind").and_then(serde_json::Value::as_str),
                    params.get("owner_id").and_then(serde_json::Value::as_str),
                )?;
                let principal = crate::dispatch::file_stash::authorize_owner(
                    &self.access_runtime,
                    identity.clone(),
                    self.authority_ceiling(context)?,
                    kind,
                    id,
                    action,
                )
                .await?;
                let validated_grantee = match recipient {
                    Some(recipient) => {
                        let (_owner, recipient, lease) = self
                            .access_runtime
                            .resolve_and_lease_file_stash_participants(identity, recipient)
                            .await
                            .map_err(|_| forbidden())?;
                        Some((recipient, lease))
                    }
                    None => None,
                };
                principal
                    .validate_before_commit()
                    .await
                    .map_err(crate::dispatch::file_stash::map_principal_resolution)?;
                let service = self.file_stash_service();
                if action == "stash.save_text" {
                    let action = action.to_owned();
                    return tokio::spawn(async move {
                        checked_stash_action(
                            &service,
                            principal,
                            &action,
                            params,
                            validated_grantee,
                        )
                        .await
                    })
                    .await
                    .map_err(|_| ToolError::Sdk {
                        sdk_kind: "service_unavailable".into(),
                        message: "Context save task failed; check Stash before retrying".into(),
                    })?;
                }
                checked_stash_action(&service, principal, action, params, validated_grantee).await
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
    ) -> Result<crate::access::FileStashOwnerAuthorization, ToolError> {
        let caller = resolve_caller_authorization(
            auth_context_from_extensions(&context.extensions),
            self.absent_auth_trust(),
            propagated_caller_auth(meta),
        );
        if !caller.can_read() {
            return Err(forbidden());
        }
        if let Some(identity) = verified_identity_from_extensions(&context.extensions) {
            let (kind, id) = selected_stash_owner(meta)?;
            let (kind, id) = self.route_stash_owner(kind.as_deref(), id.as_deref())?;
            return crate::dispatch::file_stash::authorize_owner(
                &self.access_runtime,
                identity.clone(),
                self.authority_ceiling(context)?,
                kind,
                id,
                "stash.resources.read",
            )
            .await;
        }
        Err(forbidden())
    }

    pub(crate) async fn file_stash_resources(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<Vec<Resource>, ErrorData> {
        let caller = resolve_caller_authorization(
            auth_context_from_extensions(&context.extensions),
            self.absent_auth_trust(),
            propagated_caller_auth(Some(&context.meta)),
        );
        // Resource listing is additive across services. A caller without the
        // Stash read scope should simply not see Stash resources; once the
        // caller is authorized, identity/storage failures remain observable.
        if !caller.can_read() {
            return Ok(Vec::new());
        }
        let Some(identity) = verified_identity_from_extensions(&context.extensions).cloned() else {
            return Ok(Vec::new());
        };
        crate::dispatch::file_stash::observe_result(
            "mcp",
            "stash.resources.list",
            None,
            None,
            None,
            false,
            async {
                if !self.file_stash_resources_visible(context).await
                    || !self.action_allowed_on_mcp("stash", "stash.list").await
                {
                    return Ok(Vec::new());
                }
                let principal = {
                    let (kind, id) = selected_stash_owner(Some(&context.meta))?;
                    let (kind, id) = self.route_stash_owner(kind.as_deref(), id.as_deref())?;
                    match crate::dispatch::file_stash::authorize_owner(
                        &self.access_runtime,
                        identity,
                        self.authority_ceiling(context)?,
                        kind,
                        id,
                        "stash.resources.read",
                    )
                    .await
                    {
                        Ok(authority) => authority,
                        // Resource listing is additive. A verified identity
                        // without durable Stash authority in the selected scope
                        // contributes no Stash resources without hiding other
                        // providers. Storage failures stay observable.
                        Err(error) if error.kind() == "not_found" => return Ok(Vec::new()),
                        Err(error) => return Err(error),
                    }
                };
                if principal.validate_before_commit().await.is_err() {
                    return Ok(Vec::new());
                }
                collect_file_stash_resources(
                    &self.file_stash_service(),
                    &principal,
                    self.file_stash_runtime.page_limit(),
                )
                .await
            },
        )
        .await
        .map_err(|error| list_error(&error))
    }

    pub(crate) async fn read_file_stash_resource(
        &self,
        uri: &str,
        context: &RequestContext<RoleServer>,
    ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
        let observed_file_id = parse_stash_uri(uri).ok();
        crate::dispatch::file_stash::observe_result(
            "mcp",
            "stash.resource.read",
            observed_file_id.as_deref(),
            None,
            None,
            false,
            async {
                let file_id = parse_stash_uri(uri)?;
                let principal = self
                    .file_stash_principal(context, Some(&context.meta))
                    .await?;
                let stash = self.file_stash_service();
                let (metadata, mut blob) = stash.open_download(&principal, &file_id, true).await?;
                principal
                    .validate_before_commit()
                    .await
                    .map_err(crate::dispatch::file_stash::map_principal_resolution)?;
                let capacity = usize::try_from(blob.size).map_err(|_| ToolError::Sdk {
                    sdk_kind: "quota_exceeded".to_owned(),
                    message: "File Stash operation failed".to_owned(),
                })?;
                let mut bytes = Vec::with_capacity(capacity);
                let read_limit = blob.size.saturating_add(1);
                (&mut blob)
                    .take(read_limit)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|_| ToolError::Sdk {
                        sdk_kind: "service_unavailable".to_owned(),
                        message: "File Stash operation failed".to_owned(),
                    })?;
                if bytes.len() != capacity {
                    return Err(ToolError::Sdk {
                        sdk_kind: "integrity_error".to_owned(),
                        message: "File Stash operation failed".to_owned(),
                    });
                }
                crate::dispatch::file_stash::capture_observation_details(
                    Some(&file_id),
                    None,
                    Some(u64::try_from(bytes.len()).unwrap_or(u64::MAX)),
                );
                let contents = if matches!(
                    metadata.content_type.as_str(),
                    "text/markdown" | "text/plain"
                ) {
                    let text = String::from_utf8(bytes).map_err(|_| ToolError::Sdk {
                        sdk_kind: "integrity_error".into(),
                        message: "Context document is not UTF-8".into(),
                    })?;
                    ResourceContents::text(text, uri.to_owned())
                        .with_mime_type(metadata.content_type)
                } else {
                    ResourceContents::blob(
                        base64::engine::general_purpose::STANDARD.encode(bytes),
                        uri.to_owned(),
                    )
                    .with_mime_type("application/octet-stream")
                };
                Ok(ReadResourceResult::new(vec![contents]).into())
            },
        )
        .await
        .map_err(|error| map_resource_read_error(&error, uri))
    }
}

/// Keep the caller and any grantee leases alive through the shared commit gate.
async fn checked_stash_action(
    service: &FileStashService,
    principal: crate::access::FileStashOwnerAuthorization,
    action: &str,
    params: serde_json::Value,
    validated_grantee: Option<(PrincipalId, crate::access::ActiveFileStashPrincipalLease)>,
) -> Result<serde_json::Value, ToolError> {
    let owner = (*principal).clone();
    let grantee = validated_grantee
        .as_ref()
        .map(|(recipient, _)| recipient.clone());
    crate::dispatch::file_stash::dispatch_with_final_check(
        service,
        &owner,
        "mcp",
        action,
        params,
        grantee.as_ref(),
        async move {
            principal
                .validate_before_commit()
                .await
                .map_err(crate::dispatch::file_stash::map_principal_resolution)?;
            Ok((principal, validated_grantee))
        },
    )
    .await
}

/// Parse the optional `_meta` owner selection into `(owner_kind, owner_id)`
/// strings. Malformed selections are rejected before any authority lookup.
fn selected_stash_owner(
    meta: Option<&rmcp::model::RequestMetaObject>,
) -> Result<(Option<String>, Option<String>), ToolError> {
    let Some(value) = meta.and_then(|meta| meta.get(OWNER_META_KEY)) else {
        return Ok((None, None));
    };
    let selection: StashOwnerSelection =
        serde_json::from_value(value.clone()).map_err(|_| ToolError::InvalidParam {
            param: OWNER_META_KEY.to_owned(),
            message: "invalid File Stash owner selection".to_owned(),
        })?;
    Ok((Some(selection.kind), selection.id))
}

fn setup_required_error(error: &ToolError, action: &str) -> ErrorData {
    let context =
        labby_runtime::agent_error::AgentErrorContext::for_service_action("stash", action);
    ErrorData::invalid_request(
        error.user_message().to_owned(),
        Some(error.to_agent_value_with_context(&context)),
    )
}

pub(super) fn list_error(error: &ToolError) -> ErrorData {
    if error.kind() == "access_setup_required" {
        return setup_required_error(error, "stash.resources.list");
    }
    ErrorData::internal_error(
        "File Stash resources could not be listed",
        Some(serde_json::json!({"kind": error.kind()})),
    )
}

fn map_resource_read_error(error: &ToolError, uri: &str) -> ErrorData {
    match error.kind() {
        "access_setup_required" => setup_required_error(error, "stash.resources.read"),
        "invalid_param" | "forbidden" | "not_found" => unknown(uri),
        "quota_exceeded" => quota_exceeded(uri),
        "busy" => busy(uri),
        _ => unavailable(uri),
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

fn resource_for_file(file: FileView) -> Resource {
    let name = if file.folder.is_empty() {
        file.display_name
    } else {
        format!("{}/{}", file.folder, file.display_name)
    };
    super::native_metadata::native_resource(
        Resource::new(file.uri, name)
            .with_description(if file.owned {
                "File Stash file owned by the caller"
            } else {
                "File Stash file shared with the caller"
            })
            .with_mime_type(file.content_type)
            .with_size(file.size_bytes),
    )
}

pub(crate) fn template() -> ResourceTemplate {
    // The template covers both saved text and binary uploads. Each concrete
    // resource and read response supplies its own MIME type.
    super::native_metadata::native_resource_template(
        ResourceTemplate::new(TEMPLATE_URI, "stash/file")
            .with_description("Caller-authorized File Stash object by opaque ULID"),
    )
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

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn saved_context_reads_as_native_text_for_another_authenticated_session() {
        assert!(
            serde_json::to_value(template())
                .unwrap()
                .get("mimeType")
                .is_none()
        );
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let access = Arc::new(
            crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
        );
        let identity = labby_auth::VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            "https://accounts.google.com",
            "owner",
        )
        .unwrap();
        access
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(identity.clone(), "Local", "Default")
                    .unwrap(),
            )
            .await
            .unwrap();
        let principal = access
            .resolve_file_stash_principal(identity.clone())
            .await
            .unwrap();
        let runtime = Arc::new(
            crate::file_stash::FileStashRuntime::initialize(directory.path().join("stash")).await,
        );
        let server = LabMcpServer {
            registry: Arc::new(crate::registry::build_default_registry()),
            access_runtime: access,
            installation_id: None,
            file_stash_runtime: Arc::clone(&runtime),
            #[cfg(feature = "gateway")]
            gateway_manager: None,
            peers: Default::default(),
            code_mode_app_state: Default::default(),
            last_listed_tool_contract: Default::default(),
            route_runtime: Default::default(),
            #[cfg(feature = "gateway")]
            client_registry: Default::default(),
            transport_label: "http",
            route_scope: crate::mcp::route_scope::McpRouteScope::Root,
            relay_session_id: 0,
            code_mode_widget_callbacks_enabled_for_test: false,
        };
        let (transport, _client) = tokio::io::duplex(64);
        let running = rmcp::service::serve_directly::<RoleServer, _, _, std::io::Error, _>(
            server, transport, None,
        );
        let context_for = |request_id, identity: labby_auth::VerifiedIdentity| {
            let mut context = RequestContext::new(
                rmcp::model::NumberOrString::Number(request_id),
                running.peer().clone(),
            );
            let (mut parts, _) = axum::http::Request::new(()).into_parts();
            parts.extensions.insert(identity);
            parts
                .extensions
                .insert(labby_auth::auth_context::AuthContext {
                    sub: "owner".into(),
                    issuer: "https://accounts.google.com".into(),
                    scopes: vec!["lab".into()],
                    actor_key: None,
                    email: None,
                    via_session: false,
                    csrf_token: None,
                });
            context.extensions.insert(parts);
            context
        };
        let content = "# Handoff\nExact café 🦀 context\n";
        let writer = context_for(1, identity.clone());
        let saved = running.service().dispatch_caller_bound_service("stash", "stash.save_text", serde_json::json!({"filename":"handoff.md","content":content,"folder":"dinglebear-ai/labby"}), &writer).await.unwrap();
        let uri = saved["uri"].as_str().unwrap();
        #[cfg(feature = "gateway")]
        {
            let mut unavailable = context_for(4, identity.clone());
            assert!(
                running
                    .service()
                    .file_stash_resources_visible(&unavailable)
                    .await
            );
            unavailable
                .extensions
                .get_mut::<axum::http::request::Parts>()
                .unwrap()
                .extensions
                .insert(crate::mcp::bound_access::ProjectAccessObservation::Unavailable);
            assert_eq!(
                running
                    .service()
                    .file_stash_resources(&unavailable)
                    .await
                    .unwrap(),
                []
            );
            let denied = running
                .service()
                .read_resource_impl(
                    rmcp::model::ReadResourceRequestParams::new(uri),
                    unavailable,
                )
                .await
                .unwrap_err();
            assert!(!serde_json::to_string(&denied).unwrap().contains(content));
            assert_eq!(denied.code, rmcp::model::ErrorCode::RESOURCE_NOT_FOUND);
        }
        let reader = context_for(2, identity);
        let result = running
            .service()
            .read_file_stash_resource(uri, &reader)
            .await
            .unwrap();
        let rmcp::model::ReadResourceResponse::Complete(result) = result else {
            panic!("expected complete resource")
        };
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["contents"][0]["text"], content);
        assert_eq!(json["contents"][0]["mimeType"], "text/markdown");
        assert!(json["contents"][0].get("blob").is_none());
        let service = running.service().file_stash_service();
        let resources = collect_file_stash_resources(&service, &principal, 50)
            .await
            .unwrap();
        let descriptor = serde_json::to_value(&resources[0]).unwrap();
        assert_eq!(descriptor["name"], "dinglebear-ai/labby/handoff.md");
        assert_eq!(descriptor["uri"], uri);
        assert_eq!(descriptor["mimeType"], "text/markdown");

        let (reservation, admission) = service
            .reserve_upload(&principal, "binary.md", 2)
            .await
            .unwrap();
        let binary_id = service
            .finalize_upload(
                reservation,
                admission,
                &b"\xff\x00"[..],
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
        let binary_uri = format!("stash://me/files/{binary_id}");
        let result = running
            .service()
            .read_file_stash_resource(&binary_uri, &reader)
            .await
            .unwrap();
        let rmcp::model::ReadResourceResponse::Complete(result) = result else {
            panic!("expected complete resource")
        };
        let json = serde_json::to_value(result).unwrap();
        assert_eq!(json["contents"][0]["blob"], "/wA=");
        assert_eq!(json["contents"][0]["mimeType"], "application/octet-stream");
        let stranger = context_for(
            3,
            labby_auth::VerifiedIdentity::external(
                labby_auth::Authenticator::BrowserSession,
                "https://accounts.google.com",
                "stranger",
            )
            .unwrap(),
        );
        assert_eq!(
            running
                .service()
                .read_file_stash_resource(uri, &stranger)
                .await
                .unwrap_err()
                .code,
            rmcp::model::ErrorCode::RESOURCE_NOT_FOUND
        );
        running.cancel().await.unwrap();
        runtime.shutdown().await;
    }

    #[test]
    fn coco_stash_setup_gate_keeps_actionable_error_on_list_and_read() {
        use crate::access::{
            AccessRuntimeError, AccessSetupReason, FileStashPrincipalResolutionError,
        };
        for reason in [
            AccessSetupReason::Missing,
            AccessSetupReason::Uninitialized,
            AccessSetupReason::ProofPending,
        ] {
            let error = crate::dispatch::file_stash::map_principal_resolution(
                FileStashPrincipalResolutionError::Runtime(AccessRuntimeError::SetupRequired(
                    reason,
                )),
            );
            assert_eq!(error.kind(), "access_setup_required");
            for response in [
                list_error(&error),
                map_resource_read_error(&error, "stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            ] {
                let data = response.data.expect("actionable setup error");
                assert_eq!(data["kind"], "access_setup_required");
                assert_eq!(data["contract_version"], 1);
                assert!(data["recovery"].is_object());
                assert!(
                    response.message.contains("setup") || response.message.contains("bootstrap")
                );
            }
        }
    }

    #[test]
    fn coco_stash_blocked_stores_remain_outages_not_setup_or_empty_catalogs() {
        use crate::access::{
            AccessBlockedReason, AccessRuntimeError, FileStashPrincipalResolutionError,
        };
        for reason in [
            AccessBlockedReason::Corrupt,
            AccessBlockedReason::Insecure,
            AccessBlockedReason::NewerSchema,
            AccessBlockedReason::Locked,
            AccessBlockedReason::ReadOnly,
            AccessBlockedReason::Unavailable,
        ] {
            let error = crate::dispatch::file_stash::map_principal_resolution(
                FileStashPrincipalResolutionError::Runtime(AccessRuntimeError::Blocked(reason)),
            );
            assert_eq!(error.kind(), "service_unavailable");
            assert_eq!(
                list_error(&error).data.unwrap()["kind"],
                "service_unavailable"
            );
            assert_eq!(
                map_resource_read_error(&error, "stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV")
                    .data
                    .unwrap()["kind"],
                "service_unavailable"
            );
        }
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

    #[test]
    fn resource_read_validation_auth_and_absence_are_non_enumerating() {
        let uri = "stash://me/files/01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let errors = [
            ToolError::InvalidParam {
                param: "uri".to_owned(),
                message: "invalid File Stash parameter".to_owned(),
            },
            forbidden(),
            ToolError::Sdk {
                sdk_kind: "not_found".to_owned(),
                message: "File Stash operation failed".to_owned(),
            },
        ];
        for error in errors {
            let response = map_resource_read_error(&error, uri);
            assert_eq!(response.code, rmcp::model::ErrorCode::RESOURCE_NOT_FOUND);
            assert_eq!(response.message, "File Stash resource is unavailable");
            assert_eq!(
                response.data.as_ref().and_then(|data| data["uri"].as_str()),
                Some(uri)
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn resource_snapshot_walks_beyond_the_service_page_limit() {
        use std::os::unix::fs::PermissionsExt as _;
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
            .expect("secure temporary directory");
        let preferences = crate::config::FileStashPreferences {
            page_size: 2,
            ..crate::config::FileStashPreferences::default()
        };
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
        let principal = PrincipalId::for_test("principal-1");
        let resources = collect_file_stash_resources(&service, &principal, 2)
            .await
            .expect("resources");
        assert_eq!(resources.len(), 3, "must not freeze the first service page");
        let page = rmcp::model::ListResourcesResult::with_all_items(resources);
        assert!(!serde_json::to_string(&page).unwrap().contains("data:image"));
        runtime.shutdown().await;
    }
}
