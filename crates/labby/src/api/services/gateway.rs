use std::{net::SocketAddr, sync::Arc};

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, header},
    response::IntoResponse,
    routing::post,
};
use labby_auth::VerifiedIdentity;
use serde::Deserialize;
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::access_errors::map_runtime_error;
use crate::dispatch::error::ToolError;

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    let mut descriptors = descriptors().into_iter();
    RouteGroup::empty()
        .route(descriptors.next().unwrap(), post(handle))
        .route(descriptors.next().unwrap(), post(search_tools))
        .route(descriptors.next().unwrap(), post(describe_tool))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "gateway", RouteAuth::V1),
        RouteDescriptor::new(
            "POST",
            "/codemode/tools/search",
            "search_tools",
            "gateway",
            RouteAuth::V1,
        ),
        RouteDescriptor::new(
            "POST",
            "/codemode/tools/describe",
            "describe_tool",
            "gateway",
            RouteAuth::V1,
        ),
    ]
    .into_iter()
    .map(|route| {
        route
            .feature("gateway")
            .when("mounted only when API authentication is configured")
    })
    .collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolSearchRequest {
    query: String,
    #[serde(default = "default_tool_search_limit")]
    limit: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolDescribeRequest {
    target: String,
}

const fn default_tool_search_limit() -> usize {
    50
}

async fn search_tools(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(request): Json<ToolSearchRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let started = std::time::Instant::now();
    private_tool_browser_admin(&auth)
        .map_err(|error| private_tool_error(error, "tools.search", started, &headers))?;
    if request.query.len() > labby_codemode::QUERY_MAX_BYTES {
        return Err(private_tool_error(
            ToolError::InvalidParam {
                message: format!(
                    "query exceeds {} UTF-8 bytes",
                    labby_codemode::QUERY_MAX_BYTES
                ),
                param: "query".into(),
            },
            "tools.search",
            started,
            &headers,
        ));
    }
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(manager_not_wired)
        .map_err(|error| private_tool_error(error, "tools.search", started, &headers))?;
    let subject = auth.as_ref().map(|value| value.0.sub.clone());
    let response = manager
        .search_admin_tools(subject, &request.query, request.limit)
        .await
        .map_err(|error| private_tool_error(error, "tools.search", started, &headers))?;
    tracing::info!(
        surface = "api",
        service = "gateway",
        action = "tools.search",
        elapsed_ms = started.elapsed().as_millis(),
        result_count = response.results.len(),
        response_bytes = serde_json::to_vec(&response).map_or(0, |bytes| bytes.len()),
        request_id = headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok()),
        "dispatch completed"
    );
    Ok(no_referrer(Json(response)))
}

async fn describe_tool(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(request): Json<ToolDescribeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let started = std::time::Instant::now();
    private_tool_browser_admin(&auth)
        .map_err(|error| private_tool_error(error, "tools.describe", started, &headers))?;
    if request.target.len() > labby_codemode::TARGET_MAX_BYTES {
        return Err(private_tool_error(
            ToolError::InvalidParam {
                message: format!(
                    "target exceeds {} UTF-8 bytes",
                    labby_codemode::TARGET_MAX_BYTES
                ),
                param: "target".into(),
            },
            "tools.describe",
            started,
            &headers,
        ));
    }
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(manager_not_wired)
        .map_err(|error| private_tool_error(error, "tools.describe", started, &headers))?;
    let subject = auth.as_ref().map(|value| value.0.sub.clone());
    let response = manager
        .describe_admin_tool(subject, &request.target)
        .await
        .map_err(|error| private_tool_error(error, "tools.describe", started, &headers))?;
    tracing::info!(
        surface = "api",
        service = "gateway",
        action = "tools.describe",
        elapsed_ms = started.elapsed().as_millis(),
        response_bytes = serde_json::to_vec(&response).map_or(0, |bytes| bytes.len()),
        request_id = headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok()),
        "dispatch completed"
    );
    Ok(no_referrer(Json(response)))
}

fn private_tool_browser_admin(auth: &Option<Extension<AuthContext>>) -> Result<(), ToolError> {
    if has_admin_scope(auth.as_ref()) {
        return Ok(());
    }
    Err(ToolError::Forbidden {
        message: "tool browser requires `lab:admin` scope".into(),
        required_scopes: vec!["lab:admin".into()],
    })
}

fn manager_not_wired() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "internal_error".into(),
        message: "gateway manager not wired".into(),
    }
}

fn private_tool_error(
    error: ToolError,
    action: &'static str,
    started: std::time::Instant,
    headers: &HeaderMap,
) -> ApiError {
    let elapsed_ms = started.elapsed().as_millis();
    let request_id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok());
    if error.kind() == "internal_error" {
        let diagnostic =
            labby_runtime::agent_error::redact_secret_like_segments(&error.to_string());
        tracing::error!(
            surface = "api",
            service = "gateway",
            action,
            elapsed_ms,
            kind = error.kind(),
            error = %diagnostic,
            request_id,
            "dispatch failed"
        );
    } else {
        tracing::warn!(
            surface = "api",
            service = "gateway",
            action,
            elapsed_ms,
            kind = error.kind(),
            request_id,
            "dispatch failed"
        );
    }
    ApiError::new(error).with_service_action("gateway", action)
}

fn no_referrer<T: serde::Serialize>(body: Json<T>) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    (headers, body)
}

/// Returns true when the action requires `lab:admin` scope.
///
/// `ActionSpec.requires_admin` in the gateway catalog is the single source of
/// truth: it is `true` exactly for installation-scoped platform
/// administration and `false` for Team-scoped policy actions, which the
/// domain evaluator authorizes by durable Team role instead. No second
/// per-surface table exists.
fn gateway_action_requires_admin(action: &str) -> bool {
    // Universal built-ins are never admin-gated, whether the caller passes them
    // bare (`help`) or service-prefixed (`gateway.help`). The catalog stores them
    // bare, so strip any `gateway.` prefix before the discovery check.
    let bare = action.strip_prefix("gateway.").unwrap_or(action);
    if bare == "help" || bare == "schema" {
        return false;
    }
    crate::dispatch::gateway::ACTIONS
        .iter()
        .find(|spec| spec.name == action)
        .map(|spec| spec.requires_admin)
        // Unknown actions default to admin-required (fail-safe).
        .unwrap_or(true)
}

/// Team authority selector. An absent header is a legitimate installation
/// scope; a present but malformed value is caller-fixable, never silently
/// treated as absent.
fn selected_team_id(headers: &HeaderMap) -> Result<Option<&str>, ToolError> {
    headers
        .get("x-labby-team-id")
        .map(|value| {
            value
                .to_str()
                .ok()
                .filter(|team| !team.trim().is_empty() && team.is_ascii())
                .ok_or_else(|| ToolError::InvalidParam {
                    message: "team context header is invalid".to_owned(),
                    param: "x-labby-team-id".to_owned(),
                })
        })
        .transpose()
}

/// Returns true when the authenticated context carries `lab:admin`.
///
/// T1 fix: when auth IS configured on the HTTP surface, `None` auth means the
/// request arrived without credentials — it must be DENIED admin actions.
/// `is_none_or(...)` is only safe for stdio (which is handled separately via
/// the MCP surface and never reaches this API handler).
fn has_admin_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_some_and(|ctx| ctx.0.scopes.iter().any(|scope| scope == "lab:admin"))
}

fn http_oauth_subject(auth: Option<&AuthContext>, request_subject: Option<&str>) -> Option<String> {
    auth.and_then(|auth| {
        crate::dispatch::oauth_subject::oauth_upstream_subject_for_request(
            Some(auth),
            request_subject,
        )
        .map(|subject| subject.into_owned())
    })
}

fn require_gateway_admin(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
) -> Result<(), ToolError> {
    if !gateway_action_requires_admin(action) || has_admin_scope(auth) {
        return Ok(());
    }

    tracing::warn!(
        surface = "api",
        service = "gateway",
        action,
        request_id,
        kind = "forbidden",
        "gateway action rejected: lab:admin scope required"
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
    identity: Option<Extension<VerifiedIdentity>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    require_gateway_admin(&req.action, request_id, auth.as_ref())?;
    let auth_context = auth.as_ref().ok_or_else(|| {
        ApiError::from(ToolError::Forbidden {
            message: "Gateway operation is not authorized".into(),
            required_scopes: Vec::new(),
        })
    })?;
    let identity = identity
        .ok_or_else(|| {
            ApiError::from(ToolError::Forbidden {
                message: "Gateway operation is not authorized".into(),
                required_scopes: Vec::new(),
            })
        })?
        .0;
    // Installation-scoped platform actions authorize against the real
    // installation id; a process without a bound id reads it from the durable
    // store rather than guessing a literal.
    // Until an installation binding exists the store has no id either; the
    // evaluator authorizes installation scope on platform-administrator
    // status, so the placeholder label only names the resource.
    let installation_id = match state.installation_id.as_deref() {
        Some(id) => id.to_owned(),
        None => match state.access_runtime.store().await {
            Ok(store) => store.installation_id().await.ok().flatten(),
            Err(_) => None,
        }
        .unwrap_or_else(|| "installation".to_owned()),
    };
    let team_id = selected_team_id(&headers)
        .map_err(|error| ApiError::new(error).with_service_action("gateway", &req.action))?;
    let gateway_authority = crate::access::authorize_gateway_action(
        &state.access_runtime,
        identity,
        crate::access::AuthorityCeiling::from_auth_context(&auth_context.0),
        &installation_id,
        team_id,
        &req.action,
    )
    .await
    .map_err(ApiError::from)?;
    let team_id = team_id.map(str::to_owned);
    let access_runtime = Arc::clone(&state.access_runtime);
    let subject = auth.as_ref().map(|value| value.0.sub.clone());
    let auth_for_dispatch = auth.clone();
    let gateway_authority_for_dispatch = gateway_authority.clone();
    let manager = state
        .gateway_manager
        .clone()
        .ok_or_else(|| ToolError::Sdk {
            sdk_kind: "internal_error".to_string(),
            message: "gateway manager not wired".to_string(),
        })?;

    handle_action_with_meta(
        "gateway",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        crate::dispatch::gateway::ACTIONS,
        move |action, params| {
            let manager = Arc::clone(&manager);
            let subject = subject.clone();
            let auth = auth_for_dispatch.clone();
            let gateway_authority = gateway_authority_for_dispatch.clone();
            async move {
                if let Some(team_id) = team_id.as_deref()
                    && crate::dispatch::gateway::team_scoped_gateway_action(&action)
                {
                    let store = access_runtime
                        .store()
                        .await
                        .map_err(|error| map_runtime_error("gateway", error))?;
                    crate::dispatch::gateway::validate_team_scoped_upstream_references(
                        &store, team_id, &action, &params,
                    )
                    .await?;
                }
                let params = crate::access::qualify_team_gateway_params(
                    &action,
                    team_id.as_deref(),
                    params,
                )?;
                let params = inject_gateway_owner(&action, params, subject.as_deref(), request_id);
                if let Some(authority) = gateway_authority.as_ref() {
                    authority.validate_before_external_effect().await?;
                }
                // Unlike trusted stdio MCP, an unauthenticated HTTP request
                // must never inherit the shared gateway OAuth credential.
                let oauth_subject =
                    http_oauth_subject(auth.as_ref().map(|value| &value.0), subject.as_deref());
                let oauth_subject = crate::access::gateway_runtime_subject(
                    &action,
                    team_id.as_deref(),
                    oauth_subject.as_deref(),
                );
                let mut response = crate::dispatch::gateway::dispatch_with_manager_scoped(
                    &manager,
                    &action,
                    params,
                    crate::dispatch::gateway::GatewayEnrichmentScope {
                        route_visible_upstreams: None,
                        oauth_subject,
                    },
                )
                .await?;
                // Only Team-scoped policy responses are projected through the
                // Team namespace; platform responses (upstream lists, OAuth
                // state) must stay complete for an administrator who happens
                // to have a Team selected.
                if crate::dispatch::gateway::team_scoped_gateway_action(&action) {
                    crate::access::filter_team_gateway_projection(
                        team_id.as_deref(),
                        &mut response,
                    );
                }
                Ok(response)
            }
        },
    )
    .await
}

fn inject_gateway_owner(
    action: &str,
    params: Value,
    subject: Option<&str>,
    request_id: Option<&str>,
) -> Value {
    if !crate::dispatch::gateway::shared::action_accepts_runtime_owner(action) {
        return params;
    }
    let Some(mut object) = params.as_object().cloned() else {
        return params;
    };
    let owner = crate::dispatch::gateway::shared::make_api_runtime_owner(subject, request_id);
    let origin = owner.raw.clone();
    // Serialize the owner struct into its JSON shape for the params object.
    // The fields match the GatewayRuntimeOwnerParams shape consumed by dispatch.
    object.insert(
        "owner".to_string(),
        serde_json::json!({
            "surface": owner.surface,
            "subject": owner.subject,
            "request_id": owner.request_id,
            "raw": owner.raw,
        }),
    );
    if let Some(origin) = origin {
        object.insert("origin".to_string(), Value::String(origin));
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::{
        Extension, Router,
        body::Body,
        http::{HeaderMap, HeaderValue, Request, StatusCode, header},
    };
    use serde_json::json;
    use tower::ServiceExt;

    use super::{gateway_action_requires_admin, http_oauth_subject, inject_gateway_owner};

    use crate::api::oauth::AuthContext;
    use crate::api::{
        router::{build_router_with_bearer, build_router_with_external_auth},
        state::AppState,
    };
    use crate::config::{
        LabConfig, UpstreamConfig, VirtualServerConfig, VirtualServerSurfacesConfig,
    };
    use crate::dispatch::gateway::config_store::{
        load_gateway_config, test_gateway_manager, write_gateway_config,
    };
    use crate::dispatch::gateway::manager::{GatewayManager, GatewayRuntimeHandle};
    use crate::registry::build_default_registry;

    // ── Test fixtures ────────────────────────────────────────────────────────

    fn test_manager_with_path() -> (Arc<GatewayManager>, std::path::PathBuf) {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        let path = std::env::temp_dir().join(format!(
            "labby-gateway-api-test-{}-{}.toml",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        (
            Arc::new(test_gateway_manager(
                path.clone(),
                GatewayRuntimeHandle::default(),
            )),
            path,
        )
    }

    fn test_manager() -> Arc<GatewayManager> {
        test_manager_with_path().0
    }

    /// Build a test app WITH bearer auth configured (gateway routes are mounted).
    ///
    /// T1 fix: `test_app()` previously used `build_router_with_bearer(state, None, None)`
    /// which set `needs_auth=false` and, before the fix, mounted gateway routes without
    /// any authentication gate.  Now gateway routes are only mounted when auth IS
    /// configured.  Tests that exercise gateway actions must use an authenticated app.
    async fn authorized_test_state(manager: Arc<GatewayManager>) -> AppState {
        let directory = tempfile::Builder::new()
            .prefix("labby-gateway-access-test-")
            .tempdir_in(std::env::current_dir().expect("test working directory"))
            .expect("access tempdir");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("secure access tempdir");
        }
        let directory = directory.keep();
        let runtime =
            Arc::new(crate::access::AccessRuntime::initialize(directory.join("access.db")).await);
        let identity = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .expect("static bearer identity");
        runtime
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(identity, "Local", "Default")
                    .expect("bootstrap input"),
            )
            .await
            .expect("bootstrap access authority");
        AppState::from_registry(build_default_registry())
            .with_gateway_manager(manager)
            .with_access_runtime(runtime)
    }

    async fn test_app_with_manager(manager: Arc<GatewayManager>) -> Router {
        let state = authorized_test_state(manager).await;
        // Use a static bearer token so needs_auth=true and /v1/gateway is mounted.
        build_router_with_bearer(state, Some("test-token".into()), None)
    }

    async fn test_app() -> Router {
        test_app_with_manager(test_manager()).await
    }

    /// App with bearer auth + an injected AuthContext (for scope-gated tests).
    async fn test_app_with_auth_context(manager: Arc<GatewayManager>, auth: AuthContext) -> Router {
        test_app_with_manager(manager).await.layer(Extension(auth))
    }

    /// Mount ONLY the gateway route group with a layered `AuthContext` and no
    /// bearer-auth middleware — exercises the per-action scope gate in isolation.
    ///
    /// The full-router static bearer path always injects `lab:admin`, so it
    /// cannot model a non-admin caller. Mounting `services::gateway::routes`
    /// directly (mirroring `upstream_oauth_routes_require_admin_scope`) lets the
    /// layered read-only context survive to the handler's scope gate.
    async fn gateway_routes_with_auth_context(
        manager: Arc<GatewayManager>,
        auth: AuthContext,
    ) -> Router {
        let state = authorized_test_state(manager).await;
        let identity = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .expect("static bearer identity");
        super::routes(state.clone())
            .router
            .layer(Extension(auth))
            .layer(Extension(identity))
            .with_state(state)
    }

    /// POST to a directly-mounted gateway route group (no bearer header).
    async fn post_gateway_routes(app: Router, body: serde_json::Value) -> axum::response::Response {
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

    fn admin_auth_context() -> AuthContext {
        AuthContext {
            sub: "admin-user".to_string(),
            actor_key: None,
            scopes: vec!["lab:admin".to_string()],
            issuer: "test".to_string(),
            via_session: false,
            csrf_token: None,
            email: Some("admin@example.com".to_string()),
        }
    }

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

    #[test]
    fn gateway_owner_injection_skips_strict_read_only_actions() {
        let params = json!({"upstream": "fixture"});
        let enriched = inject_gateway_owner(
            "gateway.skills.list",
            params.clone(),
            Some("admin-user"),
            Some("request-1"),
        );
        assert_eq!(enriched, params);
    }

    #[test]
    fn gateway_owner_injection_preserves_mutation_provenance() {
        let enriched = inject_gateway_owner(
            "gateway.add",
            json!({"spec": {"name": "fixture"}}),
            Some("admin-user"),
            Some("request-1"),
        );
        assert_eq!(enriched["owner"]["surface"], "api");
        assert_eq!(enriched["owner"]["subject"], "admin-user");
        assert_eq!(enriched["owner"]["request_id"], "request-1");
        assert_eq!(enriched["origin"], "api:admin-user:request-1");
    }

    #[cfg(feature = "skills")]
    #[tokio::test]
    async fn gateway_skills_list_api_keeps_strict_action_params_clean() {
        let response = post_gateway_as_admin(
            test_manager(),
            json!({"action": "gateway.skills.list", "params": {}}),
        )
        .await;

        assert_ne!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        if !response.status().is_success() {
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            let payload: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_ne!(payload["kind"], "invalid_param");
            assert!(!payload.to_string().contains("unknown field `owner`"));
        }
    }

    #[test]
    fn http_oauth_subject_fails_closed_without_verified_auth_context() {
        assert!(http_oauth_subject(None, None).is_none());
        assert!(http_oauth_subject(None, Some("forged-subject")).is_none());

        let admin = admin_auth_context();
        assert_eq!(
            http_oauth_subject(Some(&admin), Some("admin-user")).as_deref(),
            Some(crate::dispatch::gateway::SHARED_GATEWAY_OAUTH_SUBJECT)
        );

        let reader = read_only_auth_context();
        assert_eq!(
            http_oauth_subject(Some(&reader), Some("read-only-user")).as_deref(),
            Some("read-only-user")
        );
    }

    // ── Request helpers ──────────────────────────────────────────────────────

    async fn post_gateway(app: Router, body: serde_json::Value) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/gateway")
                .header(header::CONTENT_TYPE, "application/json")
                // Include the static bearer token so the auth middleware passes.
                .header(header::AUTHORIZATION, "Bearer test-token")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response")
    }

    /// Post to /v1/gateway as admin (bearer token + lab:admin AuthContext injected).
    async fn post_gateway_as_admin(
        manager: Arc<GatewayManager>,
        body: serde_json::Value,
    ) -> axum::response::Response {
        let app = test_app_with_auth_context(manager, admin_auth_context()).await;
        post_gateway(app, body).await
    }

    async fn get_gateway_actions(app: Router) -> axum::response::Response {
        app.oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/gateway/actions")
                .header(header::AUTHORIZATION, "Bearer test-token")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response")
    }

    // ── T1: Security posture tests ───────────────────────────────────────────

    /// T1 (Critical): when auth IS configured, a request arriving with NO
    /// AuthContext (no bearer token, no session) must be DENIED on all admin
    /// gateway actions — not silently allowed.
    #[tokio::test]
    async fn gateway_admin_actions_refused_when_no_auth_context_present() {
        // App has bearer auth configured (gateway IS mounted), but the request
        // carries no Authorization header → no AuthContext in extensions.
        let app = test_app().await;

        for action in [
            "gateway.list",
            "gateway.get",
            "gateway.status",
            "gateway.add",
            "gateway.reload",
            "gateway.oauth.probe",
            "gateway.mcp.cleanup",
            "gateway.service_config.get",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/gateway")
                        .header(header::CONTENT_TYPE, "application/json")
                        // No Authorization header → no AuthContext
                        .body(Body::from(
                            json!({
                                "action": action,
                                "params": {
                                    "confirm": true,
                                    "name": "fixture",
                                    "spec": {"name": "fixture", "url": "https://fixture.example.com/mcp"}
                                }
                            })
                            .to_string(),
                        ))
                        .expect("request"),
                )
                .await
                .expect("response");
            // Bearer auth middleware rejects unauthenticated requests before the
            // gateway handler, so we accept either 401 (middleware) or 403 (handler).
            let status = response.status();
            assert!(
                status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
                "action `{action}` with no auth should be 401 or 403, got {status}"
            );
        }