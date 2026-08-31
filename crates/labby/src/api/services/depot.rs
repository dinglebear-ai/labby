use axum::{
    Extension, Json,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{get, post},
};
use labby_auth::VerifiedIdentity;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{
    oauth::AuthContext,
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
        .route(
            routes.next().expect("upload descriptor"),
            axum::routing::put(upload).layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
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
        RouteDescriptor::new(
            "PUT",
            "/uploads/{upload_id}",
            "upload",
            "depot",
            RouteAuth::V1,
        )
        .private_no_store()
        .side_effects("bounded principal-owned Depot upload"),
    ]
}

async fn status(State(state): State<AppState>) -> Json<Value> {
    Json(json!({"depot": state.depot.status()}))
}

fn actor(
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<String, (StatusCode, Json<Value>)> {
    identity
        .map(|Extension(value)| value.safe_fingerprint().to_string())
        .ok_or_else(|| {
            (
                StatusCode::FORBIDDEN,
                Json(json!({"error":"verified_identity_required"})),
            )
        })
}

fn require_admin(auth: Option<Extension<AuthContext>>) -> Result<(), (StatusCode, Json<Value>)> {
    if auth.is_some_and(|Extension(ctx)| {
        ctx.scopes
            .iter()
            .any(|scope| matches!(scope.as_str(), "lab" | "lab:admin"))
    }) {
        return Ok(());
    }
    Err((
        StatusCode::FORBIDDEN,
        Json(json!({"error":"lab_admin_required"})),
    ))
}

async fn session(
    State(state): State<AppState>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let actor = actor(identity)?;
    state
        .depot
        .session(&actor)
        .await
        .map(Json)
        .map_err(map_error)
}

async fn operations(
    State(state): State<AppState>,
    identity: Option<Extension<VerifiedIdentity>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let actor = actor(identity)?;
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
    require_admin(auth)?;
    let actor = actor(identity)?;
    state
        .depot
        .call(&request.operation, request.params, &actor)
        .await
        .map(Json)
        .map_err(map_error)
}

async fn upload(
    State(state): State<AppState>,
    Path(upload_id): Path<String>,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    bytes: Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    require_admin(auth)?;
    let actor = actor(identity)?;
    state
        .depot
        .upload(&upload_id, bytes.to_vec(), &actor)
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
        DepotError::Unavailable | DepotError::InvalidResponse => StatusCode::BAD_GATEWAY,
    };
    (status, Json(error_body(&error)))
}
