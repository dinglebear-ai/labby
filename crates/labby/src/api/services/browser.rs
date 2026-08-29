//! Thin HTTP adapters for the Rust browser bridge.

use std::net::SocketAddr;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use futures::{SinkExt as _, StreamExt as _};
use labby_browser::{BrowserEnvelope, BrowserMessage};
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::browser::runtime::browser_bridge;
use crate::dispatch::error::ToolError;

pub fn routes(_state: AppState) -> Router<AppState> {
    Router::new().route("/", post(handle_action))
}

pub fn public_routes() -> Router<AppState> {
    Router::new().route("/browser/socket", get(upgrade))
}

async fn handle_action(
    State(_state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(request): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let requires_admin = crate::dispatch::browser::ACTIONS
        .iter()
        .find(|spec| spec.name == request.action)
        .is_some_and(|spec| spec.requires_admin);
    let admin = auth
        .as_ref()
        .is_some_and(|context| context.0.scopes.iter().any(|scope| scope == "lab:admin"));
    if requires_admin && !admin {
        return Err(ApiError::new(ToolError::Forbidden {
            message: format!("action `{}` requires `lab:admin` scope", request.action),
            required_scopes: vec!["lab:admin".to_string()],
        }));
    }
    handle_action_with_meta(
        "browser",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|context| &context.0),
            peer.map(|Extension(ConnectInfo(address))| address),
        ),
        request,
        crate::dispatch::browser::ACTIONS,
        |action, params| async move { crate::dispatch::browser::dispatch(&action, params).await },
    )
    .await
}

async fn upgrade(
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let loopback = peer
        .as_ref()
        .is_some_and(|Extension(ConnectInfo(address))| address.ip().is_loopback());
    let extension_origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| {
            origin.starts_with("chrome-extension://") || origin.starts_with("moz-extension://")
        });
    if !loopback || !extension_origin {
        return Err(ApiError::new(ToolError::Forbidden {
            message: "browser bridge accepts only loopback extension connections".to_string(),
            required_scopes: Vec::new(),
        }));
    }
    browser_bridge()?;
    Ok(upgrade.on_upgrade(handle_socket))
}

async fn handle_socket(socket: WebSocket) {
    if let Err(error) = run_socket(socket).await {
        tracing::warn!(
            surface = "api",
            service = "browser",
            kind = error.kind(),
            "browser extension connection ended"
        );
    }
}

async fn run_socket(socket: WebSocket) -> Result<(), labby_browser::BrowserError> {
    let bridge = browser_bridge()
        .map_err(|error| labby_browser::BrowserError::InvalidRequest(error.to_string()))?;
    let (mut sink, mut source) = socket.split();
    let mut authenticated = None;

    while authenticated.is_none() {
        let Some(message) = source.next().await else {
            return Ok(());
        };
        let Message::Text(text) =
            message.map_err(|_| labby_browser::BrowserError::ConnectionClosed)?
        else {
            continue;
        };
        let envelope: BrowserEnvelope = serde_json::from_str(text.as_str())?;
        envelope.validate_version()?;
        let request_id = envelope.request_id.clone();
        let reply = match envelope.message {
            BrowserMessage::PairingRequest {
                display_name,
                extension_id,
                public_key,
            } => {
                let pairing = bridge.request_pairing(&display_name, &extension_id, &public_key)?;
                BrowserEnvelope::new(
                    request_id,
                    BrowserMessage::PairingPending {
                        pairing_id: pairing.id,
                        expires_at: pairing.expires_at,
                    },
                )
            }
            BrowserMessage::PairingStatus { pairing_id } => {
                let pairing = bridge
                    .store()
                    .pairing(&pairing_id)?
                    .ok_or(labby_browser::BrowserError::NotFound)?;
                match pairing.browser_id {
                    Some(browser_id) => BrowserEnvelope::new(
                        request_id,
                        BrowserMessage::PairingApproved { browser_id },
                    ),
                    None => BrowserEnvelope::new(
                        request_id,
                        BrowserMessage::PairingPending {
                            pairing_id: pairing.id,
                            expires_at: pairing.expires_at,
                        },
                    ),
                }
            }
            BrowserMessage::AuthChallenge { browser_id } => {
                let mut challenge = bridge.issue_challenge(&browser_id)?;
                challenge.request_id = request_id;
                challenge
            }
            BrowserMessage::AuthResponse {
                challenge_id,
                signature,
            } => {
                let connection = bridge.authenticate(&challenge_id, &signature)?;
                let browser_id = connection.browser_id.clone();
                authenticated = Some(connection);
                BrowserEnvelope::new(request_id, BrowserMessage::Authenticated { browser_id })
            }
            _ => BrowserEnvelope::new(
                request_id,
                BrowserMessage::Error {
                    kind: "not_authenticated".to_string(),
                    message: "pair or authenticate before sending browser events".to_string(),
                },
            ),
        };
        send_envelope(&mut sink, &reply).await?;
    }

    let mut connection = authenticated.expect("authenticated connection set");
    let browser_id = connection.browser_id.clone();
    let connection_id = connection.connection_id.clone();
    loop {
        tokio::select! {
            outbound = connection.receiver.recv() => {
                let Some(event) = outbound else { break; };
                send_envelope(&mut sink, &event.0).await?;
            }
            inbound = source.next() => {
                let Some(inbound) = inbound else { break; };
                let message = inbound.map_err(|_| labby_browser::BrowserError::ConnectionClosed)?;
                let Message::Text(text) = message else { continue; };
                let envelope: BrowserEnvelope = serde_json::from_str(text.as_str())?;
                envelope.validate_version()?;
                let request_id = envelope.request_id.clone();
                let received = match envelope.message {
                    BrowserMessage::Observe(observation) => { bridge.observe(&browser_id, &observation)?; "observe" }
                    BrowserMessage::DocumentClosed { tab_id, document_id } => { bridge.close_document(&browser_id, tab_id, &document_id)?; "document_closed" }
                    completion @ (BrowserMessage::ToolResult { .. } | BrowserMessage::ToolError { .. }) => {
                        let _matched = bridge.complete(&browser_id, completion)?;
                        "tool_completion"
                    }
                    _ => continue,
                };
                if request_id.is_some() {
                    send_envelope(&mut sink, &BrowserEnvelope::new(request_id, BrowserMessage::Acknowledged { received: received.to_string() })).await?;
                }
            }
        }
    }
    bridge.disconnect(&browser_id, &connection_id)?;
    Ok(())
}

async fn send_envelope(
    sink: &mut futures::stream::SplitSink<WebSocket, Message>,
    envelope: &BrowserEnvelope,
) -> Result<(), labby_browser::BrowserError> {
    let json = serde_json::to_string(envelope)?;
    sink.send(Message::Text(json.into()))
        .await
        .map_err(|_| labby_browser::BrowserError::ConnectionClosed)
}
