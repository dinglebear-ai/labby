use axum::{
    Extension, Json,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use labby_auth::browser_authority::BrowserAuthority;
use labby_auth::{AuthContext, Authenticator, PrincipalLink, VerifiedIdentity};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{
    route_registry::{RouteAuth, RouteDescriptor, RouteGroup},
    state::AppState,
};
use crate::dispatch::depot::discovery::{self, DiscoveryError, DiscoveryRequest};
use crate::dispatch::depot::{DepotError, error_body};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationRequest {
    operation: String,
    #[serde(default)]
    params: Value,
}

pub fn routes(_state: AppState) -> RouteGroup {
    let mut routes = descriptors().into_iter();
    RouteGroup::empty()
        .route(routes.next().expect("status descriptor"), get(status))
        .route(routes.next().expect("session descriptor"), get(session))
        .route(
            routes.next().expect("operations descriptor"),
            get(operations),
        )
        .route(routes.next().expect("call descriptor"), post(call))
        .route(routes.next().expect("discover descriptor"), post(discover))
        .route(routes.next().expect("detail descriptor"), post(detail))
        .route(routes.next().expect("providers descriptor"), get(providers))
}

pub(crate) fn descriptors() -> Vec<RouteDescriptor> {
    vec![
        RouteDescriptor::new("GET", "/status", "status", "depot", RouteAuth::V1).private_no_store(),
        RouteDescriptor::new("GET", "/session", "session", "depot", RouteAuth::V1)
            .private_no_store(),
        RouteDescriptor::new("GET", "/operations", "operations", "depot", RouteAuth::V1)
            .private_no_store(),
        RouteDescriptor::new("POST", "/operations", "call", "depot", RouteAuth::V1)
            .private_no_store()
            .side_effects("bounded canonical Depot operation"),
        RouteDescriptor::new("POST", "/discover", "discover_v2", "depot", RouteAuth::V1)
            .private_no_store(),
        RouteDescriptor::new(
            "POST",
            "/artifacts/detail",
            "detail_v2",
            "depot",
            RouteAuth::V1,
        )
        .private_no_store(),
        RouteDescriptor::new("GET", "/providers", "providers", "depot", RouteAuth::V1)
            .private_no_store(),
    ]
}

async fn discover(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    Json(request): Json<DiscoveryRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    discovery::discover(
        &state.depot_manager,
        &authority,
        &request,
        tokio::time::Instant::now(),
    )
    .await
    .and_then(|response| {
        serde_json::to_value(response).map_err(|_| DiscoveryError::InvalidProvider)
    })
    .map(Json)
    .map_err(map_discovery_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DetailRequest {
    provider_id: String,
    artifact_id: String,
}

async fn detail(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    Json(request): Json<DetailRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    discovery::detail(
        &state.depot_manager,
        &authority,
        &request.provider_id,
        &request.artifact_id,
        tokio::time::Instant::now(),
    )
    .await
    .and_then(|response| {
        serde_json::to_value(response).map_err(|_| DiscoveryError::InvalidProvider)
    })
    .map(Json)
    .map_err(map_discovery_error)
}

async fn providers(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let grant = authority.revalidate().await.map_err(|_| forbidden())?;
    if !grant.has_scope("lab:admin") {
        return Err(forbidden());
    }
    serde_json::to_value(state.depot_manager.admin_status())
        .map(Json)
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"kind":"internal","message":"provider status unavailable"})),
            )
        })
}

fn map_discovery_error(error: DiscoveryError) -> (StatusCode, Json<Value>) {
    let (status, kind, message) = match error {
        DiscoveryError::InvalidQuery
        | DiscoveryError::InvalidLimit
        | DiscoveryError::InvalidProvider => (
            StatusCode::BAD_REQUEST,
            "validation_failed",
            error.to_string(),
        ),
        DiscoveryError::CursorExpired => {
            (StatusCode::CONFLICT, "cursor_expired", error.to_string())
        }
        DiscoveryError::ProviderUnavailable => {
            (StatusCode::NOT_FOUND, "not_found", error.to_string())
        }
        DiscoveryError::Capacity => (
            StatusCode::SERVICE_UNAVAILABLE,
            "capacity",
            error.to_string(),
        ),
        DiscoveryError::ResponseTooLarge => (
            StatusCode::BAD_GATEWAY,
            "upstream_invalid",
            error.to_string(),
        ),
    };
    (
        status,
        Json(json!({"kind":kind,"message":message,"recovery":{"action":"restart_discovery"}})),
    )
}

async fn status(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    actor(auth, identity)?;
    Ok(Json(json!({"depot": state.depot.status()})))
}

fn actor(
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<String, (StatusCode, Json<Value>)> {
    let Some(Extension(auth)) = auth else {
        return Err(forbidden());
    };
    let Some(Extension(identity)) = identity else {
        return Err(forbidden());
    };
    let durable_browser_actor = auth.via_session
        && identity.authenticator() == Authenticator::BrowserSession
        && matches!(identity.principal_link(), PrincipalLink::External { subject, .. } if subject == &auth.sub);
    durable_browser_actor
        .then(|| identity.safe_fingerprint().to_string())
        .ok_or_else(forbidden)
}

fn forbidden() -> (StatusCode, Json<Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error":"verified_identity_required"})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_auth::Authenticator;

    #[test]
    fn depot_rejects_web_ui_auth_disabled_identity() {
        let identity =
            VerifiedIdentity::local_credential(Authenticator::StaticBearer, "web-ui-dev:local")
                .unwrap();

        let (status, body) = actor(None, Some(Extension(identity))).unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body.0, json!({"error":"verified_identity_required"}));
    }

    #[test]
    fn depot_accepts_only_durable_browser_identity() {
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "subject-1",
        )
        .unwrap();
        let auth = AuthContext {
            sub: "subject-1".into(),
            actor_key: None,
            scopes: vec![],
            issuer: "browser-session".into(),
            via_session: true,
            csrf_token: None,
            email: None,
        };
        assert_eq!(
            actor(Some(Extension(auth)), Some(Extension(identity)))
                .unwrap()
                .len(),
            12
        );
    }

    #[test]
    fn depot_rejects_static_bearer_and_non_session_oauth() {
        for authenticator in [Authenticator::StaticBearer, Authenticator::OauthBearer] {
            let identity = VerifiedIdentity::local_credential(authenticator, "credential").unwrap();
            let auth = AuthContext {
                sub: "subject-1".into(),
                actor_key: None,
                scopes: vec![],
                issuer: "local".into(),
                via_session: false,
                csrf_token: None,
                email: None,
            };
            assert!(actor(Some(Extension(auth)), Some(Extension(identity))).is_err());
        }
    }

    #[test]
    fn v2_federation_routes_are_literal_private_browser_contracts() {
        let routes = descriptors();
        for (method, path) in [
            ("POST", "/discover"),
            ("POST", "/artifacts/detail"),
            ("GET", "/providers"),
        ] {
            let route = routes
                .iter()
                .find(|route| route.method == method && route.path == path)
                .unwrap();
            assert_eq!(route.auth, RouteAuth::V1);
            assert_eq!(route.cache_posture, "private, no-store");
        }
    }
}

async fn session(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let actor = actor(auth, identity)?;
    state
        .depot
        .session(&actor)
        .await
        .map(Json)
        .map_err(map_error)
}

async fn operations(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let actor = actor(auth, identity)?;
    state
        .depot
        .operations(&actor)
        .await
        .map(Json)
        .map_err(map_error)
}

async fn call(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    Json(request): Json<OperationRequest>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let actor = actor(auth, identity)?;
    state
        .depot
        .call(&request.operation, request.params, &actor)
        .await
        .map(Json)
        .map_err(map_error)
}

fn map_error(error: DepotError) -> (StatusCode, Json<Value>) {
    let status = match &error {
        DepotError::Disabled | DepotError::Unconfigured => StatusCode::SERVICE_UNAVAILABLE,
        DepotError::UnsupportedOperation => StatusCode::BAD_REQUEST,
        DepotError::Upstream(status, _) => *status,
        DepotError::ResponseTooLarge => StatusCode::BAD_GATEWAY,
        DepotError::QueueTimeout => StatusCode::SERVICE_UNAVAILABLE,
        DepotError::Unavailable(_) | DepotError::InvalidResponse => StatusCode::BAD_GATEWAY,
    };
    (status, Json(error_body(&error)))
}
