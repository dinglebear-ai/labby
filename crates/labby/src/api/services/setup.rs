//! HTTP route group for the `setup` Bootstrap orchestrator.
//!
//! Mounted at `/v1/setup` behind the host-validation layer. Public hosts may
//! be allowlisted for authenticated setup operations, but local-only actions
//! require both a loopback socket peer and a loopback `Host` header.

use std::net::SocketAddr;

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::{HeaderMap, header::HOST},
    routing::post,
};
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{
    ApiDispatchMeta, dispatch_meta_from_headers, handle_action_with_meta,
};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::error::ToolError;
use crate::dispatch::setup::ACTIONS;

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    RouteGroup::empty().route(descriptors().remove(0), post(handle))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![RouteDescriptor::new("POST", "/", "handle", "setup", RouteAuth::V1).host_validated()]
}

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<labby_auth::VerifiedIdentity>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let request_id = headers.get("x-request-id").and_then(|v| v.to_str().ok());
    let peer_addr = peer.as_ref().map(|Extension(ConnectInfo(addr))| *addr);
    let has_local_capability = request_has_local_capability(peer_addr, &headers);
    let caller = setup_caller(
        &state,
        auth.as_ref().map(|Extension(context)| context),
        identity.as_ref().map(|Extension(identity)| identity),
        has_local_capability,
    );
    require_setup_admin(&req.action, request_id, auth.as_ref(), has_local_capability)?;
    if local_only_action(&req.action) && !has_local_capability {
        return Err(ApiError::new(ToolError::Sdk {
            sdk_kind: "forbidden".into(),
            message: format!("setup action `{}` is only available locally", req.action),
        }));
    }
    let mut dispatch_meta =
        dispatch_meta_from_headers(&headers, auth.as_ref().map(|value| &value.0), peer_addr);
    apply_local_bootstrap_capability(&mut dispatch_meta, &req.action, has_local_capability);
    let access_runtime = state.access_runtime.clone();
    let authenticated_identity = identity.map(|Extension(identity)| identity);
    let request_auth = auth.as_ref().map(|Extension(auth)| auth.clone());
    let request_headers = headers.clone();
    #[cfg(feature = "gateway")]
    let gateway_manager = state.gateway_manager.clone();
    #[cfg(feature = "gateway")]
    let installation_id = state.installation_id.as_deref().map(str::to_owned);
    handle_action_with_meta(
        "setup",
        "api",
        dispatch_meta,
        req,
        ACTIONS,
        move |action, params| async move {
            if matches!(action.as_str(), "clients.session.start" | "clients.session.revoke") {
                super::require_session_csrf(&action, &request_headers, request_auth.as_ref())?;
                let identity = authenticated_identity.ok_or_else(|| ToolError::Forbidden { message: "Client observation requires authenticated identity".into(), required_scopes: vec![] })?;
                let store = access_runtime.store().await.map_err(|error| crate::dispatch::access_errors::map_runtime_error("setup", error))?;
                if action == "clients.session.start" { return crate::dispatch::setup::client_evidence::start(store, identity, params).await; }
                if params.as_object().is_none_or(|row|!row.is_empty()) { return Err(ToolError::InvalidParam { param: "params".into(), message: "Revocation uses the authenticated user's session; no parameters are accepted".into() }); }
                return crate::dispatch::setup::client_evidence::revoke(store, identity).await;
            }
            if matches!(action.as_str(), "readiness.state" | "readiness.clients.defer") {
                if params.as_object().is_none_or(|params| !params.is_empty()) {
                    return Err(ToolError::InvalidParam { param: "params".into(), message: "Readiness always uses the authenticated user; no parameters are accepted".into() });
                }
                if action == "readiness.clients.defer" {
                    super::require_session_csrf(&action, &request_headers, request_auth.as_ref())?;
                }
                let identity = authenticated_identity.ok_or_else(|| ToolError::Forbidden { message: "First-use evidence requires host-established identity".into(), required_scopes: vec![] })?;
                let store = access_runtime.store().await.map_err(|error| crate::dispatch::access_errors::map_runtime_error("setup", error))?;
                return if action == "readiness.clients.defer" {
                    crate::dispatch::setup::readiness::defer_clients_for_identity(store, identity).await
                } else {
                    crate::dispatch::setup::readiness::state_for_identity(store, identity).await
                };
            }
            #[cfg(feature = "gateway")]
            if matches!(action.as_str(), "mcp.verification.tools" | "mcp.verification.call") {
                if request_headers.contains_key("x-labby-team-id") || ["owner", "owner_kind", "owner_id", "team_id", "principal_id"].iter().any(|key| params.get(*key).is_some()) {
                    return Err(ToolError::Forbidden { message: "First-tool verification requires the installation gateway context".into(), required_scopes: vec![] });
                }
                let identity = authenticated_identity.ok_or_else(|| ToolError::Forbidden { message: "First-tool verification requires host-established identity".into(), required_scopes: vec![] })?;
                let auth = request_auth.as_ref().ok_or_else(|| ToolError::Forbidden { message: "First-tool verification requires authentication".into(), required_scopes: vec![] })?;
                if action == "mcp.verification.call" { super::require_session_csrf(&action, &request_headers, Some(auth))?; }
                let store = access_runtime.store().await.map_err(|error| crate::dispatch::access_errors::map_runtime_error("setup", error))?;
                let installation = match installation_id { Some(id) => id, None => store.installation_id().await.ok().flatten().unwrap_or_else(|| "installation".into()) };
                let authority = crate::access::authorize_gateway_action(&access_runtime, identity.clone(), crate::access::AuthorityCeiling::from_auth_context(auth), &installation, None, &format!("gateway.setup.{action}")).await?.ok_or_else(|| ToolError::Forbidden { message: "Gateway verification is not authorized".into(), required_scopes: vec![] })?;
                authority.validate_before_external_effect().await?;
                let manager = gateway_manager.ok_or_else(|| ToolError::Sdk { sdk_kind: "service_unavailable".into(), message: "The gateway runtime is unavailable".into() })?;
                if action == "mcp.verification.tools" { return crate::dispatch::setup::mcp_verification::tools(&manager, &params).await; }
                let principal = store.resolve_file_stash_principal(identity).await.map_err(|error| crate::dispatch::access_errors::map_store_error("setup", error, || ToolError::Forbidden { message: "Gateway verification is not authorized".into(), required_scopes: vec![] }))?;
                return crate::dispatch::setup::mcp_verification::call(&manager, store, principal.as_str().into(), params, authority).await;
            }
            crate::dispatch::setup::dispatch_for_caller(caller, &action, params).await
        },
    )
    .await
}

/// Report transport evidence to shared setup dispatch, which owns the decision
/// of who may stage or commit authentication environment keys.
fn setup_caller(
    state: &AppState,
    auth: Option<&AuthContext>,
    identity: Option<&labby_auth::VerifiedIdentity>,
    has_local_capability: bool,
) -> crate::dispatch::setup::SetupCaller {
    use crate::dispatch::setup::{SetupCaller, SetupCallerEvidence};
    let configured_admin_emails = state
        .oauth_state
        .as_ref()
        .map(|auth_state| auth_state.config.admin_emails.as_slice())
        .or_else(|| {
            state
                .auth_config
                .as_ref()
                .map(|config| config.admin_emails.as_slice())
        })
        .unwrap_or_default();
    SetupCaller::classify(SetupCallerEvidence {
        local_capability: has_local_capability,
        operator_credential: identity.is_some_and(|identity| {
            matches!(
                identity.authenticator(),
                labby_auth::Authenticator::StaticBearer | labby_auth::Authenticator::UnixPeer
            )
        }),
        session_email: auth
            .filter(|context| context.via_session)
            .and_then(|context| context.email.as_deref()),
        configured_admin_emails,
    })
}

fn setup_action_requires_admin(action: &str) -> bool {
    let bare = action.strip_prefix("setup.").unwrap_or(action);
    if bare == "help" || bare == "schema" {
        return false;
    }
    ACTIONS
        .iter()
        .find(|spec| spec.name == action)
        .map(|spec| spec.requires_admin)
        .unwrap_or(true)
}

fn has_admin_scope(auth: Option<&Extension<AuthContext>>) -> bool {
    auth.is_some_and(|ctx| ctx.0.scopes.iter().any(|scope| scope == "lab:admin"))
}

fn require_setup_admin(
    action: &str,
    request_id: Option<&str>,
    auth: Option<&Extension<AuthContext>>,
    has_local_capability: bool,
) -> Result<(), ToolError> {
    let local_first_run_capability = local_bootstrap_capability(action, has_local_capability);
    if !setup_action_requires_admin(action) || has_admin_scope(auth) || local_first_run_capability {
        return Ok(());
    }

    tracing::warn!(
        surface = "api",
        service = "setup",
        action,
        request_id,
        kind = "forbidden",
        "setup action rejected: lab:admin scope required"
    );
    Err(ToolError::Sdk {
        sdk_kind: "forbidden".to_string(),
        message: format!("action `{action}` requires `lab:admin` scope"),
    })
}

fn local_bootstrap_capability(action: &str, has_local_capability: bool) -> bool {
    action.strip_prefix("setup.").unwrap_or(action) == "bootstrap" && has_local_capability
}

fn apply_local_bootstrap_capability(
    dispatch_meta: &mut ApiDispatchMeta<'_>,
    action: &str,
    has_local_capability: bool,
) {
    if local_bootstrap_capability(action, has_local_capability) {
        // The route-specific gate accepts this as the first-run capability.
        // Carry that decision through the shared requires_admin gate for this
        // action only; otherwise it would reject the same request a second time.
        dispatch_meta.is_lab_admin = Some(true);
    }
}

fn local_only_action(action: &str) -> bool {
    let bare = action.strip_prefix("setup.").unwrap_or(action);
    crate::dispatch::setup::LOCAL_ONLY_ACTIONS.contains(&bare)
}

fn request_is_loopback(peer: Option<SocketAddr>) -> bool {
    peer.is_some_and(|addr| addr.ip().is_loopback())
}

fn request_has_local_capability(peer: Option<SocketAddr>, headers: &HeaderMap) -> bool {
    request_is_loopback(peer)
        && headers
            .get(HOST)
            .and_then(|value| value.to_str().ok())
            .is_some_and(crate::api::host_validation::is_loopback_host_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(scopes: &[&str]) -> Extension<AuthContext> {
        Extension(AuthContext {
            sub: "tester@example.com".to_string(),
            actor_key: None,
            issuer: "test".to_string(),
            scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
            via_session: true,
            email: Some("tester@example.com".to_string()),
            csrf_token: None,
        })
    }

    #[cfg(feature = "gateway")]
    #[tokio::test]
    async fn mcp_verification_http_payloads_reach_authenticated_operations() {
        use axum::{Router, body::Body, http::Request};
        use std::sync::Arc;
        use tower::ServiceExt;
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let root = directory.path().canonicalize().unwrap();
        let runtime =
            Arc::new(crate::access::AccessRuntime::initialize(root.join("access.db")).await);
        let identity = labby_auth::VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            "https://accounts.google.com",
            "tester@example.com",
        )
        .unwrap();
        runtime
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(identity.clone(), "Local", "Default")
                    .unwrap(),
            )
            .await
            .unwrap();
        let manager = Arc::new(
            crate::dispatch::gateway::config_store::test_gateway_manager(
                root.join("config.toml"),
                crate::dispatch::gateway::manager::GatewayRuntimeHandle::default(),
            ),
        );
        let state = AppState::new()
            .with_access_runtime(runtime)
            .with_gateway_manager(manager);
        let mut context = auth(&["lab:admin"]).0;
        context.issuer = "https://accounts.google.com".into();
        context.csrf_token = Some("fixture-csrf".into());
        let router = Router::new()
            .route("/v1/setup", post(handle))
            .with_state(state)
            .layer(Extension(identity))
            .layer(Extension(context));
        for action in ["mcp.verification.tools", "mcp.verification.call"] {
            for prefix in ["", "setup."] {
                let payload = serde_json::json!({"action":format!("{prefix}{action}"), "params":{
                    "name":"invalid:name", "expected_url":"https://example.org/mcp",
                    "tool":"version", "expected_fingerprint":"a".repeat(64), "arguments":{}, "approved":true
                }});
                let mut payload = payload;
                if action.ends_with("tools") {
                    for key in ["tool", "expected_fingerprint", "arguments", "approved"] {
                        payload["params"].as_object_mut().unwrap().remove(key);
                    }
                }
                let response = router
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method("POST")
                            .uri("/v1/setup")
                            .header("content-type", "application/json")
                            .header("x-csrf-token", "fixture-csrf")
                            .body(Body::from(payload.to_string()))
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert!(!response.status().is_success());
                let bytes = axum::body::to_bytes(response.into_body(), 16384)
                    .await
                    .unwrap();
                let body = String::from_utf8(bytes.to_vec()).unwrap();
                if prefix.is_empty() {
                    assert!(
                        body.contains("Server identity exceeds its supported bounds"),
                        "{action}: {body}"
                    );
                } else {
                    assert!(
                        body.contains("Unknown action") || body.contains("unknown_action"),
                        "{action}: {body}"
                    );
                }
            }
        }
    }

    #[test]
    fn credential_bootstrap_and_proxy_configuration_are_local_only() {
        assert!(local_only_action("bootstrap"));
        assert!(local_only_action("setup.bootstrap"));
        assert!(local_only_action("proxy.configure"));
        assert!(!local_only_action("draft.commit"));

        assert!(request_is_loopback(Some("127.0.0.1:1234".parse().unwrap())));
        assert!(request_is_loopback(Some("[::1]:1234".parse().unwrap())));
        assert!(!request_is_loopback(Some("10.0.0.5:1234".parse().unwrap())));
        assert!(!request_is_loopback(None));
    }

    #[test]
    fn readiness_uses_authenticated_self_access_without_platform_admin_requirement() {
        let reader = auth(&["lab:read"]);
        assert!(require_setup_admin("readiness.state", None, Some(&reader), false).is_ok());
        assert!(require_setup_admin("readiness.clients.defer", None, Some(&reader), false).is_ok());
        // Host-established identity is enforced separately inside the adapter.
        assert!(!local_bootstrap_capability("readiness.state", true));
        assert!(!local_bootstrap_capability("readiness.clients.defer", true));
    }

    #[test]
    fn first_tool_verification_retains_the_platform_admin_transport_gate() {
        for action in ["mcp.verification.tools", "mcp.verification.call"] {
            assert!(require_setup_admin(action, None, Some(&auth(&["lab:read"])), false).is_err());
            assert!(require_setup_admin(action, None, Some(&auth(&["lab:admin"])), false).is_ok());
            assert!(!local_bootstrap_capability(action, true));
        }
    }

    #[test]
    fn setup_settings_mutations_require_admin_scope_on_api_gate() {
        let read_only = auth(&["lab:read"]);
        for action in [
            "settings.update",
            "settings.config.update",
            "settings.env.update",
        ] {
            assert!(require_setup_admin(action, None, Some(&read_only), false).is_err());
            assert!(require_setup_admin(action, None, Some(&auth(&["lab:admin"])), false).is_ok());
        }
    }

    #[test]
    fn bootstrap_allows_unauthenticated_loopback_first_run_only() {
        require_setup_admin("bootstrap", None, None, true).expect("local first-run capability");
        assert_eq!(
            require_setup_admin("bootstrap", None, None, false)
                .expect_err("remote bootstrap denied")
                .kind(),
            "forbidden"
        );
        assert!(local_bootstrap_capability("bootstrap", true));
        assert!(local_bootstrap_capability("setup.bootstrap", true));
        assert!(!local_bootstrap_capability("bootstrap", false));
        assert!(!local_bootstrap_capability("settings.update", true));

        let mut local_bootstrap_meta = ApiDispatchMeta::default();
        apply_local_bootstrap_capability(&mut local_bootstrap_meta, "bootstrap", true);
        assert_eq!(local_bootstrap_meta.is_lab_admin, Some(true));

        let mut remote_bootstrap_meta = ApiDispatchMeta::default();
        apply_local_bootstrap_capability(&mut remote_bootstrap_meta, "bootstrap", false);
        assert_eq!(remote_bootstrap_meta.is_lab_admin, None);

        let mut local_settings_meta = ApiDispatchMeta::default();
        apply_local_bootstrap_capability(&mut local_settings_meta, "settings.update", true);
        assert_eq!(local_settings_meta.is_lab_admin, None);
    }

    #[test]
    fn loopback_proxy_peer_with_public_host_has_no_local_capability() {
        let peer = Some("127.0.0.1:1234".parse().unwrap());
        let mut headers = HeaderMap::new();
        headers.insert(HOST, "lab.example.com".parse().unwrap());
        assert!(!request_has_local_capability(peer, &headers));

        headers.insert(HOST, "localhost:8765".parse().unwrap());
        assert!(request_has_local_capability(peer, &headers));
    }
}
