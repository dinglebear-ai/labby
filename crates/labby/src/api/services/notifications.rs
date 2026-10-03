//! Operator feed plus authenticated publication into the separate agent inbox.
use crate::api::oauth::AuthContext;
use crate::api::route_registry::{RouteAuth, RouteDescriptor, RouteGroup};
use crate::api::state::AppState;
use axum::{Extension, Json, extract::State, http::StatusCode, routing::get};
use serde_json::{Value, json};

pub fn routes(_state: AppState) -> RouteGroup {
    let routes = RouteGroup::empty().route(descriptors().remove(0), get(list));
    #[cfg(feature = "gateway")]
    let routes = routes.route(
        descriptors().remove(1),
        axum::routing::post(publish_agent).layer(axum::extract::DefaultBodyLimit::max(8192)),
    );
    routes
}
pub(crate) fn descriptors() -> Vec<RouteDescriptor> {
    vec![
        RouteDescriptor::new("GET", "/", "list", "notifications", RouteAuth::V1),
        #[cfg(feature = "gateway")]
        RouteDescriptor::new(
            "POST",
            "/agent",
            "publish_agent",
            "notifications",
            RouteAuth::V1,
        ),
    ]
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
    Ok(Json(
        json!({"notifications":state.notifications.list().await}),
    ))
}
#[cfg(feature = "gateway")]
async fn publish_agent(
    State(state): State<AppState>,
    auth: Option<Extension<AuthContext>>,
    Json(input): Json<crate::dispatch::codemode_notices::PublishNotice>,
) -> Result<Json<crate::dispatch::codemode_notices::PublishReceipt>, crate::api::error::ApiError> {
    use crate::dispatch::codemode_notices::NoticeProducer;
    let producer = NoticeProducer {
        actor: auth
            .as_ref()
            .map(|a| a.0.actor_key.as_deref().unwrap_or(&a.0.sub).to_owned())
            .unwrap_or_default(),
        admin: auth
            .as_ref()
            .is_some_and(|a| a.0.scopes.iter().any(|s| s == "lab:admin")),
    };
    state
        .agent_notifications
        .publish(producer, input)
        .await
        .map(Json)
        .map_err(|error| {
            crate::api::error::ApiError::new(crate::api::error::ToolError::Sdk {
                sdk_kind: error.kind().to_owned(),
                message: error.to_string(),
            })
            .with_service_action("notifications", "publish_agent")
        })
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::Request};
    use tower::ServiceExt as _;
    fn auth(admin: bool) -> AuthContext {
        AuthContext {
            sub: "notification-test".into(),
            actor_key: None,
            scopes: if admin {
                vec!["lab:admin".into()]
            } else {
                vec!["lab".into()]
            },
            issuer: "test".into(),
            via_session: false,
            csrf_token: None,
            email: None,
        }
    }
    fn app(state: AppState, admin: bool) -> Router {
        Router::new()
            .merge(routes(state.clone()).router)
            .layer(Extension(auth(admin)))
            .with_state(state)
    }
    #[tokio::test]
    async fn admin_can_list_notifications() {
        let response = app(AppState::new(), true)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn notice_production_http_sender_auth_idempotency_and_input_limits() {
        use crate::dispatch::codemode_notices::{NoticeConsumer, NoticeRecipient};
        let mut state = AppState::new();
        state.agent_notifications =
            crate::dispatch::codemode_notices::NoticeStore::memory().unwrap();
        let who = NoticeRecipient {
            actor: Some("alice".into()),
            route: "root".into(),
            consumer: NoticeConsumer::Client {
                id: "client".into(),
                conversation: None,
            },
        };
        let inbox = state
            .agent_notifications
            .register(who.clone())
            .await
            .unwrap();
        let payload = json!({"inbox_id":inbox.id,"source":"integration-test","level":"info","message":"Indexing finished.","dedupe_key":"job-1","ttl_seconds":3600});
        let request = |payload: &Value| {
            Request::builder()
                .method("POST")
                .uri("/agent")
                .header("content-type", "application/json")
                .body(Body::from(payload.to_string()))
                .unwrap()
        };
        let denied = app(state.clone(), false)
            .oneshot(request(&payload))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert!(
            state
                .agent_notifications
                .deliver(who.clone(), |v| Some(v.clone()))
                .await
                .unwrap()
                .is_none()
        );
        let sent = app(state.clone(), true)
            .oneshot(request(&payload))
            .await
            .unwrap();
        assert_eq!(sent.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&axum::body::to_bytes(sent.into_body(), 8192).await.unwrap())
                .unwrap();
        assert_eq!(body["duplicate"], false);
        let duplicate = app(state.clone(), true)
            .oneshot(request(&payload))
            .await
            .unwrap();
        let duplicate: Value = serde_json::from_slice(
            &axum::body::to_bytes(duplicate.into_body(), 8192)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(duplicate["id"], body["id"]);
        assert_eq!(duplicate["duplicate"], true);
        let mut changed = payload.clone();
        changed["message"] = json!("Different.");
        let conflict = app(state.clone(), true)
            .oneshot(request(&changed))
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let mut forged = payload.clone();
        forged["admin"] = json!(true);
        let rejected = app(state.clone(), true)
            .oneshot(request(&forged))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let mut big = payload;
        big["message"] = json!("x".repeat(9000));
        assert_eq!(
            app(state.clone(), true)
                .oneshot(request(&big))
                .await
                .unwrap()
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            state
                .agent_notifications
                .deliver(who, |v| Some(v.clone()))
                .await
                .unwrap()
                .unwrap()
                .notifications[0]
                .id,
            body["id"].as_str().unwrap()
        );
    }
}
