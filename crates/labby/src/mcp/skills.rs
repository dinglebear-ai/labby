//! Native MCP adapter for Agent Skills (SEP-2640).
//!
//! Canonical list/get/read semantics live in crate::skills::facade. This module
//! owns only MCP authorization, JSON-RPC shapes, request-context projection,
//! and native extension observability.

pub(crate) use crate::skills::is_skill_uri;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use labby_runtime::error::ToolError;
use labby_runtime::skills::wire::{
    CACHE_SCOPE_PRIVATE, SKILLS_GET_METHOD, SkillsGetParams, SkillsGetResult, SkillsListResult,
};
use rmcp::RoleServer;
use rmcp::model::{CustomRequest, CustomResult, ErrorData};
use rmcp::service::RequestContext;

use crate::mcp::context::{
    auth_context_from_extensions, code_mode_read_scope_allowed, propagated_caller_auth,
    propagated_caller_upstream_scope,
};
use crate::mcp::server::LabMcpServer;
use crate::skills::aggregate::ToolAccess;
use crate::skills::facade::{
    SkillCallerScope, SkillRegistryContext, get_visible_skill, list_visible_skills,
};

fn optional_header_str<'a>(
    headers: &'a axum::http::HeaderMap,
    name: &'static str,
) -> Result<Option<&'a str>, ToolError> {
    headers
        .get(name)
        .map(|value| {
            value.to_str().map_err(|_| ToolError::InvalidParam {
                message: "Skill Library request header is invalid".to_owned(),
                param: name.to_owned(),
            })
        })
        .transpose()
}

pub(crate) async fn dispatch_at_in_process_boundary(
    registry: &SkillRegistryContext,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    crate::dispatch::skills::dispatch_with_context(registry, action, params).await
}

#[cfg(feature = "skills")]
fn parse_public_import_params(
    params: serde_json::Value,
) -> Result<crate::dispatch::skill_library::params::ImportParams, ToolError> {
    serde_json::from_value(params).map_err(|_| ToolError::InvalidParam {
        message: "Skill Library import parameters are invalid".to_owned(),
        param: "params".to_owned(),
    })
}

#[cfg(feature = "skills")]
async fn dispatch_public_import<F, Fut>(
    params: serde_json::Value,
    execute: F,
) -> Result<serde_json::Value, ToolError>
where
    F: FnOnce(crate::dispatch::skill_library::params::ImportParams) -> Fut,
    Fut: Future<Output = Result<serde_json::Value, ToolError>>,
{
    execute(parse_public_import_params(params)?).await
}

#[cfg(feature = "gateway")]
fn private_artifact_access_for_in_process_meta(
    transport_label: &str,
    meta: Option<&rmcp::model::RequestMetaObject>,
) -> Result<Option<crate::skills::facade::ArtifactAccessSnapshot>, ToolError> {
    if transport_label != crate::mcp::in_process_peer::IN_PROCESS_TRANSPORT_LABEL {
        return Ok(None);
    }
    let Some(auth) = propagated_caller_auth(meta) else {
        return Ok(None);
    };
    let Some(token) = auth.private_context_token.as_deref() else {
        return Ok(None);
    };
    private_artifact_context(token, auth.sub.as_deref()).map(Some)
}

#[cfg(feature = "gateway")]
fn attach_private_artifact_context(
    registry: SkillRegistryContext,
    transport_label: &str,
    meta: Option<&rmcp::model::RequestMetaObject>,
) -> Result<SkillRegistryContext, ToolError> {
    Ok(
        match private_artifact_access_for_in_process_meta(transport_label, meta)? {
            Some(access) => registry.with_artifact_access(access),
            None => registry,
        },
    )
}

impl LabMcpServer {
    pub(crate) async fn artifact_access_for_request(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<Option<crate::skills::facade::ArtifactAccessSnapshot>, ToolError> {
        let Some(parts) = context.extensions.get::<axum::http::request::Parts>() else {
            return Ok(None);
        };
        let (Some(identity), Some(auth), Some(project_header)) = (
            parts.extensions.get::<labby_auth::VerifiedIdentity>(),
            parts
                .extensions
                .get::<labby_auth::auth_context::AuthContext>(),
            parts.headers.get("x-labby-project-id"),
        ) else {
            return Ok(None);
        };
        let project_id = project_header
            .to_str()
            .map_err(|_| ToolError::InvalidParam {
                message: "Skill Library project context is invalid".to_owned(),
                param: "x-labby-project-id".to_owned(),
            })?;
        let caller = crate::dispatch::skill_library::auth::SkillLibraryCaller::new(
            identity.clone(),
            auth.scopes.clone(),
            crate::dispatch::skill_library::auth::SkillLibraryTransport::bearer(
                crate::dispatch::skill_library::auth::SkillLibrarySurface::Mcp,
                true,
            ),
        );
        let request_id =
            optional_header_str(&parts.headers, "x-request-id")?.unwrap_or("mcp-skills-read");
        let correlation =
            crate::dispatch::skill_library::audit::SkillLibraryCorrelationId::parse(request_id)
                .map_err(|_| ToolError::InvalidParam {
                    message: "Skill Library request correlation is invalid".to_owned(),
                    param: "x-request-id".to_owned(),
                })?;
        let decision = crate::dispatch::skill_library::auth::authorize_at_boundary(
            &self.access_runtime,
            caller,
            project_id,
            crate::dispatch::skill_library::auth::SkillLibraryAction::List,
            &crate::dispatch::skill_library::audit::CanonicalArtifactId::parse("library").map_err(
                |_| ToolError::Sdk {
                    sdk_kind: "internal_error".to_owned(),
                    message: "Skill Library authorization request is invalid".to_owned(),
                },
            )?,
            crate::dispatch::skill_library::auth::SkillLibraryTarget::SharedActive,
            &correlation,
        )
        .await
        .map_err(|error| {
            crate::dispatch::skill_library::map_dispatch_error(
                crate::dispatch::skill_library::dispatch::SkillLibraryDispatchError::Authorization(
                    error,
                ),
            )
        })?;
        Ok(Some(decision.artifact_access_snapshot()))
    }

    #[cfg(feature = "skills")]
    async fn dispatch_skill_library_management(
        &self,
        context: &RequestContext<RoleServer>,
        action: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        let service =
            crate::dispatch::skill_library::process_service().ok_or_else(|| ToolError::Sdk {
                sdk_kind: "service_unavailable".to_owned(),
                message: "Skill Library is unavailable".to_owned(),
            })?;
        let parts = context
            .extensions
            .get::<axum::http::request::Parts>()
            .ok_or_else(|| ToolError::Forbidden {
                message: "Skill Library requires an authenticated transport context".to_owned(),
                required_scopes: Vec::new(),
            })?;
        let boundary = super::call_tool::skill_library_callback_boundary(parts)?;
        let project_id =
            optional_header_str(&parts.headers, "x-labby-project-id")?.ok_or_else(|| {
                ToolError::Forbidden {
                    message: "Skill Library project context is required".to_owned(),
                    required_scopes: Vec::new(),
                }
            })?;
        let request_id = optional_header_str(&parts.headers, "x-request-id")?;
        let correlation = super::call_tool::skill_library_callback_correlation(request_id)?;
        let caller = crate::dispatch::skill_library::auth::SkillLibraryCaller::new(
            boundary.identity,
            boundary.scopes,
            crate::dispatch::skill_library::auth::SkillLibraryTransport::app_callback(true, true),
        );
        if action == "skill_library.import" {
            let imports = crate::dispatch::skill_library::process_imports().ok_or_else(|| {
                ToolError::Sdk {
                    sdk_kind: "source_unavailable".to_owned(),
                    message: "Skill import sources are not configured".to_owned(),
                }
            })?;
            return dispatch_public_import(params, |import_params| async move {
                imports
                    .import_selected(
                        &service,
                        &self.access_runtime,
                        caller,
                        project_id,
                        import_params.source,
                        import_params.expected_library_version,
                        import_params.idempotency_key,
                        &correlation,
                    )
                    .await
                    .map_err(crate::dispatch::skill_library::map_import_error)
            })
            .await;
        }
        service
            .dispatch(
                &self.access_runtime,
                caller,
                project_id,
                action,
                params,
                &correlation,
            )
            .await
            .map_err(crate::dispatch::skill_library::map_dispatch_error)
    }

    /// Project MCP route/auth state into the transport-neutral Skills context.
    pub(crate) async fn skill_registry_context(
        &self,
        context: &RequestContext<RoleServer>,
    ) -> Result<SkillRegistryContext, ToolError> {
        #[cfg(feature = "gateway")]
        {
            let Some(manager) = self.gateway_manager.as_ref() else {
                return Ok(SkillRegistryContext::first_party_only());
            };
            let access = if manager.code_mode_enabled().await {
                ToolAccess::CodeModeOnly
            } else {
                ToolAccess::Direct
            };
            let subject = self.request_subject(context).map(str::to_string);
            let scope = match self.route_scope.allowed_upstreams() {
                None => SkillCallerScope::root(subject, access),
                Some(allowed) => {
                    SkillCallerScope::restricted(allowed.iter().cloned(), subject, access)
                }
            };
            let registry =
                SkillRegistryContext::with_manager(std::sync::Arc::clone(manager), scope);
            return Ok(match self.artifact_access_for_request(context).await? {
                Some(access) => registry.with_artifact_access(access),
                None => registry,
            });
        }

        #[cfg(not(feature = "gateway"))]
        {
            let _ = context;
            let registry = SkillRegistryContext::first_party_only();
            Ok(match self.artifact_access_for_request(context).await? {
                Some(access) => registry.with_artifact_access(access),
                None => registry,
            })
        }
    }

    /// Reconstruct the outer caller context on Labby's private in-process
    /// Code Mode hop. Metadata is ignored everywhere else and missing pieces
    /// fail closed to first-party-only visibility.
    pub(crate) async fn skill_registry_context_for_tool(
        &self,
        context: &RequestContext<RoleServer>,
        meta: Option<&rmcp::model::RequestMetaObject>,
    ) -> Result<SkillRegistryContext, ToolError> {
        #[cfg(feature = "gateway")]
        {
            if self.gateway_manager.is_some() {
                return self.skill_registry_context(context).await;
            }
            if self.transport_label != crate::mcp::in_process_peer::IN_PROCESS_TRANSPORT_LABEL {
                return Ok(SkillRegistryContext::first_party_only());
            }
            let Some(auth) = propagated_caller_auth(meta) else {
                return Ok(SkillRegistryContext::first_party_only());
            };
            let Some(propagated_scope) = propagated_caller_upstream_scope(meta) else {
                return Ok(SkillRegistryContext::first_party_only());
            };
            let Some(manager) = labby_gateway::gateway::current_gateway_manager() else {
                return Ok(SkillRegistryContext::first_party_only());
            };
            let subject =
                if auth.trusted_local || auth.scopes.iter().any(|scope| scope == "lab:admin") {
                    Some(crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT.to_string())
                } else {
                    auth.sub.clone()
                };
            let scope = match propagated_scope.allowed_upstreams {
                None => SkillCallerScope::root(subject, ToolAccess::CodeModeOnly),
                Some(allowed) => {
                    SkillCallerScope::restricted(allowed, subject, ToolAccess::CodeModeOnly)
                }
            };
            let registry = SkillRegistryContext::with_manager(manager, scope);
            return attach_private_artifact_context(registry, &self.transport_label, meta);
        }

        #[cfg(not(feature = "gateway"))]
        {
            let _ = (context, meta);
            Ok(SkillRegistryContext::first_party_only())
        }
    }

    /// Dispatch the fixed compatibility tool behind a heap boundary.
    ///
    /// `call_tool_impl` is already a large async state machine. Returning an
    /// erased boxed future here prevents the concrete Skills list/get/read
    /// future from inflating that parent stack frame while preserving the same
    /// caller-scoped registry and authorization semantics.
    pub(crate) fn dispatch_compat_tool_boxed<'a>(
        &'a self,
        context: &'a RequestContext<RoleServer>,
        meta: Option<&'a rmcp::model::RequestMetaObject>,
        action: &'a str,
        params: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn Future<Output = Result<serde_json::Value, ToolError>> + Send + 'a>>
    {
        Box::pin(async move {
            if action.starts_with("skill_library.") {
                return self
                    .dispatch_skill_library_management(context, action, params)
                    .await;
            }
            let registry = self.skill_registry_context_for_tool(context, meta).await?;
            dispatch_at_in_process_boundary(&registry, action, params).await
        })
    }

    /// Answer native Skills extension list/get requests.
    pub(crate) async fn handle_skills_request(
        &self,
        request: &CustomRequest,
        context: &RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        let start = Instant::now();
        let action = if request.method == SKILLS_GET_METHOD {
            "skills.get"
        } else {
            "skills.list"
        };
        let subject_log = self.request_subject_log_tag(context);
        let outcome = self.dispatch_skills_request(request, context).await;

        match &outcome {
            Ok(_) => tracing::info!(
                surface = "mcp",
                service = "labby",
                action,
                subject = %subject_log,
                elapsed_ms = start.elapsed().as_millis(),
                "dispatch finish"
            ),
            Err(error) => tracing::warn!(
                surface = "mcp",
                service = "labby",
                action,
                subject = %subject_log,
                elapsed_ms = start.elapsed().as_millis(),
                kind = %error.code.0,
                "dispatch error"
            ),
        }
        outcome
    }

    async fn dispatch_skills_request(
        &self,
        request: &CustomRequest,
        context: &RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        let auth = auth_context_from_extensions(&context.extensions);
        if !code_mode_read_scope_allowed(auth) {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_REQUEST,
                "reading skills requires the lab:read scope".to_string(),
                None,
            ));
        }
        if !self.route_scope.exposes_skills() {
            if request.method == SKILLS_GET_METHOD {
                return Err(ErrorData::new(
                    rmcp::model::ErrorCode::INVALID_REQUEST,
                    "Agent Skills are disabled by this loadout; ask the operator to enable Skills and Resources for this loadout".to_string(),
                    None,
                ));
            }
            tracing::info!(
                surface = "mcp",
                service = "labby",
                action = "skills.list",
                route_scope = %self.route_scope.label(),
                "Skills catalog hidden by loadout"
            );
            return serde_json::to_value(SkillsListResult {
                result_type: Default::default(),
                skills: Vec::new(),
                next_cursor: None,
                ttl_ms: Some(0),
                cache_scope: Some(CACHE_SCOPE_PRIVATE.to_string()),
                meta: None,
            })
            .map(CustomResult::new)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None));
        }

        let registry = self
            .skill_registry_context(context)
            .await
            .map_err(skill_read_error)?;
        tracing::debug!(
            surface = "mcp",
            service = "labby",
            method = %request.method,
            skill_generation = registry.generation_id(),
            skill_generation_digest = registry.generation_digest(),
            "captured Skill generation"
        );
        dispatch_native_with_registry(request, &registry).await
    }
}

async fn dispatch_native_with_registry(
    request: &CustomRequest,
    registry: &SkillRegistryContext,
) -> Result<CustomResult, ErrorData> {
    if request.method == SKILLS_GET_METHOD {
        let params = request
            .params_as::<SkillsGetParams>()
            .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?
            .ok_or_else(|| ErrorData::invalid_params("skills/get requires uri", None))?;
        let entry = get_visible_skill(registry, &params.uri)
            .await
            .ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("'{}' is not a skill this server serves", params.uri),
                    None,
                )
            })?;
        let result = SkillsGetResult {
            result_type: Default::default(),
            skill: entry,
        };
        return serde_json::to_value(result)
            .map(CustomResult::new)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None));
    }

    serde_json::to_value(list_visible_skills(registry).await)
        .map(CustomResult::new)
        .map_err(|error| ErrorData::internal_error(error.to_string(), None))
}

/// Preserve native resources/read wire semantics while the canonical reader
/// returns the shared ToolError contract.
pub(crate) fn skill_read_error(error: ToolError) -> ErrorData {
    let payload = serde_json::to_string(&error).unwrap_or_else(|_| error.to_string());
    match error.kind() {
        labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH
        | labby_runtime::skills::KIND_SKILL_MANIFEST_STALE => {
            ErrorData::internal_error(payload, None)
        }
        _ => ErrorData::invalid_params(payload, None),
    }
}

const PRIVATE_ARTIFACT_CONTEXT_TTL: Duration = Duration::from_mins(2);
const MAX_PRIVATE_ARTIFACT_CONTEXTS: usize = 1024;

struct PrivateArtifactContext {
    expires: Instant,
    subject: Option<String>,
    access: crate::skills::facade::ArtifactAccessSnapshot,
}

fn private_artifact_contexts() -> &'static Mutex<BTreeMap<String, PrivateArtifactContext>> {
    static CONTEXTS: OnceLock<Mutex<BTreeMap<String, PrivateArtifactContext>>> = OnceLock::new();
    CONTEXTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn mint_private_artifact_context(
    subject: Option<String>,
    access: crate::skills::facade::ArtifactAccessSnapshot,
) -> Result<String, ToolError> {
    mint_private_artifact_context_in(
        private_artifact_contexts(),
        subject,
        access,
        MAX_PRIVATE_ARTIFACT_CONTEXTS,
        PRIVATE_ARTIFACT_CONTEXT_TTL,
    )
}

fn private_context_unavailable(kind: &str, message: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: kind.to_owned(),
        message: message.to_owned(),
    }
}

fn mint_private_artifact_context_in(
    store: &Mutex<BTreeMap<String, PrivateArtifactContext>>,
    subject: Option<String>,
    access: crate::skills::facade::ArtifactAccessSnapshot,
    capacity: usize,
    ttl: Duration,
) -> Result<String, ToolError> {
    let now = Instant::now();
    let mut contexts = store.lock().map_err(|_| {
        private_context_unavailable(
            "service_unavailable",
            "private Code Mode authorization context is unavailable",
        )
    })?;
    contexts.retain(|_, context| context.expires > now);
    if contexts.len() >= capacity {
        return Err(private_context_unavailable(
            "queue_saturated",
            "private Code Mode authorization context capacity is exhausted",
        ));
    }
    let token = ulid::Ulid::new().to_string();
    contexts.insert(
        token.clone(),
        PrivateArtifactContext {
            expires: now + ttl,
            subject,
            access,
        },
    );
    Ok(token)
}

fn private_artifact_context(
    token: &str,
    subject: Option<&str>,
) -> Result<crate::skills::facade::ArtifactAccessSnapshot, ToolError> {
    private_artifact_context_in(private_artifact_contexts(), token, subject)
}

fn private_artifact_context_in(
    store: &Mutex<BTreeMap<String, PrivateArtifactContext>>,
    token: &str,
    subject: Option<&str>,
) -> Result<crate::skills::facade::ArtifactAccessSnapshot, ToolError> {
    let now = Instant::now();
    let mut contexts = store.lock().map_err(|_| {
        private_context_unavailable(
            "service_unavailable",
            "private Code Mode authorization context is unavailable",
        )
    })?;
    contexts.retain(|_, context| context.expires > now);
    let context = contexts.get(token).ok_or_else(|| ToolError::Forbidden {
        message: "private Code Mode authorization context is invalid".to_owned(),
        required_scopes: Vec::new(),
    })?;
    if context.subject.as_deref() != subject {
        return Err(ToolError::Forbidden {
            message: "private Code Mode authorization context is invalid".to_owned(),
            required_scopes: Vec::new(),
        });
    }
    Ok(context.access.clone())
}

#[cfg(test)]
mod serve_tests {
    use super::*;

    #[tokio::test]
    async fn mcp_import_rejects_acquisition_bytes_and_routes_exact_selector() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let calls = AtomicUsize::new(0);
        let raw = serde_json::json!({
            "acquisition": { "interchange": {}, "files": [] },
            "expected_library_version": 0,
            "idempotency_key": "raw-bytes"
        });
        assert!(
            dispatch_public_import(raw, |_| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::Value::Null)
            })
            .await
            .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let selector = serde_json::json!({
            "source": {
                "kind": "depot",
                "connection_id": "configured-depot",
                "artifact_id": "artifact",
                "revision_id": format!("sha256:{}", "0".repeat(64))
            },
            "expected_library_version": 0,
            "idempotency_key": "selector"
        });
        let result = dispatch_public_import(selector, |params| async {
            calls.fetch_add(1, Ordering::SeqCst);
            match params.source {
                crate::dispatch::skill_library::params::SourceSelector::Depot {
                    connection_id,
                    artifact_id,
                    revision_id,
                } => {
                    assert_eq!(connection_id, "configured-depot");
                    assert_eq!(artifact_id, "artifact");
                    assert_eq!(revision_id, format!("sha256:{}", "0".repeat(64)));
                }
                _ => panic!("MCP selector changed source family"),
            }
            Ok(serde_json::json!({"outcome": "committed"}))
        })
        .await
        .unwrap();
        assert_eq!(result["outcome"], "committed");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[cfg(feature = "gateway")]
    fn private_hop_meta(subject: &str, token: String) -> rmcp::model::RequestMetaObject {
        use labby_runtime::caller_auth::{
            CALLER_AUTH_META_KEY, CALLER_UPSTREAM_SCOPE_META_KEY, PropagatedCallerAuth,
            PropagatedCallerUpstreamScope,
        };

        let mut meta = rmcp::model::RequestMetaObject::default();
        meta.insert(
            CALLER_AUTH_META_KEY.to_owned(),
            serde_json::to_value(
                PropagatedCallerAuth::scoped(vec!["lab:read".to_owned()], Some(subject.to_owned()))
                    .with_private_context_token(token),
            )
            .unwrap(),
        );
        meta.insert(
            CALLER_UPSTREAM_SCOPE_META_KEY.to_owned(),
            serde_json::to_value(PropagatedCallerUpstreamScope::default()).unwrap(),
        );
        meta
    }

    #[cfg(feature = "gateway")]
    fn artifact_generation() -> std::sync::Arc<crate::skills::registry::FirstPartyGeneration> {
        use std::collections::BTreeMap;

        use crate::skills::local::LocalSkill;
        use crate::skills::providers::{ArtifactSkillAccess, FirstPartySkillProviders};
        use labby_runtime::artifacts::{
            LibraryActorId, LibraryOwnership, LibraryTenantId, SkillVisibility,
        };
        use labby_runtime::skills::ResourceDigest;
        use labby_runtime::skills::wire::{SkillEntry, SkillResource};

        let ownership = LibraryOwnership::canonical(
            LibraryTenantId::from_canonical_projection("bootstrap-local").unwrap(),
            LibraryActorId::from_canonical_projection("bootstrap-owner").unwrap(),
        );
        let skill = |name: &str, visibility: SkillVisibility| {
            let manifest = format!("skill://labby/{name}/SKILL.md");
            let support = format!("skill://labby/{name}/notes.md");
            let body = format!("---\nname: {name}\ndescription: artifact {name}\n---\n\nbody\n");
            let notes = format!("support-{name}\n");
            (
                LocalSkill {
                    entry: SkillEntry {
                        uri: manifest.clone(),
                        frontmatter: labby_runtime::skills::parse_skill_md_frontmatter(&body)
                            .unwrap(),
                        resources: Some(vec![
                            SkillResource {
                                uri: manifest.clone(),
                                digest: ResourceDigest::of_bytes(body.as_bytes()).to_wire(),
                                size: body.len() as u64,
                            },
                            SkillResource {
                                uri: support.clone(),
                                digest: ResourceDigest::of_bytes(notes.as_bytes()).to_wire(),
                                size: notes.len() as u64,
                            },
                        ]),
                        meta: None,
                    },
                    files: BTreeMap::from([(manifest, body), (support, notes)]),
                },
                ArtifactSkillAccess {
                    ownership: ownership.clone(),
                    visibility,
                },
            )
        };
        let providers = FirstPartySkillProviders::from_artifact_skills([
            skill("private-hop", SkillVisibility::Private),
            skill("tenant-hop", SkillVisibility::Tenant),
        ]);
        std::sync::Arc::new(crate::skills::registry::FirstPartyGeneration {
            id: 41,
            digest: "sha256:private-hop-generation".to_owned(),
            active_digest: "sha256:private-hop-active".to_owned(),
            providers,
            rejected: Vec::new(),
            bytes: 0,
            resources: 4,
            degraded: None,
        })
    }

    #[cfg(feature = "gateway")]
    async fn live_private_hop_context(
        runtime: &crate::access::AccessRuntime,
        identity: labby_auth::VerifiedIdentity,
        project_id: &str,
        subject: &str,
    ) -> Option<SkillRegistryContext> {
        use crate::dispatch::skill_library::audit::{
            CanonicalArtifactId, SkillLibraryCorrelationId,
        };
        use crate::dispatch::skill_library::auth::{
            SkillLibraryAction, SkillLibraryCaller, SkillLibrarySurface, SkillLibraryTarget,
            SkillLibraryTransport, authorize_at_boundary,
        };

        let caller = SkillLibraryCaller::new(
            identity,
            vec!["lab:read".to_owned()],
            SkillLibraryTransport::bearer(SkillLibrarySurface::Mcp, true),
        );
        let decision = authorize_at_boundary(
            runtime,
            caller,
            project_id,
            SkillLibraryAction::List,
            &CanonicalArtifactId::parse("library").unwrap(),
            SkillLibraryTarget::SharedActive,
            &SkillLibraryCorrelationId::parse(format!("private-hop-{project_id}")).unwrap(),
        )
        .await
        .ok()?;
        let token = mint_private_artifact_context(
            Some(subject.to_owned()),
            decision.artifact_access_snapshot(),
        )
        .ok()?;
        let meta = private_hop_meta(subject, token);
        let access = private_artifact_access_for_in_process_meta(
            crate::mcp::in_process_peer::IN_PROCESS_TRANSPORT_LABEL,
            Some(&meta),
        )
        .ok()??;
        Some(
            SkillRegistryContext::from_generation(artifact_generation())
                .with_artifact_access(access),
        )
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn private_code_mode_route_uses_live_access_once_and_preserves_native_compat_parity() {
        use crate::access::{AccessRuntime, AccessStore, BootstrapOwnerInput};
        use labby_auth::{Authenticator, VerifiedIdentity};
        use labby_runtime::skills::wire::{SKILLS_GET_METHOD, SkillsGetResult};

        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = directory.path().join("access.db");
        let owner_identity = VerifiedIdentity::external(
            Authenticator::OauthBearer,
            "https://accounts.google.com",
            "code-mode-subject",
        )
        .unwrap();
        let member_identity = VerifiedIdentity::external(
            Authenticator::OauthBearer,
            "https://accounts.google.com",
            "code-mode-member",
        )
        .unwrap();
        let store = AccessStore::open(path.clone()).await.unwrap();
        store
            .bootstrap_owner(
                BootstrapOwnerInput::new(owner_identity.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        store.execute_test_statement(
            "INSERT INTO organizations VALUES('foreign-company','Foreign','active',0,2,2);\
             INSERT INTO principals VALUES('code-mode-member','bootstrap-local','user','active',NULL,2,2);\
             INSERT INTO principal_links VALUES('code-mode-member-link','code-mode-member','external','https://accounts.google.com','code-mode-member',NULL,'active',1,1,2,2);\
             INSERT INTO projects VALUES('other-project','bootstrap-local','Other','active',0,2,2),('foreign-project','foreign-company','Foreign','active',0,2,2);\
             INSERT INTO project_memberships VALUES('other-membership','bootstrap-local','other-project','code-mode-member','member','active','bootstrap-owner',2,2);\
             ",
        )
        .await
        .unwrap();
        drop(store);
        let runtime = AccessRuntime::initialize(path).await;
        let store = runtime.store().await.unwrap();

        let owner = live_private_hop_context(
            &runtime,
            owner_identity,
            "bootstrap-default",
            "code-mode-subject",
        )
        .await
        .unwrap();
        assert_eq!(store.skill_library_authorization_count_for_test(), 1);
        let native = dispatch_native_with_registry(
            &CustomRequest::new(
                SKILLS_GET_METHOD,
                Some(serde_json::json!({"uri": "skill://labby/private-hop/SKILL.md"})),
            ),
            &owner,
        )
        .await
        .unwrap();
        let native: SkillsGetResult = serde_json::from_value(native.0).unwrap();
        let compat = dispatch_at_in_process_boundary(
            &owner,
            "skills.get",
            serde_json::json!({"uri": "skill://labby/private-hop/SKILL.md"}),
        )
        .await
        .unwrap();
        let expected_compat = serde_json::to_value(
            crate::dispatch::skills::types::SkillSummary::from(native.skill.clone()),
        )
        .unwrap();
        assert_eq!(compat["skill"], expected_compat);
        let support = native
            .skill
            .resources
            .as_ref()
            .unwrap()
            .iter()
            .find(|resource| resource.uri.ends_with("/notes.md"))
            .unwrap();
        let read =
            crate::mcp::handlers_resources::read_skill_resource_with_registry(&owner, &support.uri)
                .await
                .unwrap();
        assert_eq!(read.text, "support-private-hop\n");
        assert_eq!(read.digest, support.digest);

        let member = live_private_hop_context(
            &runtime,
            member_identity.clone(),
            "other-project",
            "code-mode-member",
        )
        .await
        .unwrap();
        assert_eq!(store.skill_library_authorization_count_for_test(), 2);
        assert!(
            get_visible_skill(&member, "skill://labby/private-hop/SKILL.md")
                .await
                .is_none()
        );
        assert!(
            get_visible_skill(&member, "skill://labby/tenant-hop/SKILL.md")
                .await
                .is_some()
        );

        store.execute_test_statement(
            "UPDATE project_memberships SET role='admin' WHERE membership_id='other-membership'",
        )
        .await
        .unwrap();
        let admin = live_private_hop_context(
            &runtime,
            member_identity.clone(),
            "other-project",
            "code-mode-member",
        )
        .await
        .unwrap();
        assert_eq!(store.skill_library_authorization_count_for_test(), 3);
        assert!(
            get_visible_skill(&admin, "skill://labby/private-hop/SKILL.md")
                .await
                .is_some()
        );

        store.execute_test_statement(
            "UPDATE project_memberships SET role='member' WHERE membership_id='other-membership'",
        )
        .await
        .unwrap();
        let demoted = live_private_hop_context(
            &runtime,
            member_identity.clone(),
            "other-project",
            "code-mode-member",
        )
        .await
        .unwrap();
        assert_eq!(store.skill_library_authorization_count_for_test(), 4);
        assert!(
            get_visible_skill(&demoted, "skill://labby/private-hop/SKILL.md")
                .await
                .is_none()
        );

        store.execute_test_statement(
            "UPDATE project_memberships SET status='suspended' WHERE membership_id='other-membership'",
        )
        .await
        .unwrap();
        assert!(
            live_private_hop_context(
                &runtime,
                member_identity.clone(),
                "other-project",
                "code-mode-member",
            )
            .await
            .is_none()
        );
        assert_eq!(store.skill_library_authorization_count_for_test(), 5);

        assert!(
            live_private_hop_context(
                &runtime,
                member_identity,
                "foreign-project",
                "code-mode-member",
            )
            .await
            .is_none()
        );
        assert_eq!(store.skill_library_authorization_count_for_test(), 6);
    }

    #[test]
    fn private_artifact_context_rejects_forged_and_mismatched_tokens() {
        use labby_runtime::artifacts::{LibraryActorId, LibraryTenantId};

        let access = crate::skills::facade::ArtifactAccessSnapshot::new(
            LibraryTenantId::from_canonical_projection("tenant-a").unwrap(),
            LibraryActorId::from_canonical_projection("owner").unwrap(),
            false,
        );
        let token = mint_private_artifact_context(Some("subject-a".to_owned()), access).unwrap();
        assert!(private_artifact_context("forged", Some("subject-a")).is_err());
        assert!(private_artifact_context(&token, Some("subject-b")).is_err());
        assert!(private_artifact_context(&token, Some("subject-a")).is_ok());
    }

    #[test]
    fn private_artifact_context_saturation_is_typed_and_does_not_downgrade() {
        use labby_runtime::artifacts::{LibraryActorId, LibraryTenantId};

        let access = crate::skills::facade::ArtifactAccessSnapshot::new(
            LibraryTenantId::from_canonical_projection("tenant-a").unwrap(),
            LibraryActorId::from_canonical_projection("owner").unwrap(),
            false,
        );
        let store = Mutex::new(BTreeMap::new());
        mint_private_artifact_context_in(
            &store,
            Some("subject-a".to_owned()),
            access.clone(),
            1,
            Duration::from_secs(30),
        )
        .unwrap();
        let error = mint_private_artifact_context_in(
            &store,
            Some("subject-b".to_owned()),
            access,
            1,
            Duration::from_secs(30),
        )
        .unwrap_err();
        assert_eq!(error.kind(), "queue_saturated");
    }

    #[test]
    fn private_artifact_context_expiry_is_a_redacted_denial() {
        use labby_runtime::artifacts::{LibraryActorId, LibraryTenantId};

        let access = crate::skills::facade::ArtifactAccessSnapshot::new(
            LibraryTenantId::from_canonical_projection("tenant-a").unwrap(),
            LibraryActorId::from_canonical_projection("owner").unwrap(),
            false,
        );
        let store = Mutex::new(BTreeMap::new());
        let token = mint_private_artifact_context_in(
            &store,
            Some("subject-a".to_owned()),
            access,
            1,
            Duration::ZERO,
        )
        .unwrap();
        let error = match private_artifact_context_in(&store, &token, Some("subject-a")) {
            Ok(_) => panic!("expired private context must fail"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), "forbidden");
        assert!(!error.to_string().contains(&token));
        assert!(!error.to_string().contains("subject-a"));
    }

    #[test]
    fn private_artifact_context_poison_is_typed_and_redacted() {
        use labby_runtime::artifacts::{LibraryActorId, LibraryTenantId};

        let access = crate::skills::facade::ArtifactAccessSnapshot::new(
            LibraryTenantId::from_canonical_projection("tenant-a").unwrap(),
            LibraryActorId::from_canonical_projection("owner").unwrap(),
            false,
        );
        let store = Mutex::new(BTreeMap::new());
        let _poisoned = std::panic::catch_unwind(|| {
            let _guard = store.lock().unwrap();
            panic!("poison test store");
        });
        let error = mint_private_artifact_context_in(
            &store,
            Some("secret-subject".to_owned()),
            access,
            1,
            Duration::from_secs(30),
        )
        .unwrap_err();
        assert_eq!(error.kind(), "service_unavailable");
        assert!(!error.to_string().contains("secret-subject"));
    }

    #[test]
    fn malformed_skill_library_headers_are_not_treated_as_missing() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "x-labby-project-id",
            axum::http::HeaderValue::from_bytes(b"\xff").unwrap(),
        );
        headers.insert(
            "x-request-id",
            axum::http::HeaderValue::from_bytes(b"\xfe").unwrap(),
        );

        let project_error = optional_header_str(&headers, "x-labby-project-id").unwrap_err();
        assert_eq!(project_error.kind(), "invalid_param");
        assert_eq!(project_error.extra_fields()["param"], "x-labby-project-id");
        let request_error = optional_header_str(&headers, "x-request-id").unwrap_err();
        assert_eq!(request_error.kind(), "invalid_param");
        assert_eq!(request_error.extra_fields()["param"], "x-request-id");
    }

    #[cfg(feature = "gateway")]
    #[test]
    fn private_code_mode_registry_propagates_invalid_context_instead_of_downgrading() {
        let meta = private_hop_meta("subject-a", "forged-private-context".to_owned());
        let error = match attach_private_artifact_context(
            SkillRegistryContext::from_generation(artifact_generation()),
            crate::mcp::in_process_peer::IN_PROCESS_TRANSPORT_LABEL,
            Some(&meta),
        ) {
            Ok(_) => panic!("forged private context must not yield a downgraded registry"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), "forbidden");
        assert!(!error.to_string().contains("forged-private-context"));
        assert!(!error.to_string().contains("subject-a"));
    }

    fn write_native_skill(root: &std::path::Path, version: &str) {
        let dir = root.join("native-race");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: native-race\ndescription: {version}\n---\n\n{version}\n"),
        )
        .unwrap();
        std::fs::write(dir.join("notes.md"), format!("support-{version}\n")).unwrap();
    }

    #[tokio::test]
    async fn native_list_and_get_are_pinned_during_refresh() {
        use crate::skills::registry::{FirstPartyGenerationManager, GenerationLimits};
        use labby_runtime::skills::wire::SKILLS_LIST_METHOD;

        let temp = tempfile::tempdir().unwrap();
        write_native_skill(temp.path(), "old");
        let manager = FirstPartyGenerationManager::new(
            temp.path().to_path_buf(),
            GenerationLimits::default(),
        );
        let pinned = SkillRegistryContext::from_generation(manager.generation());
        write_native_skill(temp.path(), "new");
        manager.refresh(None).unwrap();

        let listed =
            dispatch_native_with_registry(&CustomRequest::new(SKILLS_LIST_METHOD, None), &pinned)
                .await
                .unwrap();
        let listing: SkillsListResult = serde_json::from_value(listed.0).unwrap();
        let entry = listing
            .skills
            .iter()
            .find(|entry| entry.uri == "skill://labby/native-race/SKILL.md")
            .unwrap();
        assert_eq!(entry.frontmatter["description"], "old");

        let got = dispatch_native_with_registry(
            &CustomRequest::new(
                SKILLS_GET_METHOD,
                Some(serde_json::json!({ "uri": entry.uri })),
            ),
            &pinned,
        )
        .await
        .unwrap();
        let result: SkillsGetResult = serde_json::from_value(got.0).unwrap();
        let resource = result
            .skill
            .resources
            .as_ref()
            .unwrap()
            .iter()
            .find(|resource| resource.uri.ends_with("/notes.md"))
            .unwrap();
        let file = crate::skills::facade::read_visible_skill_file(&pinned, &resource.uri)
            .await
            .unwrap();
        assert_eq!(resource.digest, file.digest);
        assert!(
            labby_runtime::skills::parse_digest(&resource.digest)
                .unwrap()
                .matches(file.text.as_bytes())
        );
        let resource_file = crate::mcp::handlers_resources::read_skill_resource_with_registry(
            &pinned,
            &resource.uri,
        )
        .await
        .unwrap();
        assert_eq!(resource_file.digest, resource.digest);
        assert_eq!(resource_file.text, file.text);
        assert_eq!(resource_file.text, "support-old\n");

        let current = SkillRegistryContext::from_generation(manager.generation());
        let current_file = crate::mcp::handlers_resources::read_skill_resource_with_registry(
            &current,
            &resource.uri,
        )
        .await
        .unwrap();
        assert_eq!(current_file.text, "support-new\n");
        assert_ne!(current_file.digest, resource_file.digest);
    }

    #[test]
    fn the_capability_is_advertised_with_no_optional_features() {
        let extensions = crate::mcp::server::mcp_extensions_for_test();
        let declared = extensions
            .get(labby_runtime::skills::wire::SKILLS_EXTENSION_KEY)
            .expect("skills extension is advertised when the feature is on");
        assert!(declared.is_empty(), "directoryRead must not be advertised");
    }

    #[tokio::test]
    async fn first_party_get_accepts_a_supporting_file_uri() {
        let uri = "skill://labby/creating-snippets/README.md";
        let registry = SkillRegistryContext::first_party_only();
        let entry = get_visible_skill(&registry, uri).await.expect("resolves");
        assert_eq!(entry.uri, "skill://labby/creating-snippets/SKILL.md");
        assert!(
            entry
                .resources
                .as_ref()
                .expect("manifest")
                .iter()
                .any(|resource| resource.uri == uri)
        );
    }

    #[tokio::test]
    async fn every_first_party_manifest_file_verifies_against_served_bytes() {
        let registry = SkillRegistryContext::first_party_only();
        let listing = list_visible_skills(&registry).await;
        assert!(!listing.skills.is_empty());
        for entry in &listing.skills {
            for resource in entry.resources.as_ref().expect("manifest") {
                let file = crate::skills::facade::read_visible_skill_file(&registry, &resource.uri)
                    .await
                    .expect("every listed file is served");
                let digest =
                    labby_runtime::skills::parse_digest(&resource.digest).expect("valid digest");
                assert!(
                    digest.matches(file.text.as_bytes()),
                    "{} failed",
                    resource.uri
                );
            }
        }
    }

    #[tokio::test]
    async fn unknown_first_party_skill_uris_are_not_served() {
        let registry = SkillRegistryContext::first_party_only();
        for uri in [
            "skill://labby/using-labby/../escape.md",
            "skill://labby/nonexistent/SKILL.md",
        ] {
            assert!(get_visible_skill(&registry, uri).await.is_none());
            assert!(
                crate::skills::facade::read_visible_skill_file(&registry, uri)
                    .await
                    .is_err()
            );
        }
    }

    #[test]
    fn proxied_uri_reconstruction_removes_the_gateway_label() {
        assert_eq!(
            labby_runtime::skills::parse_skill_uri("skill://gh/skill/acme/refunds/SKILL.md")
                .expect("published URI")
                .upstream_uri_for_origin("gh")
                .expect("reconstructable skill URI"),
            "skill://acme/refunds/SKILL.md"
        );
        assert!(
            labby_runtime::skills::parse_skill_uri("skill://other/skill/acme/refunds/SKILL.md")
                .expect("published URI")
                .upstream_uri_for_origin("gh")
                .is_none()
        );
    }
}
