//! Thin HTTP adapters for the Rust browser bridge.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
};
use futures::{SinkExt as _, StreamExt as _};
use labby_browser::{BrowserEnvelope, BrowserMessage, LEGACY_PROTOCOL_VERSION, PairingStatus};
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
const MAX_PAIRING_REQUESTS_PER_CLIENT: usize = 8;
const PAIRING_REQUEST_WINDOW: Duration = Duration::from_mins(5);
static SOCKET_CAPACITY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(64);
static PREAUTH_CLIENTS: LazyLock<Mutex<HashMap<IpAddr, ClientAdmission>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Default)]
struct ClientAdmission {
    active_sockets: usize,
    pairing_committed: usize,
    pairing_reserved: usize,
    pairing_window_started: Option<Instant>,
}

impl ClientAdmission {
    fn refresh_pairing_window(&mut self, now: Instant) {
        if self.pairing_reserved == 0
            && self
                .pairing_window_started
                .is_some_and(|started| now.duration_since(started) >= PAIRING_REQUEST_WINDOW)
        {
            self.pairing_committed = 0;
            self.pairing_window_started = None;
        }
    }

    fn idle(&self) -> bool {
        self.active_sockets == 0 && self.pairing_committed == 0 && self.pairing_reserved == 0
    }
}

struct PreauthClientPermit {
    client_ip: IpAddr,
}

impl PreauthClientPermit {
    fn acquire(client_ip: IpAddr) -> Result<Self, ApiError> {
        let now = Instant::now();
        let mut clients = PREAUTH_CLIENTS.lock().map_err(|_| {
            ApiError::new(ToolError::Sdk {
                sdk_kind: "server_busy".to_string(),
                message: "browser admission state is unavailable".to_string(),
            })
        })?;
        clients.retain(|_, state| {
            state.refresh_pairing_window(now);
            !state.idle()
        });
        let state = clients.entry(client_ip).or_default();
        if state.active_sockets >= MAX_PREAUTH_SOCKETS_PER_CLIENT {
            return Err(ApiError::new(ToolError::Sdk {
                sdk_kind: "server_busy".to_string(),
                message: "browser unauthenticated connection capacity is exhausted for this client"
                    .to_string(),
            }));
        }
        state.active_sockets += 1;
        Ok(Self { client_ip })
    }

    fn reserve_pairing_request(
        &self,
    ) -> Result<PairingRequestReservation, labby_browser::BrowserError> {
        let now = Instant::now();
        let mut clients = PREAUTH_CLIENTS
            .lock()
            .map_err(|_| labby_browser::BrowserError::ServerBusy)?;
        let state = clients
            .get_mut(&self.client_ip)
            .ok_or(labby_browser::BrowserError::ServerBusy)?;
        state.refresh_pairing_window(now);
        if state.pairing_committed + state.pairing_reserved >= MAX_PAIRING_REQUESTS_PER_CLIENT {
            return Err(labby_browser::BrowserError::ServerBusy);
        }
        state.pairing_window_started.get_or_insert(now);
        state.pairing_reserved += 1;
        Ok(PairingRequestReservation {
            client_ip: self.client_ip,
            committed: false,
        })
    }
}

impl Drop for PreauthClientPermit {
    fn drop(&mut self) {
        let Ok(mut clients) = PREAUTH_CLIENTS.lock() else {
            return;
        };
        let Some(state) = clients.get_mut(&self.client_ip) else {
            return;
        };
        state.active_sockets = state.active_sockets.saturating_sub(1);
        state.refresh_pairing_window(Instant::now());
        if state.idle() {
            clients.remove(&self.client_ip);
        }
    }
}

struct PairingRequestReservation {
    client_ip: IpAddr,
    committed: bool,
}

impl PairingRequestReservation {
    fn commit(mut self) -> Result<(), labby_browser::BrowserError> {
        let mut clients = PREAUTH_CLIENTS
            .lock()
            .map_err(|_| labby_browser::BrowserError::ServerBusy)?;
        let state = clients
            .get_mut(&self.client_ip)
            .ok_or(labby_browser::BrowserError::ServerBusy)?;
        state.pairing_reserved = state.pairing_reserved.saturating_sub(1);
        state.pairing_committed += 1;
        self.committed = true;
        Ok(())
    }
}

impl Drop for PairingRequestReservation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let Ok(mut clients) = PREAUTH_CLIENTS.lock() else {
            return;
        };
        let Some(state) = clients.get_mut(&self.client_ip) else {
            return;
        };
        state.pairing_reserved = state.pairing_reserved.saturating_sub(1);
        if state.idle() {
            clients.remove(&self.client_ip);
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
            .rsplit(',')
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
    let mut negotiated_version = None;
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
        envelope.validate_server_version()?;
        let protocol_version = match negotiated_version {
            Some(version) if version == envelope.version => version,
            Some(_) => {
                return Err(labby_browser::BrowserError::InvalidRequest(
                    "browser protocol version changed during connection".to_string(),
                ));
            }
            None => {
                negotiated_version = Some(envelope.version);
                envelope.version
            }
        };
        let request_id = envelope.request_id.clone();
        let reply = match envelope.message {
            (BrowserMessage::PairingRequest { .. } | BrowserMessage::PairingStatus { .. })
                if protocol_version == LEGACY_PROTOCOL_VERSION =>
            {
                protocol_upgrade_required(protocol_version, request_id)
            }
            BrowserMessage::PairingRequest {
                display_name,
                extension_id: claimed_extension_id,
                public_key,
            } => {
                if claimed_extension_id != extension_id {
                    return Err(labby_browser::BrowserError::AuthenticationFailed);
                }
                let pairing_reservation = preauth_permit
                    .as_ref()
                    .ok_or(labby_browser::BrowserError::AuthenticationFailed)?
                    .reserve_pairing_request()?;
                let pairing = bridge
                    .request_pairing(&display_name, extension_id, &public_key)
                    .await?;
                pairing_reservation.commit()?;
                let pairing_fingerprint = pairing.pairing_fingerprint();
                BrowserEnvelope::for_version(
                    protocol_version,
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
                            (PairingStatus::Approved, Some(browser_id)) => {
                                BrowserEnvelope::for_version(
                                    protocol_version,
                                    request_id,
                                    BrowserMessage::PairingApproved { browser_id },
                                )
                            }
                            (PairingStatus::Pending, None) => BrowserEnvelope::for_version(
                                protocol_version,
                                request_id,
                                BrowserMessage::PairingPending {
                                    pairing_id: pairing.id,
                                    expires_at: pairing.expires_at,
                                    pairing_fingerprint,
                                },
                            ),
                            (status, _) => BrowserEnvelope::for_version(
                                protocol_version,
                                request_id,
                                BrowserMessage::Error {
                                    kind: "pairing_not_pending".to_string(),
                                    message: format!("pairing request is {status:?}")
                                        .to_lowercase(),
                                },
                            ),
                        }
                    }
                    Some(_) | None => BrowserEnvelope::for_version(
                        protocol_version,
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
                        match bridge.issue_challenge(&browser_id).await {
                            Ok(mut challenge) => {
                                challenge.version = protocol_version;
                                challenge.request_id = request_id;
                                challenge
                            }
                            Err(labby_browser::BrowserError::AuthenticationFailed) => {
                                authentication_failed(protocol_version, request_id)
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    _ => authentication_failed(protocol_version, request_id),
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
                    BrowserEnvelope::for_version(
                        protocol_version,
                        request_id,
                        BrowserMessage::Authenticated { browser_id },
                    )
                }
                Err(labby_browser::BrowserError::AuthenticationFailed) => {
                    authentication_failed(protocol_version, request_id)
                }
                Err(error) => return Err(error),
            },
            BrowserMessage::Heartbeat => BrowserEnvelope::for_version(
                protocol_version,
                request_id,
                BrowserMessage::Acknowledged {
                    received: "heartbeat".to_string(),
                },
            ),
            _ => BrowserEnvelope::for_version(
                protocol_version,
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

    let protocol_version = negotiated_version.expect("protocol version negotiated before auth");
    let mut connection = authenticated.expect("authenticated connection set");
    let browser_id = connection.browser_id.clone();
    let connection_id = connection.connection_id.clone();
    let loop_result: Result<(), labby_browser::BrowserError> = async {
      loop {
        tokio::select! {
            outbound = connection.receiver.recv() => {
                let Some(mut event) = outbound else { break; };
                event.0.version = protocol_version;
                send_envelope(&mut sink, &event.0).await?;
            }
            inbound = source.next() => {
                let Some(inbound) = inbound else { break; };
                let message = inbound.map_err(|_| labby_browser::BrowserError::ConnectionClosed)?;
                let Message::Text(text) = message else { continue; };
                let envelope: BrowserEnvelope = serde_json::from_str(text.as_str())?;
                envelope.validate_server_version()?;
                if envelope.version != protocol_version {
                    return Err(labby_browser::BrowserError::InvalidRequest(
                        "browser protocol version changed during connection".to_string(),
                    ));
                }
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
                        send_envelope(&mut sink, &BrowserEnvelope::for_version(protocol_version, request_id, BrowserMessage::Error { kind: "invalid_message_for_state".to_string(), message: "message is not valid after authentication".to_string() })).await?;
                        continue;
                    },
                };
                if request_id.is_some() {
                    send_envelope(&mut sink, &BrowserEnvelope::for_version(protocol_version, request_id, BrowserMessage::Acknowledged { received: received.to_string() })).await?;
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

fn authentication_failed(protocol_version: u32, request_id: Option<String>) -> BrowserEnvelope {
    BrowserEnvelope::for_version(
        protocol_version,
        request_id,
        BrowserMessage::Error {
            kind: "auth_failed".to_string(),
            message: "browser authentication failed".to_string(),
        },
    )
}

fn protocol_upgrade_required(protocol_version: u32, request_id: Option<String>) -> BrowserEnvelope {
    BrowserEnvelope::for_version(
        protocol_version,
        request_id,
        BrowserMessage::Error {
            kind: "protocol_upgrade_required".to_string(),
            message: "browser pairing requires protocol version 2".to_string(),
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
            Some("127.0.0.1".parse().unwrap())
        );
        headers.insert("x-forwarded-for", HeaderValue::from_static("not-an-ip"));
        assert_eq!(admission_client_ip(&headers, true, Some(peer)), None);
    }

    #[test]
    fn pairing_creation_budget_is_bounded_and_failed_reservations_roll_back() {
        let ip: IpAddr = "198.51.100.43".parse().unwrap();
        let permit = PreauthClientPermit::acquire(ip).unwrap();

        // A store-side rejection drops its uncommitted reservation and must not
        // consume the client's five-minute pairing budget.
        drop(permit.reserve_pairing_request().unwrap());
        for _ in 0..MAX_PAIRING_REQUESTS_PER_CLIENT {
            permit.reserve_pairing_request().unwrap().commit().unwrap();
        }
        assert!(permit.reserve_pairing_request().is_err());

        {
            let mut clients = PREAUTH_CLIENTS.lock().unwrap();
            let state = clients.get_mut(&ip).unwrap();
            state.pairing_window_started =
                Some(Instant::now() - PAIRING_REQUEST_WINDOW - Duration::from_secs(1));
        }
        permit.reserve_pairing_request().unwrap().commit().unwrap();
        drop(permit);
        PREAUTH_CLIENTS.lock().unwrap().remove(&ip);
    }

    #[test]
    fn pairing_window_does_not_reset_while_a_reservation_is_live() {
        let ip: IpAddr = "198.51.100.44".parse().unwrap();
        let permit = PreauthClientPermit::acquire(ip).unwrap();
        let held = permit.reserve_pairing_request().unwrap();
        {
            let mut clients = PREAUTH_CLIENTS.lock().unwrap();
            clients.get_mut(&ip).unwrap().pairing_window_started =
                Some(Instant::now() - PAIRING_REQUEST_WINDOW - Duration::from_secs(1));
        }
        for _ in 1..MAX_PAIRING_REQUESTS_PER_CLIENT {
            permit.reserve_pairing_request().unwrap().commit().unwrap();
        }
        assert!(permit.reserve_pairing_request().is_err());
        drop(held);
        permit.reserve_pairing_request().unwrap().commit().unwrap();
        drop(permit);
        PREAUTH_CLIENTS.lock().unwrap().remove(&ip);
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
