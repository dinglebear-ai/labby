use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect};
use axum::{Json, response::Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

#[cfg(test)]
static CALLBACK_PROVIDER_LOCK_REACHED: std::sync::LazyLock<tokio::sync::Notify> =
    std::sync::LazyLock::new(tokio::sync::Notify::new);
#[cfg(test)]
static CALLBACK_CAS_PAUSE_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
static CALLBACK_CAS_OBSERVED: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(0));
#[cfg(test)]
static CALLBACK_CAS_RESUME: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| tokio::sync::Semaphore::new(0));

use crate::error::AuthError;
use crate::google::{AuthorizeUrlRequest, merge_google_scopes};
use crate::session::{append_set_cookie, build_browser_session_cookie, create_browser_session};
use crate::state::AuthState;
use crate::types::{
    AuthorizationCodeRow, AuthorizationRequestRow, AuthorizeQuery, BrowserLoginQuery,
    BrowserLoginStateRow, CallbackQuery, ClientRegistrationRequest, ClientRegistrationResponse,
    NativeAuthorizationResultRow, NativeAuthorizationStartResponse, NativeCallbackQuery,
    NativePollQuery, NativePollResponse, RegisteredClient,
};
use crate::util::{expires_at, fingerprint, now_unix, random_token};

const AUTH_REQUEST_TTL_SECS: i64 = 300;
const NATIVE_START_MEDIA_TYPE: &str = labby_oauth_wire::NATIVE_AUTHORIZATION_START_MEDIA_TYPE;
const NATIVE_SUCCESS_PAGE: &str = r#"<!doctype html><html><body style="font-family:sans-serif;background:#07131c;color:#e6f4fb;text-align:center;padding-top:4rem"><h2>Signed in to Labby</h2><p>You can close this tab and return to the app.</p></body></html>"#;
const NATIVE_CALLBACK_EXPIRED_PAGE: &str = r#"<!doctype html><html><body style="font-family:sans-serif;background:#07131c;color:#e6f4fb;text-align:center;padding-top:4rem"><h2>Sign-in link expired</h2><p>Return to the app and start sign-in again.</p></body></html>"#;

/// Extract the `IpAddr` from a `SocketAddr`, normalizing IPv4-mapped IPv6
/// addresses (`::ffff:a.b.c.d`) back to plain IPv4 so per-IP rate-limiting
/// keys are consistent regardless of listener address family (lab-77y5.10).
fn remote_ip(addr: SocketAddr) -> IpAddr {
    match addr.ip() {
        IpAddr::V6(v6) => v6
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

/// Enforces the configured email allowlist.
///
/// Access is granted by either an exact address match against `allowed_emails`
/// or, for Google Workspace accounts, a match of the ID token's `hd` (hosted
/// domain) claim against `allowed_domains`.
///
/// `email_verified` is enforced before the email comparison: without this guard,
/// an attacker who creates a Google account with someone else's address (without
/// verifying it) could bypass the allowlist.
///
/// Domain access deliberately keys on `hd` rather than the address suffix.
/// Google asserts `hd` only for accounts genuinely hosted in that Workspace
/// domain, so a consumer account cannot present one; matching on the address
/// would instead accept lookalikes such as `user@evil-example.com`.
fn check_email_allowlist(
    email: Option<&str>,
    email_verified: Option<bool>,
    hosted_domain: Option<&str>,
    allowed_emails: &[String],
    allowed_domains: &[String],
) -> Result<(), AuthError> {
    if allowed_emails.is_empty() && allowed_domains.is_empty() {
        return Ok(());
    }
    if email_verified != Some(true) {
        warn!("oauth callback rejected: google did not return a verified email address");
        return Err(AuthError::AuthFailed(
            "google did not return a verified email address".to_string(),
        ));
    }
    let Some(e) = email else {
        warn!("oauth callback rejected: google did not return an email address");
        return Err(AuthError::AuthFailed(
            "google did not return an email address".to_string(),
        ));
    };
    let trimmed = e.trim();
    if allowed_emails
        .iter()
        .any(|a| a.eq_ignore_ascii_case(trimmed))
    {
        return Ok(());
    }
    if let Some(domain) = hosted_domain.map(str::trim).filter(|d| !d.is_empty())
        && allowed_domains
            .iter()
            .any(|d| d.eq_ignore_ascii_case(domain))
    {
        return Ok(());
    }
    warn!(
        email_id = %fingerprint(trimmed),
        "oauth callback rejected: email not in allowed list"
    );
    Err(AuthError::AuthFailed(
        "google account is not permitted to access this gateway".to_string(),
    ))
}

pub async fn browser_login(
    State(state): State<AuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Query(query): Query<BrowserLoginQuery>,
) -> Result<Response, AuthError> {
    state.check_authorize_rate_limit(remote_ip(addr)).await?;
    state.ensure_pending_oauth_state_capacity().await?;
    let return_to = sanitize_return_to(&state, query.return_to.as_deref());
    let provider_code_verifier = random_token(32)?;
    let provider_code_challenge =
        URL_SAFE_NO_PAD.encode(Sha256::digest(provider_code_verifier.as_bytes()));
    let request_state = random_token(24)?;
    let oauth_state_id = fingerprint(&request_state);

    state
        .store
        .insert_browser_login_state(BrowserLoginStateRow {
            state: request_state.clone(),
            return_to: return_to.clone(),
            provider_code_verifier,
            created_at: now_unix(),
            expires_at: now_unix() + AUTH_REQUEST_TTL_SECS,
        })
        .await?;

    let location = state.google.authorize_url(&AuthorizeUrlRequest {
        state: request_state,
        scope: state.config.default_scope.clone(),
        code_challenge: provider_code_challenge,
        code_challenge_method: "S256".to_string(),
        offline_access: false,
        force_consent: false,
    })?;
    info!(
        oauth_state_id = %oauth_state_id,
        return_to = %return_to,
        "browser login redirected to upstream provider"
    );

    Ok((
        StatusCode::FOUND,
        [(header::LOCATION, location.to_string())],
    )
        .into_response())
}

pub async fn register_client(
    State(state): State<AuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(request): Json<ClientRegistrationRequest>,
) -> Result<Json<ClientRegistrationResponse>, AuthError> {
    state.check_register_rate_limit(remote_ip(addr)).await?;
    if request.redirect_uris.is_empty() {
        warn!("oauth register rejected: no redirect URIs provided");
        return Err(AuthError::Validation(
            "at least one redirect URI is required".to_string(),
        ));
    }
    let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
    for redirect_uri in &request.redirect_uris {
        if redirect_uri != &native_callback_endpoint
            && !is_allowed_redirect_uri(redirect_uri, &state.config.allowed_client_redirect_uris)
        {
            warn!(
                redirect_uri = %redirect_uri,
                native_callback_endpoint = %native_callback_endpoint,
                allowed_patterns = ?state.config.allowed_client_redirect_uris,
                "oauth register rejected: redirect URI is not in the allowlist, native callback, or loopback set"
            );
            return Err(AuthError::Validation(format!(
                "redirect URI `{redirect_uri}` must target a loopback host, match the native callback endpoint, or match an allowed redirect pattern"
            )));
        }
    }

    let client = RegisteredClient {
        client_id: random_token(18)?,
        redirect_uris: request.redirect_uris,
        created_at: now_unix(),
        token_endpoint_auth_method: "none".to_string(),
        token_endpoint_auth_methods: Vec::new(),
        jwks: None,
        jwks_uri: None,
    };
    state.store.register_client(client.clone()).await?;
    info!(
        client_id = %client.client_id,
        redirect_uri_count = client.redirect_uris.len(),
        redirect_uris = ?client.redirect_uris,
        "oauth client registration accepted"
    );
    Ok(Json(ClientRegistrationResponse {
        client_id: client.client_id,
        redirect_uris: client.redirect_uris,
        token_endpoint_auth_method: "none".to_string(),
    }))
}

pub async fn authorize(
    State(state): State<AuthState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<AuthorizeQuery>,
) -> Result<Response, AuthError> {
    state.check_authorize_rate_limit(remote_ip(addr)).await?;
    state.ensure_pending_oauth_state_capacity().await?;
    let client_state_id = fingerprint(&query.state);
    let client = crate::cimd::resolve_client(&state, &query.client_id)
        .await?
        .ok_or_else(|| {
            warn!(
                client_id = %query.client_id,
                client_state_id = %client_state_id,
                "oauth authorize rejected: unknown client_id"
            );
            AuthError::InvalidGrant("unknown client_id".to_string())
        })?;
    if !client
        .redirect_uris
        .iter()
        .any(|uri| uri == &query.redirect_uri)
    {
        warn!(
            client_id = %query.client_id,
            redirect_uri = %query.redirect_uri,
            client_state_id = %client_state_id,
            "oauth authorize rejected: redirect URI does not match registered client"
        );
        return Err(AuthError::Validation(
            "redirect_uri does not match the registered client".to_string(),
        ));
    }
    // A CIMD URL is the client ID, not an RFC 7591 registration.  Keep a
    // validated local reference nonetheless: refresh_tokens has a foreign key
    // to registered_clients, and the authorization-code grant must be able to
    // issue a durable refresh token for a CIMD client.  `resolve_client`
    // continues to fetch URL-based clients from their metadata document, so
    // this reference cannot downgrade private_key_jwt authentication.
    if crate::cimd::is_metadata_document_client_id(&query.client_id) {
        state.store.register_client(client.clone()).await?;
    }
    if let Err(error) = validate_response_type(&query.response_type) {
        return authorization_error_redirect(&state, &query, "unsupported_response_type", error);
    }
    let resource = match validate_resource(&state, query.resource.as_deref()) {
        Ok(resource) => resource,
        Err(error) => {
            return authorization_error_redirect(&state, &query, "invalid_target", error);
        }
    };
    let scope = match validate_scope(&state, &resource, &query.scope) {
        Ok(scope) => scope,
        Err(error) => {
            return authorization_error_redirect(&state, &query, "invalid_scope", error);
        }
    };
    info!(
        client_id = %query.client_id,
        redirect_uri = %query.redirect_uri,
        client_state_id = %client_state_id,
        resource = %resource,
        requested_scope = %query.scope,
        normalized_scope = %scope,
        "oauth authorize request received"
    );
    if query.code_challenge_method != "S256" {
        warn!(
            client_id = %query.client_id,
            client_state_id = %client_state_id,
            code_challenge_method = %query.code_challenge_method,
            "oauth authorize rejected: unsupported PKCE method"
        );
        return authorization_error_redirect(
            &state,
            &query,
            "invalid_request",
            AuthError::Validation("code_challenge_method must be S256".to_string()),
        );
    }

    let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
    let is_native = query.redirect_uri == native_callback_endpoint;
    let accepts_native_start = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(',').any(|part| {
                part.split(';').next().is_some_and(|essence| {
                    essence.trim().eq_ignore_ascii_case(NATIVE_START_MEDIA_TYPE)
                })
            })
        });
    if is_native && !accepts_native_start {
        return Err(AuthError::Validation(format!(
            "native OAuth clients must request `{NATIVE_START_MEDIA_TYPE}` and use the returned poll_token"
        )));
    }
    let native_poll_token = is_native
        .then(|| random_token(32))
        .transpose()?
        .map(|token| {
            let hash = URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()));
            (token, hash)
        });

    let provider_code_verifier = random_token(32)?;
    let provider_code_challenge =
        URL_SAFE_NO_PAD.encode(Sha256::digest(provider_code_verifier.as_bytes()));
    let request_state = random_token(24)?;
    let oauth_state_id = fingerprint(&request_state);

    state
        .store
        .insert_authorization_request(AuthorizationRequestRow {
            state: request_state.clone(),
            client_id: query.client_id.clone(),
            redirect_uri: query.redirect_uri.clone(),
            client_state: query.state.clone(),
            native_poll_token_hash: native_poll_token.as_ref().map(|(_, hash)| hash.clone()),
            resource: resource.clone(),
            scope: scope.clone(),
            provider_code_verifier,
            code_challenge: query.code_challenge.clone(),
            code_challenge_method: query.code_challenge_method.clone(),
            created_at: now_unix(),
            expires_at: now_unix() + AUTH_REQUEST_TTL_SECS,
        })
        .await?;

    // Google's refresh credential belongs to the Google account and Labby's
    // Google OAuth client, not to the downstream DCR/CIMD client. On a
    // single-account gateway, one verified email-scoped provider credential
    // can therefore serve every local OAuth client without minting hundreds
    // of duplicate Google refresh tokens. With multiple allowed accounts we
    // still force consent because the selected subject is unknown until the
    // callback returns.
    let allowed_emails = state.resolve_allowed_emails().await?;
    let sole_account_has_credential = match allowed_emails.as_slice() {
        [email] => state
            .store
            .find_google_provider_credential_by_email(email)
            .await?
            .is_some_and(|credential| credential.client_id == state.google.client_id),
        _ => false,
    };
    let force_consent = allowed_emails.len() != 1 || !sole_account_has_credential;
    let location = state.google.authorize_url(&AuthorizeUrlRequest {
        state: request_state,
        scope: scope.clone(),
        code_challenge: provider_code_challenge,
        code_challenge_method: "S256".to_string(),
        offline_access: true,
        force_consent,
    })?;
    info!(
        client_id = %query.client_id,
        redirect_uri = %query.redirect_uri,
        client_state_id = %client_state_id,
        oauth_state_id = %oauth_state_id,
        resource = %resource,
        scope = %scope,
        provider = "google",
        allowed_email_count = allowed_emails.len(),
        provider_credential_present = sole_account_has_credential,
        force_consent,
        "oauth authorize request redirected to upstream provider"
    );
    debug!(
        client_id = %query.client_id,
        oauth_state_id = %oauth_state_id,
        provider_authorization_endpoint = %sanitized_authorization_endpoint(&location),
        "oauth authorize redirect URL generated"
    );

    if let Some((poll_token, _)) = native_poll_token {
        let mut response = Json(NativeAuthorizationStartResponse {
            authorization_url: location.to_string(),
            poll_token,
        })
        .into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        Ok(response)
    } else {
        Ok((
            StatusCode::FOUND,
            [(header::LOCATION, location.to_string())],
        )
            .into_response())
    }
}

fn sanitized_authorization_endpoint(location: &url::Url) -> String {
    let mut endpoint = location.clone();
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    let _ = endpoint.set_username("");
    let _ = endpoint.set_password(None);
    endpoint.to_string()
}

fn authorization_error_redirect(
    state: &AuthState,
    query: &AuthorizeQuery,
    error_code: &str,
    error: AuthError,
) -> Result<Response, AuthError> {
    let mut redirect = url::Url::parse(&query.redirect_uri).map_err(|parse_error| {
        AuthError::Config(format!(
            "validated redirect_uri could not be parsed: {parse_error}"
        ))
    })?;
    redirect
        .query_pairs_mut()
        .append_pair("error", error_code)
        .append_pair("error_description", &error.to_string())
        .append_pair("state", &query.state);
    append_authorization_response_issuer(state, &mut redirect);
    Ok(Redirect::to(redirect.as_str()).into_response())
}

pub async fn callback(
    State(state): State<AuthState>,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AuthError> {
    let oauth_state_id = fingerprint(&query.state);
    info!(
        oauth_state_id = %oauth_state_id,
        provider = "google",
        "oauth callback received"
    );
    if let Some(login) = state.store.take_browser_login_state(&query.state).await? {
        let google = state
            .google
            .exchange_code(&query.code, &login.provider_code_verifier)
            .await?;
        let allowed = state.resolve_allowed_emails().await?;
        check_email_allowlist(
            google.email.as_deref(),
            google.email_verified,
            google.hosted_domain.as_deref(),
            &allowed,
            &state.config.allowed_email_domains,
        )?;
        let session = create_browser_session(&state, google.subject, google.email).await?;
        let mut response = Redirect::to(&login.return_to).into_response();
        append_set_cookie(
            &mut response,
            &build_browser_session_cookie(&state, &session.session_id),
        );
        info!(
            oauth_state_id = %oauth_state_id,
            return_to = %login.return_to,
            subject_id = %fingerprint(&session.subject),
            "browser login callback issued session cookie"
        );
        return Ok(response);
    }

    let request = state
        .store
        .take_authorization_request(&query.state)
        .await
        .map_err(|_| {
            warn!(
                oauth_state_id = %oauth_state_id,
                "oauth callback rejected: authorization state is invalid or expired"
            );
            AuthError::InvalidGrant("authorization state is invalid or expired".to_string())
        })?;
    info!(
        client_id = %request.client_id,
        redirect_uri = %request.redirect_uri,
        oauth_state_id = %oauth_state_id,
        client_state_id = %fingerprint(&request.client_state),
        resource = %request.resource,
        scope = %request.scope,
        "oauth callback state redeemed"
    );
    let observed_revocation_epoch = state.store.google_provider_fence_epoch().await?;
    let google = state
        .google
        .exchange_code(&query.code, &request.provider_code_verifier)
        .await?;

    // RFC 6749 §4.1.2.1: errors must redirect to the client's redirect_uri,
    // not surface as a JSON HTTP error. The denial reason is sourced from the
    // AuthError so we only log once (inside check_email_allowlist).
    let allowed = state.resolve_allowed_emails().await?;
    if let Err(denial) = check_email_allowlist(
        google.email.as_deref(),
        google.email_verified,
        google.hosted_domain.as_deref(),
        &allowed,
        &state.config.allowed_email_domains,
    ) {
        let mut redirect_target = url::Url::parse(&request.redirect_uri).map_err(|error| {
            // Unreachable in practice: redirect_uri was validated against the
            // client's registered URIs before being stored.
            AuthError::Config(format!("failed to parse registered redirect_uri: {error}"))
        })?;
        redirect_target
            .query_pairs_mut()
            .append_pair("error", "access_denied")
            .append_pair("error_description", &denial.to_string())
            .append_pair("state", &request.client_state);
        append_authorization_response_issuer(&state, &mut redirect_target);
        return Ok(Redirect::to(redirect_target.as_str()).into_response());
    }

    let subject_id = fingerprint(&google.subject);
    let verified_email = google
        .email
        .as_deref()
        .map(str::trim)
        .filter(|email| !email.is_empty())
        .ok_or_else(|| {
            AuthError::AuthFailed(
                "google did not return a verified email address after allowlist validation"
                    .to_string(),
            )
        })?;
    // Serialize callback installation with refresh/invalidation for this Google
    // account. SQLite generation CAS below also protects deployments with more
    // than one Labby process sharing the auth database.
    #[cfg(test)]
    CALLBACK_PROVIDER_LOCK_REACHED.notify_waiters();
    let _provider_guard = crate::google_refresh::lock(&google.subject)
        .lock_owned()
        .await;
    let received_provider_refresh_token = google.refresh_token.is_some();
    let existing_credential = state
        .store
        .find_google_provider_credential(&google.subject)
        .await?;
    #[cfg(test)]
    if request.client_state == "generation-loss-client-state"
        && CALLBACK_CAS_PAUSE_ENABLED.load(std::sync::atomic::Ordering::Acquire)
    {
        CALLBACK_CAS_OBSERVED.add_permits(1);
        CALLBACK_CAS_RESUME
            .acquire()
            .await
            .expect("test semaphore open")
            .forget();
    }
    let granted_scopes = merge_google_scopes(
        existing_credential
            .as_ref()
            .map(|credential| credential.granted_scopes.as_slice())
            .unwrap_or_default(),
        &google.granted_scopes,
    );
    let scope_upgraded = existing_credential.as_ref().is_none_or(|existing| {
        granted_scopes
            .iter()
            .any(|scope| !existing.granted_scopes.contains(scope))
    });
    let (provider_refresh_token, reused_provider_refresh_token) = if let Some(refresh_token) =
        google.refresh_token.clone()
    {
        (refresh_token, false)
    } else if let Some(existing) = existing_credential
        .as_ref()
        .filter(|credential| credential.client_id == state.google.client_id)
    {
        (existing.refresh_token.clone(), true)
    } else {
        warn!(
            client_id = %request.client_id,
            oauth_state_id = %oauth_state_id,
            subject_id = %subject_id,
            kind = "oauth_needs_reauth",
            "oauth callback rejected: google did not provide a reusable refresh credential"
        );
        let mut redirect_target = url::Url::parse(&request.redirect_uri).map_err(|error| {
            AuthError::Config(format!("failed to parse registered redirect_uri: {error}"))
        })?;
        redirect_target
                .query_pairs_mut()
                .append_pair("error", "server_error")
                .append_pair(
                    "error_description",
                    "Google did not issue a reusable offline credential; reconnect and grant access again",
                )
                .append_pair("state", &request.client_state);
        append_authorization_response_issuer(&state, &mut redirect_target);
        return Ok(Redirect::to(redirect_target.as_str()).into_response());
    };
    let provider_token_received_at = now_unix();
    let provider_update = crate::types::GoogleProviderCredentialUpdate {
        subject: google.subject.clone(),
        email: Some(verified_email.to_string()),
        client_id: state.google.client_id.clone(),
        granted_scopes: granted_scopes.clone(),
        access_token: google.access_token.clone(),
        refresh_token: provider_refresh_token,
        token_received_at: provider_token_received_at,
        access_token_expires_at: provider_token_received_at
            .saturating_add(i64::try_from(google.expires_in.unwrap_or(3600)).unwrap_or(i64::MAX)),
        issuer: Some("https://accounts.google.com".to_string()),
        refreshed: false,
        scope_upgraded,
    };
    let provider_update_persisted = if let Some(existing) = existing_credential.as_ref() {
        state
            .store
            .replace_google_provider_token_bundle_if_generation(
                provider_update,
                existing.generation,
            )
            .await?
    } else {
        state
            .store
            .insert_google_provider_token_bundle_if_absent(
                provider_update,
                observed_revocation_epoch,
            )
            .await?
    };
    if !provider_update_persisted {
        let replacement_present = state
            .store
            .has_google_provider_credential_for_subject(&google.subject)
            .await?;
        warn!(
            client_id = %request.client_id,
            oauth_state_id = %oauth_state_id,
            subject_id = %subject_id,
            observed_provider_generation = ?existing_credential.as_ref().map(|row| row.generation),
            replacement_provider_credential_present = replacement_present,
            kind = "oauth_needs_reauth",
            "oauth callback discarded stale provider exchange after generation changed"
        );
        return Err(AuthError::OauthNeedsReauth(
            "google provider credential changed during authorization; retry authorization"
                .to_string(),
        ));
    }
    info!(
        client_id = %request.client_id,
        oauth_state_id = %oauth_state_id,
        subject_id = %subject_id,
        provider_credential_present = true,
        received_provider_refresh_token,
        reused_provider_refresh_token,
        "oauth callback exchanged upstream code successfully"
    );
    let auth_code = random_token(24)?;
    let auth_code_id = fingerprint(&auth_code);
    // The user just passed `check_email_allowlist`, which IS the admin gate:
    // operators are added to the allowlist explicitly to grant access. Elevate
    // their scope to include `<default_scope>:admin` so MCP clients (which
    // typically don't know to request elevated scopes) can call destructive
    // gateway/setup actions without a separate flow. If they explicitly
    // requested only the base scope, this is a no-op deny — they get admin.
    let elevated_scope =
        elevate_scope_for_allowed_user(&request.scope, &state.config.default_scope);
    let request_client_id = request.client_id.clone();
    let request_resource = request.resource.clone();
    let request_scope = elevated_scope.clone();
    state
        .store
        .insert_auth_code(AuthorizationCodeRow {
            code: auth_code.clone(),
            client_id: request.client_id,
            subject: google.subject,
            redirect_uri: request.redirect_uri.clone(),
            resource: request.resource,
            scope: elevated_scope,
            code_challenge: request.code_challenge,
            code_challenge_method: request.code_challenge_method,
            provider_refresh_token: None,
            created_at: now_unix(),
            expires_at: expires_at(
                now_unix(),
                state.config.auth_code_ttl,
                &format!("{}_AUTH_CODE_TTL_SECS", state.config.env_prefix),
            )?,
        })
        .await?;
    info!(
        auth_code_id = %auth_code_id,
        oauth_state_id = %oauth_state_id,
        client_id = %request_client_id,
        resource = %request_resource,
        scope = %request_scope,
        redirect_uri = %request.redirect_uri,
        "oauth callback issued local authorization code"
    );

    // Native-flow clients (desktop/mobile apps with no loopback listener or
    // custom URI scheme) register `redirect_uri = native_callback_endpoint` —
    // our own HTTPS route — instead of a client-hosted URL. In that case there
    // is no redirect target to send the browser back to: stash the code keyed
    // by the independently generated polling-token hash for the client to
    // retrieve via `POST /native/poll`, and show a
    // plain "signed in" page directly.
    let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
    if request.redirect_uri == native_callback_endpoint {
        let now = now_unix();
        state
            .store
            .insert_native_authorization_result(NativeAuthorizationResultRow {
                poll_token_hash: request.native_poll_token_hash.ok_or_else(|| {
                    AuthError::Storage(
                        "native authorization request is missing its polling credential"
                            .to_string(),
                    )
                })?,
                code: auth_code,
                created_at: now,
                expires_at: expires_at(
                    now,
                    state.config.auth_code_ttl,
                    &format!("{}_AUTH_CODE_TTL_SECS", state.config.env_prefix),
                )?,
            })
            .await?;
        let mut response = axum::response::Html(NATIVE_SUCCESS_PAGE).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        debug!(
            auth_code_id = %auth_code_id,
            native_callback_endpoint = %native_callback_endpoint,
            "oauth callback stored native authorization code for polling"
        );
        return Ok(response);
    }

    let redirect_uri = reqwest::Url::parse(&request.redirect_uri).map_err(|error| {
        AuthError::Storage(format!(
            "registered redirect_uri is not a valid URL: {error}"
        ))
    })?;
    let mut redirect_uri = redirect_uri;
    redirect_uri
        .query_pairs_mut()
        .append_pair("code", &auth_code)
        .append_pair("state", &request.client_state);
    append_authorization_response_issuer(&state, &mut redirect_uri);
    let (has_code, has_state, has_issuer) = authorization_response_query_presence(&redirect_uri);
    info!(
        auth_code_id = %auth_code_id,
        oauth_state_id = %oauth_state_id,
        client_id = %request_client_id,
        authorization_response_has_code = has_code,
        authorization_response_has_state = has_state,
        authorization_response_has_issuer = has_issuer,
        redirect_scheme = redirect_uri.scheme(),
        redirect_host = redirect_uri.host_str(),
        redirect_path = redirect_uri.path(),
        "oauth callback authorization response prepared"
    );

    Ok(Redirect::to(redirect_uri.as_str()).into_response())
}

fn append_authorization_response_issuer(state: &AuthState, redirect: &mut url::Url) {
    if !state.config.codex_issuer_compatibility {
        redirect
            .query_pairs_mut()
            .append_pair("iss", &crate::metadata::public_base_url(state));
    }
}

fn authorization_response_query_presence(redirect: &url::Url) -> (bool, bool, bool) {
    let mut has_code = false;
    let mut has_state = false;
    let mut has_issuer = false;
    for (name, _) in redirect.query_pairs() {
        match name.as_ref() {
            "code" => has_code = true,
            "state" => has_state = true,
            "iss" => has_issuer = true,
            _ => {}
        }
    }
    (has_code, has_state, has_issuer)
}

/// Direct-hit fallback for the registered native `redirect_uri`. In the real
/// flow this path is never dereferenced by an actual browser redirect —
/// Google's redirect target is always `/auth/google/callback`, which detects
/// a native-flow authorization request and short-circuits into stashing the
/// code for `/native/poll` instead of redirecting here. This handler only
/// answers a stray direct visit (e.g. a stale bookmark or a misconfigured
/// client), so `state` is validated for URL-shape consistency but
/// deliberately not looked up — there's nothing to correlate it against.
pub async fn native_callback(
    Query(query): Query<NativeCallbackQuery>,
) -> Result<Response, AuthError> {
    let state_param = query.state.trim();
    if state_param.is_empty() {
        return Err(AuthError::Validation(
            "missing `state` parameter".to_string(),
        ));
    }
    let mut response = (
        StatusCode::GONE,
        axum::response::Html(NATIVE_CALLBACK_EXPIRED_PAGE),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

pub async fn native_poll(
    State(state): State<AuthState>,
    Json(query): Json<NativePollQuery>,
) -> Result<Response, AuthError> {
    let poll_token = query.poll_token.trim();
    if poll_token.is_empty() {
        return Err(AuthError::Validation(
            "missing `poll_token` parameter".to_string(),
        ));
    }
    let poll_token_hash = URL_SAFE_NO_PAD.encode(Sha256::digest(poll_token.as_bytes()));
    let mut response = if let Some(row) = state
        .store
        .take_native_authorization_result(&poll_token_hash)
        .await?
    {
        Json(NativePollResponse {
            code: Some(row.code),
        })
        .into_response()
    } else {
        (
            StatusCode::ACCEPTED,
            Json(NativePollResponse { code: None }),
        )
            .into_response()
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

fn sanitize_return_to(state: &AuthState, requested: Option<&str>) -> String {
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return "/".to_string();
    };
    if requested.starts_with('/') && !requested.starts_with("//") {
        return requested.to_string();
    }
    let Some(public_url) = state.config.public_url.as_ref() else {
        return "/".to_string();
    };
    let Ok(url) = reqwest::Url::parse(requested) else {
        return "/".to_string();
    };
    if url.scheme() != public_url.scheme()
        || url.host_str() != public_url.host_str()
        || url.port_or_known_default() != public_url.port_or_known_default()
    {
        return "/".to_string();
    }
    let mut normalized = url.path().to_string();
    if let Some(query) = url.query() {
        normalized.push('?');
        normalized.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        normalized.push('#');
        normalized.push_str(fragment);
    }
    normalized
}

fn validate_response_type(response_type: &str) -> Result<(), AuthError> {
    if response_type == "code" {
        Ok(())
    } else {
        warn!(
            response_type = %response_type,
            "oauth authorize rejected: unsupported response_type"
        );
        Err(AuthError::Validation(
            "response_type must be `code`".to_string(),
        ))
    }
}

/// Add `<base>:admin` to `scope` if not already present, where `base` is the
/// resource prefix of `default_scope` (everything before the first `:`).
///
/// For example, `default_scope = "syslog:read"` produces the admin scope
/// `"syslog:admin"`, not `"syslog:read:admin"`.
///
/// Called after `check_email_allowlist` succeeds. Being on the allowlist IS
/// the admin gate (operators add users explicitly), so the issued token
/// carries the elevated scope regardless of what the OAuth client originally
/// requested — most MCP clients use the default scope and have no way to
/// negotiate `:admin` themselves.
pub(crate) fn elevate_scope_for_allowed_user(scope: &str, default_scope: &str) -> String {
    let base = default_scope.split(':').next().unwrap_or(default_scope);
    let admin_scope = format!("{base}:admin");
    let mut scopes: Vec<&str> = scope.split_whitespace().filter(|s| !s.is_empty()).collect();
    // Always inject the default-brand admin scope (e.g. "lab:admin") for
    // allowlisted users, even when the token is for a cross-brand protected
    // route (e.g. "mcp:read mcp:write" for a cortex endpoint).  The JWT
    // audience is still bound to the specific resource URL, so a cortex token
    // carrying "lab:admin" cannot be presented to lab endpoints.  This lets
    // authenticate_protected_route_request recognise the admin unconditionally
    // without re-reading the allowlist at request time.
    if !scopes.iter().any(|s| *s == admin_scope.as_str()) {
        scopes.push(admin_scope.as_str());
    }
    scopes.join(" ")
}

pub(crate) fn validate_scope(
    state: &AuthState,
    resource: &str,
    scope: &str,
) -> Result<String, AuthError> {
    let canonical = crate::metadata::canonical_resource_url(state);
    let supported = if resource.trim_end_matches('/') == canonical {
        state.config.scopes_supported.clone()
    } else {
        state
            .allowed_resource_scopes(resource)
            .filter(|scopes| !scopes.is_empty())
            .ok_or_else(|| {
                AuthError::Validation(format!(
                    "resource must be `{canonical}` or a configured protected MCP route"
                ))
            })?
    };
    let normalized = scope.trim();
    if normalized.is_empty() {
        if resource.trim_end_matches('/') == canonical {
            let scope = state.config.default_scope.clone();
            debug!(
                resource = %resource,
                scope = %scope,
                "oauth authorize defaulted scope"
            );
            return Ok(scope);
        }
        let scope = supported.join(" ");
        debug!(
            resource = %resource,
            scope = %scope,
            "oauth authorize defaulted protected resource scope"
        );
        return Ok(scope);
    }
    let requested = normalized.split_whitespace().collect::<Vec<_>>();
    if requested
        .iter()
        .all(|scope| supported.iter().any(|allowed| allowed == scope))
    {
        let scope = requested.join(" ");
        debug!(
            resource = %resource,
            requested_scope = %normalized,
            normalized_scope = %scope,
            "oauth authorize scope accepted"
        );
        return Ok(scope);
    }
    warn!(
        scope = %normalized,
        resource = %resource,
        supported_scopes = ?supported,
        "oauth authorize rejected: unsupported scope"
    );
    Err(AuthError::Validation(format!(
        "scope must be one of: {}",
        supported.join(", ")
    )))
}

pub(crate) fn validate_resource(
    state: &AuthState,
    requested: Option<&str>,
) -> Result<String, AuthError> {
    let canonical = crate::metadata::canonical_resource_url(state);
    let Some(requested) = requested.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(canonical);
    };
    let requested = requested.trim_end_matches('/');
    if requested == canonical || state.is_allowed_resource_url(requested) {
        debug!(
            requested_resource = %requested,
            canonical_resource = %canonical,
            protected_resource = requested != canonical,
            "oauth resource accepted"
        );
        return Ok(requested.to_string());
    }

    warn!(
        requested_resource = %requested,
        expected_resource = %canonical,
        "oauth request rejected: resource does not match an allowed MCP endpoint"
    );
    Err(AuthError::Validation(format!(
        "resource must be `{canonical}` or a configured protected MCP route"
    )))
}

fn is_loopback_redirect(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    if url.scheme() != "http" {
        return false;
    }
    matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
}

/// Native-app private-use URI scheme redirects (RFC 8252 §7.1), e.g.
/// `com.raycast:/oauth`. Only an app registered for that scheme with the
/// OS can receive the redirect, so — like loopback — these don't need an
/// explicit allowlist entry per client. Deliberately excludes `http(s)`
/// (network-reachable, needs the allowlist) and script-executing pseudo
/// schemes a browser might act on directly instead of merely redirecting.
fn is_native_app_scheme_redirect(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    !matches!(
        url.scheme(),
        "http" | "https" | "javascript" | "data" | "vbscript" | "file"
    )
}

pub(crate) fn is_allowed_redirect_uri(value: &str, patterns: &[String]) -> bool {
    if is_loopback_redirect(value) || is_native_app_scheme_redirect(value) {
        return true;
    }

    let Ok(candidate) = reqwest::Url::parse(value) else {
        return false;
    };
    patterns
        .iter()
        .any(|pattern| redirect_pattern_matches(pattern, &candidate))
}

fn wildcard_matches(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == value;
    }

    let anchored_start = !pattern.starts_with('*');
    let anchored_end = !pattern.ends_with('*');
    let non_empty_parts: Vec<&str> = parts.into_iter().filter(|part| !part.is_empty()).collect();
    if non_empty_parts.is_empty() {
        return true;
    }

    let mut cursor = 0usize;
    for (index, part) in non_empty_parts.iter().enumerate() {
        if index == 0 && anchored_start {
            if !value[cursor..].starts_with(part) {
                return false;
            }
            cursor += part.len();
            continue;
        }

        match value[cursor..].find(part) {
            Some(found) => cursor += found + part.len(),
            None => return false,
        }
    }

    if anchored_end && let Some(last) = non_empty_parts.last() {
        return value.ends_with(last);
    }

    true
}

fn redirect_pattern_matches(pattern: &str, candidate: &reqwest::Url) -> bool {
    if pattern == "https://*" {
        return candidate.scheme() == "https" && candidate.host_str().is_some();
    }

    let Ok(pattern_url) = reqwest::Url::parse(pattern) else {
        return false;
    };
    if pattern_url.scheme() != candidate.scheme() {
        return false;
    }

    // Native-app custom URI schemes (e.g. `com.raycast:/oauth`) have no
    // authority component, so `host_str()` is None and can never satisfy the
    // host/port comparison below. Compare the whole URI instead.
    if pattern_url.host_str().is_none() || candidate.host_str().is_none() {
        return wildcard_matches(pattern, candidate.as_str());
    }

    if pattern_url.port_or_known_default() != candidate.port_or_known_default() {
        return false;
    }
    let Some(pattern_host) = pattern_url.host_str() else {
        return false;
    };
    let Some(candidate_host) = candidate.host_str() else {
        return false;
    };
    if !host_pattern_matches(pattern_host, candidate_host) {
        return false;
    }
    if !wildcard_matches(pattern_url.path(), candidate.path()) {
        return false;
    }

    match (pattern_url.query(), candidate.query()) {
        (Some(pattern_query), Some(candidate_query)) => {
            wildcard_matches(pattern_query, candidate_query)
        }
        (None, None) => true,
        _ => false,
    }
}

fn host_pattern_matches(pattern_host: &str, candidate_host: &str) -> bool {
    let pattern_labels = pattern_host.split('.').collect::<Vec<_>>();
    let candidate_labels = candidate_host.split('.').collect::<Vec<_>>();
    if pattern_labels.len() != candidate_labels.len() {
        return false;
    }

    pattern_labels
        .iter()
        .zip(candidate_labels.iter())
        .all(|(pattern, candidate)| {
            *pattern == "*" || (!pattern.contains('*') && pattern.eq_ignore_ascii_case(candidate))
        })
}

#[cfg(test)]
pub mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use base64::Engine;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::{EncodePrivateKey, LineEnding};
    use rsa::traits::PublicKeyParts;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tower::util::ServiceExt;
    use url::Url;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::{host_pattern_matches, is_allowed_redirect_uri, wildcard_matches};
    use crate::config::{AuthConfig, AuthMode, GoogleConfig};
    use crate::google::GoogleProvider;
    use crate::state::AuthState;
    use crate::types::{
        AuthorizationRequestRow, GoogleProviderCredentialUpdate, NativeAuthorizationResultRow,
        RegisteredClient,
    };

    use crate::util::now_unix;

    use axum::Router;
    use axum::extract::connect_info::MockConnectInfo;
    use std::net::SocketAddr;

    fn native_poll_token_hash_for(token: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
    }

    fn native_poll_request(poll_token: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/native/poll")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "poll_token": poll_token }).to_string()))
            .unwrap()
    }

    // `oneshot` bypasses the live `into_make_service_with_connect_info` layer,
    // so the rate-limit handlers' `ConnectInfo<SocketAddr>` extractor would be
    // missing and every request would 500. Wrap the real router with a mock
    // peer address; handlers that don't extract `ConnectInfo` ignore it.
    fn router(state: AuthState) -> Router {
        crate::routes::router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 9001))))
    }

    async fn seed_provider_credential(state: &AuthState, client_id: &str, refresh_token: &str) {
        let now = now_unix();
        state
            .store
            .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
                subject: "google-user".to_string(),
                email: Some("user@example.com".to_string()),
                client_id: client_id.to_string(),
                granted_scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
                access_token: "provider-access".to_string(),
                refresh_token: refresh_token.to_string(),
                token_received_at: now,
                access_token_expires_at: now + 3600,
                issuer: Some("https://accounts.google.com".to_string()),
                refreshed: false,
                scope_upgraded: true,
            })
            .await
            .unwrap();
    }

    fn assert_authorization_error(response: &axum::response::Response, expected_error: &str) {
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Url::parse(value).ok())
            .expect("authorization error redirect location");
        let query = location
            .query_pairs()
            .into_owned()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(query.get("error").map(String::as_str), Some(expected_error));
        assert_eq!(query.get("state").map(String::as_str), Some("abc"));
        assert_eq!(
            query.get("iss").map(String::as_str),
            Some("https://lab.example.com")
        );
        assert!(
            query
                .get("error_description")
                .is_some_and(|description| !description.is_empty())
        );
    }

    #[tokio::test]
    async fn register_accepts_public_dcr_and_enforces_loopback_redirects() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        let app = router(test_auth_state_with_config(config).await);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": ["http://127.0.0.1:7777/callback"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let rejected = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn register_accepts_native_callback_endpoint_without_redirect_allowlist() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        let state = test_auth_state_with_config(config).await;
        let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({ "redirect_uris": [native_callback_endpoint] }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn register_rejects_native_callback_endpoint_smuggled_with_an_unsafe_redirect_uri() {
        // The native-endpoint bypass in `register_client` is per-redirect_uri —
        // confirm a registration that mixes the native endpoint with an
        // otherwise-disallowed redirect_uri in the same request still fails
        // validation for the whole request, rather than the native match
        // short-circuiting the loop and letting the unsafe URI through.
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        let state = test_auth_state_with_config(config).await;
        let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": [
                                native_callback_endpoint,
                                "https://evil.example/callback",
                            ]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn native_poll_returns_202_with_no_code_for_an_unknown_poll_token() {
        let app = router(test_auth_state().await);
        let response = app
            .oneshot(native_poll_request("never-issued"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.get("code").is_none());
    }

    #[tokio::test]
    async fn native_authorize_returns_server_generated_polling_credential() {
        let state = test_auth_state().await;
        let native_callback = crate::metadata::native_callback_endpoint(&state);
        state
            .store
            .register_client(RegisteredClient {
                client_id: "native-start-client".to_string(),
                redirect_uris: vec![native_callback.clone()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        let mut uri = Url::parse("https://lab.example.com/authorize").unwrap();
        uri.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", "native-start-client")
            .append_pair("redirect_uri", &native_callback)
            .append_pair("state", "attacker-known-state")
            .append_pair("scope", "lab")
            .append_pair("code_challenge", "pkce")
            .append_pair("code_challenge_method", "S256");
        let uri = format!("{}?{}", uri.path(), uri.query().unwrap());
        let response = router(state)
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(
                        header::ACCEPT,
                        "text/html, Application/Vnd.Labby.Native-Oauth-Start+Json; charset=utf-8; q=1",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let poll_token = json["poll_token"].as_str().unwrap();
        assert_ne!(poll_token, "attacker-known-state");
        assert!(poll_token.len() >= 32);
        assert!(json["authorization_url"].as_str().is_some());
    }

    #[tokio::test]
    async fn native_poll_rejects_missing_poll_token() {
        let app = router(test_auth_state().await);
        let response = app.oneshot(native_poll_request("")).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn native_poll_never_accepts_a_polling_secret_in_the_request_uri() {
        let response = router(test_auth_state().await)
            .oneshot(
                Request::builder()
                    .uri("/native/poll?poll_token=must-not-enter-access-logs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn native_poll_is_one_shot_and_returns_the_code_exactly_once() {
        let state = test_auth_state().await;
        state
            .store
            .insert_native_authorization_result(NativeAuthorizationResultRow {
                poll_token_hash: native_poll_token_hash_for("poll-me"),
                code: "the-code".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let app = router(state);

        let first = app
            .clone()
            .oneshot(native_poll_request("poll-me"))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let body = axum::body::to_bytes(first.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "the-code");

        // Second poll for the same token must not still return the code —
        // `take_native_authorization_result` is a one-shot read-and-delete.
        let second = app.oneshot(native_poll_request("poll-me")).await.unwrap();
        assert_eq!(second.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn native_callback_direct_hit_shows_expired_page_and_never_stores_a_code() {
        let app = router(test_auth_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/native/callback?state=whatever&code=attacker-supplied")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::GONE);
    }

    #[tokio::test]
    async fn insert_native_authorization_result_overwrites_on_token_hash_collision() {
        // The effectively impossible hash-collision case remains deterministic:
        // last-write-wins, not `DO NOTHING`.
        let state = test_auth_state().await;
        state
            .store
            .insert_native_authorization_result(NativeAuthorizationResultRow {
                poll_token_hash: native_poll_token_hash_for("collide"),
                code: "first-code".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        state
            .store
            .insert_native_authorization_result(NativeAuthorizationResultRow {
                poll_token_hash: native_poll_token_hash_for("collide"),
                code: "second-code".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let fetched = state
            .store
            .take_native_authorization_result(&native_poll_token_hash_for("collide"))
            .await
            .unwrap()
            .expect("row should still be present");
        assert_eq!(fetched.code, "second-code");
    }

    #[tokio::test]
    async fn callback_stores_native_flow_code_for_polling_instead_of_redirecting() {
        let native_state = test_auth_state_with_mock_google_native().await;
        let native_callback = crate::metadata::native_callback_endpoint(&native_state);
        let app = router(native_state);
        let mut authorize_uri = Url::parse("https://lab.example.com/authorize").unwrap();
        authorize_uri
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", "native-client")
            .append_pair("redirect_uri", &native_callback)
            .append_pair("state", "native-client-state")
            .append_pair("scope", "lab")
            .append_pair("code_challenge", "challenge")
            .append_pair("code_challenge_method", "S256");
        let authorize_uri = format!(
            "{}?{}",
            authorize_uri.path(),
            authorize_uri.query().unwrap()
        );
        let start = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(authorize_uri)
                    .header(
                        header::ACCEPT,
                        "application/vnd.labby.native-oauth-start+json",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(start.status(), StatusCode::OK);
        let start_body = axum::body::to_bytes(start.into_body(), usize::MAX)
            .await
            .unwrap();
        let start_json: serde_json::Value = serde_json::from_slice(&start_body).unwrap();
        let poll_token = start_json["poll_token"].as_str().unwrap().to_string();
        let provider_url = Url::parse(start_json["authorization_url"].as_str().unwrap()).unwrap();
        let provider_state = provider_url
            .query_pairs()
            .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
            .unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/google/callback?state={provider_state}&code=upstream-code"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // The native branch never redirects the browser — it shows a static
        // "signed in" page directly.
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains("Signed in"));

        let attacker_poll = app
            .clone()
            .oneshot(native_poll_request("native-client-state"))
            .await
            .unwrap();
        assert_eq!(attacker_poll.status(), StatusCode::ACCEPTED);

        let poll = app.oneshot(native_poll_request(&poll_token)).await.unwrap();
        assert_eq!(poll.status(), StatusCode::OK);
        let poll_body = axum::body::to_bytes(poll.into_body(), usize::MAX)
            .await
            .unwrap();
        let poll_json: serde_json::Value = serde_json::from_slice(&poll_body).unwrap();
        assert!(poll_json["code"].as_str().is_some());
    }

    #[tokio::test]
    async fn native_poll_rejects_an_attacker_who_only_knows_the_client_state() {
        let native_state = test_auth_state_with_mock_google_native().await;
        let app = router(native_state);
        let callback = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=native-good-state&code=upstream-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback.status(), StatusCode::OK);

        let attacker_poll = app
            .oneshot(native_poll_request("native-client-state"))
            .await
            .unwrap();
        assert_eq!(attacker_poll.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn register_accepts_allowed_non_loopback_redirect_patterns() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        config.allowed_client_redirect_uris =
            vec!["https://callback.example.com/callback/*".to_string()];
        let app = router(test_auth_state_with_config(config).await);
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": ["https://callback.example.com/callback/node-a"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn register_is_rate_limited_after_configured_burst() {
        let mut config = test_auth_config();
        config.enable_dynamic_registration = true;
        config.register_requests_per_minute = 1;
        let app = router(test_auth_state_with_config(config).await);

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": ["http://127.0.0.1:7777/callback"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let second = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/register")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "redirect_uris": ["http://127.0.0.1:8888/callback"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn wildcard_redirect_patterns_support_leading_and_infix_matches() {
        assert!(wildcard_matches(
            "https://callback.example.com/callback/*",
            "https://callback.example.com/callback/node-a"
        ));
        assert!(wildcard_matches(
            "https://callback.*.com/callback/*",
            "https://callback.example.com/callback/node-a"
        ));
        assert!(!wildcard_matches("/callback", "/callback/extra"));
    }

    #[test]
    fn host_patterns_support_full_label_wildcards_only() {
        assert!(host_pattern_matches(
            "callback.*.com",
            "callback.example.com"
        ));
        assert!(host_pattern_matches(
            "*.example.com",
            "callback.example.com"
        ));
        assert!(!host_pattern_matches(
            "callback.example.com*",
            "callback.example.com"
        ));
        assert!(!host_pattern_matches(
            "*.example.com",
            "callback.nested.example.com"
        ));
    }

    #[test]
    fn wildcard_redirect_patterns_do_not_overmatch_similar_hosts() {
        assert!(!is_allowed_redirect_uri(
            "https://callback.example.com.evil.example/callback/node-a",
            &[String::from("https://callback.example.com/callback/*")]
        ));
        assert!(!is_allowed_redirect_uri(
            "https://callback.example.com.evil.example/callback",
            &[String::from("https://callback.example.com*")]
        ));
    }

    #[test]
    fn native_app_scheme_redirect_uris_are_always_allowed() {
        // Native-app redirects (RFC 8252 §7.1) like `com.raycast:/oauth` or
        // `warp://mcp/oauth2callback` are scoped to whatever app the OS has
        // registered for that private-use scheme, so — like loopback — they
        // don't need a per-client allowlist entry.
        assert!(is_allowed_redirect_uri("com.raycast:/oauth", &[]));
        assert!(is_allowed_redirect_uri("warp://mcp/oauth2callback", &[]));
        assert!(is_allowed_redirect_uri(
            "com.raycast:/oauth",
            &[String::from("https://callback.tootie.tv/callback/*")]
        ));
    }

    #[test]
    fn script_executing_pseudo_schemes_are_never_auto_allowed() {
        assert!(!is_allowed_redirect_uri("javascript:alert(1)", &[]));
        assert!(!is_allowed_redirect_uri("data:text/html,evil", &[]));
        assert!(!is_allowed_redirect_uri("file:///etc/passwd", &[]));
    }

    #[test]
    fn https_redirects_still_require_the_allowlist() {
        assert!(!is_allowed_redirect_uri(
            "https://evil.example/callback",
            &[String::from("https://callback.tootie.tv/callback/*")]
        ));
        assert!(is_allowed_redirect_uri(
            "https://callback.tootie.tv/callback/node-a",
            &[String::from("https://callback.tootie.tv/callback/*")]
        ));
        assert!(is_allowed_redirect_uri(
            "https://chatgpt.com/connector/oauth/test-callback-id",
            &[String::from("https://chatgpt.com/connector/oauth/*")]
        ));
    }

    #[test]
    fn all_https_redirect_pattern_allows_any_https_callback_only() {
        assert!(is_allowed_redirect_uri(
            "https://gemini.google.com/mcp/oauth/callback",
            &[String::from("https://*")]
        ));
        assert!(is_allowed_redirect_uri(
            "https://example.deeply.nested.client.invalid/path/callback?state=ok",
            &[String::from("https://*")]
        ));
        assert!(!is_allowed_redirect_uri(
            "http://example.deeply.nested.client.invalid/path/callback",
            &[String::from("https://*")]
        ));
    }

    #[tokio::test]
    async fn authorize_persists_full_state_and_redirects_to_google() {
        let app = router(test_auth_state_with_registered_client().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("accounts.google.com"));
        assert!(location.contains("prompt=consent"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn authorize_logs_only_sanitized_provider_endpoint_and_state_fingerprint() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
        let buf = crate::test_support::global_tracing_buffer();
        let app = router(test_auth_state_with_registered_client().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=raw-client-state&scope=lab&code_challenge=raw-client-verifier&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);

        let location = Url::parse(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let query: std::collections::HashMap<_, _> = location.query_pairs().into_owned().collect();
        let provider_state = query.get("state").expect("provider state");
        let provider_code_challenge = query
            .get("code_challenge")
            .expect("provider PKCE challenge");
        let logs = crate::test_support::captured_logs(buf);

        for secret in [
            "raw-client-state",
            "raw-client-verifier",
            provider_state,
            provider_code_challenge,
        ] {
            assert!(
                !logs.contains(secret),
                "OAuth authorization secret leaked into logs: {secret}\n{logs}"
            );
            let encoded: String = url::form_urlencoded::byte_serialize(secret.as_bytes()).collect();
            assert!(
                !logs.contains(&encoded),
                "encoded OAuth authorization secret leaked into logs: {encoded}\n{logs}"
            );
        }
        assert!(
            logs.contains(
                "\"provider_authorization_endpoint\":\"https://accounts.google.com/o/oauth2/v2/auth\""
            ),
            "{logs}"
        );
        assert!(logs.contains("\"oauth_state_id\":"), "{logs}");
        assert!(!logs.contains("\"location\":"), "{logs}");
    }

    #[tokio::test]
    async fn authorize_persists_a_cimd_client_reference_for_token_issuance() {
        let mut config = test_auth_config();
        config.allowed_client_redirect_uris =
            vec!["https://chatgpt.com/connector/oauth/*".to_string()];
        let state = test_auth_state_with_config(config).await;
        let client_id = "https://chatgpt.com/oauth/test-client/client.json";
        state.cimd_cache.insert(
            client_id.to_string(),
            (
                RegisteredClient {
                    client_id: client_id.to_string(),
                    redirect_uris: vec![
                        "https://chatgpt.com/connector/oauth/test-client".to_string(),
                    ],
                    created_at: now_unix(),
                    token_endpoint_auth_method: "none".to_string(),
                    token_endpoint_auth_methods: Vec::new(),
                    jwks: None,
                    jwks_uri: None,
                },
                now_unix() + 60,
            ),
        );

        let app = router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=https%3A%2F%2Fchatgpt.com%2Foauth%2Ftest-client%2Fclient.json&redirect_uri=https%3A%2F%2Fchatgpt.com%2Fconnector%2Foauth%2Ftest-client&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FOUND);
        assert!(state.store.find_client(client_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn authorize_omits_forced_consent_once_the_allowed_account_has_a_provider_credential() {
        let state = test_auth_state_with_registered_client().await;
        seed_provider_credential(&state, "client-id", "provider-refresh").await;
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("accounts.google.com"));
        assert!(!location.contains("prompt="));
    }

    #[tokio::test]
    async fn authorize_forces_consent_when_provider_credential_belongs_to_another_google_client() {
        let state = test_auth_state_with_registered_client().await;
        seed_provider_credential(&state, "old-google-client", "provider-refresh").await;
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("prompt=consent"));
    }

    #[tokio::test]
    async fn authorize_reuses_the_allowed_account_credential_for_a_new_downstream_client() {
        let state = test_auth_state_with_registered_client().await;
        state
            .store
            .register_client(RegisteredClient {
                client_id: "other-client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:8888/callback".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        seed_provider_credential(&state, "client-id", "provider-refresh").await;
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=other-client&redirect_uri=http://127.0.0.1:8888/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("accounts.google.com"));
        assert!(
            !location.contains("prompt="),
            "new downstream clients must reuse the sole allowed account's provider credential \
             instead of minting another Google refresh token"
        );
    }

    #[tokio::test]
    async fn authorize_forces_consent_when_multiple_accounts_are_allowed_even_with_a_provider_credential()
     {
        let state = test_auth_state_with_registered_client().await;
        // A second allowed Google account, on top of the default admin_email —
        // resolve_allowed_emails() now returns 2 entries.
        state
            .store
            .add_allowed_user("second-admin@example.com", "admin", now_unix())
            .await
            .unwrap();
        // One allowed account already has a provider credential, but the
        // selected Google subject is unknown until the callback returns.
        seed_provider_credential(&state, "client-id", "provider-refresh").await;
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("accounts.google.com"));
        assert!(
            location.contains("prompt=consent"),
            "with more than one allowed Google account, a provider credential must not \
             suppress consent because it may belong to a different selected account"
        );
    }

    #[tokio::test]
    async fn authorize_accepts_configured_protected_resource_scopes() {
        let state = test_auth_state_with_registered_client().await;
        state.set_allowed_resource_scopes([(
            "https://mcp.example.com/syslog".to_string(),
            vec!["mcp:read".to_string(), "mcp:write".to_string()],
        )]);
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&resource=https%3A%2F%2Fmcp.example.com%2Fsyslog&scope=mcp%3Aread%20mcp%3Awrite&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FOUND);
    }

    #[tokio::test]
    async fn authorize_is_rate_limited_after_configured_burst() {
        let mut config = test_auth_config();
        config.authorize_requests_per_minute = 1;
        let state = test_auth_state_with_config(config).await;
        state
            .store
            .register_client(RegisteredClient {
                client_id: "client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        let app = router(state);

        let first = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::FOUND);

        let second = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=def&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn browser_login_starts_upstream_flow_and_persists_return_to_state() {
        let state = test_auth_state().await;
        let app = router(state.clone());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/auth/login?return_to=%2Fgateways%2F%3Ftab%3Dlab")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let location = Url::parse(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        assert!(
            !location.query_pairs().any(|(key, _)| key == "access_type"),
            "browser login must not request an offline refresh credential"
        );
        assert!(!location.query_pairs().any(|(key, _)| key == "prompt"));
        let upstream_state = location
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .unwrap();
        let stored = state
            .store
            .take_browser_login_state(&upstream_state)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.return_to, "/gateways/?tab=lab");
    }

    #[tokio::test]
    async fn browser_login_rejects_when_pending_oauth_state_cap_is_reached() {
        let mut config = test_auth_config();
        config.max_pending_oauth_states = 1;
        let state = test_auth_state_with_config(config).await;
        state
            .store
            .insert_browser_login_state(crate::types::BrowserLoginStateRow {
                state: "existing-login".to_string(),
                return_to: "/".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();

        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/auth/login?return_to=%2Fgateways%2F")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn callback_rejects_expired_or_mismatched_state() {
        let app = router(test_auth_state_with_mock_google().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=bad-state&code=upstream-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn oauth_callback_generation_loss_cannot_issue_code_over_fresh_credential() {
        let base_state = test_auth_state_with_registered_client().await;
        let now = now_unix();
        base_state
            .store
            .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
                subject: "google-subject-123".to_string(),
                email: Some("admin@example.com".to_string()),
                client_id: "client-id".to_string(),
                granted_scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
                access_token: "existing-provider-access".to_string(),
                refresh_token: "existing-provider-refresh".to_string(),
                token_received_at: now,
                access_token_expires_at: now + 3600,
                issuer: Some("https://accounts.google.com".to_string()),
                refreshed: false,
                scope_upgraded: true,
            })
            .await
            .unwrap();
        base_state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: "generation-loss-state".to_string(),
                client_id: "client".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                client_state: "generation-loss-client-state".to_string(),
                native_poll_token_hash: None,
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                code_challenge: "challenge".to_string(),
                code_challenge_method: "S256".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();

        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "late-provider-refresh",
                "expires_in": 3600,
                "id_token": signed_test_id_token(),
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(server)
            .await;
        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
        let state = AuthState::for_tests(
            (*base_state.config).clone(),
            base_state.store.clone(),
            (*base_state.signing_keys).clone(),
            google,
        );

        super::CALLBACK_CAS_PAUSE_ENABLED.store(true, std::sync::atomic::Ordering::Release);
        let request_state = state.clone();
        let response_task = tokio::spawn(async move {
            router(request_state)
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=generation-loss-state&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            super::CALLBACK_CAS_OBSERVED.acquire(),
        )
        .await
        .expect("callback reached generation CAS")
        .unwrap()
        .forget();
        let generation = state
            .store
            .find_google_provider_credential("google-subject-123")
            .await
            .unwrap()
            .unwrap()
            .generation;
        let peer_store = crate::sqlite::SqliteStore::open_with_key(
            state.config.sqlite_path.clone(),
            state.config.token_encryption_key.clone(),
        )
        .await
        .unwrap();
        let now = now_unix();
        assert!(
            peer_store
                .replace_google_provider_token_bundle_if_generation(
                    GoogleProviderCredentialUpdate {
                        subject: "google-subject-123".to_string(),
                        email: Some("user@example.com".to_string()),
                        client_id: "client-id".to_string(),
                        granted_scopes: vec!["openid".to_string(), "email".to_string()],
                        access_token: "fresh-provider-access".to_string(),
                        refresh_token: "fresh-provider-refresh".to_string(),
                        token_received_at: now,
                        access_token_expires_at: now + 3600,
                        issuer: Some("https://accounts.google.com".to_string()),
                        refreshed: false,
                        scope_upgraded: true,
                    },
                    generation
                )
                .await
                .unwrap()
        );
        super::CALLBACK_CAS_PAUSE_ENABLED.store(false, std::sync::atomic::Ordering::Release);
        super::CALLBACK_CAS_RESUME.add_permits(1);
        let response = response_task.await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        if response.status() == StatusCode::SEE_OTHER {
            let location = response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap();
            assert!(
                !Url::parse(location)
                    .unwrap()
                    .query_pairs()
                    .any(|(key, _)| key == "code"),
                "a pre-revoke callback must not issue a Labby authorization code"
            );
        }
        let credential = state
            .store
            .find_google_provider_credential("google-subject-123")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(credential.refresh_token, "fresh-provider-refresh");
    }

    #[tokio::test]
    async fn browser_login_callback_sets_session_cookie_and_redirects_home() {
        let state = test_auth_state_with_mock_google().await;
        let app = router(state.clone());
        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/login?return_to=%2Fgateways%2F")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let location = Url::parse(
            login
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let upstream_state = location
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .unwrap();

        let callback = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/google/callback?state={upstream_state}&code=upstream-code"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            callback.headers().get(header::LOCATION).unwrap(),
            "/gateways/"
        );
        let cookie = callback
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .find_map(|value| value.to_str().ok())
            .unwrap();
        assert!(cookie.contains("lab_session="));
    }

    #[tokio::test]
    async fn oauth_client_callback_redirects_with_access_denied_when_email_not_in_allowlist() {
        let mut config = test_auth_config();
        config.admin_email = "allowed@example.com".to_string();
        let base_state = test_auth_state_with_config(config).await;
        base_state
            .store
            .register_client(RegisteredClient {
                client_id: "client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        // Pre-insert an authorization request (OAuth-client flow, not browser-login).
        base_state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: "good-state".to_string(),
                client_id: "client".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                client_state: "client-abc".to_string(),
                native_poll_token_hash: None,
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                code_challenge: "challenge".to_string(),
                code_challenge_method: "S256".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();

        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(), // email=user@example.com, not in allowlist
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(server)
            .await;

        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());

        let state = AuthState::for_tests(
            (*base_state.config).clone(),
            base_state.store.clone(),
            (*base_state.signing_keys).clone(),
            google,
        );
        let app = router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=good-state&code=upstream-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Must redirect (not 401) with error=access_denied and the original client state.
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        let redirect = Url::parse(location).unwrap();
        let params: std::collections::HashMap<_, _> = redirect.query_pairs().collect();
        assert_eq!(
            params.get("error").map(|v| v.as_ref()),
            Some("access_denied")
        );
        assert_eq!(params.get("state").map(|v| v.as_ref()), Some("client-abc"));
        assert_eq!(
            params.get("iss").map(|v| v.as_ref()),
            Some("https://lab.example.com")
        );
    }

    #[tokio::test]
    async fn browser_login_callback_rejects_email_not_in_allowlist() {
        let mut config = test_auth_config();
        // "allowed@example.com" is permitted; the mock id_token returns
        // "user@example.com" → callback must be denied with 401.
        config.admin_email = "allowed@example.com".to_string();
        let base_state = test_auth_state_with_config(config).await;
        base_state
            .store
            .register_client(RegisteredClient {
                client_id: "client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();

        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(),
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(server)
            .await;

        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());

        let state = AuthState::for_tests(
            (*base_state.config).clone(),
            base_state.store.clone(),
            (*base_state.signing_keys).clone(),
            google,
        );
        let app = router(state.clone());

        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/auth/login?return_to=%2F")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let location = Url::parse(
            login
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        let upstream_state = location
            .query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
            .unwrap();

        let callback = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/google/callback?state={upstream_state}&code=upstream-code"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(callback.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn authorize_rejects_missing_or_invalid_response_type() {
        let app = router(test_auth_state_with_registered_client().await);
        for uri in [
            "/authorize?client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256",
            "/authorize?response_type=token&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab&code_challenge=pkce&code_challenge_method=S256",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_authorization_error(&response, "unsupported_response_type");
        }
    }

    #[tokio::test]
    async fn validate_scope_accepts_supported_scopes_and_rejects_others() {
        let state = test_auth_state().await;
        let canonical = crate::metadata::canonical_resource_url(&state);
        // Empty scope falls back to configured default ("lab").
        assert_eq!(
            super::validate_scope(&state, &canonical, "").unwrap(),
            "lab"
        );
        // Base scope passes.
        assert_eq!(
            super::validate_scope(&state, &canonical, "lab").unwrap(),
            "lab"
        );
        // `:admin` is in `scopes_supported` by default — MCP clients can request
        // it explicitly. (Allowed-emails users also receive it implicitly via
        // elevate_scope_for_allowed_user at callback time.)
        assert_eq!(
            super::validate_scope(&state, &canonical, "lab:admin").unwrap(),
            "lab:admin"
        );
        // Anything not in scopes_supported is rejected.
        let err = super::validate_scope(&state, &canonical, "lab:write").unwrap_err();
        assert!(err.to_string().contains("lab"), "got: {err}");
    }

    #[test]
    fn elevate_scope_adds_admin_when_missing() {
        assert_eq!(
            super::elevate_scope_for_allowed_user("lab", "lab"),
            "lab lab:admin"
        );
        // Already has admin → no duplication.
        assert_eq!(
            super::elevate_scope_for_allowed_user("lab lab:admin", "lab"),
            "lab lab:admin"
        );
        // Empty scope → just admin (rare; OAuth default normally fills `lab`).
        assert_eq!(
            super::elevate_scope_for_allowed_user("", "lab"),
            "lab:admin"
        );
        // Different brand prefix (syslog, axon, etc.) uses its own default.
        assert_eq!(
            super::elevate_scope_for_allowed_user("syslog", "syslog"),
            "syslog syslog:admin"
        );
        // default_scope with verb suffix (e.g. syslog:read) → admin uses base prefix only,
        // not syslog:read:admin.
        assert_eq!(
            super::elevate_scope_for_allowed_user("syslog:read", "syslog:read"),
            "syslog:read syslog:admin"
        );
        // Already has correct admin even when default_scope carries a suffix.
        assert_eq!(
            super::elevate_scope_for_allowed_user("syslog:read syslog:admin", "syslog:read"),
            "syslog:read syslog:admin"
        );
        // Cross-brand: protected route token (mcp:read mcp:write) for a lab
        // default_scope gets lab:admin injected so authenticate_protected_route_request
        // can recognise the admin without re-reading the allowlist.
        assert_eq!(
            super::elevate_scope_for_allowed_user("mcp:read mcp:write", "lab"),
            "mcp:read mcp:write lab:admin"
        );
        // Cross-brand already has admin → no duplication.
        assert_eq!(
            super::elevate_scope_for_allowed_user("mcp:read mcp:write lab:admin", "lab"),
            "mcp:read mcp:write lab:admin"
        );
    }

    #[tokio::test]
    async fn authorize_rejects_invalid_scope() {
        let app = router(test_auth_state_with_registered_client().await);
        // `lab:write` is NOT in default scopes_supported; should be rejected.
        // (`lab:admin` IS in scopes_supported as of 2026-05; use a different
        // unsupported scope here.)
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&scope=lab:write&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_authorization_error(&response, "invalid_scope");
    }

    #[tokio::test]
    async fn authorize_rejects_mismatched_resource_parameter() {
        let app = router(test_auth_state_with_registered_client().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/authorize?response_type=code&client_id=client&redirect_uri=http://127.0.0.1:7777/callback&state=abc&resource=https://other.example.com/mcp&scope=lab&code_challenge=pkce&code_challenge_method=S256")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_authorization_error(&response, "invalid_target");
    }

    #[tokio::test]
    async fn callback_rejects_expired_state() {
        let state = test_auth_state_with_registered_client().await;
        state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: "expired-state".to_string(),
                client_id: "client".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                client_state: "client-state".to_string(),
                native_poll_token_hash: None,
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                code_challenge: "challenge".to_string(),
                code_challenge_method: "S256".to_string(),
                created_at: now_unix() - 300,
                expires_at: now_unix() - 1,
            })
            .await
            .unwrap();
        let app = router(state);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/auth/google/callback?state=expired-state&code=upstream-code")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    pub async fn test_auth_state() -> AuthState {
        test_auth_state_with_config(test_auth_config()).await
    }

    pub async fn test_auth_state_with_config(config: AuthConfig) -> AuthState {
        AuthState::new(config).await.unwrap()
    }

    pub(crate) fn test_auth_config() -> AuthConfig {
        let dir = Box::leak(Box::new(tempfile::tempdir().unwrap()));
        AuthConfig {
            mode: AuthMode::OAuth,
            public_url: Some(Url::parse("https://lab.example.com").unwrap()),
            sqlite_path: dir.path().join("auth.db"),
            key_path: dir.path().join("auth-jwt.pem"),
            bootstrap_secret: Some("bootstrap-secret".to_string()),
            enable_dynamic_registration: true,
            allowed_client_redirect_uris: Vec::new(),
            // Matches the mock id_token email returned by signed_test_id_token,
            // so happy-path callback tests pass the allowlist check.
            admin_email: "user@example.com".to_string(),
            google: GoogleConfig {
                client_id: "client-id".to_string(),
                client_secret: "client-secret".to_string(),
                callback_url: None,
                callback_path: "/auth/google/callback".to_string(),
                scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
            },
            token_encryption_key: Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                "test-google-provider-encryption-key",
            )),
            ..AuthConfig::default()
        }
    }

    pub async fn test_auth_state_with_registered_client() -> AuthState {
        let state = test_auth_state().await;
        state
            .store
            .register_client(RegisteredClient {
                client_id: "client".to_string(),
                redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        state
    }

    pub(crate) async fn test_auth_state_with_mock_google() -> AuthState {
        let state = test_auth_state_with_registered_client().await;
        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(),
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(server)
            .await;
        state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: "good-state".to_string(),
                client_id: "client".to_string(),
                redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                client_state: "client-state".to_string(),
                native_poll_token_hash: None,
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                code_challenge: "challenge".to_string(),
                code_challenge_method: "S256".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
        AuthState::for_tests(
            (*state.config).clone(),
            state.store.clone(),
            (*state.signing_keys).clone(),
            google,
        )
    }

    /// Same mocked-Google harness as [`test_auth_state_with_mock_google`], but
    /// the pending authorization request's `redirect_uri` is the server's own
    /// `native_callback_endpoint` — exercising the native-flow branch of
    /// `callback()` instead of the normal client-redirect branch.
    async fn test_auth_state_with_mock_google_native() -> AuthState {
        let state = test_auth_state().await;
        let native_callback_endpoint = crate::metadata::native_callback_endpoint(&state);
        state
            .store
            .register_client(RegisteredClient {
                client_id: "native-client".to_string(),
                redirect_uris: vec![native_callback_endpoint.clone()],
                created_at: now_unix(),
                token_endpoint_auth_method: "none".to_string(),
                token_endpoint_auth_methods: Vec::new(),
                jwks: None,
                jwks_uri: None,
            })
            .await
            .unwrap();
        let server = Box::leak(Box::new(MockServer::start().await));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "google-access-token",
                "refresh_token": "refresh-token",
                "expires_in": 3600,
                "id_token": signed_test_id_token(),
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/certs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
            .mount(server)
            .await;
        state
            .store
            .insert_authorization_request(AuthorizationRequestRow {
                state: "native-good-state".to_string(),
                client_id: "native-client".to_string(),
                redirect_uri: native_callback_endpoint,
                client_state: "native-client-state".to_string(),
                native_poll_token_hash: Some(native_poll_token_hash_for("legitimate-poll-token")),
                resource: "https://lab.example.com/mcp".to_string(),
                scope: "lab".to_string(),
                provider_code_verifier: "provider-verifier".to_string(),
                code_challenge: "challenge".to_string(),
                code_challenge_method: "S256".to_string(),
                created_at: now_unix(),
                expires_at: now_unix() + 300,
            })
            .await
            .unwrap();
        let google = GoogleProvider::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
        )
        .unwrap()
        .with_endpoints(
            server.uri().parse::<Url>().unwrap(),
            server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
        )
        .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
        AuthState::for_tests(
            (*state.config).clone(),
            state.store.clone(),
            (*state.signing_keys).clone(),
            google,
        )
    }

    pub(crate) fn signed_test_id_token() -> String {
        let claims = json!({
            "iss": "https://accounts.google.com",
            "aud": "client-id",
            "sub": "google-subject-123",
            "email": "user@example.com",
            "email_verified": true,
            "iat": now_unix() as usize,
            "exp": (now_unix() + 3600) as usize,
        });
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-kid".to_string());
        encode(&header, &claims, &test_encoding_key()).unwrap()
    }

    pub(crate) fn test_jwks() -> serde_json::Value {
        let key = test_rsa_key();
        let public_key = key.to_public_key();
        json!({
            "keys": [{
                "kid": "test-kid",
                "alg": "RS256",
                "kty": "RSA",
                "use": "sig",
                "n": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.n_bytes()),
                "e": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(public_key.e_bytes()),
            }]
        })
    }

    fn test_rsa_key() -> RsaPrivateKey {
        use std::sync::OnceLock;

        static KEY: OnceLock<RsaPrivateKey> = OnceLock::new();
        KEY.get_or_init(|| {
            let mut rng = rand::rng();
            RsaPrivateKey::new(&mut rng, 2048).expect("generate Google RS256 fixture key")
        })
        .clone()
    }

    fn test_encoding_key() -> EncodingKey {
        let pem = test_rsa_key().to_pkcs8_pem(LineEnding::LF).unwrap();
        EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap()
    }

    /// Tests that exercise the merged allowlist path through real callback handlers.
    /// These verify that `resolve_allowed_emails` is correctly wired at both call
    /// sites (browser-login branch and oauth-client branch).
    mod merged_allowlist_callback_tests {
        use axum::body::Body;
        use axum::http::{Request, StatusCode, header};
        use serde_json::json;
        use tower::util::ServiceExt;
        use url::Url;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        use super::{
            signed_test_id_token, test_auth_config, test_auth_state_with_config,
            test_auth_state_with_mock_google, test_jwks,
        };
        use crate::google::GoogleProvider;
        use crate::routes::router;
        use crate::state::AuthState;
        use crate::types::{AuthorizationRequestRow, BrowserLoginStateRow, RegisteredClient};
        use crate::util::now_unix;

        /// Helper that mounts Google mock endpoints on a fresh server and builds
        /// an `AuthState` with that mock, reusing an existing base state's store
        /// and signing keys (so DB writes made to `base_state.store` are visible).
        async fn state_with_mock_google_from(base_state: &AuthState) -> AuthState {
            let server = Box::leak(Box::new(MockServer::start().await));
            Mock::given(method("POST"))
                .and(path("/token"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "access_token": "google-access-token",
                    "refresh_token": "refresh-token",
                    "expires_in": 3600,
                    "id_token": signed_test_id_token(),
                })))
                .mount(server)
                .await;
            Mock::given(method("GET"))
                .and(path("/certs"))
                .respond_with(ResponseTemplate::new(200).set_body_json(test_jwks()))
                .mount(server)
                .await;
            let google = GoogleProvider::new(
                "client-id".to_string(),
                "client-secret".to_string(),
                Url::parse("https://lab.example.com/auth/google/callback").unwrap(),
            )
            .unwrap()
            .with_endpoints(
                server.uri().parse::<Url>().unwrap(),
                server.uri().parse::<Url>().unwrap().join("/token").unwrap(),
            )
            .with_jwks_endpoint(server.uri().parse::<Url>().unwrap().join("/certs").unwrap());
            AuthState::for_tests(
                (*base_state.config).clone(),
                base_state.store.clone(),
                (*base_state.signing_keys).clone(),
                google,
            )
        }

        /// The mock id_token always returns `user@example.com`. When admin is set
        /// to a *different* email and that address is added to `allowed_users`, the
        /// browser-login callback must succeed (DB row authorises the login).
        #[tokio::test]
        async fn browser_login_succeeds_for_allowlisted_non_admin_email() {
            let mut config = test_auth_config();
            // Set admin to something other than the id_token email.
            config.admin_email = "admin@example.com".to_string();
            let base_state = test_auth_state_with_config(config).await;

            // Insert id_token email into allowed_users.
            base_state
                .store
                .add_allowed_user("user@example.com", "admin", now_unix())
                .await
                .unwrap();

            let state = state_with_mock_google_from(&base_state).await;

            // Seed the browser-login state row so the callback recognises the flow.
            state
                .store
                .insert_browser_login_state(BrowserLoginStateRow {
                    state: "browser-state".to_string(),
                    return_to: "/".to_string(),
                    provider_code_verifier: "provider-verifier".to_string(),
                    created_at: now_unix(),
                    expires_at: now_unix() + 300,
                })
                .await
                .unwrap();

            let app = router(state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=browser-state&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            // Successful browser login → redirect with a Set-Cookie header (session).
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert!(response.headers().contains_key(header::SET_COOKIE));
        }

        /// Admin email is always authorised even when the `allowed_users` table is
        /// empty (browser-login branch).
        #[tokio::test]
        async fn browser_login_succeeds_for_admin_when_allowed_users_is_empty() {
            // Default test config sets admin_email = "user@example.com", which
            // matches the id_token returned by signed_test_id_token.
            let base_state = test_auth_state_with_mock_google().await;

            // Confirm no extra rows exist.
            assert!(
                base_state
                    .store
                    .list_allowed_users()
                    .await
                    .unwrap()
                    .is_empty()
            );

            // Seed browser-login state.
            base_state
                .store
                .insert_browser_login_state(BrowserLoginStateRow {
                    state: "browser-state-2".to_string(),
                    return_to: "/".to_string(),
                    provider_code_verifier: "provider-verifier".to_string(),
                    created_at: now_unix(),
                    expires_at: now_unix() + 300,
                })
                .await
                .unwrap();

            let app = router(base_state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=browser-state-2&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert!(response.headers().contains_key(header::SET_COOKIE));
        }

        async fn oauth_client_callback_location(codex_issuer_compatibility: bool) -> Url {
            let mut config = test_auth_config();
            config.admin_email = "admin@example.com".to_string();
            config.codex_issuer_compatibility = codex_issuer_compatibility;
            let base_state = test_auth_state_with_config(config).await;

            // Register a client.
            base_state
                .store
                .register_client(RegisteredClient {
                    client_id: "client".to_string(),
                    redirect_uris: vec!["http://127.0.0.1:7777/callback".to_string()],
                    created_at: now_unix(),
                    token_endpoint_auth_method: "none".to_string(),
                    token_endpoint_auth_methods: Vec::new(),
                    jwks: None,
                    jwks_uri: None,
                })
                .await
                .unwrap();

            // Add id_token email to allowed_users.
            base_state
                .store
                .add_allowed_user("user@example.com", "admin", now_unix())
                .await
                .unwrap();

            let state = state_with_mock_google_from(&base_state).await;

            // Seed an authorization request row.
            state
                .store
                .insert_authorization_request(AuthorizationRequestRow {
                    state: "oauth-state".to_string(),
                    client_id: "client".to_string(),
                    redirect_uri: "http://127.0.0.1:7777/callback".to_string(),
                    client_state: "client-xyz".to_string(),
                    native_poll_token_hash: None,
                    resource: "https://lab.example.com/mcp".to_string(),
                    scope: "lab".to_string(),
                    provider_code_verifier: "provider-verifier".to_string(),
                    code_challenge: "challenge".to_string(),
                    code_challenge_method: "S256".to_string(),
                    created_at: now_unix(),
                    expires_at: now_unix() + 300,
                })
                .await
                .unwrap();

            let app = router(state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=oauth-state&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let location = response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap();
            Url::parse(location).unwrap()
        }

        /// The oauth-client callback must also succeed for a non-admin email that
        /// exists in `allowed_users`.
        #[tokio::test]
        async fn oauth_client_callback_succeeds_for_allowlisted_non_admin_email() {
            let redirect = oauth_client_callback_location(false).await;
            let params: std::collections::HashMap<_, _> = redirect.query_pairs().collect();
            assert!(
                params.contains_key("code"),
                "expected code in redirect: {redirect}"
            );
            assert_eq!(
                params.get("state").map(|value| value.as_ref()),
                Some("client-xyz")
            );
            assert_eq!(
                params.get("iss").map(|value| value.as_ref()),
                Some("https://lab.example.com")
            );
            assert!(
                !params.contains_key("error"),
                "unexpected error in redirect: {redirect}"
            );
        }

        #[tokio::test]
        async fn oauth_client_callback_omits_issuer_in_explicit_codex_compatibility_mode() {
            let redirect = oauth_client_callback_location(true).await;
            let params: std::collections::HashMap<_, _> = redirect.query_pairs().collect();
            assert!(params.contains_key("code"));
            assert_eq!(
                params.get("state").map(|value| value.as_ref()),
                Some("client-xyz")
            );
            assert!(!params.contains_key("iss"));
        }

        #[tokio::test(flavor = "current_thread")]
        async fn oauth_client_callback_logs_redact_redirect_query_values() {
            let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
            let buf = crate::test_support::global_tracing_buffer();
            let redirect = oauth_client_callback_location(false).await;
            let params: std::collections::HashMap<_, _> =
                redirect.query_pairs().into_owned().collect();
            let authorization_code = params.get("code").unwrap();

            let logs = crate::test_support::captured_logs(&buf);
            for secret in [
                authorization_code.as_str(),
                "client-xyz",
                "iss=https%3A%2F%2Flab.example.com",
                redirect.as_str(),
            ] {
                assert!(
                    !logs.contains(secret),
                    "OAuth redirect secret leaked into debug logs: {secret}\n{logs}"
                );
            }
            assert!(logs.contains("\"redirect_path\":\"/callback\""), "{logs}");
        }

        /// Email not in admin or allowed_users must be rejected in the browser-login
        /// branch (401 Unauthorized).
        #[tokio::test]
        async fn browser_login_rejects_email_absent_from_both_admin_and_db() {
            let mut config = test_auth_config();
            // Neither admin nor allowed_users contains "user@example.com" (the id_token email).
            config.admin_email = "admin@example.com".to_string();
            let base_state = test_auth_state_with_config(config).await;

            let state = state_with_mock_google_from(&base_state).await;

            state
                .store
                .insert_browser_login_state(BrowserLoginStateRow {
                    state: "browser-state-3".to_string(),
                    return_to: "/".to_string(),
                    provider_code_verifier: "provider-verifier".to_string(),
                    created_at: now_unix(),
                    expires_at: now_unix() + 300,
                })
                .await
                .unwrap();

            let app = router(state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=browser-state-3&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        /// Admin also in the DB table must not appear twice (dedup check via
        /// resolve_allowed_emails, verified indirectly: the callback still succeeds
        /// and there is no panic from duplicate iteration).
        #[tokio::test]
        async fn admin_in_db_table_is_deduped_and_still_authorised() {
            // Default config: admin_email = "user@example.com".
            let base_state = test_auth_state_with_mock_google().await;

            // Also add the admin email to allowed_users — this is the duplicate.
            base_state
                .store
                .add_allowed_user("user@example.com", "self", now_unix())
                .await
                .unwrap();

            // Seed browser-login state.
            base_state
                .store
                .insert_browser_login_state(BrowserLoginStateRow {
                    state: "browser-state-4".to_string(),
                    return_to: "/".to_string(),
                    provider_code_verifier: "provider-verifier".to_string(),
                    created_at: now_unix(),
                    expires_at: now_unix() + 300,
                })
                .await
                .unwrap();

            let app = router(base_state);
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/auth/google/callback?state=browser-state-4&code=upstream-code")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            // Must still succeed — dedup should not break the check.
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert!(response.headers().contains_key(header::SET_COOKIE));
        }
    }

    mod allowlist_tests {
        use super::super::check_email_allowlist;

        #[test]
        fn empty_allowlist_permits_any_email() {
            assert!(
                check_email_allowlist(Some("anyone@example.com"), Some(true), None, &[], &[])
                    .is_ok()
            );
        }

        #[test]
        fn empty_allowlist_permits_even_unverified_email() {
            // When no allowlist is configured, email_verified is not enforced.
            assert!(
                check_email_allowlist(Some("anyone@example.com"), Some(false), None, &[], &[])
                    .is_ok()
            );
        }

        #[test]
        fn matching_verified_email_is_permitted() {
            let list = vec!["alice@example.com".to_string()];
            assert!(
                check_email_allowlist(Some("alice@example.com"), Some(true), None, &list, &[])
                    .is_ok()
            );
        }

        #[test]
        fn matching_email_is_case_insensitive() {
            // Allowlist is pre-normalized to lowercase at config load.
            // Incoming email from Google may have any case.
            let list = vec!["alice@example.com".to_string()];
            assert!(
                check_email_allowlist(Some("Alice@Example.com"), Some(true), None, &list, &[])
                    .is_ok()
            );
        }

        #[test]
        fn matching_hosted_domain_is_permitted() {
            // A Workspace account whose `hd` matches an allowed domain gets in
            // without being listed individually.
            assert!(
                check_email_allowlist(
                    Some("newhire@lime-technology.com"),
                    Some(true),
                    Some("lime-technology.com"),
                    &["admin@example.com".to_string()],
                    &["lime-technology.com".to_string()],
                )
                .is_ok()
            );
        }

        #[test]
        fn hosted_domain_match_is_case_insensitive() {
            assert!(
                check_email_allowlist(
                    Some("newhire@lime-technology.com"),
                    Some(true),
                    Some("Lime-Technology.COM"),
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_ok()
            );
        }

        #[test]
        fn hosted_domain_must_be_verified() {
            // An unverified address is rejected even when `hd` matches.
            assert!(
                check_email_allowlist(
                    Some("newhire@lime-technology.com"),
                    Some(false),
                    Some("lime-technology.com"),
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_err()
            );
        }

        #[test]
        fn address_suffix_alone_does_not_grant_domain_access() {
            // The whole point of keying on `hd`: a consumer account cannot claim
            // a Workspace domain, so a lookalike address must not be admitted.
            assert!(
                check_email_allowlist(
                    Some("attacker@lime-technology.com"),
                    Some(true),
                    None,
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_err()
            );
        }

        #[test]
        fn lookalike_hosted_domain_is_rejected() {
            assert!(
                check_email_allowlist(
                    Some("attacker@evil-lime-technology.com"),
                    Some(true),
                    Some("evil-lime-technology.com"),
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_err()
            );
        }

        #[test]
        fn subdomain_of_allowed_domain_is_rejected() {
            assert!(
                check_email_allowlist(
                    Some("attacker@sub.lime-technology.com"),
                    Some(true),
                    Some("sub.lime-technology.com"),
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_err()
            );
        }

        #[test]
        fn domain_allowlist_alone_still_enforces_the_gate() {
            // With only a domain configured, a non-member is still rejected.
            assert!(
                check_email_allowlist(
                    Some("outsider@example.com"),
                    Some(true),
                    None,
                    &[],
                    &["lime-technology.com".to_string()],
                )
                .is_err()
            );
        }

        #[test]
        fn non_matching_email_is_rejected() {
            let list = vec!["alice@example.com".to_string()];
            assert!(
                check_email_allowlist(Some("eve@example.com"), Some(true), None, &list, &[])
                    .is_err()
            );
        }

        #[test]
        fn unverified_email_is_rejected_even_when_in_allowlist() {
            let list = vec!["alice@example.com".to_string()];
            assert!(
                check_email_allowlist(Some("alice@example.com"), Some(false), None, &list, &[])
                    .is_err()
            );
        }

        #[test]
        fn missing_email_verified_claim_is_rejected_when_allowlist_is_set() {
            let list = vec!["alice@example.com".to_string()];
            assert!(
                check_email_allowlist(Some("alice@example.com"), None, None, &list, &[]).is_err()
            );
        }

        #[test]
        fn none_email_is_rejected_when_allowlist_is_set() {
            let list = vec!["alice@example.com".to_string()];
            assert!(check_email_allowlist(None, Some(true), None, &list, &[]).is_err());
        }
    }
}
