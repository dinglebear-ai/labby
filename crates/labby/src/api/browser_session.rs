use axum::Extension;
use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::time::Instant;

use crate::access::{
    AccessBlockedReason, AccessRuntimeError, AccessRuntimeStatus, AccessStoreError,
    SessionAuthoritySnapshot,
};
use crate::api::ToolError;
use crate::api::auth_helpers::{log_auth_dispatch, log_auth_dispatch_start, request_id};
use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::state::AppState;
use crate::dispatch::access_errors::{map_runtime_error, map_store_error};

use labby_auth::browser_authority::BrowserAuthority;
use labby_auth::reauth::ProofError;
use labby_auth::reauth_browser::PurposeInput;
use labby_auth::session::BROWSER_CSRF_HEADER_NAME;

const DEV_SESSION_EXPIRES_AT: u64 = 253_402_300_799;

fn oauth_state(state: &AppState) -> Option<&labby_auth::state::AuthState> {
    state.oauth_state.as_ref().map(|state| state.as_ref())
}

fn no_store_json(body: serde_json::Value) -> Response {
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(body),
    )
        .into_response()
}

fn unauthenticated_session_response(login_available: bool) -> Response {
    no_store_json(serde_json::json!({
        "authenticated": false,
        "login_available": login_available,
    }))
}

fn session_cookie(
    headers: &HeaderMap,
    auth_state: &labby_auth::state::AuthState,
) -> Option<String> {
    labby_auth::session::read_cookie(headers, &auth_state.config.session_cookie_name)
}

fn actor_key_for_session(
    state: &AppState,
    session: &labby_auth::types::BrowserSessionRow,
) -> Option<std::sync::Arc<str>> {
    state
        .actor_key_deriver
        .as_deref()
        .and_then(|deriver| deriver.derive_subject(&session.subject))
        .map(crate::observability::activity::ActorKey::into_arc)
}

async fn load_browser_session(
    auth_state: &labby_auth::state::AuthState,
    headers: &HeaderMap,
) -> Result<Option<labby_auth::types::BrowserSessionRow>, labby_auth::error::AuthError> {
    let has_cookie_header = headers.contains_key(header::COOKIE);
    let browser_session_cookie = session_cookie(headers, auth_state);
    let has_browser_session_cookie = browser_session_cookie.is_some();
    tracing::info!(
        has_cookie_header,
        has_browser_session_cookie,
        "auth session request received"
    );

    let Some(session_id) = browser_session_cookie else {
        return Ok(None);
    };

    match auth_state.store.find_browser_session(&session_id).await {
        Ok(session) => {
            tracing::info!(
                has_cookie_header,
                has_browser_session_cookie,
                session_found = session.is_some(),
                "auth session lookup completed"
            );
            Ok(session)
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                has_cookie_header,
                has_browser_session_cookie,
                "auth session lookup failed"
            );
            Err(error)
        }
    }
}

/// Map a typed product error through the shared HTTP envelope with the
/// session cache posture. The `ToolError` structure is preserved until here.
fn tool_error_response(error: ToolError) -> Response {
    let mut response = ApiError::new(error).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    response
}

fn internal_error_response(message: &'static str) -> Response {
    tool_error_response(ToolError::internal_message(message))
}

fn invalid_csrf_response() -> Response {
    let mut response = (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({
            "kind": "validation_failed",
            "message": "missing or invalid csrf token",
        })),
    )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-store"),
    );
    response
}

fn proof_error_response(error: ProofError) -> Response {
    let status = match error {
        ProofError::Denied | ProofError::Required => StatusCode::UNAUTHORIZED,
        ProofError::RateLimited | ProofError::Capacity => StatusCode::TOO_MANY_REQUESTS,
        ProofError::InvalidPurpose => StatusCode::UNPROCESSABLE_ENTITY,
        ProofError::Unsupported => StatusCode::NOT_IMPLEMENTED,
        ProofError::Expired | ProofError::Replayed => StatusCode::GONE,
        ProofError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(serde_json::json!({"kind": error.kind(), "message": error.to_string()})),
    )
        .into_response()
}

fn trusted_origin(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(expected) = state
        .auth_config
        .as_ref()
        .and_then(|config| config.public_url.as_ref())
        .map(|url| url.origin().ascii_serialization())
    else {
        return false;
    };
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        == Some(expected.as_str())
}

pub async fn reauth_start(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    headers: HeaderMap,
    Json(input): Json<PurposeInput>,
) -> Response {
    if !trusted_origin(&state, &headers) {
        return invalid_csrf_response();
    }
    let Some(auth) = oauth_state(&state) else {
        return proof_error_response(ProofError::Unsupported);
    };
    match labby_auth::reauth_browser::start(auth, &authority, &input).await {
        Ok(started) => no_store_json(serde_json::to_value(started).unwrap_or_default()),
        Err(error) => proof_error_response(error),
    }
}

pub async fn reauth_poll(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    axum::extract::Path(interaction): axum::extract::Path<String>,
) -> Response {
    let Some(auth) = oauth_state(&state) else {
        return proof_error_response(ProofError::Unsupported);
    };
    match labby_auth::reauth_browser::poll(auth, &authority, &interaction).await {
        Ok(result) => no_store_json(serde_json::to_value(result).unwrap_or_default()),
        Err(error) => proof_error_response(error),
    }
}

pub async fn reauth_cancel(
    State(state): State<AppState>,
    Extension(authority): Extension<BrowserAuthority>,
    headers: HeaderMap,
    axum::extract::Path(interaction): axum::extract::Path<String>,
) -> Response {
    if !trusted_origin(&state, &headers) {
        return invalid_csrf_response();
    }
    let Some(auth) = oauth_state(&state) else {
        return proof_error_response(ProofError::Unsupported);
    };
    match labby_auth::reauth_browser::cancel(auth, &authority, &interaction).await {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => proof_error_response(error),
    }
}

pub async fn reauth_return() -> Response {
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "private, no-store")],
        "<!doctype html><meta charset=utf-8><title>Authentication complete</title><p>Authentication complete. Return to Labby.</p>",
    )
        .into_response()
}

/// Session-authority resolution outcome for an authenticated identity.
enum SessionAuthority {
    /// The identity is linked to a durable principal; the snapshot is the
    /// projected authority for this request.
    Ready(SessionAuthoritySnapshot),
    /// The identity authenticated but has no active principal link yet. The
    /// caller is logged in; nothing in the access store grants it authority.
    Unprovisioned,
    /// No durable authority store exists on this process yet (owner
    /// bootstrap has not run, or this state container is not the lifecycle
    /// owner). Authority is projected from the transport credential alone so
    /// bootstrap and local-operator flows stay reachable; nothing durable is
    /// claimed.
    Transport { is_admin: bool },
}

const UNPROVISIONED_REMEDIATION: &str = "This identity is authenticated but has no access authority yet. \
     Ask an administrator to add this identity to a team, or complete owner bootstrap.";
const TRANSPORT_REMEDIATION: &str = "Durable access authority is not initialized on this process; \
     authority is projected from the transport credential only. Complete owner bootstrap to enable \
     multi-user authority.";

fn session_authority_denied() -> ToolError {
    ToolError::Forbidden {
        message: "session authority is unavailable for this identity".to_owned(),
        required_scopes: Vec::new(),
    }
}

/// Resolve the durable session authority for a verified identity without
/// collapsing the typed store cause.
///
/// - A `Ready` runtime answers from the store. `IdentityUnavailable` /
///   `NotAuthorized` mean "no principal link yet" and are a legitimate
///   authenticated-but-unprovisioned state, not a failure; every other store
///   failure maps through the shared access error map so the surface agrees
///   with the dispatchers on `service_unavailable`.
/// - A runtime that has no durable store yet (`SetupRequired`, or the
///   non-owner `Blocked(Unavailable)` sentinel) projects transport authority
///   so owner bootstrap and local-operator sessions keep working.
/// - Every other blocked runtime (corrupt, locked, insecure, newer schema,
///   read-only) is a real outage and answers 503.
async fn resolve_session_authority(
    state: &AppState,
    identity: labby_auth::VerifiedIdentity,
    transport_admin: bool,
) -> Result<SessionAuthority, ToolError> {
    match state.access_runtime.status().await {
        AccessRuntimeStatus::Ready => {}
        AccessRuntimeStatus::SetupRequired(reason) => {
            tracing::info!(
                surface = "api",
                service = "auth",
                action = "session.get",
                reason = ?reason,
                "durable access authority not initialized; projecting transport authority"
            );
            return Ok(SessionAuthority::Transport {
                is_admin: transport_admin,
            });
        }
        AccessRuntimeStatus::Blocked(AccessBlockedReason::Unavailable) => {
            tracing::info!(
                surface = "api",
                service = "auth",
                action = "session.get",
                "access runtime is not wired on this process; projecting transport authority"
            );
            return Ok(SessionAuthority::Transport {
                is_admin: transport_admin,
            });
        }
        AccessRuntimeStatus::Blocked(reason) => {
            return Err(map_runtime_error(
                "auth",
                AccessRuntimeError::Blocked(reason),
            ));
        }
    }
    // `AccessRuntime::session_authority` erases every store failure into
    // `AccessRuntimeError::LifecycleUnavailable`, which cannot distinguish an
    // unprovisioned identity (200 + `authority_state: "unprovisioned"`) from a
    // store outage (503). This handler therefore goes through the Ready-only
    // `store()` handle and consumes the store's typed `AccessStoreError`.
    let store = state
        .access_runtime
        .store()
        .await
        .map_err(|error| map_runtime_error("auth", error))?;
    match store.session_authority(identity).await {
        Ok(snapshot) => Ok(SessionAuthority::Ready(snapshot)),
        Err(AccessStoreError::IdentityUnavailable | AccessStoreError::NotAuthorized) => {
            Ok(SessionAuthority::Unprovisioned)
        }
        Err(error) => Err(map_store_error("auth", error, session_authority_denied)),
    }
}

fn scopes_grant_admin(scopes: &[String]) -> bool {
    scopes.iter().any(|scope| scope == "lab:admin")
}

/// Identity for an OAuth-backed browser session row.
///
/// Must stay identical to the derivation in `labby_auth::middleware` for the
/// browser-session cookie path so the same human maps to the same principal
/// whether the request reached this handler through the auth layer or
/// through the anonymous cookie fallback.
fn oauth_browser_session_identity(
    auth_state: &labby_auth::state::AuthState,
    session: &labby_auth::types::BrowserSessionRow,
) -> Result<labby_auth::VerifiedIdentity, ToolError> {
    labby_auth::VerifiedIdentity::external(
        labby_auth::Authenticator::BrowserSession,
        &auth_state.inbound_provider_binding().identity_issuer,
        session.subject.clone(),
    )
    .map_err(|_| ToolError::internal_message("authenticated identity is invalid"))
}

/// Identity for a project-bound browser session.
///
/// Must stay identical to the project-session derivation in
/// `labby_auth::middleware` (`BrowserSession` authenticator, binding issuer,
/// source credential id).
fn project_session_identity(
    binding: &labby_auth::types::ProjectSessionBinding,
) -> Result<labby_auth::VerifiedIdentity, ToolError> {
    labby_auth::VerifiedIdentity::local_credential_with_issuer(
        labby_auth::Authenticator::BrowserSession,
        binding.issuer.clone(),
        binding.source_credential_id.clone(),
    )
    .map_err(|_| ToolError::internal_message("authenticated identity is invalid"))
}

/// Look up Labby's project-bound browser session from the request cookies.
async fn load_project_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<labby_auth::types::BrowserSessionRow>, ToolError> {
    let Some(session_state) = state.project_session_state.as_ref() else {
        return Ok(None);
    };
    let Some(session_id) = labby_auth::session::read_cookie(headers, &session_state.cookie_name)
    else {
        return Ok(None);
    };
    session_state
        .store
        .find_browser_session(&session_id)
        .await
        .map_err(|error| {
            tracing::error!(error = %error, "failed to load project browser session");
            ToolError::internal_message("failed to load browser session")
        })
}

/// Derive the verified identity from the browser cookies when the auth layer
/// authenticated the request via a session but did not attach the identity
/// extension. Uses the same helpers as the anonymous branches so both paths
/// map one human to one principal.
async fn fallback_session_identity(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<labby_auth::VerifiedIdentity>, ToolError> {
    if let Some(session) = load_project_session(state, headers).await?
        && let Some(binding) = session.project_binding.as_ref()
    {
        return project_session_identity(binding).map(Some);
    }
    let Some(auth_state) = oauth_state(state) else {
        return Ok(None);
    };
    match load_browser_session(auth_state, headers).await {
        Ok(Some(session)) => oauth_browser_session_identity(auth_state, &session).map(Some),
        Ok(None) => Ok(None),
        Err(_) => Err(ToolError::internal_message(
            "failed to load browser session",
        )),
    }
}

fn wire_roles<'a>(
    teams: impl Iterator<Item = &'a (String, crate::access::TeamRole, u64, u64)>,
) -> Vec<serde_json::Value> {
    teams
        .map(|(id, role, membership_epoch, policy_epoch)| {
            serde_json::json!({
                "id": id,
                "role": role.as_wire(),
                "membership_epoch": membership_epoch,
                "policy_epoch": policy_epoch,
            })
        })
        .collect()
}

/// Build the authenticated `/auth/session` body.
///
/// The `user`, `csrf_token`, and `expires_at` fields are emitted in both the
/// ready and unprovisioned states so the UI can fail closed on authority
/// while still holding a usable session.
fn authenticated_session_body(
    login_available: bool,
    user: serde_json::Value,
    project_id: Option<&str>,
    expires_at: serde_json::Value,
    csrf_token: &str,
    authority: &SessionAuthority,
) -> serde_json::Value {
    match authority {
        SessionAuthority::Ready(authority) => serde_json::json!({
            "authenticated": true,
            "login_available": login_available,
            "authority_state": "ready",
            "authority": {
                "principal_id": authority.principal_id,
                "organization_id": authority.organization_id,
                "authority_generation": authority.authority_generation,
            },
            // OAuth scopes are only a transport ceiling. Domain administration
            // is projected from durable access authority, never inferred here.
            "is_admin": authority.capabilities.iter().any(|capability| capability.as_wire() == "platform.manage"),
            "user": user,
            "project_id": project_id,
            "owner": { "kind": "personal", "id": authority.principal_id },
            "organization_id": authority.organization_id,
            "teams": wire_roles(authority.teams.iter()),
            "projects": authority.projects.iter().map(|(id, role)| serde_json::json!({"id":id,"role":role.as_wire()})).collect::<Vec<_>>(),
            "project": project_id,
            "capabilities": authority.capabilities.iter().map(|capability| capability.as_wire()).collect::<Vec<_>>(),
            "authority_generation": authority.authority_generation,
            "expires_at": expires_at,
            "csrf_token": csrf_token,
        }),
        SessionAuthority::Transport { is_admin } => serde_json::json!({
            "authenticated": true,
            "login_available": login_available,
            "authority_state": "transport",
            "authority": serde_json::Value::Null,
            "remediation": TRANSPORT_REMEDIATION,
            "is_admin": is_admin,
            "user": user,
            "project_id": project_id,
            "owner": serde_json::Value::Null,
            "organization_id": serde_json::Value::Null,
            "teams": Vec::<serde_json::Value>::new(),
            "projects": Vec::<serde_json::Value>::new(),
            "project": project_id,
            "capabilities": Vec::<serde_json::Value>::new(),
            "authority_generation": serde_json::Value::Null,
            "expires_at": expires_at,
            "csrf_token": csrf_token,
        }),
        SessionAuthority::Unprovisioned => serde_json::json!({
            "authenticated": true,
            "login_available": login_available,
            "authority_state": "unprovisioned",
            "authority": serde_json::Value::Null,
            "remediation": UNPROVISIONED_REMEDIATION,
            "is_admin": false,
            "user": user,
            "project_id": project_id,
            "owner": serde_json::Value::Null,
            "organization_id": serde_json::Value::Null,
            "teams": Vec::<serde_json::Value>::new(),
            "projects": Vec::<serde_json::Value>::new(),
            "project": project_id,
            "capabilities": Vec::<serde_json::Value>::new(),
            "authority_generation": serde_json::Value::Null,
            "expires_at": expires_at,
            "csrf_token": csrf_token,
        }),
    }
}

/// Finish a `session.get` dispatch. Success and unprovisioned outcomes log
/// the dispatch event without a `kind`; failures log the typed error kind
/// and map through the shared HTTP error envelope.
fn finish_session_get(
    request_id: Option<&str>,
    start: Instant,
    actor_key: Option<&str>,
    outcome: Result<serde_json::Value, ToolError>,
) -> Response {
    match outcome {
        Ok(body) => {
            let authority_state = body
                .get("authority_state")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("bypass");
            tracing::info!(
                surface = "api",
                service = "auth",
                action = "session.get",
                request_id,
                authority_state,
                "auth session authority projected"
            );
            log_auth_dispatch("session.get", request_id, start, None, actor_key);
            no_store_json(body)
        }
        Err(error) => {
            let kind = error.kind().to_owned();
            log_auth_dispatch("session.get", request_id, start, Some(&kind), actor_key);
            tool_error_response(error)
        }
    }
}

pub async fn auth_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    identity: Option<Extension<labby_auth::VerifiedIdentity>>,
) -> impl IntoResponse {
    let start = Instant::now();
    let request_id = request_id(&headers).map(ToOwned::to_owned);
    log_auth_dispatch_start("session.get", request_id.as_deref());
    let login_available = state.oauth_state.is_some();

    if let Some(Extension(context)) = auth {
        let actor_key = context.actor_key.clone();
        let outcome = authenticated_context_session(&state, &headers, context, identity).await;
        return finish_session_get(request_id.as_deref(), start, actor_key.as_deref(), outcome);
    }

    // This route intentionally remains outside the auth middleware so an
    // anonymous browser can discover login availability. Resolve Labby's
    // project-bound cookie explicitly before the development UI fallback;
    // otherwise bearer-only deployments render an authenticated-but-unbound
    // shell even though the browser holds a valid project session.
    match load_project_session(&state, &headers).await {
        Ok(Some(session)) if session.project_binding.is_some() => {
            let actor_key = actor_key_for_session(&state, &session);
            let binding = session.project_binding.as_ref().expect("guarded above");
            let outcome = match project_session_identity(binding) {
                Ok(identity) => {
                    resolve_session_authority(&state, identity, scopes_grant_admin(&binding.scopes))
                        .await
                        .map(|authority| {
                            authenticated_session_body(
                                login_available,
                                serde_json::json!({
                                    "sub": binding.principal_id,
                                    "email": session.email,
                                }),
                                Some(binding.project_id.as_str()),
                                serde_json::json!(session.expires_at),
                                &session.csrf_token,
                                &authority,
                            )
                        })
                }
                Err(error) => Err(error),
            };
            return finish_session_get(request_id.as_deref(), start, actor_key.as_deref(), outcome);
        }
        Ok(_) => {}
        Err(error) => {
            return finish_session_get(request_id.as_deref(), start, None, Err(error));
        }
    }

    if state.web_ui_auth_disabled {
        // Dev mode bypasses auth entirely — treat the synthetic dev user as admin
        // so admin UI is reachable in local development without real credentials.
        let body = serde_json::json!({
            "authenticated": true,
            "login_available": false,
            "is_admin": true,
            "dev_authority_bypass": true,
            "user": {
                "sub": "labby-dev",
                "email": serde_json::Value::Null,
            },
            "expires_at": DEV_SESSION_EXPIRES_AT,
            "csrf_token": "",
        });
        return finish_session_get(request_id.as_deref(), start, None, Ok(body));
    }

    // If a valid static bearer token is presented, treat the caller as a
    // first-class authenticated session for browser-state purposes. This lets
    // automation tools (e.g. agent-browser with --headers) drive the UI while
    // OAuth remains enabled for normal browser users.
    if let Some(expected) = state.bearer_token.as_ref()
        && let Some(token) = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(labby_auth::parse_bearer_token)
        && labby_auth::tokens_equal(&token, expected.as_ref())
    {
        let outcome = match labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        ) {
            // The static bearer is the local operator credential.
            Ok(identity) => {
                resolve_session_authority(&state, identity, true)
                    .await
                    .map(|authority| {
                        authenticated_session_body(
                            login_available,
                            serde_json::json!({
                                "sub": "static-bearer",
                                "email": serde_json::Value::Null,
                            }),
                            None,
                            serde_json::json!(DEV_SESSION_EXPIRES_AT),
                            "",
                            &authority,
                        )
                    })
            }
            Err(_) => Err(ToolError::internal_message(
                "authenticated identity is invalid",
            )),
        };
        return finish_session_get(request_id.as_deref(), start, None, outcome);
    }

    let Some(auth_state) = oauth_state(&state) else {
        let response = unauthenticated_session_response(false);
        log_auth_dispatch("session.get", request_id.as_deref(), start, None, None);
        return response;
    };

    match load_browser_session(auth_state, &headers).await {
        Ok(Some(session)) => {
            let actor_key = actor_key_for_session(&state, &session);
            let outcome = match oauth_browser_session_identity(auth_state, &session) {
                Ok(identity) => {
                    // An OAuth cookie alone never carries transport admin.
                    resolve_session_authority(&state, identity, false)
                        .await
                        .map(|authority| {
                            let project_id = session
                                .project_binding
                                .as_ref()
                                .map(|binding| binding.project_id.as_str());
                            authenticated_session_body(
                                login_available,
                                serde_json::json!({
                                    "sub": session.subject,
                                    "email": session.email,
                                }),
                                project_id,
                                serde_json::json!(session.expires_at),
                                &session.csrf_token,
                                &authority,
                            )
                        })
                }
                Err(error) => Err(error),
            };
            finish_session_get(request_id.as_deref(), start, actor_key.as_deref(), outcome)
        }
        Ok(None) => {
            let response = unauthenticated_session_response(login_available);
            log_auth_dispatch("session.get", request_id.as_deref(), start, None, None);
            response
        }
        Err(error) => {
            tracing::error!(error = %error, "failed to load browser session for auth session");
            finish_session_get(
                request_id.as_deref(),
                start,
                None,
                Err(ToolError::internal_message(
                    "failed to load browser session",
                )),
            )
        }
    }
}

/// Authenticated branch: the auth layer already verified the caller. Project
/// the durable authority for the attached identity, falling back to the
/// cookie-derived identity when the extension is missing so a session user
/// maps to the same principal as the anonymous cookie path.
async fn authenticated_context_session(
    state: &AppState,
    headers: &HeaderMap,
    context: AuthContext,
    identity: Option<Extension<labby_auth::VerifiedIdentity>>,
) -> Result<serde_json::Value, ToolError> {
    let project_session = if context.via_session {
        load_project_session(state, headers).await?
    } else {
        None
    };
    let (project_id, expires_at) = match project_session {
        Some(session) => (
            session
                .project_binding
                .as_ref()
                .map(|binding| binding.project_id.clone()),
            u64::try_from(session.expires_at).unwrap_or(DEV_SESSION_EXPIRES_AT),
        ),
        None => (None, DEV_SESSION_EXPIRES_AT),
    };
    let identity = match identity {
        Some(Extension(identity)) => identity,
        None => {
            let fallback = if context.via_session {
                fallback_session_identity(state, headers).await?
            } else {
                None
            };
            fallback.ok_or_else(|| {
                tracing::error!(
                    via_session = context.via_session,
                    "authenticated request reached /auth/session without a verified identity"
                );
                ToolError::internal_message("authenticated identity is unavailable")
            })?
        }
    };
    let authority =
        resolve_session_authority(state, identity, scopes_grant_admin(&context.scopes)).await?;
    Ok(authenticated_session_body(
        false,
        serde_json::json!({
            "sub": context.sub,
            "email": context.email,
        }),
        project_id.as_deref(),
        serde_json::json!(expires_at),
        context.csrf_token.as_deref().unwrap_or_default(),
        &authority,
    ))
}

pub async fn auth_logout(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let start = Instant::now();
    let request_id = request_id(&headers).map(ToOwned::to_owned);
    log_auth_dispatch_start("session.logout", request_id.as_deref());

    if state.web_ui_auth_disabled {
        let mut response = StatusCode::NO_CONTENT.into_response();
        if let Some(auth_state) = oauth_state(&state) {
            labby_auth::session::append_set_cookie(
                &mut response,
                &labby_auth::session::clear_browser_session_cookie(auth_state),
            );
        }
        log_auth_dispatch("session.logout", request_id.as_deref(), start, None, None);
        return response;
    }

    let Some(auth_state) = oauth_state(&state) else {
        log_auth_dispatch("session.logout", request_id.as_deref(), start, None, None);
        return StatusCode::NO_CONTENT.into_response();
    };

    let mut response = StatusCode::NO_CONTENT.into_response();
    let mut actor_key = None;
    if let Some(session_id) = session_cookie(&headers, auth_state) {
        let csrf = headers
            .get(BROWSER_CSRF_HEADER_NAME)
            .and_then(|value| value.to_str().ok());
        match auth_state.store.find_browser_session(&session_id).await {
            Ok(Some(session)) => {
                actor_key = actor_key_for_session(&state, &session);
                if csrf != Some(session.csrf_token.as_str()) {
                    tracing::warn!(
                        has_csrf_header = csrf.is_some(),
                        "auth logout rejected: missing or invalid csrf token"
                    );
                    log_auth_dispatch(
                        "session.logout",
                        request_id.as_deref(),
                        start,
                        Some("validation_failed"),
                        actor_key.as_deref(),
                    );
                    return invalid_csrf_response();
                }
                if let Err(error) = auth_state.store.revoke_browser_session(&session_id).await {
                    tracing::error!(error = %error, "failed to revoke browser session");
                    log_auth_dispatch(
                        "session.logout",
                        request_id.as_deref(),
                        start,
                        Some("internal_error"),
                        actor_key.as_deref(),
                    );
                    return internal_error_response("failed to revoke browser session");
                }
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(error = %error, "failed to load browser session for logout");
                log_auth_dispatch(
                    "session.logout",
                    request_id.as_deref(),
                    start,
                    Some("internal_error"),
                    None,
                );
                return internal_error_response("failed to load browser session");
            }
        }
    }
    labby_auth::session::append_set_cookie(
        &mut response,
        &labby_auth::session::clear_browser_session_cookie(&auth_state),
    );
    log_auth_dispatch(
        "session.logout",
        request_id.as_deref(),
        start,
        None,
        actor_key.as_deref(),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use labby_auth::types::{BrowserSessionRow, ProjectSessionBinding};

    #[test]
    fn reauthentication_requires_the_exact_configured_origin() {
        let config = labby_auth::config::AuthConfig {
            public_url: Some("https://lab.example.com/base".parse().unwrap()),
            ..Default::default()
        };
        let state = AppState::new().with_auth_config(config);
        let mut headers = HeaderMap::new();
        assert!(!trusted_origin(&state, &headers));
        headers.insert(header::ORIGIN, HeaderValue::from_static("null"));
        assert!(!trusted_origin(&state, &headers));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://attacker.example"),
        );
        assert!(!trusted_origin(&state, &headers));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://lab.example.com"),
        );
        assert!(trusted_origin(&state, &headers));
    }

    #[tokio::test]
    async fn recent_auth_errors_keep_stable_public_kinds() {
        for (error, status, kind) in [
            (
                ProofError::Unsupported,
                StatusCode::NOT_IMPLEMENTED,
                "recent_auth_unsupported",
            ),
            (ProofError::Expired, StatusCode::GONE, "recent_auth_expired"),
            (ProofError::Denied, StatusCode::UNAUTHORIZED, "auth_failed"),
        ] {
            let response = proof_error_response(error);
            assert_eq!(response.status(), status);
            assert_eq!(
                response.headers().get(header::CACHE_CONTROL).unwrap(),
                "private, no-store"
            );
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["kind"], kind);
        }
    }

    #[tokio::test]
    async fn authenticated_project_session_preserves_binding_and_expiry() {
        let directory = tempfile::tempdir().unwrap();
        let session_state = labby_auth::project_session::ProjectSessionState::open(
            directory.path().join("auth.db"),
            "__Host-labby-session",
        )
        .await
        .unwrap();
        let expires_at = 2_000_000_000_i64;
        let session = BrowserSessionRow {
            session_id: "project-session".into(),
            subject: "subject".into(),
            email: None,
            csrf_token: "csrf-token".into(),
            created_at: 1,
            expires_at,
            project_binding: Some(ProjectSessionBinding {
                installation_id: "installation".into(),
                issuer: "issuer".into(),
                subject: "subject".into(),
                principal_id: "principal".into(),
                organization_id: "organization".into(),
                project_id: "project-42".into(),
                loadout_id: "loadout".into(),
                loadout_generation: 1,
                assignment_generation: 1,
                catalog_generation: 1,
                route_id: "route".into(),
                route_generation: 1,
                membership_epoch: 1,
                organization_policy_epoch: 1,
                project_policy_epoch: 1,
                source_credential_id: "credential".into(),
                source_credential_generation: 1,
                scopes: vec!["lab:read".into()],
                resource: "https://lab.example.com".into(),
                audience: "labby".into(),
                source_credential_expires_at: u64::try_from(expires_at).unwrap(),
            }),
        };
        session_state
            .store
            .upsert_browser_session(session)
            .await
            .unwrap();
        let state = AppState::new().with_project_session_state(session_state);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-labby-session=project-session"),
        );
        let auth = AuthContext {
            sub: "principal".into(),
            actor_key: None,
            scopes: vec!["lab:read".into()],
            issuer: "issuer".into(),
            via_session: true,
            csrf_token: Some("csrf-token".into()),
            email: None,
        };

        // The default state container has no durable authority store, so the
        // session projects transport authority (never a 500) and still
        // preserves the project binding and expiry from the cookie.
        let response = auth_session(State(state.clone()), headers.clone(), None, None)
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authenticated"], true);
        assert_eq!(json["authority_state"], "transport");
        assert_eq!(json["authority"], serde_json::Value::Null);
        assert_eq!(json["is_admin"], false, "lab:read binding is not admin");
        assert_eq!(json["project_id"], "project-42");
        assert_eq!(json["expires_at"], expires_at);
        assert_eq!(json["csrf_token"], "csrf-token");

        let response = auth_session(State(state), headers, Some(Extension(auth)), None)
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["authority_state"], "transport");
        assert_eq!(json["project_id"], "project-42");
        assert_eq!(json["expires_at"], expires_at);
    }

    /// B-I7: an authenticated identity with no principal link is a 200
    /// `unprovisioned` projection with a remediation hint, never a 500; a
    /// provisioned owner projects durable authority; a blocked store is 503.
    #[tokio::test]
    async fn session_authority_states_are_explicit() {
        let directory = tempfile::Builder::new()
            .prefix("labby-session-authority-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let owner = labby_auth::VerifiedIdentity::local_credential(
            labby_auth::Authenticator::StaticBearer,
            "static-bearer:primary",
        )
        .unwrap();
        let runtime = std::sync::Arc::new(
            crate::access::AccessRuntime::initialize(directory.path().join("access.db")).await,
        );
        runtime
            .bootstrap_owner(
                crate::access::BootstrapOwnerInput::new(owner.clone(), "Local", "Default").unwrap(),
            )
            .await
            .unwrap();
        let state = AppState::new().with_access_runtime(runtime);

        let ready = resolve_session_authority(&state, owner, false)
            .await
            .unwrap();
        assert!(matches!(ready, SessionAuthority::Ready(_)));
        let body = authenticated_session_body(
            false,
            serde_json::json!({"sub": "owner"}),
            None,
            serde_json::json!(1),
            "",
            &ready,
        );
        assert_eq!(body["authority_state"], "ready");
        assert_eq!(body["is_admin"], true);

        let stranger = labby_auth::VerifiedIdentity::external(
            labby_auth::Authenticator::BrowserSession,
            "https://accounts.google.com",
            "stranger",
        )
        .unwrap();
        let unprovisioned = resolve_session_authority(&state, stranger, true)
            .await
            .unwrap();
        assert!(matches!(unprovisioned, SessionAuthority::Unprovisioned));
        let body = authenticated_session_body(
            false,
            serde_json::json!({"sub": "stranger"}),
            None,
            serde_json::json!(1),
            "",
            &unprovisioned,
        );
        assert_eq!(body["authority_state"], "unprovisioned");
        assert_eq!(
            body["is_admin"], false,
            "transport scope never grants durable admin"
        );
        assert!(body["remediation"].as_str().unwrap().contains("bootstrap"));

        // A blocked store that is not the non-owner sentinel is a typed outage.
        assert_eq!(
            map_runtime_error(
                "auth",
                AccessRuntimeError::Blocked(AccessBlockedReason::Corrupt)
            )
            .kind(),
            "service_unavailable"
        );
    }
}
