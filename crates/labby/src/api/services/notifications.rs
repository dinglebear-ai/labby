//! Admin-only operator notification feed.

use axum::{Extension, Json, extract::State, http::StatusCode, routing::get};
use serde_json::{Value, json};

use crate::api::oauth::AuthContext;
use crate::api::route_registry::{RouteAuth, RouteDescriptor, RouteGroup};
use crate::api::state::AppState;

pub fn routes(_state: AppState) -> RouteGroup {
    RouteGroup::empty().route(descriptors().remove(0), get(list))
}

pub(crate) fn descriptors() -> Vec<RouteDescriptor> {
    vec![RouteDescriptor::new(
        "GET",
        "/",
        "list",
        "notifications",
        RouteAuth::V1,
    )]
}

async fn list(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let admin = auth
        .as_ref()
        .is_some_and(|context| context.0.scopes.iter().any(|scope| scope == "lab:admin"));
    if !admin {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({"kind":"forbidden","message":"lab:admin scope required"})),
        ));
    }
    Ok(Json(json!({
        "notifications": state.notifications.list().await
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::Request};
    use tower::ServiceExt as _;

    fn auth() -> AuthContext {
        AuthContext {
            sub: "notification-test".into(),
            actor_key: None,
            scopes: vec!["lab:read".into(), "lab:admin".into()],
            issuer: "test".into(),
            via_session: false,
            csrf_token: None,
            email: Some("admin@example.com".into()),
        }
    }

    #[tokio::test]
    async fn admin_can_list_notifications() {
        let state = AppState::new();
        let app = Router::new()
            .merge(routes(state.clone()).router)
            .layer(Extension(auth()))
            .with_state(state);
        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
