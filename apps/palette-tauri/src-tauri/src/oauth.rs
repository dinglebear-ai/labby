//! OAuth 2.0 (Authorization Code + PKCE) login client for a Labby server.
//!
//! `labby-auth` is a full RFC 8414/7591/8252 native-app OAuth provider (fronting
//! Google login): discovery, dynamic client registration with loopback
//! `redirect_uri`s always allowed, mandatory PKCE (S256), and `/token` for both
//! `authorization_code` and `refresh_token` grants. So unlike some IdPs, this
//! client runs the whole flow itself: it binds a loopback listener
//! (`callback_server`), registers that as its `redirect_uri`, opens the system
//! browser, and waits for the browser to redirect back with the code — no
//! server-side "native polling" extension needed.
//!
//! Credentials are cached in `OauthState` (Tauri-managed). Separate refresh,
//! login, and persistence guards provide single-flight behavior without
//! holding the credential-cache lock across network or filesystem I/O.

pub(crate) mod callback_server;
pub(crate) mod flow;
pub(crate) mod pkce;
pub(crate) mod secret;
pub(crate) mod status;
pub(crate) mod store;

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tauri::AppHandle;

use crate::labby_bridge::BridgeClient;
use crate::oauth::status::{OauthStatus, status_for};
use crate::oauth::store::StoredCredentials;
use crate::{merged_settings, validate_saved_server_url};

/// Client login timeout, kept below the server's 300s auth-request TTL so the
/// client times out first with a clear message.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(240);
/// Refresh the access token this many seconds before its stated expiry.
const EXPIRY_SKEW_SECS: i64 = 60;
/// Hard ceiling on a token refresh so a stalled `/token` can't hold the
/// credential lock (and freeze all bridge calls) indefinitely.
const REFRESH_TIMEOUT: Duration = Duration::from_secs(30);
const SCOPE: &str = "lab";

/// Cached credentials for the current process. `Unloaded` until first access,
/// then `Loaded(Some|None)`.
enum CredCache {
    Unloaded,
    Loaded(Option<StoredCredentials>),
}

/// Tauri-managed OAuth state with short-lived cache locking and independent
/// single-flight guards for refresh, login, and durable persistence.
pub(crate) struct OauthState {
    creds: tokio::sync::Mutex<CredCache>,
    login: tokio::sync::Mutex<()>,
    refresh: tokio::sync::Mutex<()>,
    persist: tokio::sync::Mutex<()>,
    revision: AtomicU64,
}

impl OauthState {
    pub(crate) fn new() -> Self {
        OauthState {
            creds: tokio::sync::Mutex::new(CredCache::Unloaded),
            login: tokio::sync::Mutex::new(()),
            refresh: tokio::sync::Mutex::new(()),
            persist: tokio::sync::Mutex::new(()),
            revision: AtomicU64::new(0),
        }
    }
}

impl Default for OauthState {
    fn default() -> Self {
        Self::new()
    }
}

#[tauri::command]
pub(crate) async fn labby_oauth_login(
    app: AppHandle,
    bridge: tauri::State<'_, BridgeClient>,
    oauth_state: tauri::State<'_, OauthState>,
) -> Result<OauthStatus, String> {
    // Serialize interactive logins — a second concurrent click is rejected.
    let _login_guard = oauth_state
        .login
        .try_lock()
        .map_err(|_| "a sign-in is already in progress".to_string())?;

    let settings = merged_settings(&app).await?;
    let server_url = validate_saved_server_url(&settings.server_url)?;
    let client = bridge.client().clone();

    let creds = run_login(&client, &server_url).await?;
    replace_and_persist(&app, &oauth_state, Some(creds.clone())).await?;
    Ok(status_for(Some(&creds), &server_url))
}

#[tauri::command]
pub(crate) async fn labby_oauth_logout(
    app: AppHandle,
    oauth_state: tauri::State<'_, OauthState>,
) -> Result<OauthStatus, String> {
    replace_and_persist(&app, &oauth_state, None).await?;
    Ok(OauthStatus::signed_out())
}

#[tauri::command]
pub(crate) async fn labby_oauth_status(
    app: AppHandle,
    oauth_state: tauri::State<'_, OauthState>,
) -> Result<OauthStatus, String> {
    let settings = merged_settings(&app).await?;
    let server_url = validate_saved_server_url(&settings.server_url)?;
    ensure_loaded(&app, &oauth_state).await;
    let snapshot = credential_snapshot(&oauth_state).await;
    Ok(status_for(snapshot.as_ref(), &server_url))
}

/// The cached OAuth access token for `server_url`, refreshed if expired. Holds
/// the cache lock across any refresh so concurrent callers single-flight.
async fn effective_access_token(
    app: &AppHandle,
    client: &reqwest::Client,
    server_url: &str,
    state: &OauthState,
) -> Option<String> {
    // Defense in depth: never hand an OAuth token to a cleartext non-loopback
    // server (e.g. a hand-edited/migrated oauth.json with an http:// URL).
    if flow::require_secure_url(server_url).is_err() {
        return None;
    }
    ensure_loaded(app, state).await;
    let snapshot = credential_snapshot(state).await;

    // Fast path: valid cached token for this server, no refresh needed.
    {
        let creds = snapshot.as_ref()?;
        if !creds.matches_server(server_url) {
            return None;
        }
        if !creds.is_expired(now_unix(), EXPIRY_SKEW_SECS) {
            return Some(creds.access_token.expose().to_string());
        }
    }

    // Expired — a dedicated refresh guard provides single-flight without
    // holding the credential cache across network or disk I/O.
    let _refresh_guard = state.refresh.lock().await;
    let (snapshot_revision, snapshot) = credential_snapshot_with_revision(state).await;
    if let Some(creds) = snapshot.as_ref() {
        if creds.matches_server(server_url) && !creds.is_expired(now_unix(), EXPIRY_SKEW_SECS) {
            return Some(creds.access_token.expose().to_string());
        }
    }
    match refresh_snapshot(app, client, server_url, state, snapshot_revision, snapshot).await {
        RefreshResult::Refreshed(token) => Some(token),
        RefreshResult::Cleared | RefreshResult::Kept => None,
    }
}

/// The decision a `/token` refresh yields, separated from its side effects so it
/// is unit-testable without a Tauri `AppHandle`.
enum RefreshOutcome {
    /// Success — persist, update the cache, and emit `oauth-changed`.
    Refreshed(StoredCredentials),
    /// Definitive grant rejection — wipe the session and emit `oauth-changed`.
    Cleared,
    /// Transient failure or timeout — keep the session untouched.
    Kept,
}

/// Outcome of a refresh attempt, surfaced to the bridge so a reactive 401 becomes a precise message.
enum RefreshResult {
    /// A fresh access token to retry the request with.
    Refreshed(String),
    /// The OAuth session was definitively revoked/expired and has been cleared.
    Cleared,
    /// No change — no OAuth session for this server, a transient failure, or a timeout.
    Kept,
}

/// Classify a refresh result into an outcome. Pure (no I/O, no clock).
fn classify_refresh(
    result: Result<flow::TokenResponse, flow::TokenError>,
    client_id: String,
    server_url: &str,
    token_endpoint: String,
    prior_refresh_token: Option<crate::oauth::secret::Secret>,
    now_unix: i64,
) -> RefreshOutcome {
    match result {
        Ok(token) => RefreshOutcome::Refreshed(credentials_from_token(
            client_id,
            server_url,
            token_endpoint,
            prior_refresh_token,
            token,
            now_unix,
        )),
        Err(err) if err.rejected => RefreshOutcome::Cleared,
        Err(_) => RefreshOutcome::Kept,
    }
}

/// Refresh from an owned cache snapshot. Network and durable persistence both
/// happen without holding `OauthState::creds`; a revision/token comparison
/// prevents a stale refresh from replacing a newer login/logout.
async fn refresh_snapshot(
    app: &AppHandle,
    client: &reqwest::Client,
    server_url: &str,
    state: &OauthState,
    snapshot_revision: u64,
    snapshot: Option<StoredCredentials>,
) -> RefreshResult {
    let prior_access_token = snapshot
        .as_ref()
        .map(|creds| creds.access_token.expose().to_string());
    let refresh_parts = snapshot
        .as_ref()
        .filter(|creds| creds.matches_server(server_url))
        .and_then(|creds| {
            creds.refresh_token.as_ref().map(|rt| {
                (
                    creds.client_id.clone(),
                    creds.token_endpoint.clone(),
                    rt.expose().to_string(),
                    // Owned copy of the prior token so a refresh response that
                    // omits refresh_token can reuse it (RFC 6749 §6).
                    creds.refresh_token.clone(),
                )
            })
        });
    let Some((client_id, token_endpoint, refresh_token, prior_refresh_token)) = refresh_parts
    else {
        return RefreshResult::Kept;
    };
    let Ok(token_endpoint) = flow::require_secure_url(&token_endpoint) else {
        return RefreshResult::Kept;
    };
    let token_endpoint = token_endpoint.to_string();
    let refresh = flow::refresh_access_token(client, &token_endpoint, &client_id, &refresh_token);
    let result = match tokio::time::timeout(REFRESH_TIMEOUT, refresh).await {
        Ok(r) => r,
        Err(_) => Err(flow::TokenError {
            rejected: false,
            message: "token refresh timed out".to_string(),
        }),
    };
    match classify_refresh(
        result,
        client_id,
        server_url,
        token_endpoint,
        prior_refresh_token,
        now_unix(),
    ) {
        RefreshOutcome::Refreshed(refreshed) => {
            let access = refreshed.access_token.expose().to_string();
            let mut cache = state.creds.lock().await;
            let current_matches = state.revision.load(Ordering::Acquire) == snapshot_revision
                && cache_matches_access(&cache, prior_access_token.as_deref());
            if !current_matches {
                return RefreshResult::Kept;
            }
            *cache = CredCache::Loaded(Some(refreshed));
            let applied_revision = state.revision.fetch_add(1, Ordering::AcqRel) + 1;
            drop(cache);
            if let Err(err) = persist_cached_credentials(app, state).await {
                crate::warn(format!("refreshed OAuth token not persisted: {err}"));
            }
            if state.revision.load(Ordering::Acquire) != applied_revision {
                return credential_snapshot(state)
                    .await
                    .filter(|current| current.matches_server(server_url))
                    .map(|current| {
                        RefreshResult::Refreshed(current.access_token.expose().to_string())
                    })
                    .unwrap_or(RefreshResult::Kept);
            }
            emit_oauth_changed(app);
            RefreshResult::Refreshed(access)
        }
        RefreshOutcome::Cleared => {
            let mut cache = state.creds.lock().await;
            let current_matches = state.revision.load(Ordering::Acquire) == snapshot_revision
                && cache_matches_access(&cache, prior_access_token.as_deref());
            if !current_matches {
                return RefreshResult::Kept;
            }
            *cache = CredCache::Loaded(None);
            let applied_revision = state.revision.fetch_add(1, Ordering::AcqRel) + 1;
            drop(cache);
            if let Err(err) = persist_cached_credentials(app, state).await {
                crate::warn(format!(
                    "failed to clear dead OAuth session from disk: {err}"
                ));
            }
            if state.revision.load(Ordering::Acquire) != applied_revision {
                return RefreshResult::Kept;
            }
            emit_oauth_changed(app);
            RefreshResult::Cleared
        }
        RefreshOutcome::Kept => RefreshResult::Kept,
    }
}

fn emit_oauth_changed(app: &AppHandle) {
    use tauri::Emitter;
    if let Err(err) = app.emit("palette://oauth-changed", ()) {
        crate::warn(format!("failed to emit oauth-changed event: {err}"));
    }
}

/// Force a refresh regardless of apparent validity — used by the bridge on a 401
/// (proactive expiry can miss clock skew or a server-side revocation).
async fn force_refresh(
    app: &AppHandle,
    client: &reqwest::Client,
    server_url: &str,
    state: &OauthState,
) -> RefreshResult {
    if flow::require_secure_url(server_url).is_err() {
        return RefreshResult::Kept;
    }
    ensure_loaded(app, state).await;
    let _refresh_guard = state.refresh.lock().await;
    let (snapshot_revision, snapshot) = credential_snapshot_with_revision(state).await;
    refresh_snapshot(app, client, server_url, state, snapshot_revision, snapshot).await
}

/// Send a bridge request with the resolved auth token; if the server answers
/// 401 and a forced refresh yields a new token, resend once. `make`
/// builds a fresh `RequestBuilder` (method/URL/headers/body) given the token to
/// attach, so the request can be rebuilt for the retry.
pub(crate) async fn send_with_reauth<F>(
    app: &AppHandle,
    client: &reqwest::Client,
    server_url: &str,
    static_token: Option<&str>,
    state: &OauthState,
    make: F,
) -> Result<reqwest::Response, String>
where
    F: Fn(Option<&str>) -> reqwest::RequestBuilder,
{
    let oauth = effective_access_token(app, client, server_url, state).await;
    let token = pick_token(oauth, static_token.map(str::to_string));
    let response = make(token.as_deref())
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        match force_refresh(app, client, server_url, state).await {
            RefreshResult::Refreshed(fresh) => {
                return make(Some(&fresh))
                    .send()
                    .await
                    .map_err(|err| err.to_string());
            }
            RefreshResult::Cleared => {
                // Session revoked/expired and cleared: fall back to a static bearer
                // token if configured, else tell the user (shown as the action error).
                if let Some(static_token) = static_token {
                    return make(Some(static_token))
                        .send()
                        .await
                        .map_err(|err| err.to_string());
                }
                return Err(
                    "Your Labby OAuth session expired or was revoked — sign in again in Settings."
                        .to_string(),
                );
            }
            RefreshResult::Kept => {}
        }
    }
    Ok(response)
}

/// Run the browser-based authorization-code flow and return fresh credentials.
///
/// Binds a loopback listener first (RFC 8252 §7.3), registers that port's
/// `redirect_uri` with the server (dynamic client registration), opens the
/// system browser to `/authorize`, then waits on the loopback listener for the
/// browser's redirect carrying the authorization code.
async fn run_login(
    client: &reqwest::Client,
    server_url: &str,
) -> Result<StoredCredentials, String> {
    flow::require_secure_url(server_url)?;
    let meta = flow::discover(client, server_url).await?;
    let registration_endpoint = meta.registration_endpoint.clone().ok_or_else(|| {
        "this server does not support OAuth login (dynamic client registration is disabled) — \
         use a static bearer token instead"
            .to_string()
    })?;
    // Validate every server-supplied endpoint before using it.
    flow::require_secure_url(&meta.authorization_endpoint)?;
    flow::require_secure_url(&meta.token_endpoint)?;
    flow::require_secure_url(&registration_endpoint)?;

    // Prefer the server-hosted native callback + poll flow when advertised:
    // the browser only ever talks to the real HTTPS server, never a
    // client-run loopback listener, so there's no HTTP/HTTPS loopback
    // ambiguity for the browser to get wrong (Chrome et al. upgrading a
    // `http://localhost:PORT` redirect to HTTPS, breaking a plain-HTTP
    // loopback listener). Fall back to the RFC 8252 loopback listener for a
    // server that doesn't support it.
    match (&meta.native_callback_endpoint, &meta.native_poll_endpoint) {
        (Some(native_callback_endpoint), Some(native_poll_endpoint)) => {
            flow::require_secure_url(native_callback_endpoint)?;
            flow::require_secure_url(native_poll_endpoint)?;
            run_login_via_native_poll(
                client,
                server_url,
                &meta,
                &registration_endpoint,
                native_callback_endpoint,
                native_poll_endpoint,
            )
            .await
        }
        _ => run_login_via_loopback(client, server_url, &meta, &registration_endpoint).await,
    }
}

async fn run_login_via_native_poll(
    client: &reqwest::Client,
    server_url: &str,
    meta: &flow::AuthServerMetadata,
    registration_endpoint: &str,
    native_callback_endpoint: &str,
    native_poll_endpoint: &str,
) -> Result<StoredCredentials, String> {
    let client_id =
        flow::register_client(client, registration_endpoint, native_callback_endpoint).await?;

    let verifier = pkce::generate_code_verifier();
    let challenge = pkce::code_challenge_s256(&verifier);
    let state = pkce::generate_state();
    let authorize_url = flow::build_authorize_url(
        meta,
        &client_id,
        native_callback_endpoint,
        SCOPE,
        &state,
        &challenge,
    )?;

    if let Err(err) = open::that(&authorize_url) {
        return Err(format!(
            "failed to open the system browser — open this URL manually to sign in:\n{authorize_url}\n({err})"
        ));
    }

    let code = flow::poll_native_code(client, native_poll_endpoint, &state, LOGIN_TIMEOUT)
        .await
        .map_err(|err| {
            format!("{err}. If the browser did not open, sign in here:\n{authorize_url}")
        })?;

    let token = flow::exchange_code(
        client,
        &meta.token_endpoint,
        &code,
        &client_id,
        native_callback_endpoint,
        &verifier,
    )
    .await?;

    Ok(credentials_from_token(
        client_id,
        server_url,
        meta.token_endpoint.clone(),
        None,
        token,
        now_unix(),
    ))
}

async fn run_login_via_loopback(
    client: &reqwest::Client,
    server_url: &str,
    meta: &flow::AuthServerMetadata,
    registration_endpoint: &str,
) -> Result<StoredCredentials, String> {
    let listener = callback_server::bind().await?;

    let client_id =
        flow::register_client(client, registration_endpoint, &listener.redirect_uri).await?;

    let verifier = pkce::generate_code_verifier();
    let challenge = pkce::code_challenge_s256(&verifier);
    let state = pkce::generate_state();
    let authorize_url = flow::build_authorize_url(
        meta,
        &client_id,
        &listener.redirect_uri,
        SCOPE,
        &state,
        &challenge,
    )?;

    if let Err(err) = open::that(&authorize_url) {
        return Err(format!(
            "failed to open the system browser — open this URL manually to sign in:\n{authorize_url}\n({err})"
        ));
    }

    let code = listener
        .await_code(&state, LOGIN_TIMEOUT)
        .await
        .map_err(|err| {
            format!("{err}. If the browser did not open, sign in here:\n{authorize_url}")
        })?;

    let token = flow::exchange_code(
        client,
        &meta.token_endpoint,
        &code,
        &client_id,
        &listener.redirect_uri,
        &verifier,
    )
    .await?;

    Ok(credentials_from_token(
        client_id,
        server_url,
        meta.token_endpoint.clone(),
        None,
        token,
        now_unix(),
    ))
}

fn credentials_from_token(
    client_id: String,
    server_url: &str,
    token_endpoint: String,
    prior_refresh_token: Option<crate::oauth::secret::Secret>,
    token: flow::TokenResponse,
    now_unix: i64,
) -> StoredCredentials {
    StoredCredentials {
        client_id,
        access_token: token.access_token,
        // RFC 6749 §6: a refresh response MAY omit refresh_token, meaning reuse
        // the prior one. Falling through to None would break all later refreshes.
        refresh_token: token.refresh_token.or(prior_refresh_token),
        token_endpoint,
        // Clamp so a malformed huge `expires_in` can't wrap negative (→ permanently expired).
        expires_at_unix: now_unix.saturating_add(token.expires_in.min(i64::MAX as u64) as i64),
        scope: token.scope,
        server_url: server_url.trim_end_matches('/').to_string(),
    }
}

async fn credential_snapshot(state: &OauthState) -> Option<StoredCredentials> {
    match &*state.creds.lock().await {
        CredCache::Loaded(credentials) => credentials.clone(),
        CredCache::Unloaded => None,
    }
}

async fn credential_snapshot_with_revision(state: &OauthState) -> (u64, Option<StoredCredentials>) {
    let cache = state.creds.lock().await;
    let snapshot = match &*cache {
        CredCache::Loaded(credentials) => credentials.clone(),
        CredCache::Unloaded => None,
    };
    (state.revision.load(Ordering::Acquire), snapshot)
}

fn cache_matches_access(cache: &CredCache, expected: Option<&str>) -> bool {
    matches!(cache, CredCache::Loaded(Some(current)) if Some(current.access_token.expose()) == expected)
}

/// Populate the cache from disk on first use without holding the cache mutex
/// across filesystem I/O. A concurrent login/logout wins the final race.
async fn ensure_loaded(app: &AppHandle, state: &OauthState) {
    if matches!(&*state.creds.lock().await, CredCache::Loaded(_)) {
        return;
    }
    let path = match store::credentials_path(app) {
        Ok(path) => path,
        Err(err) => {
            crate::warn(err);
            return;
        }
    };
    let loaded = crate::persistence::run_blocking_io(move || Ok(store::load(&path)))
        .await
        .unwrap_or_else(|err| {
            crate::warn(format!("OAuth credential reader failed: {err}"));
            None
        });
    let mut cache = state.creds.lock().await;
    if matches!(*cache, CredCache::Unloaded) {
        *cache = CredCache::Loaded(loaded);
    }
}

/// Serialize durable writes, but snapshot the cache only while briefly locked.
/// The revision loop guarantees the final on-disk value is the newest cache
/// generation even when login, logout, and refresh race.
async fn persist_cached_credentials(app: &AppHandle, state: &OauthState) -> Result<(), String> {
    let _persist_guard = state.persist.lock().await;
    let path = store::credentials_path(app)?;
    loop {
        let (revision, snapshot) = credential_snapshot_with_revision(state).await;
        let worker_path = path.clone();
        crate::persistence::run_blocking_io(move || match snapshot {
            Some(credentials) => store::save(&worker_path, &credentials),
            None => store::clear(&worker_path),
        })
        .await?;
        if state.revision.load(Ordering::Acquire) == revision {
            return Ok(());
        }
    }
}

async fn replace_and_persist(
    app: &AppHandle,
    state: &OauthState,
    replacement: Option<StoredCredentials>,
) -> Result<(), String> {
    let mut cache = state.creds.lock().await;
    let previous = match &*cache {
        CredCache::Loaded(credentials) => credentials.clone(),
        CredCache::Unloaded => None,
    };
    *cache = CredCache::Loaded(replacement);
    let revision = state.revision.fetch_add(1, Ordering::AcqRel) + 1;
    drop(cache);

    if let Err(error) = persist_cached_credentials(app, state).await {
        let mut cache = state.creds.lock().await;
        if state.revision.load(Ordering::Acquire) == revision {
            *cache = CredCache::Loaded(previous);
            state.revision.fetch_add(1, Ordering::AcqRel);
        }
        return Err(error);
    }
    Ok(())
}

/// Prefer an OAuth token over the static bearer token.
pub(crate) fn pick_token(oauth: Option<String>, static_token: Option<String>) -> Option<String> {
    oauth.or(static_token)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "oauth_tests.rs"]
mod tests;
