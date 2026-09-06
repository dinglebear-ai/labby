//! Thin HTTP adapters for the Rust browser bridge.

use std::net::SocketAddr;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use futures::{SinkExt as _, StreamExt as _};
use labby_browser::{BrowserEnvelope, BrowserMessage, PairingStatus};
use serde_json::Value;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::route_registry::{RouteAuth, RouteDescriptor, RouteGroup};
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::browser::runtime::browser_bridge;
use crate::dispatch::error::ToolError;

pub fn routes(_state: AppState) -> RouteGroup {
    RouteGroup::empty().route(
        descriptors().into_iter().next().expect("call descriptor"),
        post(handle_action),
    )
}

pub(crate) fn descriptors() -> Vec<RouteDescriptor> {
    vec![
        RouteDescriptor::new("POST", "/", "call", "browser", RouteAuth::V1)
            .private_no_store()
            .when("mounted only when API authentication is configured on a standalone host")
            .side_effects("browser pairing or page-tool invocation"),
    ]
}

pub fn public_routes() -> RouteGroup {
    RouteGroup::empty().route(
        public_descriptors()
            .into_iter()
            .next()
            .expect("browser socket descriptor"),
        get(upgrade),
    )
}

pub(crate) fn public_descriptors() -> Vec<RouteDescriptor> {
    vec![
        RouteDescriptor::new(
            "GET",
            "/browser/socket",
            "browser_socket",
            "browser",
            RouteAuth::Public,
        )
        .side_effects("loopback browser-extension WebSocket upgrade"),
    ]
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
    let extension_id = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(|origin| origin.strip_prefix("chrome-extension://"))
        .filter(|id| id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte)))
        .map(str::to_string);
    if !loopback || extension_id.is_none() {
        return Err(ApiError::new(ToolError::Forbidden {
            message: "browser bridge accepts only loopback extension connections".to_string(),
            required_scopes: Vec::new(),
        }));
    }
    browser_bridge().await?;
    Ok(upgrade
        .max_message_size(512 * 1024)
        .max_frame_size(512 * 1024)
        .on_upgrade(move |socket| {
            handle_socket(socket, extension_id.expect("validated extension id"))
        }))
}

async fn handle_socket(socket: WebSocket, extension_id: String) {
    if let Err(error) = run_socket(socket, &extension_id).await {
        tracing::warn!(
            surface = "api",
            service = "browser",
            kind = error.kind(),
            "browser extension connection ended"
        );
    }
}

async fn run_socket(
    socket: WebSocket,
    extension_id: &str,
) -> Result<(), labby_browser::BrowserError> {
    let bridge = browser_bridge()
        .await
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
                extension_id: claimed_extension_id,
                public_key,
            } => {
                if claimed_extension_id != extension_id {
                    return Err(labby_browser::BrowserError::AuthenticationFailed);
                }
                let pairing = bridge
                    .request_pairing(&display_name, extension_id, &public_key)
                    .await?;
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
                    .pairing(&pairing_id)
                    .await?
                    .ok_or(labby_browser::BrowserError::NotFound)?;
                match (pairing.status, pairing.browser_id) {
                    (PairingStatus::Approved, Some(browser_id)) => BrowserEnvelope::new(
                        request_id,
                        BrowserMessage::PairingApproved { browser_id },
                    ),
                    (PairingStatus::Pending, None) => BrowserEnvelope::new(
                        request_id,
                        BrowserMessage::PairingPending {
                            pairing_id: pairing.id,
                            expires_at: pairing.expires_at,
                        },
                    ),
                    (status, _) => BrowserEnvelope::new(
                        request_id,
                        BrowserMessage::Error {
                            kind: "pairing_not_pending".to_string(),
                            message: format!("pairing request is {status:?}").to_lowercase(),
                        },
                    ),
                }
            }
            BrowserMessage::AuthChallenge { browser_id } => {
                let browser = bridge
                    .store()
                    .browser(&browser_id)
                    .await?
                    .ok_or(labby_browser::BrowserError::AuthenticationFailed)?;
                if browser.extension_id != extension_id || browser.revoked_at.is_some() {
                    return Err(labby_browser::BrowserError::AuthenticationFailed);
                }
                let mut challenge = bridge.issue_challenge(&browser_id).await?;
                challenge.request_id = request_id;
                challenge
            }
            BrowserMessage::AuthResponse {
                challenge_id,
                signature,
            } => {
                let connection = bridge.authenticate(&challenge_id, &signature).await?;
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
        if let Err(error) = send_envelope(&mut sink, &reply).await {
            if let Some(connection) = authenticated.as_ref() {
                bridge.disconnect(&connection.browser_id, &connection.connection_id)?;
            }
            return Err(error);
        }
    }

    let mut connection = authenticated.expect("authenticated connection set");
    let browser_id = connection.browser_id.clone();
    let connection_id = connection.connection_id.clone();
    let loop_result: Result<(), labby_browser::BrowserError> = async {
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
                    BrowserMessage::Observe(observation) => { bridge.observe(&browser_id, &connection_id, &observation).await?; "observe" }
                    BrowserMessage::DocumentClosed { tab_id, document_id } => { bridge.close_document(&browser_id, &connection_id, tab_id, &document_id).await?; "document_closed" }
                    completion @ (BrowserMessage::ToolResult { .. } | BrowserMessage::ToolError { .. }) => {
                        let _matched = bridge.complete(&browser_id, &connection_id, completion)?;
                        "tool_completion"
                    }
                    _ => {
                        send_envelope(&mut sink, &BrowserEnvelope::new(request_id, BrowserMessage::Error { kind: "invalid_message_for_state".to_string(), message: "message is not valid after authentication".to_string() })).await?;
                        continue;
                    },
                };
                if request_id.is_some() {
                    send_envelope(&mut sink, &BrowserEnvelope::new(request_id, BrowserMessage::Acknowledged { received: received.to_string() })).await?;
                }
            }
        }
      }
      Ok(())
    }.await;
    let cleanup_result = bridge.disconnect(&browser_id, &connection_id);
    match (loop_result, cleanup_result) {
        (Err(primary), Err(cleanup)) => {
            tracing::warn!(
                browser_id,
                connection_id,
                error_kind = cleanup.kind(),
                "browser socket cleanup failed after connection error"
            );
            Err(primary)
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
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
