//! Browser adapter for consuming exact offline operator consent.
use crate::access::owner_link::OwnerLinkApproval;
use crate::api::{
    route_registry::{RouteAuth, RouteDescriptor, RouteGroup},
    state::AppState,
};
use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use labby_auth::{
    AuthContext, Authenticator, VerifiedIdentity, browser_authority::BrowserAuthority,
};
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) fn descriptors() -> Vec<RouteDescriptor> {
    vec![
        RouteDescriptor::new(
            "POST",
            "/consume",
            "owner_link_consume",
            "access",
            RouteAuth::BrowserSession,
        )
        .private_no_store()
        .side_effects("consume explicit offline owner-link approval"),
    ]
}
pub(crate) fn routes(_: AppState) -> RouteGroup {
    RouteGroup::empty().route(descriptors().remove(0), post(consume))
}

pub(crate) async fn current_approval(
    state: &AppState,
    authority: &BrowserAuthority,
    auth: &AuthContext,
    identity: &VerifiedIdentity,
) -> Result<OwnerLinkApproval, &'static str> {
    super::access_bootstrap::require_browser_admin(state, auth, identity)
        .map_err(|_| "google_owner_session_required")?;
    let live = authority
        .revalidate()
        .await
        .map_err(|_| "session_expired")?;
    if !auth.via_session
        || !live.has_scope("lab:admin")
        || authority.identity_provider() != Some("google")
        || identity.authenticator() != Authenticator::BrowserSession
    {
        return Err("google_owner_session_required");
    }
    let installation = state
        .installation_id
        .as_ref()
        .ok_or("project_access_unavailable")?;
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|_| "project_access_unavailable")?;
    let a = store
        .owner_link_approval(identity.clone(), installation.to_string())
        .await
        .map_err(|_| "owner_link_approval_required")?;
    validate_route(state, &a).await?;
    Ok(a)
}

#[cfg(feature = "gateway")]
pub(crate) async fn validate_route(
    state: &AppState,
    a: &OwnerLinkApproval,
) -> Result<(), &'static str> {
    validate_route_target(
        state,
        &a.route_id,
        &a.project_id,
        &a.loadout_id,
        Some(&a.resource),
    )
    .await
}

#[cfg(feature = "gateway")]
pub(crate) async fn validate_route_target(
    state: &AppState,
    route_id: &str,
    project_id: &str,
    loadout_id: &str,
    resource: Option<&str>,
) -> Result<(), &'static str> {
    let manager = state
        .gateway_manager
        .as_ref()
        .ok_or("project_access_unavailable")?;
    let route = manager
        .published_project_route_snapshot(route_id, project_id, loadout_id)
        .await
        .map_err(|_| "project_route_unavailable")?;
    if resource.is_some_and(|resource| route.resource() != resource) {
        return Err("project_route_changed");
    }
    let loadout = route.effective_loadout();
    let scope = crate::mcp::route_scope::McpRouteScope::protected_subset_with_capabilities(
        route.route_name(),
        &loadout.upstreams,
        route.effective_service_names(),
        crate::mcp::route_scope::McpRouteCapabilityGates::from_loadout(loadout),
    );
    if !scope.allows_service(crate::dispatch::depot_publish::SERVICE) {
        return Err("publish_not_in_project_loadout");
    }
    Ok(())
}
#[cfg(not(feature = "gateway"))]
pub(crate) async fn validate_route(
    _: &AppState,
    _: &OwnerLinkApproval,
) -> Result<(), &'static str> {
    Err("project_access_unavailable")
}

#[cfg(not(feature = "gateway"))]
pub(crate) async fn validate_route_target(
    _: &AppState,
    _: &str,
    _: &str,
    _: &str,
    _: Option<&str>,
) -> Result<(), &'static str> {
    Err("project_access_unavailable")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmptyRequest {}

pub(super) async fn consume(
    State(state): State<AppState>,
    authority: Option<Extension<BrowserAuthority>>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    headers: HeaderMap,
    Json(_): Json<EmptyRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let denied = |reason| {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"kind":"forbidden","message":reason})),
        )
    };
    let (Some(Extension(authority)), Some(Extension(auth)), Some(Extension(identity))) =
        (authority, auth, identity)
    else {
        return Err(denied("browser_session_required"));
    };
    super::require_session_csrf("access.owner_link.consume", &headers, Some(&auth))
        .map_err(|_| denied("csrf_required"))?;
    let a = current_approval(&state, &authority, &auth, &identity)
        .await
        .map_err(denied)?;
    let project = a.project_id.clone();
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|_| denied("project_access_unavailable"))?;
    authority
        .revalidate()
        .await
        .map_err(|_| denied("session_expired"))?;
    store
        .consume_owner_link(identity, a)
        .await
        .map_err(|_| denied("owner_link_denied"))?;
    Ok(Json(json!({"linked":true,"projectId":project})))
}
