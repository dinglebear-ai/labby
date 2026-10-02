use std::net::SocketAddr;

use axum::{
    Extension, Json,
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

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    RouteGroup::empty().route(descriptors().remove(0), post(handle))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "snippets", RouteAuth::V1)
            .feature("gateway")
            .when("mounted only when API authentication is configured"),
    ]
}

fn snippets_action_requires_admin(action: &str) -> bool {
    let bare = action.strip_prefix("snippets.").unwrap_or(action);
    if bare == "help" || bare == "schema" {
        return false;
    }
    crate::dispatch::snippets::ACTIONS
        .iter()
        .find(|spec| spec.name == action)
        .map(|spec| spec.requires_admin)
        .unwrap_or(true)
}

fn has_admin_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_some_and(|ctx| ctx.0.scopes.iter().any(|scope| scope == "lab:admin"))
}

fn require_snippets_admin(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    if !snippets_action_requires_admin(action) || has_admin_scope(auth) {
        return Ok(());
    }

    tracing::warn!(
        surface = "api",
        service = "snippets",
        action,
        request_id,
        kind = "forbidden",
        "snippets action rejected: lab:admin scope required"
    );
    Err(ToolError::Sdk {
        sdk_kind: "forbidden".to_string(),
        message: format!("action `{action}` requires `lab:admin` scope"),
    })
}

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    require_snippets_admin(&req.action, request_id, auth.as_ref())?;
    let manager = state.gateway_manager.clone();
    let execution_caller = auth.as_ref().map_or(
        crate::dispatch::gateway::code_mode::CodeModeCaller::TrustedLocal,
        |value| crate::dispatch::gateway::code_mode::CodeModeCaller::Scoped {
            capabilities: labby_codemode::CodeModeCallerCapabilities {
                can_read: true,
                can_execute: true,
                can_use_snippets: true,
                is_admin: true,
            },
            sub: Some(value.0.sub.clone()),
        },
    );
    let dispatch_context = crate::dispatch::snippets::dispatch::SnippetDispatchContext {
        actor_key: auth
            .as_ref()
            .and_then(|value| value.0.actor_key.as_deref())
            .map(ToOwned::to_owned),
        is_admin: has_admin_scope(auth.as_ref()),
        route_scope: "root".to_string(),
        capability_filter_fingerprint: crate::dispatch::gateway::code_mode::ToolScope::default()
            .fingerprint(),
        execution_scope: crate::dispatch::gateway::code_mode::ToolScope::default(),
        execution_caller,
        execution_surface: crate::dispatch::gateway::code_mode::CodeModeSurface::Api,
    };

    handle_action_with_meta(
        "snippets",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::snippets::ACTIONS,
        move |action, params| async move {
            if crate::dispatch::snippets::dispatch::requires_execution_context(&action) {
                let manager = manager
                    .as_ref()
                    .ok_or_else(|| ToolError::internal_message("gateway manager not wired"))?;
                return crate::dispatch::snippets::dispatch::dispatch_with_manager_and_context(
                    manager,
                    &action,
                    params,
                    Some(dispatch_context.clone()),
                )
                .await;
            }
            crate::dispatch::snippets::dispatch(&action, params).await
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

    fn read_only_auth_context() -> AuthContext {
        AuthContext {
            sub: "read-only-user".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: Some("reader@example.com".to_string()),
        }
    }

    fn admin_auth_context() -> AuthContext {
        AuthContext {
            sub: "admin-user".to_string(),
            actor_key: None,
            scopes: vec!["lab:read".to_string(), "lab:admin".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: Some("admin@example.com".to_string()),
        }
    }

    fn app_with_auth(auth: AuthContext) -> Router {
        let state = AppState::from_registry(crate::registry::build_default_registry());
        super::routes(state.clone())
            .router
            .layer(Extension(auth))
            .with_state(state)
    }

    async fn post_snippets(app: Router, body: serde_json::Value) -> axum::response::Response {
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
    async fn read_only_actions_do_not_require_admin_scope() {
        let app = app_with_auth(read_only_auth_context());
        for action in ["snippets.list", "help", "schema"] {
            let params = if action == "schema" {
                json!({"action": "snippets.list"})
            } else {
                json!({})
            };
            let response =
                post_snippets(app.clone(), json!({"action": action, "params": params})).await;
            assert_ne!(
                response.status(),
                StatusCode::FORBIDDEN,
                "read-only action `{action}` must not require admin scope"
            );
        }
    }

    #[tokio::test]
    async fn admin_actions_require_admin_scope() {
        let app = app_with_auth(read_only_auth_context());
        for action in [
            "snippets.get",
            "snippets.exec",
            "snippets.create",
            "snippets.remove",
            "snippets.test",
            "snippets.validate",
            "snippets.preview",
            "snippets.replay",
            "snippets.receipt",
            "snippets.history",
            "snippets.artifact",
        ] {
            let response = post_snippets(
                app.clone(),
                json!({
                    "action": action,
                    "params": {
                        "name": "fixture",
                        "body": "async () => ({ ok: true })",
                        "confirm": true
                    }
                }),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "action `{action}` should require lab:admin scope"
            );
        }
    }

    #[tokio::test]
    async fn validate_requires_admin_scope() {
        let app = app_with_auth(read_only_auth_context());
        let response = post_snippets(
            app,
            json!({
                "action": "snippets.validate",
                "params": {
                    "name": "fixture",
                    "body": "async () => ({ ok: true })"
                }
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn receipt_http_adapter_preserves_authenticated_owner_for_admins() {
        use labby_gateway::codemode_journal::{
            StepJournalStore,
            receipts::{SnippetExecutionReceipt, SnippetReceiptOwner},
        };
        use labby_gateway::gateway::manager::GatewayRuntimeHandle;
        use sha2::{Digest, Sha256};
        use std::sync::Arc;
        let home = tempfile::tempdir().unwrap();
        let store = StepJournalStore::open(home.path().join("journal.db"))
            .await
            .unwrap();
        let auth = admin_auth_context();
        let owner_value = json!({"actor":auth.actor_key,"subject":auth.sub,"trusted_local":false});
        let owner_digest = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(serde_json::to_vec(&owner_value).unwrap()))
        );
        let scope = labby_codemode::ToolScope::default().fingerprint();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        store
            .record_snippet_receipt(
                SnippetExecutionReceipt {
                    execution_id: "http-owned".into(),
                    snippet_name: "demo".into(),
                    snippet_digest: "source".into(),
                    input_digest: "input".into(),
                    effective_scope_fingerprint: scope.clone(),
                    runtime_version: "test".into(),
                    tool_schema_digests: None,
                    surface: "api".into(),
                    created_at_ms: now,
                    elapsed_ms: 0,
                    status: "failed".into(),
                    error_kind: Some("synthetic".into()),
                    result_digest: None,
                    result_bytes: None,
                    calls: vec![],
                    tool_calls: 0,
                    omitted_calls: 0,
                    artifacts: vec![],
                },
                SnippetReceiptOwner {
                    owner_key: owner_digest,
                    route_scope: "root".into(),
                    capability_fingerprint: scope,
                },
            )
            .await
            .unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                home.path().join("config.toml"),
                GatewayRuntimeHandle::default(),
            )
            .with_step_journal(Arc::new(store)),
        );
        let state = AppState::from_registry(crate::registry::build_default_registry())
            .with_gateway_manager(manager);
        let owner_app = super::routes(state.clone())
            .router
            .layer(Extension(auth.clone()))
            .with_state(state.clone());
        let result = post_snippets(
            owner_app,
            json!({"action":"snippets.receipt","params":{"execution_id":"http-owned"}}),
        )
        .await;
        assert_eq!(
            result.status(),
            StatusCode::OK,
            "authenticated receipt owner must survive HTTP adaptation"
        );
        let body = axum::body::to_bytes(result.into_body(), 64 * 1024)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("http-owned"));
        let mut other = auth;
        other.sub = "different-admin".into();
        let other_app = super::routes(state.clone())
            .router
            .layer(Extension(other))
            .with_state(state);
        let result = post_snippets(
            other_app,
            json!({"action":"snippets.receipt","params":{"execution_id":"http-owned"}}),
        )
        .await;
        assert_eq!(
            result.status(),
            StatusCode::NOT_FOUND,
            "admin cannot cross receipt owners"
        );
    }

    #[tokio::test]
    async fn remove_dispatches_immediately_after_admin_scope_passes() {
        let app = app_with_auth(admin_auth_context());
        let response = post_snippets(
            app,
            json!({
                "action": "snippets.remove",
                "params": {"name": "fixture"}
            }),
        )
        .await;

        // HTTP no longer gates destructive actions on `confirm` — the request
        // dispatches straight through and fails on the missing snippet, not on
        // a confirmation requirement.
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
