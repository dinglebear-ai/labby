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
    handle_action_with_meta(
        "setup",
        "api",
        dispatch_meta,
        req,
        ACTIONS,
        |action, params| async move {
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
