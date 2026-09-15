use axum::Extension;
use axum::extract::{Json, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use std::time::Instant;

use crate::access::{
    AccessBlockedReason, AccessRuntimeError, AccessRuntimeStatus, AccessStoreError,
    OwnerBootstrapCaller, OwnerBootstrapOffer, SessionAuthoritySnapshot,
};
use crate::api::ToolError;
use crate::api::auth_helpers::{log_auth_dispatch, log_auth_dispatch_start, request_id};
use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::state::AppState;
use crate::dispatch::access_errors::{map_runtime_error, map_store_error};
use serde::Serialize;

use labby_auth::browser_authority::BrowserAuthority;
use labby_auth::reauth::ProofError;
use labby_auth::reauth_browser::PurposeInput;
use labby_auth::session::BROWSER_CSRF_HEADER_NAME;

const DEV_SESSION_EXPIRES_AT: i64 = 253_402_300_799;

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

fn unauthenticated_session_response(
    login_available: bool,
    bearer_login_available: bool,
) -> Response {
    no_store_json(serde_json::json!({
        "authenticated": false,
        "login_available": login_available,
        "bearer_login_available": bearer_login_available,
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

fn static_bearer_login_available(state: &AppState) -> bool {
    let config = state
        .oauth_state
        .as_ref()
        .map(|auth| auth.config.as_ref())
        .or(state.auth_config.as_deref());
    state.bearer_token.is_some()
        && !config.is_some_and(|config| {
            config.disable_static_token_with_oauth
                && matches!(config.mode, labby_auth::config::AuthMode::OAuth)
        })
}

fn static_browser_session(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<labby_auth::types::BrowserSessionRow> {
    if !static_bearer_login_available(state) {
        return None;
    }
    let session_state = state.static_browser_session_state.as_ref()?;
    let session_id = labby_auth::session::read_cookie(headers, session_state.cookie_name())?;
    session_state.find(&session_id)
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

// An unprovisioned identity only exists once owner bootstrap has completed, so
// bootstrap is never a remedy for it.
const UNPROVISIONED_REMEDIATION: &str = "This identity is authenticated but has no access authority yet. \
     Ask an administrator to add this identity to a team.";
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

/// Presentation fields for an authenticated session, independent of authority.
struct SessionView {
    login_available: bool,
    user: SessionUser,
    project_id: Option<String>,
    expires_at: i64,
    csrf_token: String,
}

#[derive(Serialize)]
struct SessionUser {
    sub: String,
    email: Option<String>,
}

/// Transport facts about the authenticated caller.
struct SessionCaller {
    identity: labby_auth::VerifiedIdentity,
    via_session: bool,
    subject: String,
    email: Option<String>,
    scopes: Vec<String>,
    /// Transport admin ceiling projected in the `transport` state.
    transport_admin: bool,
}

impl SessionCaller {
    fn bootstrap_caller(&self) -> OwnerBootstrapCaller<'_> {
        OwnerBootstrapCaller {
            via_session: self.via_session,
            subject: &self.subject,
            scopes: &self.scopes,
            email: self.email.as_deref(),
            identity: &self.identity,
        }
    }
}

/// The authenticated `/auth/session` wire contract.
///
/// Every authority state emits the same keys so the UI can fail closed on
/// authority while still holding a usable session; `remediation` is present
/// only when there is no durable authority.
#[derive(Serialize)]
struct SessionBody<'a> {
    authenticated: bool,
    login_available: bool,
    bearer_login_available: bool,
    authority_state: &'static str,
    authority: Option<AuthorityRef<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    remediation: Option<&'static str>,
    is_admin: bool,
    user: &'a SessionUser,
    project_id: Option<&'a str>,
    owner: Option<OwnerRef<'a>>,
    organization_id: Option<&'a str>,
    teams: Vec<TeamRoleWire<'a>>,
    projects: Vec<ProjectRoleWire<'a>>,
    project: Option<&'a str>,
    capabilities: Vec<&'static str>,
    authority_generation: Option<u64>,
    expires_at: i64,
    csrf_token: &'a str,
    /// True only when the runtime would accept owner bootstrap now and the
    /// caller passes the shared admission rule; see [`project_session`].
    owner_bootstrap_available: bool,
}

#[derive(Serialize)]
struct AuthorityRef<'a> {
    principal_id: &'a str,
    organization_id: &'a str,
    authority_generation: u64,
}

#[derive(Serialize)]
struct OwnerRef<'a> {
    kind: &'static str,
    id: &'a str,
}

#[derive(Serialize)]
struct TeamRoleWire<'a> {
    id: &'a str,
    role: &'static str,
    membership_epoch: u64,
    policy_epoch: u64,
}

#[derive(Serialize)]
struct ProjectRoleWire<'a> {
    id: &'a str,
    role: &'static str,
}

fn authenticated_session_body(
    view: &SessionView,
    authority: &SessionAuthority,
    owner_bootstrap_available: bool,
    bearer_login_available: bool,
) -> Result<serde_json::Value, ToolError> {
    let project_id = view.project_id.as_deref();
    let mut body = SessionBody {
        authenticated: true,
        login_available: view.login_available,
        bearer_login_available,
        authority_state: "transport",
        authority: None,
        remediation: None,
        is_admin: false,
        user: &view.user,
        project_id,
        owner: None,
        organization_id: None,
        teams: Vec::new(),
        projects: Vec::new(),
        project: project_id,
        capabilities: Vec::new(),
        authority_generation: None,
        expires_at: view.expires_at,
        csrf_token: &view.csrf_token,
        owner_bootstrap_available,
    };
    match authority {
        SessionAuthority::Ready(authority) => {
            body.authority_state = "ready";
            body.authority = Some(AuthorityRef {
                principal_id: &authority.principal_id,
                organization_id: &authority.organization_id,
                authority_generation: authority.authority_generation,
            });
            body.capabilities = authority
                .capabilities
                .iter()
                .map(|capability| capability.as_wire())
                .collect();
            // OAuth scopes are only a transport ceiling. Domain administration
            // is projected from durable access authority, never inferred here.
            body.is_admin = body.capabilities.contains(&"platform.manage");
            body.owner = Some(OwnerRef {
                kind: "personal",
                id: &authority.principal_id,
            });
            body.organization_id = Some(&authority.organization_id);
            body.teams = authority
                .teams
                .iter()
                .map(|(id, role, membership_epoch, policy_epoch)| TeamRoleWire {
                    id,
                    role: role.as_wire(),
                    membership_epoch: *membership_epoch,
                    policy_epoch: *policy_epoch,
                })
                .collect();
            body.projects = authority
                .projects
                .iter()
                .map(|(id, role)| ProjectRoleWire {
                    id,
                    role: role.as_wire(),
                })
                .collect();
            body.authority_generation = Some(authority.authority_generation);
        }
        SessionAuthority::Transport { is_admin } => {
            body.remediation = Some(TRANSPORT_REMEDIATION);
            body.is_admin = *is_admin;
        }
        SessionAuthority::Unprovisioned => {
            body.authority_state = "unprovisioned";
            body.remediation = Some(UNPROVISIONED_REMEDIATION);
        }
    }
    serde_json::to_value(&body)
        .map_err(|_| ToolError::internal_message("failed to encode session authority"))
}

/// Project one authenticated caller into the `/auth/session` body.
///
/// Owner bootstrap is offered only when the caller passes the shared
/// admission rule (the same predicate `POST /v1/access/bootstrap-owner`
/// enforces) and the access runtime reports it would accept bootstrap now.
async fn project_session(
    state: &AppState,
    caller: SessionCaller,
    view: SessionView,
) -> Result<serde_json::Value, ToolError> {
    let admitted = crate::access::owner_bootstrap_admission(
        &caller.bootstrap_caller(),
        state
            .auth_config
            .as_ref()
            .map(|config| config.admin_email.as_str()),
    )
    .is_ok();
    let authority =
        resolve_session_authority(state, caller.identity, caller.transport_admin).await?;
    let owner_bootstrap_available = admitted
        && state.access_runtime.owner_bootstrap_offer().await == OwnerBootstrapOffer::Available;
    authenticated_session_body(
        &view,
        &authority,
        owner_bootstrap_available,
        static_bearer_login_available(state),
    )
}

/// Transport facts for the anonymous OAuth cookie branch.
///
/// `/auth/session` is outside the auth layer, so no `AuthContext` exists here.
/// Scopes are derived with the same `labby_auth` rule the auth layer applies to
/// this cookie. The configured admin email is always an authorized identity;
/// every other identity receives either `lab:read` or scopes with `:admin`
/// lowered, so passing `authorized = is_configured_admin` gives the same
/// answer to "does this caller hold `lab:admin`" without a second allowlist
/// lookup.
fn oauth_cookie_caller(
    auth_state: &labby_auth::state::AuthState,
    session: &labby_auth::types::BrowserSessionRow,
    identity: labby_auth::VerifiedIdentity,
) -> SessionCaller {
    let is_configured_admin = labby_auth::is_configured_admin_email(
        &auth_state.config.admin_email,
        session.email.as_deref(),
    );
    SessionCaller {
        identity,
        via_session: true,
        subject: session.subject.clone(),
        email: session.email.clone(),
        scopes: labby_auth::browser_session_scopes(
            &auth_state.config.static_token_scopes,
            is_configured_admin,
            is_configured_admin,
        ),
        transport_admin: false,
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

fn bearer_exchange_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    if state
        .auth_config
        .as_ref()
        .and_then(|config| config.public_url.as_ref())
        .is_some_and(|url| url.scheme() == "https")
    {
        return true;
    }
    let Some(authority) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Ok(url) = url::Url::parse(&format!("http://{authority}")) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Ipv4(value)) => value.is_loopback(),
        Some(url::Host::Ipv6(value)) => value.is_loopback(),
        Some(url::Host::Domain(value)) => value.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

fn bearer_exchange_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(serde_json::json!({ "ok": false, "message": message })),
    )
        .into_response()
}

pub async fn auth_bearer_session(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let start = Instant::now();
    let request_id = request_id(&headers).map(ToOwned::to_owned);
    log_auth_dispatch_start("session.bearer_exchange", request_id.as_deref());

    if !bearer_exchange_allowed(&state, &headers) {
        log_auth_dispatch(
            "session.bearer_exchange",
            request_id.as_deref(),
            start,
            Some("secure_transport_required"),
            None,
        );
        return bearer_exchange_error(
            StatusCode::BAD_REQUEST,
            "bearer browser sign-in requires HTTPS or a loopback origin",
        );
    }
    if !static_bearer_login_available(&state) {
        return bearer_exchange_error(StatusCode::UNAUTHORIZED, "invalid bearer credential");
    }
    let Some(expected) = state.bearer_token.as_ref() else {
        log_auth_dispatch(
            "session.bearer_exchange",
            request_id.as_deref(),
            start,
            Some("not_configured"),
            None,
        );
        return bearer_exchange_error(StatusCode::UNAUTHORIZED, "invalid bearer credential");
    };
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(labby_auth::parse_bearer_token)
    else {
        log_auth_dispatch(
            "session.bearer_exchange",
            request_id.as_deref(),
            start,
            Some("missing_credential"),
            None,
        );
        return bearer_exchange_error(StatusCode::UNAUTHORIZED, "invalid bearer credential");
    };
    if !labby_auth::tokens_equal(&token, expected.as_ref()) {
        log_auth_dispatch(
            "session.bearer_exchange",
            request_id.as_deref(),
            start,
            Some("invalid_credential"),
            None,
        );
        return bearer_exchange_error(StatusCode::UNAUTHORIZED, "invalid bearer credential");
    }
    match labby_auth::static_session::has_other_browser_session(
        &headers,
        state.project_session_state.as_deref(),
        state.oauth_state.as_deref(),
    )
    .await
    {
        Ok(true) => {
            return bearer_exchange_error(
                StatusCode::CONFLICT,
                "Sign out of the current browser session before using a setup token.",
            );
        }
        Err(_) => return internal_error_response("failed to load browser session"),
        Ok(false) => {}
    }
    let Some(session_state) = state.static_browser_session_state.as_ref() else {
        return bearer_exchange_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "bearer browser sessions are unavailable",
        );
    };
    let session = match session_state.create() {
        Ok(session) => session,
        Err(error) => {
            tracing::error!(error = %error, "failed to create static bearer browser session");
            return bearer_exchange_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to create browser session",
            );
        }
    };
    let mut response = no_store_json(serde_json::json!({ "ok": true }));
    labby_auth::session::append_set_cookie(
        &mut response,
        &session_state.set_cookie(&session.session_id),
    );
    log_auth_dispatch(
        "session.bearer_exchange",
        request_id.as_deref(),
        start,
        None,
        None,
    );
    response
}

async fn reject_mixed_static_sessions(state: &AppState, headers: &HeaderMap) -> Option<Response> {
    static_browser_session(state, headers)?;
    match labby_auth::static_session::has_other_browser_session(
        headers,
        state.project_session_state.as_deref(),
        state.oauth_state.as_deref(),
    )
    .await
    {
        Ok(false) => None,
        Ok(true) => Some(bearer_exchange_error(
            StatusCode::CONFLICT,
            "Multiple browser sessions are present. Clear this site's cookies and sign in again.",
        )),
        Err(_) => Some(internal_error_response("failed to load browser session")),
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

    if let Some(response) = reject_mixed_static_sessions(&state, &headers).await {
        return response;
    }

    // This route intentionally remains outside the auth middleware so an
    // anonymous browser can discover login availability. Resolve Labby's
    // project-bound cookie explicitly before the development UI fallback;
    // otherwise bearer-only deployments render an authenticated-but-unbound
    // shell even though the browser holds a valid project session.
    match load_project_session(&state, &headers).await {
        Ok(Some(session)) => {
            if let Some(binding) = session.project_binding.as_ref() {
                let actor_key = actor_key_for_session(&state, &session);
                let outcome = async {
                    let caller = SessionCaller {
                        identity: project_session_identity(binding)?,
                        via_session: true,
                        subject: binding.principal_id.clone(),
                        email: session.email.clone(),
                        scopes: binding.scopes.clone(),
                        transport_admin: scopes_grant_admin(&binding.scopes),
                    };
                    let view = SessionView {
                        login_available,
                        user: SessionUser {
                            sub: binding.principal_id.clone(),
                            email: session.email.clone(),
                        },
                        project_id: Some(binding.project_id.clone()),
                        expires_at: session.expires_at,
                        csrf_token: session.csrf_token.clone(),
                    };
                    project_session(&state, caller, view).await
                }
                .await;
                return finish_session_get(
                    request_id.as_deref(),
                    start,
                    actor_key.as_deref(),
                    outcome,
                );
            }
        }
        Ok(None) => {}
        Err(error) => {
            return finish_session_get(request_id.as_deref(), start, None, Err(error));
        }
    }

    if let Some(session) = static_browser_session(&state, &headers) {
        let outcome = async {
            let identity = labby_auth::VerifiedIdentity::local_credential(
                labby_auth::Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .map_err(|_| ToolError::internal_message("authenticated identity is invalid"))?;
            let caller = SessionCaller {
                identity,
                via_session: false,
                subject: "static-bearer".to_owned(),
                email: None,
                scopes: Vec::new(),
                transport_admin: true,
            };
            let view = SessionView {
                login_available,
                user: SessionUser {
                    sub: "static-bearer".to_owned(),
                    email: None,
                },
                project_id: None,
                expires_at: session.expires_at,
                csrf_token: session.csrf_token.clone(),
            };
            project_session(&state, caller, view).await
        }
        .await;
        return finish_session_get(request_id.as_deref(), start, None, outcome);
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
        let outcome = async {
            // The static bearer is the local operator credential.
            let identity = labby_auth::VerifiedIdentity::local_credential(
                labby_auth::Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .map_err(|_| ToolError::internal_message("authenticated identity is invalid"))?;
            let caller = SessionCaller {
                identity,
                via_session: false,
                subject: "static-bearer".to_owned(),
                email: None,
                scopes: Vec::new(),
                transport_admin: true,
            };
            let view = SessionView {
                login_available,
                user: SessionUser {
                    sub: "static-bearer".to_owned(),
                    email: None,
                },
                project_id: None,
                expires_at: DEV_SESSION_EXPIRES_AT,
                csrf_token: String::new(),
            };
            project_session(&state, caller, view).await
        }
        .await;
        return finish_session_get(request_id.as_deref(), start, None, outcome);
    }

    let Some(auth_state) = oauth_state(&state) else {
        let response =
            unauthenticated_session_response(false, static_bearer_login_available(&state));
        log_auth_dispatch("session.get", request_id.as_deref(), start, None, None);
        return response;
    };

    match load_browser_session(auth_state, &headers).await {
        Ok(Some(session)) => {
            let actor_key = actor_key_for_session(&state, &session);
            let outcome = async {
                let identity = oauth_browser_session_identity(auth_state, &session)?;
                let caller = oauth_cookie_caller(auth_state, &session, identity);
                let view = SessionView {
                    login_available,
                    user: SessionUser {
                        sub: session.subject.clone(),
                        email: session.email.clone(),
                    },
                    project_id: session
                        .project_binding
                        .as_ref()
                        .map(|binding| binding.project_id.clone()),
                    expires_at: session.expires_at,
                    csrf_token: session.csrf_token.clone(),
                };
                project_session(&state, caller, view).await
            }
            .await;
            finish_session_get(request_id.as_deref(), start, actor_key.as_deref(), outcome)
        }
        Ok(None) => {
            let response = unauthenticated_session_response(
                login_available,
                static_bearer_login_available(&state),
            );
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
    let bound_session = if context.via_session {
        load_project_session(state, headers).await?
    } else {
        None
    };
    let (project_id, expires_at) = match bound_session {
        Some(session) => (
            session
                .project_binding
                .as_ref()
                .map(|binding| binding.project_id.clone()),
            if session.expires_at >= 0 {
                session.expires_at
            } else {
                DEV_SESSION_EXPIRES_AT
            },
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
    let view = SessionView {
        login_available: false,
        user: SessionUser {
            sub: context.sub.clone(),
            email: context.email.clone(),
        },
        project_id,
        expires_at,
        csrf_token: context.csrf_token.unwrap_or_default(),
    };
    let caller = SessionCaller {
        identity,
        via_session: context.via_session,
        transport_admin: scopes_grant_admin(&context.scopes),
        subject: context.sub,
        email: context.email,
        scopes: context.scopes,
    };
    project_session(state, caller, view).await
}

pub async fn auth_logout(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let stale_cookie = state
        .static_browser_session_state
        .as_ref()
        .and_then(|sessions| {
            let id = labby_auth::session::read_cookie(&headers, sessions.cookie_name())?;
            sessions
                .find(&id)
                .is_none()
                .then(|| sessions.clear_cookie())
        });
    let mut response = auth_logout_inner(State(state), headers).await;
    if let Some(cookie) = stale_cookie {
        labby_auth::session::append_set_cookie(&mut response, &cookie);
    }
    response
}

async fn auth_logout_inner(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let start = Instant::now();
    let request_id = request_id(&headers).map(ToOwned::to_owned);
    log_auth_dispatch_start("session.logout", request_id.as_deref());

    if let Some(response) = reject_mixed_static_sessions(&state, &headers).await {
        return response;
    }

    if let Some(session_state) = state.static_browser_session_state.as_ref()
        && let Some(session_id) =
            labby_auth::session::read_cookie(&headers, session_state.cookie_name())
        && let Some(session) = session_state.find(&session_id)
    {
        let csrf = headers
            .get(BROWSER_CSRF_HEADER_NAME)
            .and_then(|value| value.to_str().ok());
        if csrf != Some(session.csrf_token.as_str()) {
            log_auth_dispatch(
                "session.logout",
                request_id.as_deref(),
                start,
                Some("validation_failed"),
                None,
            );
            return invalid_csrf_response();
        }
        session_state.revoke(&session_id);
        let mut response = StatusCode::NO_CONTENT.into_response();
        labby_auth::session::append_set_cookie(&mut response, &session_state.clear_cookie());
        log_auth_dispatch("session.logout", request_id.as_deref(), start, None, None);
        return response;
    }

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

    #[tokio::test]
    async fn logout_revokes_oauth_when_static_cookie_is_stale() {
        let directory = tempfile::tempdir().unwrap();
        let config = labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some("https://lab.example.com".parse().unwrap()),
            sqlite_path: directory.path().join("auth.db"),
            key_path: directory.path().join("auth-key.pem"),
            admin_email: "owner@different.example".into(),
            viewer_email_domains: vec!["example.org".into()],
            session_cookie_name: "__Host-labby-session".into(),
            google: labby_auth::config::GoogleConfig {
                client_id: "client".into(),
                client_secret: "secret".into(),
                callback_url: None,
                callback_path: "/auth/google/callback".into(),
                scopes: vec!["openid".into(), "email".into()],
            },
            token_encryption_key: Some(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(&"11".repeat(32)).unwrap(),
            ),
            ..Default::default()
        };
        let auth = labby_auth::state::AuthState::new(config.clone())
            .await
            .unwrap();
        let row = BrowserSessionRow {
            session_id: "oauth-session".into(),
            subject: "oauth-owner".into(),
            email: None,
            csrf_token: "oauth-csrf".into(),
            created_at: labby_auth::util::now_unix(),
            expires_at: labby_auth::util::now_unix() + 3600,
            project_binding: None,
        };
        auth.store
            .upsert_browser_session(row.clone())
            .await
            .unwrap();
        let state = AppState::new()
            .with_auth_config(config)
            .with_oauth_state(auth.clone())
            .with_static_browser_session_state(
                labby_auth::static_session::StaticBrowserSessionState::new(false),
            );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static(
                "__Host-labby-session=oauth-session; labby_bearer_session=stale-session",
            ),
        );
        headers.insert(
            BROWSER_CSRF_HEADER_NAME,
            HeaderValue::from_static("oauth-csrf"),
        );
        let response = auth_logout(State(state), headers).await.into_response();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(
            auth.store
                .find_browser_session(&row.session_id)
                .await
                .unwrap()
                .is_none()
        );
        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect();
        assert!(cookies.iter().any(
            |value| value.starts_with("labby_bearer_session=;") && value.contains("Max-Age=0")
        ));
        assert!(cookies.iter().any(
            |value| value.starts_with("__Host-labby-session=;") && value.contains("Max-Age=0")
        ));
    }

    #[tokio::test]
    async fn disabled_static_bearer_cannot_mint_or_introspect_a_browser_session() {
        let config = labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            disable_static_token_with_oauth: true,
            ..Default::default()
        };
        let state = AppState::new()
            .with_auth_config(config)
            .with_bearer_token(Some(std::sync::Arc::from("operator-token")))
            .with_static_browser_session_state(
                labby_auth::static_session::StaticBrowserSessionState::new(false),
            );
        let sessions = state.static_browser_session_state.as_ref().unwrap();
        let row = sessions.create().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8765"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer operator-token"),
        );
        headers.insert(
            header::COOKIE,
            format!("{}={}", sessions.cookie_name(), row.session_id)
                .parse()
                .unwrap(),
        );
        assert!(!static_bearer_login_available(&state));
        assert!(static_browser_session(&state, &headers).is_none());
        let response = auth_bearer_session(State(state), headers)
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }

    #[tokio::test]
    async fn static_browser_exchange_and_session_reject_other_browser_authority() {
        let directory = tempfile::tempdir().unwrap();
        let project = labby_auth::project_session::ProjectSessionState::open(
            directory.path().join("project.db"),
            "__Host-project-session",
        )
        .await
        .unwrap();
        project
            .store
            .upsert_browser_session(BrowserSessionRow {
                session_id: "project-session".into(),
                subject: "project-user".into(),
                email: None,
                csrf_token: "project-csrf".into(),
                created_at: labby_auth::util::now_unix(),
                expires_at: labby_auth::util::now_unix() + 3600,
                project_binding: None,
            })
            .await
            .unwrap();
        let mut state = AppState::new()
            .with_bearer_token(Some(std::sync::Arc::from("operator-token")))
            .with_static_browser_session_state(
                labby_auth::static_session::StaticBrowserSessionState::new(false),
            );
        state.project_session_state = Some(std::sync::Arc::new(project));
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8765"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer operator-token"),
        );
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-project-session=project-session"),
        );
        let response = auth_bearer_session(State(state.clone()), headers.clone())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(!response.headers().contains_key(header::SET_COOKIE));
        let sessions = state.static_browser_session_state.as_ref().unwrap();
        let row = sessions.create().unwrap();
        headers.remove(header::AUTHORIZATION);
        headers.insert(
            header::COOKIE,
            format!(
                "__Host-project-session=project-session; {}={}",
                sessions.cookie_name(),
                row.session_id
            )
            .parse()
            .unwrap(),
        );
        let response = auth_session(State(state.clone()), headers.clone(), None, None)
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        headers.insert(BROWSER_CSRF_HEADER_NAME, row.csrf_token.parse().unwrap());
        let response = auth_logout(State(state.clone()), headers)
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn static_browser_exchange_introspection_and_logout() {
        let state = AppState::new()
            .with_bearer_token(Some(std::sync::Arc::from("operator-token")))
            .with_static_browser_session_state(
                labby_auth::static_session::StaticBrowserSessionState::new(false),
            );
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8765"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer operator-token"),
        );
        let response = auth_bearer_session(State(state.clone()), headers.clone())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        headers.remove(header::AUTHORIZATION);
        headers.insert(header::COOKIE, cookie.parse().unwrap());
        let response = auth_session(State(state.clone()), headers.clone(), None, None)
            .await
            .into_response();
        let body = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["authenticated"], true);
        assert_eq!(payload["bearer_login_available"], true);
        let response = auth_logout(State(state.clone()), headers.clone())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        headers.insert(
            BROWSER_CSRF_HEADER_NAME,
            payload["csrf_token"].as_str().unwrap().parse().unwrap(),
        );
        let response = auth_logout(State(state.clone()), headers.clone())
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(static_browser_session(&state, &headers).is_none());
    }

    #[tokio::test]
    async fn viewer_domain_google_session_introspection_stays_authenticated_without_admin() {
        use tower::ServiceExt as _;
        let directory = tempfile::tempdir().unwrap();
        let config = labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some("https://lab.example.com".parse().unwrap()),
            sqlite_path: directory.path().join("auth.db"),
            key_path: directory.path().join("auth-key.pem"),
            admin_email: "owner@different.example".into(),
            viewer_email_domains: vec!["example.org".into()],
            session_cookie_name: "__Host-labby-session".into(),
            google: labby_auth::config::GoogleConfig {
                client_id: "client".into(),
                client_secret: "secret".into(),
                callback_url: None,
                callback_path: "/auth/google/callback".into(),
                scopes: vec!["openid".into(), "email".into()],
            },
            token_encryption_key: Some(
                labby_auth::at_rest::TokenEncryptionKey::from_encoded(&"11".repeat(32)).unwrap(),
            ),
            ..Default::default()
        };
        let auth = labby_auth::state::AuthState::new(config.clone())
            .await
            .unwrap();
        let session = labby_auth::session::create_bound_browser_session(
            &auth,
            "verified-viewer".into(),
            Some("viewer@example.org".into()),
            auth.inbound_provider_binding(),
        )
        .await
        .unwrap();
        auth.store
            .upsert_bound_verified_inbound_identity(
                &session.subject,
                session.email.as_deref().unwrap(),
                labby_auth::util::now_unix(),
                auth.inbound_provider_binding(),
            )
            .await
            .unwrap();
        let state = AppState::new()
            .with_auth_config(config)
            .with_oauth_state(auth.clone());
        let router = axum::Router::new()
            .route("/auth/session", axum::routing::get(auth_session))
            .with_state(state);
        let mut baseline = None;
        for _ in 0..3 {
            let response = router
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .uri("/auth/session")
                        .header(
                            header::COOKIE,
                            format!("__Host-labby-session={}", session.session_id),
                        )
                        .body(axum::body::Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-store"
            );
            assert!(!response.headers().contains_key(header::LOCATION));
            assert!(!response.headers().contains_key(header::SET_COOKIE));
            let body = axum::body::to_bytes(response.into_body(), 16384)
                .await
                .unwrap();
            let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(json["authenticated"], true);
            assert_eq!(json["is_admin"], false);
            assert_eq!(json["expires_at"], session.expires_at);
            assert_eq!(json["csrf_token"], session.csrf_token);
            if let Some(expected) = &baseline {
                assert_eq!(&json, expected);
            } else {
                baseline = Some(json);
            }
        }
        assert_eq!(
            auth.store
                .find_browser_session(&session.session_id)
                .await
                .unwrap(),
            Some(session)
        );
    }

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
        let state = AppState::new()
            .with_project_session_state(session_state)
            .with_bearer_token(Some(std::sync::Arc::from("operator-token")));
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
        assert_eq!(json["login_available"], false);
        assert_eq!(json["bearer_login_available"], true);
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
        assert_eq!(json["login_available"], false);
        assert_eq!(json["bearer_login_available"], true);
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
        let state = AppState::new()
            .with_access_runtime(runtime)
            .with_auth_config(labby_auth::config::AuthConfig {
                admin_email: "owner@example.com".into(),
                ..Default::default()
            });
        let view = |sub: &str| SessionView {
            login_available: false,
            user: SessionUser {
                sub: sub.to_owned(),
                email: None,
            },
            project_id: None,
            expires_at: 1,
            csrf_token: String::new(),
        };

        let ready = resolve_session_authority(&state, owner, false)
            .await
            .unwrap();
        assert!(matches!(ready, SessionAuthority::Ready(_)));
        let body = authenticated_session_body(&view("owner"), &ready, false, false).unwrap();
        assert_eq!(body["authority_state"], "ready");
        assert_eq!(body["is_admin"], true);
        assert_eq!(body["owner_bootstrap_available"], false);
        assert!(body.get("remediation").is_none());

        let stranger = || {
            labby_auth::VerifiedIdentity::external(
                labby_auth::Authenticator::BrowserSession,
                "https://accounts.google.com",
                "stranger",
            )
            .unwrap()
        };
        let unprovisioned = resolve_session_authority(&state, stranger(), true)
            .await
            .unwrap();
        assert!(matches!(unprovisioned, SessionAuthority::Unprovisioned));

        // A caller that passes the shared admission rule is still never
        // offered bootstrap once an owner exists: the runtime reports
        // `AlreadyOwned`.
        let admitted = SessionCaller {
            identity: stranger(),
            via_session: true,
            subject: "stranger".into(),
            email: Some("owner@example.com".into()),
            scopes: vec!["lab:admin".into()],
            transport_admin: true,
        };
        assert!(
            crate::access::owner_bootstrap_admission(
                &admitted.bootstrap_caller(),
                Some("owner@example.com")
            )
            .is_ok()
        );
        let body = project_session(&state, admitted, view("stranger"))
            .await
            .unwrap();
        assert_eq!(body["authority_state"], "unprovisioned");
        assert_eq!(
            body["is_admin"], false,
            "transport scope never grants durable admin"
        );
        assert_eq!(body["owner_bootstrap_available"], false);
        assert!(!body["remediation"].as_str().unwrap().contains("bootstrap"));

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
