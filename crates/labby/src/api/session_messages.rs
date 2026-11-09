use std::net::SocketAddr;

use axum::{
    Json,
    body::to_bytes,
    extract::{ConnectInfo, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{
    api::{
        AppState,
        route_registry::{RouteAuth, RouteDescriptor},
    },
    dispatch::{
        agent_messages::proof::{BRIDGE_AUTH_HEADER, BRIDGE_PROOF_PATH, BridgeProofRequest},
        codemode_notices::NoticeError,
    },
};

pub(super) fn descriptor() -> RouteDescriptor {
    RouteDescriptor::new(
        "POST",
        BRIDGE_PROOF_PATH,
        "message_bridge_proof",
        "notifications",
        RouteAuth::BearerOnly,
    )
    .when("opt-in for authenticated TCP with agent messages enabled")
    .private_no_store()
    .non_enumerating()
}

/// Unwrap only on the private proof route before the normal bearer verifier.
/// A process squatting on a loopback port sees no reusable bearer.
pub(super) async fn authenticate_bridge(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let mut response = match bridge_authorization(&state, &request) {
        Ok(value) => {
            request.headers_mut().remove(BRIDGE_AUTH_HEADER);
            request.headers_mut().insert(header::AUTHORIZATION, value);
            next.run(request).await
        }
        Err(status) => status.into_response(),
    };
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

fn bridge_authorization(state: &AppState, request: &Request) -> Result<HeaderValue, StatusCode> {
    let runtime = state.agent_messages.as_ref().ok_or(StatusCode::NOT_FOUND)?;
    if !request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|ConnectInfo(peer)| peer.ip().is_loopback())
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let headers = request.headers();
    let mut values = headers.get_all(BRIDGE_AUTH_HEADER).iter();
    let sealed = values
        .next()
        .and_then(|value| value.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if values.next().is_some() || headers.contains_key(header::AUTHORIZATION) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let opened = runtime
        .open_bridge_bearer(sealed)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let token = std::str::from_utf8(&opened).map_err(|_| StatusCode::UNAUTHORIZED)?;
    let bearer = zeroize::Zeroizing::new(format!("Bearer {token}"));
    let mut value = HeaderValue::from_str(&bearer).map_err(|_| StatusCode::UNAUTHORIZED)?;
    value.set_sensitive(true);
    Ok(value)
}

pub(super) async fn prove(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Some(runtime) = &state.agent_messages else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(bytes) = to_bytes(body, 1024).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(challenge) = serde_json::from_slice::<BridgeProofRequest>(&bytes) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match runtime
        .prove_bridge(&parts, state.access_credential_adapter.as_ref(), &challenge)
        .await
    {
        Ok(proof) => Json(proof).into_response(),
        Err(NoticeError::Unavailable) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Err(_) => StatusCode::FORBIDDEN.into_response(),
    }
}
