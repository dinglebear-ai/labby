//! Native MCP adapter for Agent Skills (SEP-2640).
//!
//! Canonical list/get/read semantics live in crate::skills::facade. This module
//! owns only MCP authorization, JSON-RPC shapes, request-context projection,
//! and native extension observability.

pub(crate) use crate::skills::is_skill_uri;
use std::future::Future;

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

pub(crate) async fn dispatch_at_in_process_boundary(
    registry: &SkillRegistryContext,
    action: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, ToolError> {
    crate::dispatch::skills::dispatch_with_context(registry, action, params).await
}

impl LabMcpServer {
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
        let identity = parts
            .extensions
            .get::<labby_auth::VerifiedIdentity>()
            .cloned()
            .ok_or_else(|| ToolError::Forbidden {
                message: "Skill Library identity is required".to_owned(),
                required_scopes: Vec::new(),
            })?;
        let auth = parts
            .extensions
            .get::<labby_auth::auth_context::AuthContext>()
            .cloned()
            .ok_or_else(|| ToolError::Forbidden {
                message: "Skill Library authentication is required".to_owned(),
                required_scopes: Vec::new(),
            })?;
        let project_id = parts
            .headers
            .get("x-labby-project-id")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| ToolError::Forbidden {
                message: "Skill Library project context is required".to_owned(),
                required_scopes: Vec::new(),
            })?;
        static REQUESTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let correlation = parts
            .headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "mcp-{}",
                    REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                )
            });
        let correlation =
            crate::dispatch::skill_library::audit::SkillLibraryCorrelationId::parse(correlation)
                .map_err(|()| ToolError::InvalidParam {
                    message: "invalid request correlation".to_owned(),
                    param: "x-request-id".to_owned(),
                })?;
        let caller = crate::dispatch::skill_library::auth::SkillLibraryCaller::new(
            identity,
            auth.scopes,
            crate::dispatch::skill_library::auth::SkillLibraryTransport::bearer(
                crate::dispatch::skill_library::auth::SkillLibrarySurface::Mcp,
                true,
            ),
        );
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
    ) -> SkillRegistryContext {
        #[cfg(feature = "gateway")]
        {
            let Some(manager) = self.gateway_manager.as_ref() else {
                return SkillRegistryContext::first_party_only();
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
            return SkillRegistryContext::with_manager(std::sync::Arc::clone(manager), scope);
        }

        #[cfg(not(feature = "gateway"))]
        {
            let _ = context;
            SkillRegistryContext::first_party_only()
        }
    }

    /// Reconstruct the outer caller context on Labby's private in-process
    /// Code Mode hop. Metadata is ignored everywhere else and missing pieces
    /// fail closed to first-party-only visibility.
    pub(crate) async fn skill_registry_context_for_tool(
        &self,
        context: &RequestContext<RoleServer>,
        meta: Option<&rmcp::model::RequestMetaObject>,
    ) -> SkillRegistryContext {
        #[cfg(feature = "gateway")]
        {
            if self.gateway_manager.is_some() {
                return self.skill_registry_context(context).await;
            }
            if self.transport_label != crate::mcp::in_process_peer::IN_PROCESS_TRANSPORT_LABEL {
                return SkillRegistryContext::first_party_only();
            }
            let Some(auth) = propagated_caller_auth(meta) else {
                return SkillRegistryContext::first_party_only();
            };
            let Some(propagated_scope) = propagated_caller_upstream_scope(meta) else {
                return SkillRegistryContext::first_party_only();
            };
            let Some(manager) = labby_gateway::gateway::current_gateway_manager() else {
                return SkillRegistryContext::first_party_only();
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
            return SkillRegistryContext::with_manager(manager, scope);
        }

        #[cfg(not(feature = "gateway"))]
        {
            let _ = (context, meta);
            SkillRegistryContext::first_party_only()
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
            let registry = self.skill_registry_context_for_tool(context, meta).await;
            dispatch_at_in_process_boundary(&registry, action, params).await
        })
    }

    /// Answer native Skills extension list/get requests.
    pub(crate) async fn handle_skills_request(
        &self,
        request: &CustomRequest,
        context: &RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        let start = std::time::Instant::now();
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
                skills: Vec::new(),
                next_cursor: None,
                ttl_ms: Some(0),
                cache_scope: Some(CACHE_SCOPE_PRIVATE.to_string()),
                meta: None,
            })
            .map(CustomResult::new)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None));
        }

        let registry = self.skill_registry_context(context).await;
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
        let result = SkillsGetResult { skill: entry };
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

#[cfg(test)]
mod serve_tests {
    use super::*;

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
