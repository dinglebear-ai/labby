//! Authenticated browser-admin projection of the explicit access owner bootstrap.
//!
//! The surrounding `/v1` auth middleware authenticates the browser session and
//! enforces CSRF before this handler runs. This handler deliberately rejects
//! every non-browser authenticator even when it carries `lab:admin`.

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json, routing};
use labby_auth::{Authenticator, PrincipalLink, VerifiedIdentity};
use serde::{Deserialize, Serialize};

use crate::api::auth_helpers::{log_auth_dispatch, log_auth_dispatch_start, request_id};
use crate::api::error::{ApiError, ToolError};
use crate::api::oauth::AuthContext;
use crate::api::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BootstrapOwnerRequest {
    organization_name: String,
    project_name: String,
}

#[derive(Debug, Serialize)]
struct BootstrapOwnerResponse {
    status: &'static str,
}

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    RouteGroup::empty().route(descriptors().remove(0), routing::post(bootstrap_owner))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new(
            "POST",
            "/",
            "bootstrap_owner",
            "access",
            RouteAuth::BrowserSession,
        )
        .when("OAuth mode only; requires a verified configured admin identity"),
    ]
}

async fn bootstrap_owner(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<VerifiedIdentity>>,
    request: Result<Json<BootstrapOwnerRequest>, JsonRejection>,
) -> Response {
    let start = std::time::Instant::now();
    let req_id = request_id(&headers).map(ToOwned::to_owned);
    let action = "access.bootstrap_owner";
    log_auth_dispatch_start(action, req_id.as_deref());
    let (Some(Extension(auth)), Some(Extension(identity))) = (auth, identity) else {
        log_auth_dispatch(action, req_id.as_deref(), start, Some("forbidden"), None);
        return no_store(stable_error(
            "forbidden",
            "access owner bootstrap requires an authenticated browser admin",
        ));
    };
    if let Err(response) = require_browser_admin(&state, &auth, &identity) {
        log_auth_dispatch(
            action,
            req_id.as_deref(),
            start,
            Some("forbidden"),
            auth.actor_key.as_deref(),
        );
        return no_store(ApiError::new(response).into_response());
    }
    let Json(request) = match request {
        Ok(request) => request,
        Err(_) => {
            log_auth_dispatch(
                action,
                req_id.as_deref(),
                start,
                Some("validation_failed"),
                auth.actor_key.as_deref(),
            );
            return no_store(stable_error(
                "validation_failed",
                "access owner bootstrap request is invalid",
            ));
        }
    };
    // Resolve mutable state only after every authorization gate has passed.
    let (response, operational_failure) = match crate::access::bootstrap_owner(
        &state.access_runtime,
        identity,
        request.organization_name,
        request.project_name,
    )
    .await
    {
        Ok(crate::access::BootstrapOutcome::Created) => (
            (
                StatusCode::CREATED,
                Json(BootstrapOwnerResponse { status: "created" }),
            )
                .into_response(),
            None,
        ),
        Ok(crate::access::BootstrapOutcome::AlreadyApplied) => (
            (
                StatusCode::OK,
                Json(BootstrapOwnerResponse {
                    status: "already_applied",
                }),
            )
                .into_response(),
            None,
        ),
        Err(crate::access::OwnerBootstrapError::Conflict) => (
            stable_error(
                "conflict",
                "access owner bootstrap conflicts with existing state",
            ),
            None,
        ),
        Err(crate::access::OwnerBootstrapError::InvalidInput) => (
            stable_error(
                "validation_failed",
                "access owner bootstrap input is invalid",
            ),
            None,
        ),
        Err(crate::access::OwnerBootstrapError::IdentityNotEligible) => (
            stable_error(
                "forbidden",
                "access owner bootstrap requires an authenticated browser admin",
            ),
            None,
        ),
        Err(crate::access::OwnerBootstrapError::Busy) => (
            stable_error(
                "service_unavailable",
                "access owner bootstrap is unavailable",
            ),
            Some("busy"),
        ),
        Err(crate::access::OwnerBootstrapError::Integrity) => (
            stable_error(
                "service_unavailable",
                "access owner bootstrap is unavailable",
            ),
            Some("integrity"),
        ),
        Err(crate::access::OwnerBootstrapError::Unavailable) => (
            stable_error(
                "service_unavailable",
                "access owner bootstrap is unavailable",
            ),
            Some("runtime_unavailable"),
        ),
    };
    if let Some(failure_reason) = operational_failure {
        tracing::error!(
            surface = "api",
            action,
            request_id = req_id.as_deref(),
            failure_reason,
            "access owner bootstrap operational failure"
        );
    }
    let error_kind =
        (!response.status().is_success()).then_some(if response.status() == StatusCode::CONFLICT {
            "access_bootstrap_conflict"
        } else {
            "access_bootstrap_failed"
        });
    log_auth_dispatch(
        action,
        req_id.as_deref(),
        start,
        error_kind,
        auth.actor_key.as_deref(),
    );
    no_store(response)
}

fn require_browser_admin(
    state: &AppState,
    auth: &AuthContext,
    identity: &VerifiedIdentity,
) -> Result<(), ToolError> {
    if !auth.via_session || identity.authenticator() != Authenticator::BrowserSession {
        return Err(stable_tool_error(
            "forbidden",
            "access owner bootstrap requires an authenticated browser admin",
        ));
    }
    if !matches!(
        identity.principal_link(),
        PrincipalLink::External { subject, .. } if subject == &auth.sub
    ) {
        return Err(stable_tool_error(
            "forbidden",
            "access owner bootstrap identity is inconsistent",
        ));
    }
    if !auth.scopes.iter().any(|scope| scope == "lab:admin") {
        return Err(stable_tool_error(
            "forbidden",
            "access owner bootstrap requires admin scope",
        ));
    }
    let Some(config) = state.auth_config.as_ref() else {
        return Err(stable_tool_error(
            "not_found",
            "access owner bootstrap is only available in OAuth mode",
        ));
    };
    if auth
        .email
        .as_deref()
        .is_none_or(|email| !email.eq_ignore_ascii_case(&config.admin_email))
    {
        return Err(stable_tool_error(
            "forbidden",
            "caller is not the configured admin",
        ));
    }
    Ok(())
}

fn stable_error(kind: &'static str, message: &'static str) -> Response {
    ApiError::new(stable_tool_error(kind, message)).into_response()
}

fn stable_tool_error(kind: &'static str, message: &'static str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: kind.to_owned(),
        message: message.to_owned(),
    }
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_auth::PrincipalLink;

    fn auth(via_session: bool, scopes: &[&str], email: Option<&str>) -> AuthContext {
        AuthContext {
            actor_key: None,
            sub: "subject".into(),
            scopes: scopes.iter().map(|scope| (*scope).to_owned()).collect(),
            issuer: "browser-session".into(),
            via_session,
            csrf_token: Some("csrf".into()),
            email: email.map(ToOwned::to_owned),
        }
    }

    fn browser_identity() -> VerifiedIdentity {
        VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "subject",
        )
        .unwrap()
    }

    fn state() -> AppState {
        let config = labby_auth::config::AuthConfig {
            admin_email: "owner@example.com".into(),
            ..Default::default()
        };
        AppState::new().with_auth_config(config)
    }

    #[test]
    fn guard_requires_browser_session_admin_scope_and_configured_email() {
        let state = state();
        let identity = browser_identity();
        assert!(
            require_browser_admin(
                &state,
                &auth(true, &["lab:admin"], Some("OWNER@example.com")),
                &identity,
            )
            .is_ok()
        );
        assert!(
            require_browser_admin(
                &state,
                &auth(false, &["lab:admin"], Some("owner@example.com")),
                &identity,
            )
            .is_err()
        );
        assert!(
            require_browser_admin(
                &state,
                &auth(true, &["lab:read"], Some("owner@example.com")),
                &identity,
            )
            .is_err()
        );
        assert!(
            require_browser_admin(
                &state,
                &auth(true, &["lab:admin"], Some("other@example.com")),
                &identity,
            )
            .is_err()
        );
    }

    #[test]
    fn guard_rejects_forged_browser_auth_context_with_non_browser_identity() {
        let state = state();
        let identity = VerifiedIdentity::local_credential(
            Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .unwrap();
        assert!(matches!(
            identity.principal_link(),
            PrincipalLink::LocalCredential { .. }
        ));
        assert!(
            require_browser_admin(
                &state,
                &auth(true, &["lab:admin"], Some("owner@example.com")),
                &identity,
            )
            .is_err()
        );
    }

    #[test]
    fn guard_rejects_browser_identity_for_a_different_subject() {
        let state = state();
        let identity = VerifiedIdentity::external(
            Authenticator::BrowserSession,
            "https://accounts.google.com",
            "different-subject",
        )
        .unwrap();
        assert!(
            require_browser_admin(
                &state,
                &auth(true, &["lab:admin"], Some("owner@example.com")),
                &identity,
            )
            .is_err()
        );
    }
}
