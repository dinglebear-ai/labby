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

/// Transport facts for a static-bearer browser session cookie, derived from
/// the same `labby_auth` rule the auth layer applies to this cookie on
/// protected routes. `/auth/session` runs outside the layer, so this is how
/// introspection stays in agreement with the middleware.
fn static_cookie_context(
    state: &AppState,
    session: &labby_auth::types::BrowserSessionRow,
) -> AuthContext {
    let deriver = state
        .actor_key_deriver
        .clone()
        .map(crate::api::router_middleware::lab_auth_deriver);
    labby_auth::static_session::browser_session_auth_context(
        deriver.as_deref(),
        &state.static_token_scopes(),
        session,
    )
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

/// Whether the login screen may offer browser token sign-in to this request.
/// The static bearer must be usable, and the exchange it leads to must be one
/// [`bearer_exchange_allowed`] would accept over this transport; advertising
/// it over plaintext from another host would invite the operator to paste the
/// long-lived credential into a request the server then refuses.
fn bearer_login_advertised(state: &AppState, headers: &HeaderMap) -> bool {
    static_bearer_login_available(state) && bearer_exchange_allowed(state, headers)
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
    /// Whether this browser may offer bearer token sign-in from where it
    /// stands; see [`bearer_login_advertised`].
    bearer_login_available: bool,
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
    /// Transport facts from an [`AuthContext`], whichever path minted it.
    fn from_context(identity: labby_auth::VerifiedIdentity, context: AuthContext) -> Self {
        Self {
            identity,
            via_session: context.via_session,
            transport_admin: scopes_grant_admin(&context.scopes),
            subject: context.sub,
            email: context.email,
            scopes: context.scopes,
        }
    }

    /// The predicate the administrator-list and allowlist routes enforce
    /// (`auth_admin::require_admin`): a browser session whose email is in
    /// `LABBY_AUTH_ADMIN_EMAIL`.
    fn is_configured_admin(&self, state: &AppState) -> bool {
        self.via_session
            && state.auth_config.as_ref().is_some_and(|config| {
                labby_auth::is_configured_admin_email(&config.admin_emails, self.email.as_deref())
            })
    }

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
    /// The caller is a browser session whose email is listed in
    /// `LABBY_AUTH_ADMIN_EMAIL`: the only caller the administrator-list and
    /// allowlist routes accept. Independent of `is_admin`, which any
    /// `platform.manage` principal holds.
    is_configured_admin: bool,
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
    is_configured_admin: bool,
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
        is_configured_admin,
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
            .map(|config| config.admin_emails.as_slice()),
    )
    .is_ok();
    let is_configured_admin = caller.is_configured_admin(state);
    let mut authority =
        resolve_session_authority(state, caller.identity.clone(), caller.transport_admin).await?;
    if matches!(authority, SessionAuthority::Unprovisioned)
        && admit_allowlisted_identity(state, &caller).await
    {
        authority =
            resolve_session_authority(state, caller.identity, caller.transport_admin).await?;
    }
    let owner_bootstrap_available = admitted
        && state.access_runtime.owner_bootstrap_offer().await == OwnerBootstrapOffer::Available;
    authenticated_session_body(
        &view,
        &authority,
        owner_bootstrap_available,
        is_configured_admin,
        view.bearer_login_available,
    )
}

/// Durable admission for a browser session whose provider-verified email is
/// on the allowlist or in `LABBY_AUTH_ADMIN_EMAIL`. Returns `true` when a
/// Principal was created or already existed, so the caller re-resolves
/// authority. The session's display email is never consulted: evidence comes
/// from the provider-verified identity row bound to this issuer and subject.
async fn admit_allowlisted_identity(state: &AppState, caller: &SessionCaller) -> bool {
    let (Some(auth_state), Some(config)) = (oauth_state(state), state.auth_config.as_ref()) else {
        return false;
    };
    if !caller.via_session {
        return false;
    }
    let labby_auth::PrincipalLink::External { issuer, subject } = caller.identity.principal_link()
    else {
        return false;
    };
    let email = match auth_state
        .store
        .current_verified_inbound_email(issuer, subject)
        .await
    {
        Ok(Some(email)) => email,
        Ok(None) => return false,
        Err(error) => {
            // Fail closed to `unprovisioned`, but leave a trace; no email,
            // subject, or issuer is logged.
            tracing::warn!(
                surface = "api",
                service = "auth",
                action = "session.get",
                error = %error,
                "verified identity lookup failed; session stays unprovisioned"
            );
            return false;
        }
    };
    // Cheap pre-check so sessions the allowlist does not admit never contend
    // for the access writer. The runtime re-runs the same resolution under
    // the writer and provisions what that re-check returns.
    if allowlist_admission(auth_state, config, &email)
        .await
        .is_none()
    {
        return false;
    }
    match state
        .access_runtime
        .provision_allowlisted(caller.identity.clone(), || {
            allowlist_admission(auth_state, config, &email)
        })
        .await
    {
        Ok(_) => true,
        Err(crate::access::AllowlistProvisionError::Withdrawn) => {
            tracing::info!(
                surface = "api",
                service = "auth",
                action = "session.get",
                "allowlist entry was withdrawn before provisioning; session stays unprovisioned"
            );
            false
        }
        // Expected while the durable state that refuses it stands (a disabled
        // or suspended membership); the session polls this on every read, so
        // it is not an operator warning.
        Err(error @ crate::access::AllowlistProvisionError::Refused) => {
            tracing::debug!(
                surface = "api",
                service = "auth",
                action = "session.get",
                reason = %error,
                "allowlist admission refused by durable state; session stays unprovisioned"
            );
            false
        }
        Err(error) => {
            // Deadline exhausted or store unavailable: fail closed to
            // `unprovisioned`, but leave a trace; identity and email are
            // never logged.
            tracing::warn!(
                surface = "api",
                service = "auth",
                action = "session.get",
                error = %error,
                "allowlist admission failed; session stays unprovisioned"
            );
            false
        }
    }
}

/// What the allowlist, or `LABBY_AUTH_ADMIN_EMAIL`, grants `email` right now.
/// `None` means not admitted; a lookup failure also yields `None` after a
/// trace, failing closed to `unprovisioned`. No email is logged.
async fn allowlist_admission(
    auth_state: &labby_auth::state::AuthState,
    config: &labby_auth::config::AuthConfig,
    email: &str,
) -> Option<(
    crate::access::AllowedUserRole,
    crate::access::AllowlistAdmission,
)> {
    if config.is_admin_email(email) {
        return Some((
            crate::access::AllowedUserRole::Admin,
            crate::access::AllowlistAdmission::ConfiguredAdminEmail,
        ));
    }
    let allowed = match auth_state.store.find_allowed_user(email).await {
        Ok(allowed) => allowed,
        Err(error) => {
            tracing::warn!(
                surface = "api",
                service = "auth",
                action = "session.get",
                error = %error,
                "allowlist lookup failed; session stays unprovisioned"
            );
            return None;
        }
    };
    // `added_by` is the adding administrator's provider subject: only its
    // fingerprint is carried into the audit record.
    allowed.map(|row| {
        (
            row.role,
            crate::access::AllowlistAdmission::AllowlistEntry {
                added_by_fingerprint: labby_auth::util::fingerprint(&row.added_by),
            },
        )
    })
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
        &auth_state.config.admin_emails,
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

/// The long-lived operator bearer may cross the wire only over TLS or a
/// loopback hop. A configured HTTPS public URL proves TLS at the proxy; without
/// it the request must be a direct loopback connection, judged by the same
/// rules as the local-session and bootstrap-proof routes: any forwarding
/// header means a proxy rewrote `Host`, and loopback is decided by the shared
/// host validator rather than a local parse.
fn bearer_exchange_allowed(state: &AppState, headers: &HeaderMap) -> bool {
    if state
        .auth_config
        .as_ref()
        .and_then(|config| config.public_url.as_ref())
        .is_some_and(|url| url.scheme() == "https")
    {
        return true;
    }
    if crate::api::host_validation::has_forwarding_headers(headers) {
        return false;
    }
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(crate::api::host_validation::is_loopback_host_value)
}

/// A bearer-exchange refusal in the shared agent-error envelope. `kind` is one
/// of the stable vocabulary entries documented in docs/dev/ERRORS.md; HTTP
/// status follows from it through `ApiError`, never from this call site.
fn bearer_exchange_refusal(kind: &str, message: &str) -> Response {
    tool_error_response(ToolError::Sdk {
        sdk_kind: kind.to_owned(),
        message: message.to_owned(),
    })
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
        return bearer_exchange_refusal(
            "forbidden",
            "bearer browser sign-in requires HTTPS or a loopback origin",
        );
    }
    if !static_bearer_login_available(&state) {
        return bearer_exchange_refusal("auth_failed", "invalid bearer credential");
    }
    let Some(expected) = state.bearer_token.as_ref() else {
        log_auth_dispatch(
            "session.bearer_exchange",
            request_id.as_deref(),
            start,
            Some("not_configured"),
            None,
        );
        return bearer_exchange_refusal("auth_failed", "invalid bearer credential");
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
        return bearer_exchange_refusal("auth_failed", "invalid bearer credential");
    };
    if !labby_auth::tokens_equal(&token, expected.as_ref()) {
        log_auth_dispatch(
            "session.bearer_exchange",
            request_id.as_deref(),
            start,
            Some("invalid_credential"),
            None,
        );
        return bearer_exchange_refusal("auth_failed", "invalid bearer credential");
    }
    match labby_auth::static_session::has_other_browser_session(
        &headers,
        state.project_session_state.as_deref(),
        state.oauth_state.as_deref(),
    )
    .await
    {
        Ok(true) => {
            return bearer_exchange_refusal(
                "conflict",
                "Sign out of the current browser session before using a setup token.",
            );
        }
        Err(_) => return internal_error_response("failed to load browser session"),
        Ok(false) => {}
    }
    let Some(session_state) = state.static_browser_session_state.as_ref() else {
        return bearer_exchange_refusal(
            "service_unavailable",
            "bearer browser sessions are unavailable",
        );
    };
    let session = match session_state.create() {
        Ok(session) => session,
        Err(error) => {
            tracing::error!(error = %error, "failed to create static bearer browser session");
            return internal_error_response("failed to create browser session");
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
        Ok(true) => Some(bearer_exchange_refusal(
            "conflict",
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
                        bearer_login_available: bearer_login_advertised(&state, &headers),
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
            let context = static_cookie_context(&state, &session);
            let view = SessionView {
                login_available,
                bearer_login_available: bearer_login_advertised(&state, &headers),
                user: SessionUser {
                    sub: context.sub.clone(),
                    email: context.email.clone(),
                },
                project_id: None,
                expires_at: session.expires_at,
                csrf_token: context.csrf_token.clone().unwrap_or_default(),
            };
            project_session(&state, SessionCaller::from_context(identity, context), view).await
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
            "is_configured_admin": true,
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
                bearer_login_available: bearer_login_advertised(&state, &headers),
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
            unauthenticated_session_response(false, bearer_login_advertised(&state, &headers));
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
                    bearer_login_available: bearer_login_advertised(&state, &headers),
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
        bearer_login_available: bearer_login_advertised(state, headers),
        user: SessionUser {
            sub: context.sub.clone(),
            email: context.email.clone(),
        },
        project_id,
        expires_at,
        csrf_token: context.csrf_token.clone().unwrap_or_default(),
    };
    project_session(state, SessionCaller::from_context(identity, context), view).await
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
            admin_emails: vec!["owner@different.example".into()],
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
    async fn bearer_exchange_refuses_forwarded_loopback_host() {
        // Labby on 127.0.0.1 behind a same-host plaintext proxy sees a loopback
        // Host header while the operator bearer crossed the LAN in clear text.
        // Any forwarding header means a proxy is in the path; only a configured
        // https public_url proves TLS, so the exchange must refuse, exactly as
        // the sibling local-session and bootstrap-proof routes do.
        let state = AppState::new()
            .with_bearer_token(Some(std::sync::Arc::from("operator-token")))
            .with_static_browser_session_state(
                labby_auth::static_session::StaticBrowserSessionState::new(false),
            );
        let exchange = |host: &'static str, forwarded: Option<(&'static str, &'static str)>| {
            let state = state.clone();
            async move {
                let mut headers = HeaderMap::new();
                headers.insert(header::HOST, HeaderValue::from_static(host));
                headers.insert(
                    header::AUTHORIZATION,
                    HeaderValue::from_static("Bearer operator-token"),
                );
                if let Some((name, value)) = forwarded {
                    headers.insert(name, HeaderValue::from_static(value));
                }
                auth_bearer_session(State(state), headers)
                    .await
                    .into_response()
            }
        };
        for (name, value) in [
            ("forwarded", "for=192.168.1.20;proto=http"),
            ("x-forwarded-for", "192.168.1.20"),
            ("x-forwarded-host", "lan-name"),
            ("x-forwarded-proto", "http"),
            ("x-forwarded-proto", "https"),
            ("x-real-ip", "192.168.1.20"),
        ] {
            let response = exchange("127.0.0.1:8765", Some((name, value))).await;
            assert!(
                response.status().is_client_error(),
                "{name}: {}",
                response.status()
            );
            assert!(
                !response.headers().contains_key(header::SET_COOKIE),
                "{name}"
            );
        }
        // Parity with the shared loopback rule: bracketed IPv6 and
        // case-insensitive localhost still mint a session without a proxy.
        for host in ["[::1]:8765", "LOCALHOST:8765", "127.0.0.1"] {
            let response = exchange(host, None).await;
            assert_eq!(response.status(), StatusCode::OK, "{host}");
            assert!(
                response.headers().contains_key(header::SET_COOKIE),
                "{host}"
            );
        }
        let response = exchange("192.168.1.20:8765", None).await;
        assert!(response.status().is_client_error());
        assert!(!response.headers().contains_key(header::SET_COOKIE));
    }

    #[tokio::test]
    async fn bearer_exchange_errors_carry_stable_kinds() {
        // Every refusal from the bearer exchange and from static-cookie
        // introspection uses the shared ApiError envelope: a stable `kind` plus
        // the versioned recovery contract, never a bare `{ok:false,message}`.
        async fn envelope(response: Response) -> (StatusCode, serde_json::Value) {
            let status = response.status();
            assert_eq!(
                response.headers()[header::CACHE_CONTROL],
                "private, no-store"
            );
            assert!(!response.headers().contains_key(header::SET_COOKIE));
            let body = axum::body::to_bytes(response.into_body(), 16384)
                .await
                .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["contract_version"], 1, "{body}");
            assert!(body["message"].is_string(), "{body}");
            assert!(body["recovery"]["action"].is_string(), "{body}");
            assert!(body.get("ok").is_none(), "{body}");
            (status, body)
        }
        let state = AppState::new()
            .with_bearer_token(Some(std::sync::Arc::from("operator-token")))
            .with_static_browser_session_state(
                labby_auth::static_session::StaticBrowserSessionState::new(false),
            );
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("192.168.1.20:8765"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer operator-token"),
        );
        let (status, body) = envelope(
            auth_bearer_session(State(state.clone()), headers.clone())
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["kind"], "forbidden");

        headers.insert(header::HOST, HeaderValue::from_static("localhost:8765"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer not-the-operator-token"),
        );
        let (status, body) = envelope(
            auth_bearer_session(State(state.clone()), headers.clone())
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["kind"], "auth_failed");
        headers.remove(header::AUTHORIZATION);
        let (status, body) = envelope(
            auth_bearer_session(State(state.clone()), headers.clone())
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["kind"], "auth_failed");

        // A competing browser session conflicts on both the exchange and the
        // static-cookie introspection.
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
        let mut mixed = state.clone();
        mixed.project_session_state = Some(std::sync::Arc::new(project));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer operator-token"),
        );
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-project-session=project-session"),
        );
        let (status, body) = envelope(
            auth_bearer_session(State(mixed.clone()), headers.clone())
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["kind"], "conflict");
        let sessions = mixed.static_browser_session_state.as_ref().unwrap();
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
        let (status, body) = envelope(
            auth_session(State(mixed), headers.clone(), None, None)
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["kind"], "conflict");

        // No static session store: the exchange is unavailable, not broken.
        let unavailable =
            AppState::new().with_bearer_token(Some(std::sync::Arc::from("operator-token")));
        headers.remove(header::COOKIE);
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer operator-token"),
        );
        let (status, body) = envelope(
            auth_bearer_session(State(unavailable), headers)
                .await
                .into_response(),
        )
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["kind"], "service_unavailable");
    }

    #[tokio::test]
    async fn unauthenticated_session_does_not_advertise_bearer_login_over_plaintext_remote_host() {
        // The login screen offers the bearer form only when the exchange it
        // leads to would be accepted: HTTPS via the public URL or a direct
        // loopback hop. A bearer-only install reached over plain HTTP from
        // another host must not invite the operator to paste the token.
        let state = AppState::new()
            .with_bearer_token(Some(std::sync::Arc::from("operator-token")))
            .with_static_browser_session_state(
                labby_auth::static_session::StaticBrowserSessionState::new(false),
            );
        let session = |host: &'static str, forwarded: bool| {
            let state = state.clone();
            async move {
                let mut headers = HeaderMap::new();
                headers.insert(header::HOST, HeaderValue::from_static(host));
                if forwarded {
                    headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));
                }
                let response = auth_session(State(state), headers, None, None)
                    .await
                    .into_response();
                assert_eq!(response.status(), StatusCode::OK);
                let body = axum::body::to_bytes(response.into_body(), 16384)
                    .await
                    .unwrap();
                serde_json::from_slice::<serde_json::Value>(&body).unwrap()
            }
        };
        let remote = session("192.168.1.20:8765", false).await;
        assert_eq!(remote["authenticated"], false);
        assert_eq!(remote["bearer_login_available"], false, "{remote}");
        let proxied = session("127.0.0.1:8765", true).await;
        assert_eq!(proxied["bearer_login_available"], false, "{proxied}");
        let local = session("localhost:8765", false).await;
        assert_eq!(local["bearer_login_available"], true, "{local}");

        // Operators must be able to find the requirement and the TLS-proxy
        // unlock without reading source.
        for (doc, path) in [
            (
                include_str!("../../../../docs/services/SETUP.md"),
                "docs/services/SETUP.md",
            ),
            (include_str!("../../../../README.md"), "README.md"),
        ] {
            assert!(
                doc.contains("HTTPS or a direct loopback connection"),
                "{path} must document when browser token sign-in is offered"
            );
            assert!(
                doc.contains("LABBY_PUBLIC_URL=https://"),
                "{path} must document that an HTTPS public URL unlocks it behind a TLS proxy"
            );
        }
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

    /// `GET /auth/session` runs outside the auth layer, so the static-cookie
    /// branch states its own transport facts. Those facts must be the ones the
    /// middleware mints for the same cookie on authenticated routes: the
    /// middleware grants the configured static-token scopes `via_session`,
    /// and introspection must not report a different admin ceiling.
    #[tokio::test]
    async fn static_cookie_session_facts_match_middleware() {
        use tower::ServiceExt as _;
        let directory = tempfile::tempdir().unwrap();
        let config = labby_auth::config::AuthConfig {
            mode: labby_auth::config::AuthMode::OAuth,
            public_url: Some("https://lab.example.com".parse().unwrap()),
            sqlite_path: directory.path().join("auth.db"),
            key_path: directory.path().join("auth-key.pem"),
            admin_emails: vec!["owner@example.com".into()],
            session_cookie_name: "__Host-labby-session".into(),
            // Deliberately below the legacy grant so a disagreement between
            // the two derivations is observable as the admin ceiling.
            static_token_scopes: vec!["lab:read".into()],
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
        let state = AppState::new()
            .with_auth_config(config)
            .with_oauth_state(auth.clone())
            .with_bearer_token(Some(std::sync::Arc::from("operator-token")))
            .with_static_browser_session_state(
                labby_auth::static_session::StaticBrowserSessionState::new(false),
            );
        let sessions = state.static_browser_session_state.clone().unwrap();
        let row = sessions.create().unwrap();
        let cookie = format!("{}={}", sessions.cookie_name(), row.session_id);

        // The facts the auth layer attaches for this cookie on a protected route.
        let layer = labby_auth::middleware::AuthLayer::from_state(std::sync::Arc::new(auth))
            .with_static_token(Some(std::sync::Arc::from("operator-token")))
            .with_allow_session_cookie(true)
            .with_static_browser_session_state(Some(sessions.clone()));
        let probe = axum::Router::new()
            .route(
                "/probe",
                axum::routing::get(|Extension(context): Extension<AuthContext>| async move {
                    Json(serde_json::json!({
                        "sub": context.sub,
                        "via_session": context.via_session,
                        "scopes": context.scopes,
                        "csrf_token": context.csrf_token,
                    }))
                }),
            )
            .route_layer(layer);
        let response = probe
            .oneshot(
                axum::http::Request::builder()
                    .uri("/probe")
                    .header(header::COOKIE, &cookie)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        let middleware: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let middleware_scopes: Vec<String> =
            serde_json::from_value(middleware["scopes"].clone()).unwrap();
        assert_eq!(middleware["via_session"], true);
        assert_eq!(middleware_scopes, vec!["lab:read".to_string()]);

        // The introspection branch derives its caller from the shared rule.
        let context = static_cookie_context(&state, &row);
        assert_eq!(middleware["via_session"], context.via_session);
        assert_eq!(middleware["sub"], context.sub);
        assert_eq!(
            middleware["csrf_token"],
            serde_json::json!(context.csrf_token)
        );
        assert_eq!(middleware_scopes, context.scopes);

        // Introspection for the same cookie must project the same facts.
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8765"));
        headers.insert(header::COOKIE, cookie.parse().unwrap());
        let response = auth_session(State(state.clone()), headers.clone(), None, None)
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["authenticated"], true);
        assert_eq!(payload["user"]["sub"], middleware["sub"]);
        assert_eq!(payload["csrf_token"], middleware["csrf_token"]);
        assert_eq!(
            payload["is_admin"],
            scopes_grant_admin(&middleware_scopes),
            "introspection admin ceiling must follow the middleware's scopes: {payload}"
        );
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
            admin_emails: vec!["owner@different.example".into()],
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
        // A browser always sends Host; bearer login is advertised only where
        // the exchange would be accepted, so this is a direct loopback hop.
        headers.insert(header::HOST, HeaderValue::from_static("localhost:8765"));
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
                admin_emails: vec!["owner@example.com".into()],
                ..Default::default()
            });
        let view = |sub: &str| SessionView {
            login_available: false,
            bearer_login_available: false,
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
        let body = authenticated_session_body(&view("owner"), &ready, false, false, false).unwrap();
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
                Some(&["owner@example.com".to_owned()])
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

    /// OAuth-mode fixture: a Ready access store owned by a static-bearer
    /// operator, `eli@example.com` allowlisted as `admin` with a verified
    /// identity, and a verified stranger who is not allowlisted.
    struct AllowlistFixture {
        _directory: tempfile::TempDir,
        state: AppState,
        auth_state: labby_auth::state::AuthState,
        auth_config: labby_auth::config::AuthConfig,
    }

    impl AllowlistFixture {
        async fn new() -> Self {
            let directory = tempfile::Builder::new()
                .prefix("labby-allowlist-admission-")
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
                    crate::access::BootstrapOwnerInput::new(owner, "Local", "Default").unwrap(),
                )
                .await
                .unwrap();
            let auth_config = labby_auth::config::AuthConfig {
                mode: labby_auth::config::AuthMode::OAuth,
                public_url: Some(url::Url::parse("https://lab.example.com").unwrap()),
                sqlite_path: directory.path().join("auth.db"),
                key_path: directory.path().join("auth-jwt.pem"),
                admin_emails: vec!["owner@example.com".into()],
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
            };
            let auth_state = labby_auth::state::AuthState::new(auth_config.clone())
                .await
                .unwrap();
            let binding = auth_state.inbound_provider_binding();
            auth_state
                .store
                .add_allowed_user("eli@example.com", "owner-sub", "admin", 1)
                .await
                .unwrap();
            for (subject, email) in [
                ("eli-sub", "eli@example.com"),
                ("stranger-sub", "stranger@example.com"),
            ] {
                auth_state
                    .store
                    .upsert_bound_verified_inbound_identity(subject, email, 2, binding.clone())
                    .await
                    .unwrap();
            }
            let state = AppState::new()
                .with_access_runtime(runtime)
                .with_auth_config(auth_config.clone())
                .with_oauth_state(auth_state.clone());
            Self {
                _directory: directory,
                state,
                auth_state,
                auth_config,
            }
        }

        fn identity(subject: &str) -> labby_auth::VerifiedIdentity {
            labby_auth::VerifiedIdentity::external(
                labby_auth::Authenticator::BrowserSession,
                "https://accounts.google.com",
                subject,
            )
            .unwrap()
        }

        fn caller(subject: &str, email: &str) -> SessionCaller {
            SessionCaller {
                identity: Self::identity(subject),
                via_session: true,
                subject: subject.into(),
                email: Some(email.into()),
                scopes: vec!["lab:read".into(), "lab".into()],
                transport_admin: false,
            }
        }

        fn view(sub: &str) -> SessionView {
            SessionView {
                login_available: true,
                bearer_login_available: false,
                user: SessionUser {
                    sub: sub.to_owned(),
                    email: None,
                },
                project_id: None,
                expires_at: 1,
                csrf_token: String::new(),
            }
        }

        async fn session(&self, subject: &str, email: &str) -> serde_json::Value {
            project_session(
                &self.state,
                Self::caller(subject, email),
                Self::view(subject),
            )
            .await
            .unwrap()
        }

        fn provision_audit_rows(&self) -> i64 {
            let connection = rusqlite::Connection::open_with_flags(
                self._directory.path().join("access.db"),
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .unwrap();
            connection
                .query_row(
                    "SELECT count(*) FROM access_audit WHERE action='access.allowlist.provision'",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
        }
    }

    /// An allowlisted identity is admitted on its first `/auth/session`:
    /// the durable authority is created and the same call projects `ready`.
    #[tokio::test]
    async fn allowlisted_session_is_provisioned_on_first_session_read() {
        let fixture = AllowlistFixture::new().await;
        let body = fixture.session("eli-sub", "eli@example.com").await;
        assert_eq!(body["authority_state"], "ready");
        assert_eq!(
            body["is_admin"], true,
            "allowlist role admin grants platform.manage"
        );

        // Display email is not evidence: a session claiming an allowlisted
        // email whose verified identity is someone else stays unprovisioned.
        let body = fixture.session("stranger-sub", "eli@example.com").await;
        assert_eq!(body["authority_state"], "unprovisioned");

        // A caller whose display email is on the allowlist but who has no
        // verified-identity row at all (never completed the provider flow)
        // stays unprovisioned: there is no evidence to admit against.
        let body = fixture.session("unverified-sub", "eli@example.com").await;
        assert_eq!(body["authority_state"], "unprovisioned");
    }

    /// Finding 3: the administrator list and the allowlist accept only a
    /// browser session whose email is in `LABBY_AUTH_ADMIN_EMAIL`. The body
    /// says so explicitly, because `is_admin` (`platform.manage`) is also true
    /// for every allowlist-admitted admin, who must not be offered those
    /// controls.
    #[tokio::test]
    async fn session_body_reports_whether_the_caller_is_a_configured_admin() {
        let fixture = AllowlistFixture::new().await;
        fixture
            .auth_state
            .store
            .upsert_bound_verified_inbound_identity(
                "owner-sub",
                "owner@example.com",
                2,
                fixture.auth_state.inbound_provider_binding(),
            )
            .await
            .unwrap();

        let body = fixture.session("eli-sub", "eli@example.com").await;
        assert_eq!(body["authority_state"], "ready");
        assert_eq!(
            body["is_admin"], true,
            "allowlist admin is a platform admin"
        );
        assert_eq!(
            body["is_configured_admin"], false,
            "an allowlist admin is not a configured admin"
        );

        let body = fixture.session("owner-sub", "owner@example.com").await;
        assert_eq!(body["authority_state"], "ready");
        assert_eq!(body["is_admin"], true);
        assert_eq!(body["is_configured_admin"], true);

        // The transport credential is the local operator but never a
        // configured-admin browser session.
        let operator = SessionCaller {
            identity: labby_auth::VerifiedIdentity::local_credential(
                labby_auth::Authenticator::StaticBearer,
                "static-bearer:primary",
            )
            .unwrap(),
            via_session: false,
            subject: "static-bearer".into(),
            email: None,
            scopes: Vec::new(),
            transport_admin: true,
        };
        let body = project_session(
            &fixture.state,
            operator,
            AllowlistFixture::view("static-bearer"),
        )
        .await
        .unwrap();
        assert_eq!(body["is_admin"], true);
        assert_eq!(body["is_configured_admin"], false);
    }

    /// Finding 6: first-sign-in admission must not fail closed after the
    /// 100 ms bootstrap-writer wait. A writer held for 300 ms (audit or
    /// policy persistence in flight) is normal contention, not an outage.
    #[tokio::test]
    async fn allowlisted_admission_waits_for_a_busy_writer() {
        let fixture = AllowlistFixture::new().await;
        let writer = fixture
            .state
            .access_runtime
            .acquire_bootstrap_writer()
            .await
            .unwrap();
        let release = async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            drop(writer);
        };
        let (body, ()) = tokio::join!(fixture.session("eli-sub", "eli@example.com"), release);
        assert_eq!(body["authority_state"], "ready");
        assert_eq!(fixture.provision_audit_rows(), 1);
    }

    /// Finding 2: the allowlist lookup and the durable provisioning live in
    /// different databases. The entry must be re-validated under the access
    /// writer; an entry that vanished in between admits nothing and leaves no
    /// Principal or audit row behind.
    #[tokio::test]
    async fn allowlist_admission_is_refused_when_the_entry_vanishes_before_provisioning() {
        let fixture = AllowlistFixture::new().await;
        let eli = AllowlistFixture::identity("eli-sub");
        let outcome = fixture
            .state
            .access_runtime
            .provision_allowlisted(eli.clone(), || async {
                // The administrator deletes the entry after the handler's
                // first lookup succeeded but before the writer was acquired.
                fixture
                    .auth_state
                    .store
                    .remove_allowed_user("eli@example.com")
                    .await
                    .unwrap();
                allowlist_admission(&fixture.auth_state, &fixture.auth_config, "eli@example.com")
                    .await
            })
            .await;
        assert_eq!(
            outcome.unwrap_err(),
            crate::access::AllowlistProvisionError::Withdrawn
        );
        let store = fixture.state.access_runtime.store().await.unwrap();
        assert!(
            matches!(
                store.session_authority(eli).await,
                Err(AccessStoreError::IdentityUnavailable)
            ),
            "no Principal may exist for an admission that was withdrawn"
        );
        assert_eq!(fixture.provision_audit_rows(), 0);
        let body = fixture.session("eli-sub", "eli@example.com").await;
        assert_eq!(body["authority_state"], "unprovisioned");
    }

    /// The same race through the real session path: while a session read
    /// waits for a busy access writer, the entry is removed. Once the writer
    /// frees up the read must re-check the allowlist and stay unprovisioned.
    #[tokio::test]
    async fn allowlist_admission_revalidates_after_waiting_for_the_writer() {
        let fixture = AllowlistFixture::new().await;
        let writer = fixture
            .state
            .access_runtime
            .acquire_bootstrap_writer()
            .await
            .unwrap();
        let session = fixture.session("eli-sub", "eli@example.com");
        let removal = async {
            // Let the session read pass its first lookup and block on the
            // writer, then withdraw the entry and release the writer well
            // inside the admission deadline.
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            fixture
                .auth_state
                .store
                .remove_allowed_user("eli@example.com")
                .await
                .unwrap();
            drop(writer);
        };
        let (body, ()) = tokio::join!(session, removal);
        assert_eq!(body["authority_state"], "unprovisioned");
        assert_eq!(fixture.provision_audit_rows(), 0);
    }
}
