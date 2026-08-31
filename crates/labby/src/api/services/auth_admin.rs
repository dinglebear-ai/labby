//! HTTP route group for admin-only allowlist management.
//!
//! All endpoints require a browser session with an email matching the
//! configured `admin_email`. JWT bearer callers receive 403 even if they
//! hold a valid token — these endpoints are intentionally browser-session-only.
//!
//! Routes:
//! - `GET  /v1/auth/allowed-emails`          → list entries (200)
//! - `POST /v1/auth/allowed-emails`           → add entry (201)
//! - `DELETE /v1/auth/allowed-emails/:email`  → remove entry (204, idempotent)
//!
//! CSRF for mutations is enforced by the /v1 auth middleware before these
//! handlers are reached — no manual CSRF check is needed here.

use std::time::Instant;

use axum::Extension;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Json, routing};
use serde::Deserialize;
use serde_json::json;

use crate::api::auth_helpers::{log_auth_dispatch, log_auth_dispatch_start, request_id};
use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::state::AppState;
use crate::dispatch::error::ToolError;
use labby_auth::util::{fingerprint, now_unix};

// ── email validation ─────────────────────────────────────────────────────────

const MAX_EMAIL_LENGTH: usize = 320;

/// Validate and normalize an email for storage.
///
/// Order: trim → empty check → length check → whitespace check → `@` check → lowercase.
fn validate_email(raw: &str) -> Result<String, ToolError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ToolError::Sdk {
            sdk_kind: "validation_failed".to_string(),
            message: "email must not be empty".to_string(),
        });
    }
    if trimmed.len() > MAX_EMAIL_LENGTH {
        return Err(ToolError::Sdk {
            sdk_kind: "validation_failed".to_string(),
            message: format!(
                "email must be at most {MAX_EMAIL_LENGTH} characters (got {})",
                trimmed.len()
            ),
        });
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(ToolError::Sdk {
            sdk_kind: "validation_failed".to_string(),
            message: "email must not contain whitespace".to_string(),
        });
    }
    if !trimmed.contains('@') {
        return Err(ToolError::Sdk {
            sdk_kind: "validation_failed".to_string(),
            message: "email must contain '@'".to_string(),
        });
    }
    Ok(trimmed.to_ascii_lowercase())
}

// ── admin guard ───────────────────────────────────────────────────────────────

/// Verify the caller is a browser-session user whose email matches `admin_email`.
///
/// Returns `Err(forbidden)` for:
/// - JWT bearer callers (`via_session == false`)
/// - Browser-session callers with no email claim
/// - Email that does not match `admin_email` (case-insensitive)
fn require_admin(ctx: &AuthContext, admin_email: &str) -> Result<(), ToolError> {
    if !ctx.via_session {
        return Err(ToolError::Sdk {
            sdk_kind: "forbidden".to_string(),
            message: "this endpoint requires a browser session, not a bearer token".to_string(),
        });
    }
    let Some(ref email) = ctx.email else {
        return Err(ToolError::Sdk {
            sdk_kind: "forbidden".to_string(),
            message: "session has no email — cannot verify admin access".to_string(),
        });
    };
    if !email.eq_ignore_ascii_case(admin_email) {
        return Err(ToolError::Sdk {
            sdk_kind: "forbidden".to_string(),
            message: "caller is not the configured admin".to_string(),
        });
    }
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Extract `oauth_state` or return a 404 `ToolError`.
///
/// 404 (not 503) is used when OAuth is not configured — the endpoint simply
/// does not exist in bearer-only mode.
fn require_oauth_state(state: &AppState) -> Result<&labby_auth::state::AuthState, ToolError> {
    state.oauth_state.as_deref().ok_or_else(|| ToolError::Sdk {
        sdk_kind: "not_found".to_string(),
        message: "allowlist management is only available in OAuth mode".to_string(),
    })
}

/// Extract `admin_email` from `auth_config`.
///
/// Returns an `internal_error` ToolError if auth config is not mounted.
fn require_admin_email(state: &AppState) -> Result<&str, ToolError> {
    state
        .auth_config
        .as_ref()
        .map(|cfg| cfg.admin_email.as_str())
        .ok_or_else(|| ToolError::internal_message("auth config not mounted"))
}

/// Map `AuthError` to a `ToolError` preserving its stable kind.
fn auth_err(e: labby_auth::error::AuthError) -> ToolError {
    ToolError::Sdk {
        sdk_kind: e.kind().to_string(),
        message: e.to_string(),
    }
}

fn no_store(response: Response) -> Response {
    let mut response = response;
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, no-store"),
    );
    response
}

// ── route registration ────────────────────────────────────────────────────────

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    let mut descriptors = descriptors().into_iter();
    RouteGroup::empty()
        .route(
            descriptors.next().unwrap(),
            routing::get(list_allowed_emails),
        )
        .route(
            descriptors.next().unwrap(),
            routing::post(add_allowed_email),
        )
        .route(
            descriptors.next().unwrap(),
            routing::delete(delete_allowed_email),
        )
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("GET", "/", "list_allowed_emails", "auth", RouteAuth::V1),
        RouteDescriptor::new("POST", "/", "add_allowed_email", "auth", RouteAuth::V1),
        RouteDescriptor::new(
            "DELETE",
            "/{email}",
            "delete_allowed_email",
            "auth",
            RouteAuth::V1,
        ),
    ]
}

// ── handlers ──────────────────────────────────────────────────────────────────

/// `GET /v1/auth/allowed-emails`
///
/// Returns `{ "entries": [{email, added_by, created_at}] }` (200).
async fn list_allowed_emails(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    let start = Instant::now();
    let req_id = request_id(&headers).map(ToOwned::to_owned);
    let action = "auth.allowed_user.list";
    let actor_key = auth.actor_key.as_deref();
    log_auth_dispatch_start(action, req_id.as_deref());

    // Require admin.
    let admin_email = match require_admin_email(&state) {
        Ok(e) => e,
        Err(err) => {
            log_auth_dispatch(
                action,
                req_id.as_deref(),
                start,
                Some(err.kind()),
                actor_key,
            );
            return no_store(ApiError::new(err).into_response());
        }
    };
    if let Err(err) = require_admin(&auth, admin_email) {
        log_auth_dispatch(
            action,
            req_id.as_deref(),
            start,
            Some(err.kind()),
            actor_key,
        );
        return no_store(ApiError::new(err).into_response());
    }

    // Require oauth_state.
    let auth_state = match require_oauth_state(&state) {
        Ok(s) => s,
        Err(err) => {
            log_auth_dispatch(
                action,
                req_id.as_deref(),
                start,
                Some(err.kind()),
                actor_key,
            );
            return no_store(ApiError::new(err).into_response());
        }
    };

    let entries = match auth_state.store.list_allowed_users().await {
        Ok(rows) => rows,
        Err(err) => {
            let kind = err.kind();
            tracing::error!(
                surface = "api",
                service = "auth",
                action,
                kind,
                elapsed_ms = start.elapsed().as_millis(),
                "auth.allowed_user.list failed"
            );
            log_auth_dispatch(action, req_id.as_deref(), start, Some(kind), actor_key);
            return no_store(ApiError::new(auth_err(err)).into_response());
        }
    };

    log_auth_dispatch(action, req_id.as_deref(), start, None, actor_key);
    no_store((StatusCode::OK, Json(json!({ "entries": entries }))).into_response())
}

#[derive(Deserialize)]
struct AddEmailBody {
    email: String,
}

/// `POST /v1/auth/allowed-emails`
///
/// Body: `{ "email": "alice@example.com" }`
/// Returns `{ "entry": {email, added_by, created_at} }` (201).
async fn add_allowed_email(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<AddEmailBody>,
) -> Response {
    let start = Instant::now();
    let req_id = request_id(&headers).map(ToOwned::to_owned);
    let action = "auth.allowed_user.add";
    let actor_key = auth.actor_key.as_deref();
    log_auth_dispatch_start(action, req_id.as_deref());

    // Require admin.
    let admin_email = match require_admin_email(&state) {
        Ok(e) => e,
        Err(err) => {
            log_auth_dispatch(
                action,
                req_id.as_deref(),
                start,
                Some(err.kind()),
                actor_key,
            );
            return no_store(ApiError::new(err).into_response());
        }
    };
    if let Err(err) = require_admin(&auth, admin_email) {
        log_auth_dispatch(
            action,
            req_id.as_deref(),
            start,
            Some(err.kind()),
            actor_key,
        );
        return no_store(ApiError::new(err).into_response());
    }

    // Require oauth_state.
    let auth_state = match require_oauth_state(&state) {
        Ok(s) => s,
        Err(err) => {
            log_auth_dispatch(
                action,
                req_id.as_deref(),
                start,
                Some(err.kind()),
                actor_key,
            );
            return no_store(ApiError::new(err).into_response());
        }
    };

    // Validate email before any side effects.
    let email = match validate_email(&body.email) {
        Ok(e) => e,
        Err(err) => {
            log_auth_dispatch(
                action,
                req_id.as_deref(),
                start,
                Some(err.kind()),
                actor_key,
            );
            return no_store(ApiError::new(err).into_response());
        }
    };

    let email_fp = fingerprint(&email);
    let added_by = auth.sub.clone();
    let created_at = now_unix();

    // Log intent before the destructive/mutating operation.
    tracing::info!(
        surface = "api",
        service = "auth",
        action,
        email_fp,
        "auth.allowed_user.add intent"
    );

    match auth_state
        .store
        .add_allowed_user(&email, &added_by, created_at)
        .await
    {
        Ok(()) => {}
        Err(err) => {
            let kind = err.kind();
            tracing::warn!(
                surface = "api",
                service = "auth",
                action,
                email_fp,
                kind,
                elapsed_ms = start.elapsed().as_millis(),
                "auth.allowed_user.add failed"
            );
            log_auth_dispatch(action, req_id.as_deref(), start, Some(kind), actor_key);
            return no_store(ApiError::new(auth_err(err)).into_response());
        }
    }

    let entry = labby_auth::types::AllowedUserRow {
        email: email.clone(),
        added_by,
        created_at,
    };

    tracing::info!(
        surface = "api",
        service = "auth",
        action,
        email_fp,
        elapsed_ms = start.elapsed().as_millis(),
        "auth.allowed_user.add complete"
    );
    log_auth_dispatch(action, req_id.as_deref(), start, None, actor_key);
    no_store((StatusCode::CREATED, Json(json!({ "entry": entry }))).into_response())
}

/// `DELETE /v1/auth/allowed-emails/:email`
///
/// Returns 204 (idempotent — returns 204 even if the email was not present).
async fn delete_allowed_email(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(auth): Extension<AuthContext>,
    Path(raw_email): Path<String>,
) -> Response {
    let start = Instant::now();
    let req_id = request_id(&headers).map(ToOwned::to_owned);
    let action = "auth.allowed_user.remove";
    let actor_key = auth.actor_key.as_deref();
    log_auth_dispatch_start(action, req_id.as_deref());

    // Require admin.
    let admin_email = match require_admin_email(&state) {
        Ok(e) => e,
        Err(err) => {
            log_auth_dispatch(
                action,
                req_id.as_deref(),
                start,
                Some(err.kind()),
                actor_key,
            );
            return no_store(ApiError::new(err).into_response());
        }
    };
    if let Err(err) = require_admin(&auth, admin_email) {
        log_auth_dispatch(
            action,
            req_id.as_deref(),
            start,
            Some(err.kind()),
            actor_key,
        );
        return no_store(ApiError::new(err).into_response());
    }

    // Require oauth_state.
    let auth_state = match require_oauth_state(&state) {
        Ok(s) => s,
        Err(err) => {
            log_auth_dispatch(
                action,
                req_id.as_deref(),
                start,
                Some(err.kind()),
                actor_key,
            );
            return no_store(ApiError::new(err).into_response());
        }
    };

    // Normalize email from URL path.
    let email = raw_email.trim().to_ascii_lowercase();
    let email_fp = fingerprint(&email);

    // Log intent before the mutating operation.
    tracing::info!(
        surface = "api",
        service = "auth",
        action,
        email_fp,
        "auth.allowed_user.remove intent"
    );

    // Preflight the runtime boundary and exclude new OAuth client/peer
    // publication before committing the durable revocation. Holding this
    // write guard through the drain closes the DB-to-runtime reuse window.
    #[cfg(feature = "gateway")]
    let gateway_manager = if email.eq_ignore_ascii_case(admin_email) {
        None
    } else {
        match &state.gateway_manager {
            Some(manager) => Some(std::sync::Arc::clone(manager)),
            None => {
                let error = ToolError::internal_message(
                    "gateway runtime invalidation is unavailable; allowlist entry was not removed",
                );
                log_auth_dispatch(
                    action,
                    req_id.as_deref(),
                    start,
                    Some(error.kind()),
                    actor_key,
                );
                return no_store(ApiError::new(error).into_response());
            }
        }
    };
    #[cfg(feature = "gateway")]
    let _oauth_lifecycle_guard = match &gateway_manager {
        Some(manager) => manager.google_provider_lifecycle_write_guard().await,
        None => None,
    };

    let removal = if email.eq_ignore_ascii_case(admin_email) {
        auth_state
            .store
            .remove_bootstrap_admin_allowlist_entry(&email)
            .await
            .map(|()| labby_auth::types::AllowedUserRevocation::default())
    } else {
        auth_state.store.remove_allowed_user(&email).await
    };
    let revocation = match removal {
        Ok(counts) => counts,
        Err(err) => {
            let kind = err.kind();
            tracing::warn!(
                surface = "api",
                service = "auth",
                action,
                email_fp,
                kind,
                elapsed_ms = start.elapsed().as_millis(),
                "auth.allowed_user.remove failed"
            );
            log_auth_dispatch(action, req_id.as_deref(), start, Some(kind), actor_key);
            return no_store(ApiError::new(auth_err(err)).into_response());
        }
    };

    let mut invalidated_runtime_sessions = 0;
    #[cfg(feature = "gateway")]
    if let Some(manager) = &gateway_manager {
        for subject in &revocation.subjects {
            invalidated_runtime_sessions += manager
                .invalidate_google_provider_subject_runtime_guarded(
                    subject,
                    "auth.allowed_user.remove",
                )
                .await
                .total();
        }
    }

    tracing::info!(
        surface = "api",
        service = "auth",
        action,
        email_fp,
        revoked_sessions = revocation.revoked_sessions,
        revoked_provider_credentials = revocation.revoked_provider_credentials,
        revoked_refresh_tokens = revocation.revoked_refresh_tokens,
        revoked_authorization_codes = revocation.revoked_authorization_codes,
        invalidated_subjects = revocation.subjects.len(),
        invalidated_runtime_sessions,
        elapsed_ms = start.elapsed().as_millis(),
        "auth.allowed_user.remove complete"
    );
    log_auth_dispatch(action, req_id.as_deref(), start, None, actor_key);
    no_store(StatusCode::NO_CONTENT.into_response())
}
