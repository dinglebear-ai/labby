use std::net::SocketAddr;

use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    routing::post,
};
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;

async fn dispatch_at_api_boundary(
    registry: &crate::skills::facade::SkillRegistryContext,
    action: &str,
    params: Value,
) -> Result<Value, ToolError> {
    crate::dispatch::skills::dispatch_with_context(registry, action, params).await
}

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/", post(handle))
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

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
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

    handle_action_with_meta(
        "skills",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::skills::ACTIONS,
        move |action, params| async move {
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
                let registry = crate::skills::facade::SkillRegistryContext::with_manager(
                    std::sync::Arc::clone(manager),
                    crate::skills::facade::SkillCallerScope::root(oauth_subject, access),
                );
                return dispatch_at_api_boundary(&registry, &action, params).await;
            }

            crate::dispatch::skills::dispatch(&action, params).await
        },
    )
    .await
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
        let app = super::routes(state.clone()).with_state(state);
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
