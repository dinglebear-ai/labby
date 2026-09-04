use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::post,
};
use labby_auth::{Authenticator, VerifiedIdentity};
use labby_primitives::product_credential::{BoundAccessGrant, ProductCredentialGrant};
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;

#[cfg(feature = "skills")]
fn map_import_adapter_error(
    error: crate::dispatch::skill_library::import::ImportAdapterError,
) -> ToolError {
    crate::dispatch::skill_library::map_import_error(error)
}

async fn dispatch_at_api_boundary(
    registry: &crate::skills::facade::SkillRegistryContext,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    crate::dispatch::skills::dispatch_with_context(registry, action, params).await
}

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    RouteGroup::empty().route(descriptors().remove(0), post(handle))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "skills", RouteAuth::V1)
            .feature("skills")
            .when("mounted only when API authentication is configured"),
    ]
}

#[cfg(all(test, feature = "skills"))]
mod skill_library_error_tests {
    use crate::dispatch::skill_library::auth::SkillLibraryAuthorizationError;
    use crate::dispatch::skill_library::dispatch::SkillLibraryDispatchError;
    use labby_runtime::artifacts::ArtifactError;

    #[test]
    fn management_errors_keep_stable_recovery_kinds() {
        let cases = [
            (
                SkillLibraryDispatchError::Artifact(ArtifactError::Busy),
                "queue_saturated",
            ),
            (
                SkillLibraryDispatchError::Artifact(ArtifactError::Conflict(
                    "library_version_changed",
                )),
                "conflict",
            ),
            (
                SkillLibraryDispatchError::Artifact(ArtifactError::NotFound("library_record")),
                "not_found",
            ),
            (
                SkillLibraryDispatchError::Authorization(SkillLibraryAuthorizationError::Denied),
                "forbidden",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(
                crate::dispatch::skill_library::map_dispatch_error(error).kind(),
                expected
            );
        }
    }

    #[test]
    fn management_internal_error_redacts_os_message_and_path() {
        const CANARY: &str = "/private/canary/never-return-this";
        let mapped = crate::dispatch::skill_library::map_dispatch_error(
            SkillLibraryDispatchError::Artifact(ArtifactError::Io(std::io::Error::other(CANARY))),
        );
        let wire = serde_json::to_string(&mapped).expect("serialize ToolError");
        assert_eq!(mapped.kind(), "internal_error");
        assert!(!wire.contains(CANARY));
        assert!(!wire.contains("private"));
    }

    #[test]
    fn indeterminate_mutation_error_requires_same_key_reconciliation() {
        let mapped = crate::dispatch::skill_library::map_dispatch_error(
            SkillLibraryDispatchError::BlockingIndeterminate {
                operation: "skill_artifact_commit",
            },
        );

        assert_eq!(mapped.kind(), "service_unavailable");
        let fields = mapped.extra_fields();
        assert_eq!(fields["operation"], "skill_artifact_commit");
        assert_eq!(fields["outcome"], "indeterminate");
        assert_eq!(fields["recovery"], "retry_with_same_idempotency_key");
    }

    #[test]
    fn worker_failure_is_not_misreported_as_a_timeout() {
        let mapped = crate::dispatch::skill_library::map_dispatch_error(
            SkillLibraryDispatchError::BlockingWorkerFailed {
                operation: "skill_library_list",
            },
        );

        assert_eq!(mapped.kind(), "internal_error");
        assert_ne!(mapped.kind(), "timeout");
    }
}

fn has_read_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_some_and(|ctx| {
        ctx.0
            .scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab:read" | "lab" | "lab:admin"))
    })
}

fn require_read_scope(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    // Keep catalog introspection aligned with the MCP compatibility tool: an
    // authenticated caller may inspect `help` / `schema` without gaining
    // access to any skill metadata or file contents.
    if auth.is_some() && matches!(action, "help" | "schema") {
        return Ok(());
    }
    if has_read_scope(auth) {
        return Ok(());
    }
    tracing::warn!(
        surface = "api",
        service = "skills",
        action,
        request_id,
        kind = "forbidden",
        "skills action rejected: read scope required"
    );
    Err(ToolError::Forbidden {
        message: "skills require one of scopes: lab:read, lab, lab:admin".to_string(),
        required_scopes: vec![
            "lab:read".to_string(),
            "lab".to_string(),
            "lab:admin".to_string(),
        ],
    })
}

fn product_credential_allows_skills(
    source: &ProductCredentialGrant,
    bound: &BoundAccessGrant,
    project_id: &str,
    config: &crate::config::LabConfig,
) -> bool {
    if !crate::dispatch::skill_library::auth::product_grants_match(source, bound)
        || bound.project_id != project_id
        || bound.audience != bound.resource
    {
        return false;
    }
    config.protected_mcp_routes.iter().any(|route| {
        if !route.enabled
            || route.name != bound.route_id
            || route.public_resource() != bound.resource
        {
            return false;
        }
        let Some(target) = route.gateway_subset_target() else {
            return false;
        };
        if target
            .project_id
            .as_deref()
            .is_some_and(|id| id != bound.project_id)
            || target
                .loadout
                .as_deref()
                .is_some_and(|id| id != bound.loadout_id)
            || !route
                .scopes
                .iter()
                .all(|scope| bound.scopes.iter().any(|granted| granted == scope))
        {
            return false;
        }
        crate::mcp::route_scope::McpRouteScope::from_protected_route(route, &config.loadouts)
            .ok()
            .flatten()
            .is_some_and(|scope| scope.allows_service("skills") && scope.exposes_skills())
    })
}

#[cfg(all(test, feature = "gateway"))]
mod product_credential_binding_tests {
    use super::*;

    fn fixture() -> (
        ProductCredentialGrant,
        BoundAccessGrant,
        crate::config::LabConfig,
    ) {
        let mut config: crate::config::LabConfig = toml::from_str(
            r#"
[[loadouts]]
name = "team-skills"
upstreams = []
services = ["skills"]
expose_skills = true

[[protected_mcp_routes]]
name = "team"
enabled = true
public_host = "team.example.com"
public_path = "/mcp"
scopes = ["lab:read", "lab:admin"]

[protected_mcp_routes.target]
kind = "gateway_subset"
project_id = "project-1"
loadout = "team-skills"
"#,
        )
        .expect("fixture config");
        config
            .normalize_protected_mcp_routes()
            .expect("fixture routes normalize");
        let resource = config.protected_mcp_routes[0].public_resource();
        let source = ProductCredentialGrant {
            issuer: "local".into(),
            subject: "operator-1".into(),
            credential_id: "credential-1".into(),
            credential_generation: 1,
            scopes: vec!["lab:read".into(), "lab:admin".into()],
            resource: resource.clone(),
            audience: resource.clone(),
            expires_at: 4_000_000_000,
        };
        let bound = BoundAccessGrant {
            installation_id: "installation-1".into(),
            issuer: source.issuer.clone(),
            subject: source.subject.clone(),
            principal_id: "principal-1".into(),
            organization_id: "organization-1".into(),
            project_id: "project-1".into(),
            loadout_id: "team-skills".into(),
            loadout_generation: 1,
            assignment_generation: 1,
            catalog_generation: 1,
            route_id: "team".into(),
            route_generation: 1,
            membership_epoch: 1,
            organization_policy_epoch: 0,
            project_policy_epoch: 0,
            credential_id: source.credential_id.clone(),
            credential_generation: source.credential_generation,
            scopes: source.scopes.clone(),
            resource: resource.clone(),
            audience: resource,
            expires_at: source.expires_at,
            requires_admin: false,
            destructive: false,
        };
        (source, bound, config)
    }

    #[test]
    fn product_credential_requires_exact_skill_route_binding() {
        let (source, bound, config) = fixture();
        assert!(product_credential_allows_skills(
            &source,
            &bound,
            "project-1",
            &config
        ));

        let mut mismatches = Vec::new();
        let mut mismatch = bound.clone();
        mismatch.project_id = "project-2".into();
        mismatches.push(mismatch);
        let mut mismatch = bound.clone();
        mismatch.route_id = "other-route".into();
        mismatches.push(mismatch);
        let mut mismatch = bound.clone();
        mismatch.resource = "https://other.example.com/mcp".into();
        mismatches.push(mismatch);
        let mut mismatch = bound.clone();
        mismatch.audience = "https://other.example.com/mcp".into();
        mismatches.push(mismatch);
        let mut mismatch = bound.clone();
        mismatch.loadout_id = "other-loadout".into();
        mismatches.push(mismatch);

        for mismatch in mismatches {
            assert!(!product_credential_allows_skills(
                &source,
                &mismatch,
                "project-1",
                &config
            ));
        }
    }
}

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    product_source: Option<Extension<ProductCredentialGrant>>,
    product_bound: Option<Extension<BoundAccessGrant>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok());
    require_read_scope(&req.action, request_id, auth.as_ref())?;

    #[cfg(feature = "gateway")]
    let manager = state.gateway_manager.clone();
    #[cfg(feature = "gateway")]
    let auth_for_dispatch = auth.clone();
    let skill_library = state.skill_library.clone();
    let skill_library_imports = state.skill_library_imports.clone();
    let access_runtime = Arc::clone(&state.access_runtime);
    let verified_identity = identity.map(|Extension(value)| value);
    let project_id = headers
        .get("x-labby-project-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let correlation = request_id.map(str::to_owned);
    let auth_for_library = auth.clone().map(|Extension(value)| value);
    let library_headers = headers.clone();
    let product_source = product_source.map(|Extension(value)| value);
    let product_bound = product_bound.map(|Extension(value)| value);
    let config = Arc::clone(&state.config);
    let product_credential_bound = verified_identity
        .as_ref()
        .is_some_and(|identity| identity.authenticator() == Authenticator::ProductCredential)
        && project_id.as_deref().is_some_and(|project_id| {
            product_source
                .as_ref()
                .zip(product_bound.as_ref())
                .is_some_and(|(source, bound)| {
                    product_credential_allows_skills(source, bound, project_id, &config)
                })
        });

    handle_action_with_meta(
        "skills",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::skills::API_ACTIONS,
        move |action, params| async move {
            let visibility_identity = verified_identity.clone();
            let visibility_auth = auth_for_library.clone();
            let visibility_project = project_id.clone();
            let visibility_correlation = correlation.clone();
            if action.starts_with("skill_library.") {
                let service = skill_library.ok_or_else(|| ToolError::Sdk {
                    sdk_kind: "skill_library_unavailable".to_owned(),
                    message: "Skill Library is unavailable".to_owned(),
                })?;
                let identity = verified_identity.ok_or_else(|| ToolError::Forbidden {
                    message: "Skill Library identity is required".to_owned(),
                    required_scopes: vec![],
                })?;
                let auth = auth_for_library.ok_or_else(|| ToolError::Forbidden {
                    message: "Skill Library authentication is required".to_owned(),
                    required_scopes: vec![],
                })?;
                let project_id = project_id.ok_or_else(|| ToolError::Forbidden {
                    message: "Skill Library project context is required".to_owned(),
                    required_scopes: vec![],
                })?;
                static REQUESTS: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(1);
                let correlation = correlation.unwrap_or_else(|| {
                    format!(
                        "api-{}",
                        REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    )
                });
                let csrf_verified = auth.via_session
                    && auth.csrf_token.as_deref().is_some_and(|token| {
                        library_headers
                            .get("x-csrf-token")
                            .and_then(|value| value.to_str().ok())
                            == Some(token)
                    });
                let transport = if auth.via_session {
                    crate::dispatch::skill_library::auth::SkillLibraryTransport::browser(
                        true,
                        csrf_verified,
                    )
                } else if identity.authenticator() == Authenticator::ProductCredential
                    && product_credential_bound
                {
                    crate::dispatch::skill_library::auth::SkillLibraryTransport::product_bearer(
                        crate::dispatch::skill_library::auth::SkillLibrarySurface::ApiBearer,
                    )
                } else {
                    crate::dispatch::skill_library::auth::SkillLibraryTransport::bearer(
                        crate::dispatch::skill_library::auth::SkillLibrarySurface::ApiBearer,
                        true,
                    )
                };
                let caller = crate::dispatch::skill_library::auth::SkillLibraryCaller::new(
                    identity,
                    auth.scopes,
                    transport,
                );
                let correlation =
                    crate::dispatch::skill_library::audit::SkillLibraryCorrelationId::parse(
                        correlation,
                    )
                    .map_err(|()| ToolError::InvalidParam {
                        message: "invalid request correlation".to_owned(),
                        param: "x-request-id".to_owned(),
                    })?;
                if action == "skill_library.import" {
                    let import_params: crate::dispatch::skill_library::params::ImportParams =
                        serde_json::from_value(params).map_err(|_| ToolError::InvalidParam {
                            message: "Skill Library import parameters are invalid".to_owned(),
                            param: "params".to_owned(),
                        })?;
                    crate::dispatch::skill_library::params::validate_idempotency_key(
                        &import_params.idempotency_key,
                    )
                    .map_err(|_| ToolError::InvalidParam {
                        message: "Skill Library idempotency key is invalid".to_owned(),
                        param: "idempotency_key".to_owned(),
                    })?;
                    let imports = skill_library_imports.ok_or_else(|| ToolError::Sdk {
                        sdk_kind: "source_unavailable".to_owned(),
                        message: "Skill import sources are not configured".to_owned(),
                    })?;
                    return imports
                        .import_selected(
                            &service,
                            &access_runtime,
                            caller,
                            &project_id,
                            import_params.source,
                            import_params.expected_library_version,
                            import_params.idempotency_key,
                            &correlation,
                        )
                        .await
                        .map_err(map_import_adapter_error);
                }
                return service
                    .dispatch(
                        &access_runtime,
                        caller,
                        &project_id,
                        &action,
                        params,
                        &correlation,
                    )
                    .await
                    .map_err(crate::dispatch::skill_library::map_dispatch_error);
            }
            #[cfg(feature = "gateway")]
            if let Some(manager) = manager.as_ref() {
                let auth = auth_for_dispatch.as_ref().map(|value| &value.0);
                let request_subject = auth.map(|value| value.sub.as_str());
                let oauth_subject = auth.and_then(|auth| {
                    crate::dispatch::oauth_subject::oauth_upstream_subject_for_request(
                        Some(auth),
                        request_subject,
                    )
                    .map(|subject| subject.into_owned())
                });
                let access = if manager.code_mode_enabled().await {
                    crate::skills::aggregate::ToolAccess::CodeModeOnly
                } else {
                    crate::skills::aggregate::ToolAccess::Direct
                };
                let mut registry = crate::skills::facade::SkillRegistryContext::with_manager(
                    Arc::clone(manager),
                    crate::skills::facade::SkillCallerScope::root(oauth_subject, access),
                );
                registry = attach_artifact_access(
                    registry,
                    &access_runtime,
                    visibility_identity,
                    visibility_auth,
                    visibility_project.as_deref(),
                    visibility_correlation.as_deref(),
                    product_credential_bound,
                )
                .await;
                return dispatch_at_api_boundary(&registry, &action, params).await;
            }

            let registry = attach_artifact_access(
                crate::skills::facade::SkillRegistryContext::first_party_only(),
                &access_runtime,
                visibility_identity,
                visibility_auth,
                visibility_project.as_deref(),
                visibility_correlation.as_deref(),
                product_credential_bound,
            )
            .await;
            dispatch_at_api_boundary(&registry, &action, params).await
        },
    )
    .await
}

async fn attach_artifact_access(
    mut registry: crate::skills::facade::SkillRegistryContext,
    access_runtime: &crate::access::AccessRuntime,
    identity: Option<VerifiedIdentity>,
    auth: Option<AuthContext>,
    project_id: Option<&str>,
    correlation: Option<&str>,
    product_credential_bound: bool,
) -> crate::skills::facade::SkillRegistryContext {
    let (Some(identity), Some(auth), Some(project_id)) = (identity, auth, project_id) else {
        return registry;
    };
    let transport = if auth.via_session {
        crate::dispatch::skill_library::auth::SkillLibraryTransport::browser(true, true)
    } else if product_credential_bound {
        crate::dispatch::skill_library::auth::SkillLibraryTransport::product_bearer(
            crate::dispatch::skill_library::auth::SkillLibrarySurface::ApiBearer,
        )
    } else {
        crate::dispatch::skill_library::auth::SkillLibraryTransport::bearer(
            crate::dispatch::skill_library::auth::SkillLibrarySurface::ApiBearer,
            true,
        )
    };
    let caller = crate::dispatch::skill_library::auth::SkillLibraryCaller::new(
        identity,
        auth.scopes,
        transport,
    );
    let Ok(correlation) = crate::dispatch::skill_library::audit::SkillLibraryCorrelationId::parse(
        correlation.unwrap_or("api-skills-read"),
    ) else {
        return registry;
    };
    if let Ok(decision) = crate::dispatch::skill_library::auth::authorize_at_boundary(
        access_runtime,
        caller,
        project_id,
        crate::dispatch::skill_library::auth::SkillLibraryAction::List,
        &crate::dispatch::skill_library::audit::CanonicalArtifactId::parse("library")
            .expect("static id"),
        crate::dispatch::skill_library::auth::SkillLibraryTarget::SharedActive,
        &correlation,
    )
    .await
    {
        registry = registry.with_artifact_access(decision.artifact_access_snapshot());
    }
    registry
}

#[cfg(test)]
mod tests {
    use axum::{
        Extension, Router,
        body::Body,
        http::{Request, StatusCode, header},
    };
    use serde_json::json;
    use tower::ServiceExt;

    use crate::api::{oauth::AuthContext, state::AppState};

    #[tokio::test]
    async fn api_boundary_keeps_a_captured_generation_during_refresh() {
        use crate::skills::facade::SkillRegistryContext;
        use crate::skills::registry::{FirstPartyGenerationManager, GenerationLimits};
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("api-race");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: api-race\ndescription: old\n---\nold\n",
        )
        .unwrap();
        let manager =
            FirstPartyGenerationManager::new(temp.path().into(), GenerationLimits::default());
        let old = SkillRegistryContext::from_generation(manager.generation());
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: api-race\ndescription: new\n---\nnew\n",
        )
        .unwrap();
        manager.refresh(None).unwrap();
        let old_value = super::dispatch_at_api_boundary(
            &old,
            "skills.read",
            json!({"uri":"skill://labby/api-race/SKILL.md"}),
        )
        .await
        .unwrap();
        let new_context = SkillRegistryContext::from_generation(manager.generation());
        let new_value = super::dispatch_at_api_boundary(
            &new_context,
            "skills.read",
            json!({"uri":"skill://labby/api-race/SKILL.md"}),
        )
        .await
        .unwrap();
        assert!(old_value["text"].as_str().unwrap().contains("old"));
        assert!(new_value["text"].as_str().unwrap().contains("new"));
        assert_ne!(old_value["digest"], new_value["digest"]);
    }

    fn auth(scopes: &[&str]) -> AuthContext {
        AuthContext {
            sub: "reader".to_string(),
            actor_key: None,
            scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: None,
        }
    }

    fn app(auth: Option<AuthContext>) -> Router {
        let state = AppState::from_registry(crate::registry::build_default_registry());
        let app = super::routes(state.clone()).router.with_state(state);
        match auth {
            Some(auth) => app.layer(Extension(auth)),
            None => app,
        }
    }

    async fn post(app: Router, body: serde_json::Value) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
    }

    #[tokio::test]
    async fn unauthenticated_request_is_forbidden() {
        let response = post(app(None), json!({ "action": "skills.list", "params": {} })).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn authenticated_non_read_scope_can_inspect_help_but_not_list() {
        let help = post(
            app(Some(auth(&["profile"]))),
            json!({ "action": "help", "params": {} }),
        )
        .await;
        assert_eq!(help.status(), StatusCode::OK);

        let list = post(
            app(Some(auth(&["profile"]))),
            json!({ "action": "skills.list", "params": {} }),
        )
        .await;
        assert_eq!(list.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn read_scope_can_list_native_skills() {
        let response = post(
            app(Some(auth(&["lab:read"]))),
            json!({ "action": "skills.list", "params": { "limit": 5 } }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}
