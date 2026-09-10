//! Thin HTTP adapters for the Rust browser bridge.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

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
use crate::dispatch::browser::runtime::{browser_bridge, initialize_browser_bridge};
use crate::dispatch::error::ToolError;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_mins(2);
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PREAUTH_SOCKETS_PER_CLIENT: usize = 8;
static SOCKET_CAPACITY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(64);
static PREAUTH_CLIENTS: LazyLock<Mutex<HashMap<IpAddr, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct PreauthClientPermit {
    client_ip: IpAddr,
}

impl PreauthClientPermit {
    fn acquire(client_ip: IpAddr) -> Result<Self, ApiError> {
        let mut active = PREAUTH_CLIENTS.lock().map_err(|_| {
            ApiError::new(ToolError::Sdk {
                sdk_kind: "server_busy".to_string(),
                message: "browser admission state is unavailable".to_string(),
            })
        })?;
        let count = active.entry(client_ip).or_default();
        if *count >= MAX_PREAUTH_SOCKETS_PER_CLIENT {
            return Err(ApiError::new(ToolError::Sdk {
                sdk_kind: "server_busy".to_string(),
                message: "browser unauthenticated connection capacity is exhausted for this client"
                    .to_string(),
            }));
        }
        *count += 1;
        Ok(Self { client_ip })
    }
}

impl Drop for PreauthClientPermit {
    fn drop(&mut self) {
        let Ok(mut active) = PREAUTH_CLIENTS.lock() else {
            return;
        };
        let Some(count) = active.get_mut(&self.client_ip) else {
            return;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            active.remove(&self.client_ip);
        }
    }
}

fn admission_client_ip(
    headers: &HeaderMap,
    trust_forwarded_headers: bool,
    peer: Option<SocketAddr>,
) -> Option<IpAddr> {
    if trust_forwarded_headers
        && let Some(forwarded) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
    {
        return forwarded
            .split(',')
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse().ok());
    }
    peer.map(|address| address.ip())
}

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
        .host_validated()
        .side_effects("browser-extension WebSocket upgrade"),
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
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let extension_id = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(|origin| origin.strip_prefix("chrome-extension://"))
        .filter(|id| id.len() == 32 && id.bytes().all(|byte| (b'a'..=b'p').contains(&byte)))
        .map(str::to_string);
    if extension_id.is_none() {
        return Err(ApiError::new(ToolError::Forbidden {
            message: "browser bridge accepts only browser-extension origins".to_string(),
            required_scopes: Vec::new(),
        }));
    }
    let peer = peer.map(|Extension(ConnectInfo(address))| address);
    let client_ip = admission_client_ip(&headers, state.config.api.trust_forwarded_headers, peer)
        .ok_or_else(|| {
        ApiError::new(ToolError::Forbidden {
            message: "browser bridge requires a direct or trusted forwarded client address"
                .to_string(),
            required_scopes: Vec::new(),
        })
    })?;
    let preauth_permit = PreauthClientPermit::acquire(client_ip)?;
    let permit = SOCKET_CAPACITY.try_acquire().map_err(|_| {
        ApiError::new(ToolError::Sdk {
            sdk_kind: "server_busy".to_string(),
            message: "browser connection capacity is exhausted".to_string(),
        })
    })?;
    initialize_browser_bridge().await?;
    Ok(upgrade
        .max_message_size(512 * 1024)
        .max_frame_size(512 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            handle_socket(
                socket,
                extension_id.expect("validated extension id"),
                preauth_permit,
            )
            .await;
        }))
}

async fn handle_socket(
    socket: WebSocket,
    extension_id: String,
    preauth_permit: PreauthClientPermit,
) {
    if let Err(error) = run_socket(socket, &extension_id, preauth_permit).await {
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
    preauth_permit: PreauthClientPermit,
) -> Result<(), labby_browser::BrowserError> {
    let bridge = browser_bridge()
        .await
        .map_err(|error| labby_browser::BrowserError::InvalidRequest(error.to_string()))?;
    let (mut sink, mut source) = socket.split();
    let mut authenticated = None;
    let mut preauth_permit = Some(preauth_permit);

    let handshake_deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
    while authenticated.is_none() {
        // Check explicitly: a continuously ready input stream must not extend admission.
        if tokio::time::Instant::now() >= handshake_deadline {
            return Err(labby_browser::BrowserError::ToolTimeout);
        }
        let Some(message) = tokio::time::timeout_at(handshake_deadline, source.next())
            .await
            .map_err(|_| labby_browser::BrowserError::ToolTimeout)?
        else {
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
                let pairing_fingerprint = pairing.pairing_fingerprint();
                BrowserEnvelope::new(
                    request_id,
                    BrowserMessage::PairingPending {
                        pairing_id: pairing.id,
                        expires_at: pairing.expires_at,
                        pairing_fingerprint,
                    },
                )
            }
            BrowserMessage::PairingStatus { pairing_id } => {
                let pairing = bridge.store().pairing(&pairing_id).await?;
                match pairing {
                    Some(pairing) if pairing.extension_id == extension_id => {
                        let pairing_fingerprint = pairing.pairing_fingerprint();
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
                                    pairing_fingerprint,
                                },
                            ),
                            (status, _) => BrowserEnvelope::new(
                                request_id,
                                BrowserMessage::Error {
                                    kind: "pairing_not_pending".to_string(),
                                    message: format!("pairing request is {status:?}")
                                        .to_lowercase(),
                                },
                            ),
                        }
                    }
                    Some(_) | None => BrowserEnvelope::new(
                        request_id,
                        BrowserMessage::Error {
                            kind: "pairing_not_pending".to_string(),
                            message: "pairing request is unavailable".to_string(),
                        },
                    ),
                }
            }
            BrowserMessage::AuthChallenge { browser_id } => {
                match bridge.store().browser(&browser_id).await? {
                    Some(browser)
                        if browser.extension_id == extension_id && browser.revoked_at.is_none() =>
                    {
                        let mut challenge = bridge.issue_challenge(&browser_id).await?;
                        challenge.request_id = request_id;
                        challenge
                    }
                    _ => authentication_failed(request_id),
                }
            }
            BrowserMessage::AuthResponse {
                challenge_id,
                signature,
            } => match bridge.authenticate(&challenge_id, &signature).await {
                Ok(connection) => {
                    let browser_id = connection.browser_id.clone();
                    authenticated = Some(connection);
                    drop(preauth_permit.take());
                    BrowserEnvelope::new(request_id, BrowserMessage::Authenticated { browser_id })
                }
                Err(labby_browser::BrowserError::AuthenticationFailed) => {
                    authentication_failed(request_id)
                }
                Err(error) => return Err(error),
            },
            BrowserMessage::Heartbeat => BrowserEnvelope::new(
                request_id,
                BrowserMessage::Acknowledged {
                    received: "heartbeat".to_string(),
                },
            ),
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
                    BrowserMessage::Heartbeat => "heartbeat",
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

fn authentication_failed(request_id: Option<String>) -> BrowserEnvelope {
    BrowserEnvelope::new(
        request_id,
        BrowserMessage::Error {
            kind: "auth_failed".to_string(),
            message: "browser authentication failed".to_string(),
        },
    )
}

async fn send_envelope(
    sink: &mut futures::stream::SplitSink<WebSocket, Message>,
    envelope: &BrowserEnvelope,
) -> Result<(), labby_browser::BrowserError> {
    let json = serde_json::to_string(envelope)?;
    tokio::time::timeout(SOCKET_WRITE_TIMEOUT, sink.send(Message::Text(json.into())))
        .await
        .map_err(|_| labby_browser::BrowserError::ToolTimeout)?
        .map_err(|_| labby_browser::BrowserError::ConnectionClosed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn client_admission_uses_forwarded_ip_only_when_proxy_headers_are_trusted() {
        let peer: SocketAddr = "127.0.0.1:8765".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.9, 127.0.0.1"),
        );
        assert_eq!(
            admission_client_ip(&headers, false, Some(peer)),
            Some("127.0.0.1".parse().unwrap())
        );
        assert_eq!(
            admission_client_ip(&headers, true, Some(peer)),
            Some("203.0.113.9".parse().unwrap())
        );
        headers.insert("x-forwarded-for", HeaderValue::from_static("not-an-ip"));
        assert_eq!(admission_client_ip(&headers, true, Some(peer)), None);
    }

    #[test]
    fn unauthenticated_admission_is_bounded_per_client_and_released_on_drop() {
        let ip: IpAddr = "198.51.100.42".parse().unwrap();
        let mut permits = Vec::new();
        for _ in 0..MAX_PREAUTH_SOCKETS_PER_CLIENT {
            permits.push(PreauthClientPermit::acquire(ip).unwrap());
        }
        assert!(PreauthClientPermit::acquire(ip).is_err());
        drop(permits.pop());
        permits.push(PreauthClientPermit::acquire(ip).unwrap());
        drop(permits);
        assert!(PreauthClientPermit::acquire(ip).is_ok());
    }
}
