//! HTTP route group for admin-only allowlist management.
//!
//! All endpoints require a browser session with an email matching the
//! configured `admin_email`. JWT bearer callers receive 403 even if they
//! hold a valid token — these endpoints are intentionally browser-session-only.
//!
//! Routes:
//! - `GET  /v1/auth/allowed-emails`          → list entries (200)
//! - `POST /v1/auth/allowed-emails`           → add entry (201)
//! - `DELETE /v1/auth/allowed-emails/:email`  → remove entry (204, idempotent);
//!   also revokes the durable grants allowlist admission created
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
use labby_auth::util::{fingerprint, normalize_email, now_unix};

// ── email validation ─────────────────────────────────────────────────────────

const MAX_EMAIL_LENGTH: usize = 320;

/// Validate and normalize an email for storage.
///
/// Order: trim → empty check → length check → whitespace check → `@` check →
/// the shared `labby_auth` email fold.
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
    Ok(normalize_email(trimmed))
}

// ── admin guard ───────────────────────────────────────────────────────────────

/// Verify the caller is a browser-session user whose email is a configured admin.
///
/// Returns `Err(forbidden)` for:
/// - JWT bearer callers (`via_session == false`)
/// - Browser-session callers with no email claim
/// - Email that is not in `admin_emails` (case-insensitive)
fn require_admin(ctx: &AuthContext, admin_emails: &[String]) -> Result<(), ToolError> {
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
    if !labby_auth::config::is_listed_admin(admin_emails, email) {
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

/// Extract the configured admin emails from `auth_config`.
///
/// Returns an `internal_error` ToolError if auth config is not mounted.
fn require_admin_email(state: &AppState) -> Result<&[String], ToolError> {
    state
        .auth_config
        .as_ref()
        .map(|cfg| cfg.admin_emails.as_slice())
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
    #[serde(default = "default_allowlist_role")]
    role: String,
}

fn default_allowlist_role() -> String {
    "member".to_string()
}

/// Validate a caller-supplied allowlist role against the store's accepted set.
fn validate_role(raw: &str) -> Result<&str, ToolError> {
    let role = raw.trim();
    if labby_auth::sqlite::SqliteStore::ALLOWED_USER_ROLES.contains(&role) {
        Ok(role)
    } else {
        Err(ToolError::Sdk {
            sdk_kind: "validation_failed".to_string(),
            message: "role must be `member` or `admin`".to_string(),
        })
    }
}

/// `POST /v1/auth/allowed-emails`
///
/// Body: `{ "email": ..., "role": "member" | "admin" }`
/// Returns `{ "entry": {email, added_by, created_at, role} }` (201).
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

    let role = match validate_role(&body.role) {
        Ok(role) => role.to_owned(),
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
        .add_allowed_user(&email, &added_by, &role, created_at)
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
        role,
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

/// Revoke the durable grants allowlist admission created for every
/// provider-verified identity of `email`; see
/// [`crate::access::AccessRuntime::revoke_allowlisted`]. Identities derive
/// exactly as the session handler derives them, so the same human maps to
/// the same Principal.
async fn revoke_durable_allowlist_authority(
    state: &AppState,
    auth_state: &labby_auth::state::AuthState,
    email: &str,
    removed_by_subject: &str,
) -> Result<crate::access::AllowlistRevocation, ToolError> {
    let subjects = auth_state
        .store
        .verified_inbound_subjects_for_email(email)
        .await
        .map_err(auth_err)?;
    let issuer = auth_state.inbound_provider_binding().identity_issuer;
    let identities = subjects
        .into_iter()
        .map(|subject| {
            labby_auth::VerifiedIdentity::external(
                labby_auth::Authenticator::BrowserSession,
                &issuer,
                subject,
            )
            .map_err(|_| {
                ToolError::internal_message(
                    "verified identity is invalid; allowlist entry was not removed",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    state
        .access_runtime
        .revoke_allowlisted(identities, fingerprint(removed_by_subject))
        .await
        .map_err(|error| crate::dispatch::access_errors::map_runtime_error("auth", error))
}

/// `DELETE /v1/auth/allowed-emails/:email`
///
/// Returns 204 (idempotent — returns 204 even if the email was not present).
/// Besides signing the identity out, removal revokes the durable Initial Team
/// membership, default-Project membership, and platform administration that
/// allowlist admission created; the Principal row is kept.
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

    // Normalize email from URL path with the shared fold.
    let email = normalize_email(&raw_email);
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
    let gateway_manager = if labby_auth::config::is_listed_admin(admin_email, &email) {
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

    // Revoke durable authority first and keep the returned admission fence
    // alive until the allowlist row is gone: a first-sign-in admission racing
    // this removal re-validates the allowlist under the same fence, so it can
    // neither keep nor recreate what was just revoked. A configured admin's
    // authority lives in configuration and is never revoked here.
    let durable = if labby_auth::config::is_listed_admin(admin_email, &email) {
        None
    } else {
        match revoke_durable_allowlist_authority(&state, auth_state, &email, &auth.sub).await {
            Ok(revocation) => Some(revocation),
            Err(err) => {
                tracing::warn!(
                    surface = "api",
                    service = "auth",
                    action,
                    email_fp,
                    kind = err.kind(),
                    elapsed_ms = start.elapsed().as_millis(),
                    "durable authority revocation failed; allowlist entry was not removed"
                );
                log_auth_dispatch(
                    action,
                    req_id.as_deref(),
                    start,
                    Some(err.kind()),
                    actor_key,
                );
                return no_store(ApiError::new(err).into_response());
            }
        }
    };

    let removal = if labby_auth::config::is_listed_admin(admin_email, &email) {
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

    let durable_outcomes = durable
        .as_ref()
        .map_or(&[][..], |durable| durable.outcomes.as_slice());
    let revoked_count = |selector: fn(&crate::access::AllowlistRevocationOutcome) -> bool| {
        durable_outcomes
            .iter()
            .filter(|outcome| selector(outcome))
            .count()
    };
    tracing::info!(
        surface = "api",
        service = "auth",
        action,
        email_fp,
        revoked_team_memberships = revoked_count(|outcome| outcome.team_membership),
        revoked_project_memberships = revoked_count(|outcome| outcome.project_membership),
        revoked_platform_administrators = revoked_count(|outcome| outcome.platform_administrator),
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
    // The allowlist row is gone; a racing admission may now run and will be
    // refused by its own re-validation.
    drop(durable);
    no_store(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use crate::access::{AccessRuntime, AllowlistAdmission, AllowlistRole, BootstrapOwnerInput};
    use crate::api::router::build_router;
    use crate::api::state::AppState;

    const ADMIN_EMAIL: &str = "admin@example.com";
    const ISSUER: &str = "https://accounts.google.com";

    fn auth_config(directory: &std::path::Path) -> labby_auth::config::AuthConfig {
        labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: directory.join("auth.db"),
            key_path: directory.join("auth-jwt.pem"),
            admin_emails: vec![ADMIN_EMAIL.into()],
            google: labby_auth::config::GoogleConfig {
                client_id: "id".into(),
                client_secret: "secret".into(),
                callback_url: None,
                callback_path: "/auth/google/callback".into(),
                scopes: vec!["openid".into(), "email".into()],
            },
            token_encryption_key: Some(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(
                    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                )
                .unwrap(),
            ),
            ..Default::default()
        }
    }

    fn browser_identity(subject: &str) -> labby_auth::VerifiedIdentity {
        labby_auth::VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            ISSUER,
            subject,
        )
        .unwrap()
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        access_path: std::path::PathBuf,
        auth_state: labby_auth::state::AuthState,
        runtime: Arc<AccessRuntime>,
        app: axum::Router,
        admin: labby_auth::types::BrowserSessionRow,
    }

    impl Fixture {
        /// OAuth mode with a configured admin session, a Ready access store
        /// owned by the admin, and the gateway runtime the removal path
        /// requires.
        async fn new() -> Self {
            let directory = crate::access::test_support::secure_tempdir();
            let config = auth_config(directory.path());
            let auth_state = labby_auth::state::AuthState::new(config.clone())
                .await
                .unwrap();
            let access_path = directory.path().join("access.db");
            let runtime = Arc::new(AccessRuntime::initialize(access_path.clone()).await);
            runtime
                .bootstrap_owner(
                    BootstrapOwnerInput::new(browser_identity("admin-sub"), "Local", "Default")
                        .unwrap(),
                )
                .await
                .unwrap();
            let manager = Arc::new(
                crate::dispatch::gateway::config_store::test_gateway_manager(
                    directory.path().join("gateway.toml"),
                    labby_gateway::gateway::manager::GatewayRuntimeHandle::default(),
                ),
            );
            let state = AppState::new()
                .with_oauth_state(auth_state.clone())
                .with_auth_config(config)
                .with_access_runtime(Arc::clone(&runtime))
                .with_gateway_manager(manager);
            let app = build_router(state, None, Some(auth_state.clone()), None, &[]);
            let binding = auth_state.inbound_provider_binding();
            let admin = labby_auth::types::BrowserSessionRow {
                session_id: "sess-admin".into(),
                subject: "admin-sub".into(),
                email: Some(ADMIN_EMAIL.into()),
                csrf_token: "csrf-admin".into(),
                created_at: 1,
                expires_at: i64::MAX,
                project_binding: None,
            };
            auth_state
                .store
                .upsert_bound_browser_session(admin.clone(), binding.clone())
                .await
                .unwrap();
            auth_state
                .store
                .upsert_bound_verified_inbound_identity("admin-sub", ADMIN_EMAIL, 1, binding)
                .await
                .unwrap();
            Self {
                _directory: directory,
                access_path,
                auth_state,
                runtime,
                app,
                admin,
            }
        }

        /// Allow `email` with `role` and record the provider-verified identity
        /// `subject` for it, exactly as a completed sign-in would.
        async fn allow_verified(&self, email: &str, subject: &str, role: &str) {
            self.auth_state
                .store
                .add_allowed_user(email, "admin-sub", role, 1)
                .await
                .unwrap();
            self.auth_state
                .store
                .upsert_bound_verified_inbound_identity(
                    subject,
                    email,
                    2,
                    self.auth_state.inbound_provider_binding(),
                )
                .await
                .unwrap();
        }

        async fn delete(&self, email: &str) -> StatusCode {
            let request = Request::builder()
                .method("DELETE")
                .uri(format!("/v1/auth/allowed-emails/{email}"))
                .header(header::HOST, "localhost")
                .header(
                    header::COOKIE,
                    format!(
                        "{}={}",
                        labby_auth::session::BROWSER_SESSION_COOKIE_NAME,
                        self.admin.session_id
                    ),
                )
                .header(
                    labby_auth::session::BROWSER_CSRF_HEADER_NAME,
                    &self.admin.csrf_token,
                )
                .body(Body::empty())
                .unwrap();
            self.app.clone().oneshot(request).await.unwrap().status()
        }

        fn query_one<T: rusqlite::types::FromSql>(&self, sql: &str) -> T {
            let connection = rusqlite::Connection::open_with_flags(
                &self.access_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            connection.query_row(sql, [], |row| row.get(0)).unwrap()
        }
    }

    /// Finding 1: removing an allowlist entry must revoke the durable grants
    /// its admission created (Initial Team membership, default-Project
    /// membership, platform administration) and leave an audit trail, while
    /// keeping the Principal row.
    #[tokio::test]
    async fn delete_allowed_email_revokes_durable_authority_it_provisioned() {
        let fixture = Fixture::new().await;
        let colleague = browser_identity("x-sub");
        fixture
            .allow_verified("x@example.com", "x-sub", "admin")
            .await;
        fixture
            .runtime
            .provision_allowlisted(colleague.clone(), || async {
                Some((
                    AllowlistRole::Admin,
                    AllowlistAdmission::AllowlistEntry {
                        added_by_fingerprint: "fp".into(),
                    },
                ))
            })
            .await
            .unwrap();
        let store = fixture.runtime.store().await.unwrap();
        assert!(
            store
                .session_authority(colleague.clone())
                .await
                .unwrap()
                .platform_administrator
        );

        assert_eq!(
            fixture.delete("x@example.com").await,
            StatusCode::NO_CONTENT
        );

        assert!(
            fixture
                .auth_state
                .store
                .find_allowed_user("x@example.com")
                .await
                .unwrap()
                .is_none()
        );
        let snapshot = store.session_authority(colleague).await.unwrap();
        assert!(
            !snapshot.platform_administrator,
            "platform administration granted by allowlist admission must be revoked"
        );
        assert!(
            snapshot.teams.is_empty(),
            "no active Team membership remains"
        );
        assert!(
            snapshot.projects.is_empty(),
            "no active Project membership remains"
        );
        assert_eq!(
            fixture.query_one::<String>(
                "SELECT status FROM team_memberships WHERE team_id='bootstrap-initial-team' AND principal_id LIKE 'team-member-%'"
            ),
            "revoked"
        );
        assert_eq!(
            fixture.query_one::<String>(
                "SELECT status FROM project_memberships WHERE project_id='bootstrap-default' AND principal_id LIKE 'team-member-%'"
            ),
            "disabled"
        );
        assert_eq!(
            fixture.query_one::<String>(
                "SELECT status FROM platform_administrators WHERE principal_id LIKE 'team-member-%'"
            ),
            "revoked"
        );
        assert_eq!(
            fixture.query_one::<i64>(
                "SELECT count(*) FROM principals WHERE principal_id LIKE 'team-member-%' AND status='active'"
            ),
            1,
            "the Principal row is kept"
        );
        assert_eq!(
            fixture.query_one::<i64>(
                "SELECT count(*) FROM access_audit WHERE action='access.allowlist.revoke'"
            ),
            1,
            "revocation is audited"
        );
    }
}
