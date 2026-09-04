use axum::{
    Extension, Json,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use labby_auth::{AuthContext, Authenticator, PrincipalLink, VerifiedIdentity};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{
    route_registry::{RouteAuth, RouteDescriptor, RouteGroup},
    state::AppState,
};
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
    ]
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
