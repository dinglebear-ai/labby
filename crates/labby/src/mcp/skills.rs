//! Native MCP adapter for Agent Skills (SEP-2640).
//!
//! Canonical list/get/read semantics live in crate::skills::facade. This module
//! owns only MCP authorization, JSON-RPC shapes, request-context projection,
//! and native extension observability.

pub(crate) use crate::skills::is_skill_uri;
#[cfg(test)]
use crate::skills::{
    first_party_skill_entry, list_first_party_skills, read_first_party_skill_file,
};
use Future;

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

impl LabMcpServer {
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
            let registry = self.skill_registry_context_for_tool(context, meta).await;
            crate::dispatch::skills::dispatch_with_context(&registry, action, params).await
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
        if request.method == SKILLS_GET_METHOD {
            let params = request
                .params_as::<SkillsGetParams>()
                .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?
                .ok_or_else(|| ErrorData::invalid_params("skills/get requires uri", None))?;
            let entry = get_visible_skill(&registry, &params.uri)
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

        serde_json::to_value(list_visible_skills(&registry).await)
            .map(CustomResult::new)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))
    }
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

    #[test]
    fn the_capability_is_advertised_with_no_optional_features() {
        let extensions = crate::mcp::server::mcp_extensions_for_test();
        let declared = extensions
            .get(labby_runtime::skills::wire::SKILLS_EXTENSION_KEY)
            .expect("skills extension is advertised when the feature is on");
        assert!(declared.is_empty(), "directoryRead must not be advertised");
    }

    #[test]
    fn first_party_get_accepts_a_supporting_file_uri() {
        let uri = "skill://labby/creating-snippets/README.md";
        let entry = first_party_skill_entry(uri).expect("resolves");
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

    #[test]
    fn every_first_party_manifest_file_verifies_against_served_bytes() {
        let listing = list_first_party_skills();
        assert!(!listing.skills.is_empty());
        for entry in &listing.skills {
            for resource in entry.resources.as_ref().expect("manifest") {
                let body = read_first_party_skill_file(&resource.uri)
                    .expect("every listed file is served");
                let digest =
                    labby_runtime::skills::parse_digest(&resource.digest).expect("valid digest");
                assert!(digest.matches(body.as_bytes()), "{} failed", resource.uri);
            }
        }
    }

    #[test]
    fn unknown_first_party_skill_uris_are_not_served() {
        assert!(read_first_party_skill_file("skill://labby/using-labby/../escape.md").is_none());
        assert!(read_first_party_skill_file("skill://labby/nonexistent/SKILL.md").is_none());
        assert!(first_party_skill_entry("skill://labby/nonexistent/SKILL.md").is_none());
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
