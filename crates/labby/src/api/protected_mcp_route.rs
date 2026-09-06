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
                        Ok(binding) => Ok(binding),
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
