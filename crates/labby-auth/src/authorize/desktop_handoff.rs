use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

use super::{
    AUTH_REQUEST_TTL_SECS, NATIVE_SUCCESS_PAGE, RemoteAddr, check_email_allowlist, remote_ip,
    sanitize_return_to,
};
use crate::{
    error::AuthError,
    google::AuthorizeUrlRequest,
    session::{append_set_cookie, build_browser_session_cookie},
    state::AuthState,
    types::{
        CallbackQuery, DesktopAuthorizeQuery, DesktopLoginStateRow, DesktopPollRequest,
        DesktopPollResponse, DesktopRedeemRequest, DesktopSessionHandoffRow, DesktopStartRequest,
        DesktopStartResponse,
    },
    util::{now_unix, random_token},
};

pub(super) async fn complete_provider_callback(
    state: &AuthState,
    query: &CallbackQuery,
) -> Result<Option<Response>, AuthError> {
    let Some(bound) = state.store.claim_desktop_login_state(&query.state).await? else {
        return Ok(None);
    };
    if query.error.is_some() || query.code.is_none() {
        return Err(AuthError::AuthFailed(
            "upstream authorization was denied".into(),
        ));
    }
    if query
        .iss
        .as_deref()
        .is_some_and(|issuer| issuer != state.inbound_provider.issuer())
    {
        return Err(AuthError::AuthFailed(
            "upstream authorization issuer mismatch".into(),
        ));
    }
    let identity = state
        .inbound_provider
        .exchange_code(
            query.code.as_deref().unwrap_or_default(),
            &bound.value.provider_code_verifier,
            &query.state,
        )
        .await?;
    let allowed = state.resolve_allowed_emails().await?;
    check_email_allowlist(
        identity.email.as_deref(),
        identity.email_verified,
        identity.hosted_domain.as_deref(),
        &allowed,
        &state.config.allowed_email_domains,
    )?;
    let session = crate::session::create_bound_browser_session(
        state,
        identity.subject,
        identity.email,
        bound.binding.clone(),
    )
    .await?;
    state
        .store
        .insert_desktop_session_handoff(
            DesktopSessionHandoffRow {
                poll_token_hash: bound.value.poll_token_hash,
                redeem_code_hash: bound.value.redeem_code_hash,
                launch_code_challenge: bound.value.launch_code_challenge,
                session_id: session.session_id,
                expires_at: bound.value.expires_at,
            },
            bound.binding,
        )
        .await?;
    Ok(Some(no_store(
        axum::response::Html(NATIVE_SUCCESS_PAGE).into_response(),
    )))
}

fn digest(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        "no-store".parse().expect("static header"),
    );
    response
}

fn validate_origin(state: &AuthState, headers: &HeaderMap) -> Result<(), AuthError> {
    let public = state
        .config
        .public_url
        .as_ref()
        .ok_or_else(|| AuthError::AuthFailed("desktop login unavailable".into()))?;
    let expected_origin = public.origin().ascii_serialization();
    let expected_host = public
        .host_str()
        .map(|host| match public.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        })
        .unwrap_or_default();
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if origin != Some(expected_origin.as_str()) || host != Some(expected_host.as_str()) {
        return Err(AuthError::AuthFailed("desktop login request denied".into()));
    }
    Ok(())
}

pub async fn desktop_start(
    State(state): State<AuthState>,
    RemoteAddr(addr): RemoteAddr,
    headers: HeaderMap,
    Json(request): Json<DesktopStartRequest>,
) -> Result<Response, AuthError> {
    state.check_authorize_rate_limit(remote_ip(addr)).await?;
    validate_origin(&state, &headers)?;
    state.ensure_pending_oauth_state_capacity().await?;
    if request.code_challenge.len() != 43
        || URL_SAFE_NO_PAD
            .decode(request.code_challenge.as_bytes())
            .map_or(true, |v| v.len() != 32)
    {
        return Err(AuthError::Validation(
            "invalid desktop launch PKCE challenge".into(),
        ));
    }
    let launch_state = random_token(32)?;
    let poll_token = random_token(32)?;
    let redeem_code = random_token(32)?;
    let now = now_unix();
    let expires_at = now + AUTH_REQUEST_TTL_SECS;
    state
        .store
        .insert_desktop_login_state(
            DesktopLoginStateRow {
                state_hash: digest(&launch_state),
                return_to: sanitize_return_to(&state, request.return_to.as_deref()),
                provider_code_verifier: String::new(),
                poll_token_hash: digest(&poll_token),
                redeem_code_hash: digest(&redeem_code),
                launch_code_challenge: request.code_challenge,
                created_at: now,
                expires_at,
            },
            state.inbound_provider_binding(),
        )
        .await?;
    let mut authorization_url = state
        .config
        .public_url
        .clone()
        .ok_or_else(|| AuthError::AuthFailed("desktop login unavailable".into()))?;
    authorization_url.set_path("/auth/desktop/authorize");
    authorization_url.set_query(Some(&format!("state={launch_state}")));
    Ok(no_store(
        (
            StatusCode::CREATED,
            Json(DesktopStartResponse {
                authorization_url: authorization_url.to_string(),
                poll_token,
                redeem_code,
                expires_at,
            }),
        )
            .into_response(),
    ))
}

pub async fn desktop_authorize(
    State(state): State<AuthState>,
    Query(query): Query<DesktopAuthorizeQuery>,
) -> Result<Response, AuthError> {
    let provider_state = random_token(24)?;
    let verifier = random_token(32)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let Some(_bound) = state
        .store
        .advance_desktop_login_state(&query.state, &provider_state, &verifier)
        .await?
    else {
        return Err(AuthError::InvalidGrant(
            "desktop login state is invalid or expired".into(),
        ));
    };
    let location = state.inbound_provider.authorize_url(&AuthorizeUrlRequest {
        state: provider_state,
        scope: state.config.default_scope.clone(),
        code_challenge: challenge,
        code_challenge_method: "S256".into(),
        offline_access: false,
        force_consent: false,
    })?;
    Ok(no_store(Redirect::to(location.as_str()).into_response()))
}

pub async fn desktop_poll(
    State(state): State<AuthState>,
    RemoteAddr(addr): RemoteAddr,
    Json(request): Json<DesktopPollRequest>,
) -> Result<Response, AuthError> {
    state.check_native_poll_rate_limit(remote_ip(addr)).await?;
    let handoff = state
        .store
        .desktop_session_handoff_status(&request.poll_token)
        .await?;
    let (ready, expires_at) = handoff.unwrap_or((false, 0));
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::ACCEPTED
    };
    Ok(no_store(
        (status, Json(DesktopPollResponse { ready, expires_at })).into_response(),
    ))
}

pub async fn desktop_redeem(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(request): Json<DesktopRedeemRequest>,
) -> Result<Response, AuthError> {
    validate_origin(&state, &headers)?;
    let Some(session_id) = state
        .store
        .take_desktop_session_handoff(&request.redeem_code, &request.code_verifier)
        .await?
    else {
        return Err(AuthError::InvalidGrant(
            "desktop login handoff is invalid or expired".into(),
        ));
    };
    let mut response = StatusCode::NO_CONTENT.into_response();
    append_set_cookie(
        &mut response,
        &build_browser_session_cookie(&state, &session_id),
    );
    Ok(no_store(response))
}
