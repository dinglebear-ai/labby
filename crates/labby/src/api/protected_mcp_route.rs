//! Protected MCP route entry and runtime interception.

#[path = "protected_mcp_route/auth.rs"]
mod auth;
#[path = "protected_mcp_route/policy.rs"]
mod policy;
#[path = "protected_mcp_route/proxy.rs"]
mod proxy;

use super::{is_public_relay_reserved_path, protected_route_metadata_response, request_host};
use crate::api::{error::ApiError, state::AppState};
use crate::config::ProtectedMcpRouteEffectiveTarget;
use crate::dispatch::error::ToolError;
use auth::{authenticate_protected_route_request, route_resource_metadata_url};
use axum::{
    body::Body,
    extract::State,
    http::{Method, Request, StatusCode},
    middleware::Next,
    response::IntoResponse,
};
use proxy::proxy_protected_mcp_route;
use tower::ServiceExt;

pub(super) use auth::auth_error_response_with_challenge;
#[cfg(test)]
pub(super) use auth::quoted_challenge_value;
#[cfg(test)]
pub(super) use policy::{
    ProtectedRouteExposureDecision, filter_protected_route_list_response,
    filter_protected_route_sse_event, filter_protected_route_sse_stream, find_sse_event_end,
    prepare_protected_route_request, protected_route_exposure_decision,
    protected_route_json_rpc_error,
};

fn team_member_auto_provision_candidate(has_oauth_delegation: bool, upstreams: &[String]) -> bool {
    has_oauth_delegation
        && upstreams
            .iter()
            .any(|upstream| upstream == crate::dispatch::depot_publish::REQUIRED_UPSTREAM)
}

fn team_admission_unavailable(stage: &'static str) -> axum::response::Response {
    tracing::warn!(
        surface = "mcp",
        stage,
        category = "unavailable",
        "team admission failed"
    );
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(serde_json::json!({
            "kind": "server_error", "message": "Team admission is temporarily unavailable"
        })),
    )
        .into_response()
}

async fn protected_mcp_route_entry(
    state: AppState,
    mut request: Request<Body>,
    route: crate::config::ProtectedMcpRouteConfig,
) -> axum::response::Response {
    let compatibility_metadata_path = format!(
        "{}/.well-known/oauth-protected-resource",
        route.public_path.trim_end_matches('/')
    );
    if *request.method() == Method::GET && request.uri().path() == compatibility_metadata_path {
        tracing::info!(
            route = %route.name,
            resource = %route.public_resource(),
            path = %request.uri().path(),
            "oauth protected resource compatibility metadata served"
        );
        return protected_route_metadata_response(&state, route).await;
    }
    if !matches!(
        *request.method(),
        Method::GET | Method::POST | Method::DELETE
    ) {
        tracing::warn!(
            route = %route.name,
            resource = %route.public_resource(),
            method = %request.method(),
            path = %request.uri().path(),
            "protected MCP route rejected unsupported method"
        );
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let authenticated = match authenticate_protected_route_request(
        &mut request,
        &route,
        state.oauth_state.as_deref(),
        state.access_credential_adapter.as_deref(),
        state.actor_key_deriver.as_deref(),
    )
    .await
    {
        Ok(authenticated) => authenticated,
        Err(response) => return response,
    };
    if let ProtectedMcpRouteEffectiveTarget::GatewaySubset(target) = route.effective_target() {
        if let Some(project_id) = target.project_id.as_deref() {
            let identity = authenticated
                .identity
                .clone()
                .expect("project-bound route authentication validates identity");
            if authenticated
                .product_bound
                .as_ref()
                .is_some_and(|bound| bound.project_id != project_id)
            {
                return auth_error_response_with_challenge(
                    "invalid bearer token",
                    &route_resource_metadata_url(&route),
                    &route.scopes,
                );
            }
            if team_member_auto_provision_candidate(
                authenticated.oauth_delegation.is_some(),
                &target.upstreams,
            ) {
                let authorized = match state.oauth_state.as_deref() {
                    Some(auth) => match auth.is_current_identity_authorized(&identity).await {
                        Ok(authorized) => authorized,
                        Err(_) => return team_admission_unavailable("identity_lookup"),
                    },
                    None => false,
                };
                if !authorized {
                    return auth_error_response_with_challenge(
                        "invalid bearer token",
                        &route_resource_metadata_url(&route),
                        &route.scopes,
                    );
                }
                if let Err(error) = state
                    .access_runtime
                    .provision_team_member(identity.clone(), project_id.to_owned())
                    .await
                {
                    if error == crate::access::TeamMemberProvisionError::Unavailable {
                        return team_admission_unavailable("member_provisioning");
                    }
                    tracing::warn!(
                        route = %route.name,
                        project_id,
                        "team member provisioning rejected"
                    );
                    return auth_error_response_with_challenge(
                        "invalid bearer token",
                        &route_resource_metadata_url(&route),
                        &route.scopes,
                    );
                }
            }
            request.extensions_mut().insert(identity.clone());
            let binding = match state.gateway_manager.as_ref() {
                Some(manager) => match crate::mcp::bound_access::bind_access_context(
                    state.access_runtime.as_ref(),
                    manager,
                    identity,
                    &route.name,
                    &route.public_resource(),
                    project_id,
                )
                .await
                {
                    Ok(core) => match crate::mcp::bound_access::TransportBoundAccessContext::new(
                        core,
                        authenticated
                            .transport
                            .expect("project-bound route authentication validates transport"),
                        std::time::SystemTime::now(),
                    ) {
                        Ok(binding) => {
                            if let Some(credential) = authenticated.oauth_delegation.as_ref() {
                                if let Some(installation_id) = state.installation_id.as_deref()
                                    && let Ok(policy) = manager
                                        .acquire_published_bootstrap_policy_lease(
                                            binding.core().catalog().access().loadout_name.as_str(),
                                            &route.name,
                                        )
                                        .await
                                    && let Ok(grant) = binding.oauth_depot_grant(
                                        installation_id,
                                        credential,
                                        &policy,
                                    )
                                {
                                    request.extensions_mut().insert(grant);
                                } else {
                                    tracing::warn!(
                                        route = %route.name,
                                        project_id,
                                        "Depot publishing grant unavailable; read-only route access preserved"
                                    );
                                }
                            }
                            Ok(binding)
                        }
                        Err(error) => {
                            tracing::warn!(
                                surface = "api",
                                route = %route.name,
                                resource = %route.public_resource(),
                                project_id,
                                error = %error,
                                "protected MCP route rejected: access context outlived its credential"
                            );
                            return auth_error_response_with_challenge(
                                "invalid bearer token",
                                &route_resource_metadata_url(&route),
                                &route.scopes,
                            );
                        }
                    },
                    // Not a rejection: the request proceeds with an `Unavailable`
                    // observation and the handler serves an empty project tool list.
                    // Without this the operator sees only that symptom, never a cause.
                    Err(error) => {
                        tracing::warn!(
                            surface = "api",
                            route = %route.name,
                            resource = %route.public_resource(),
                            project_id,
                            error = %error,
                            "project access binding failed; serving an unavailable observation"
                        );
                        Err(error)
                    }
                },
                None => {
                    tracing::error!(
                        surface = "api",
                        route = %route.name,
                        resource = %route.public_resource(),
                        project_id,
                        "project access binding unavailable: gateway manager is not mounted"
                    );
                    Err(crate::mcp::bound_access::BoundAccessContextError::Unavailable)
                }
            };
            crate::mcp::bound_access::attach_project_access_observation(
                request.extensions_mut(),
                binding,
            );
        }
        let Some(router) = state
            .protected_mcp_routers
            .as_ref()
            .and_then(|routers| routers.get(&route.name))
        else {
            tracing::error!(
                route = %route.name,
                resource = %route.public_resource(),
                "protected MCP gateway subset failed: scoped router missing"
            );
            return ApiError::new(ToolError::Sdk {
                sdk_kind: "bad_gateway".into(),
                message: "protected MCP gateway subset service is not mounted".into(),
            })
            .into_response();
        };
        return router
            .clone()
            .oneshot(request)
            .await
            .unwrap_or_else(|error| {
                tracing::error!(
                    route = %route.name,
                    resource = %route.public_resource(),
                    error = %error,
                    "protected MCP gateway subset failed: scoped service error"
                );
                ApiError::new(ToolError::Sdk {
                    sdk_kind: "bad_gateway".into(),
                    message: format!("protected MCP gateway subset service failed: {error}"),
                })
                .into_response()
            });
    }
    proxy_protected_mcp_route(&state, request, route).await
}

#[cfg(test)]
mod team_member_provisioning_tests {
    use super::team_member_auto_provision_candidate;

    #[test]
    fn admission_outages_do_not_challenge_valid_credentials() {
        for stage in ["identity_lookup", "member_provisioning"] {
            let response = super::team_admission_unavailable(stage);
            assert_eq!(
                response.status(),
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            );
            assert!(
                !response
                    .headers()
                    .contains_key(axum::http::header::WWW_AUTHENTICATE)
            );
        }
    }

    #[test]
    fn auto_provision_requires_oauth_and_the_exact_team_depot_upstream() {
        let team = vec!["team-depot".to_string()];
        assert!(team_member_auto_provision_candidate(true, &team));
        assert!(!team_member_auto_provision_candidate(false, &team));
        for upstreams in [
            Vec::<String>::new(),
            vec!["catalog-depot".to_string()],
            vec!["team-depot-evil".to_string()],
            vec!["TEAM-DEPOT".to_string()],
        ] {
            assert!(!team_member_auto_provision_candidate(true, &upstreams));
        }
    }
}

pub(super) async fn protected_mcp_intercept(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Result<axum::response::Response, std::convert::Infallible> {
    if is_public_relay_reserved_path(request.uri().path()) {
        return Ok(next.run(request).await);
    }
    let route = if let (Some(manager), Some(host)) = (
        state.gateway_manager.as_ref(),
        request_host(&request, state.config.api.trust_forwarded_headers),
    ) {
        manager
            .resolve_protected_route(&host, request.uri().path())
            .await
    } else {
        None
    };
    if let Some(route) = route {
        tracing::info!(
            route = %route.name,
            resource = %route.public_resource(),
            method = %request.method(),
            path = %request.uri().path(),
            "protected MCP route matched"
        );
        let mut response = protected_mcp_route_entry(state, request, route).await;
        response
            .extensions_mut()
            .insert(crate::api::route_observability::RuntimeMatchedRoute(
                "/{runtime_protected_mcp_route}",
            ));
        return Ok(response);
    }
    Ok(next.run(request).await)
}
