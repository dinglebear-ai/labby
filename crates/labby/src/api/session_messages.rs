use std::net::SocketAddr;

use axum::{
    Json,
    body::to_bytes,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{
    api::{
        AppState,
        route_registry::{RouteAuth, RouteDescriptor},
    },
    dispatch::{
        agent_messages::proof::{BRIDGE_PROOF_PATH, BridgeProofRequest},
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

pub(super) async fn prove(State(state): State<AppState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let Some(runtime) = &state.agent_messages else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|ConnectInfo(peer)| peer.ip().is_loopback())
    {
        return StatusCode::FORBIDDEN.into_response();
    }
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
        Ok(proof) => ([(header::CACHE_CONTROL, "private, no-store")], Json(proof)).into_response(),
        Err(NoticeError::Unavailable) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        Err(_) => StatusCode::FORBIDDEN.into_response(),
    }
}
