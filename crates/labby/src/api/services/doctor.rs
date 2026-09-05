use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use axum::{
    Extension, Json,
    extract::{ConnectInfo, State},
    http::HeaderMap,
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
};
use futures::stream;
use serde_json::Value;
use tracing::info;

use crate::api::error::ApiError;
use crate::api::oauth::AuthContext;
use crate::api::services::helpers::{dispatch_meta_from_headers, handle_action_with_meta};
use crate::api::{ActionRequest, state::AppState};
use crate::dispatch::doctor::ACTIONS;

struct AbortTaskOnDrop(tokio::task::AbortHandle);

impl Drop for AbortTaskOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct AuditEventStreamState {
    rx: tokio::sync::mpsc::Receiver<crate::dispatch::doctor::Finding>,
    request_id: Option<String>,
    opened_at: std::time::Instant,
    _producer: AbortTaskOnDrop,
}

fn audit_event_stream(
    state: AuditEventStreamState,
) -> impl futures::Stream<Item = Result<Event, Infallible>> {
    stream::unfold(state, |mut state| async move {
        match state.rx.recv().await {
            Some(finding) => match serde_json::to_string(&finding) {
                Ok(payload) => Some((Ok(Event::default().data(payload)), state)),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to serialize doctor finding; skipping");
                    Some((
                        Ok(Event::default().event("error").data(e.to_string())),
                        state,
                    ))
                }
            },
            None => {
                info!(
                    surface = "api",
                    service = "doctor",
                    action = "audit.full",
                    request_id = state.request_id.as_deref(),
                    elapsed_ms = state.opened_at.elapsed().as_millis(),
                    "dispatch finish"
                );
                None
            }
        }
    })
}

pub fn routes(_state: AppState) -> crate::api::route_registry::RouteGroup {
    use crate::api::route_registry::RouteGroup;
    let mut descriptors = descriptors().into_iter();
    RouteGroup::empty()
        .route(descriptors.next().unwrap(), post(handle))
        .route(descriptors.next().unwrap(), get(stream_audit_full))
}

pub(crate) fn descriptors() -> Vec<crate::api::route_registry::RouteDescriptor> {
    use crate::api::route_registry::{RouteAuth, RouteDescriptor};
    vec![
        RouteDescriptor::new("POST", "/", "handle", "doctor", RouteAuth::V1).host_validated(),
        RouteDescriptor::new(
            "GET",
            "/audit-full/stream",
            "stream_audit_full",
            "doctor",
            RouteAuth::V1,
        )
        .host_validated(),
    ]
}

async fn handle(
    State(state): State<AppState>,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    auth: Option<Extension<AuthContext>>,
    Json(req): Json<ActionRequest>,
) -> Result<Json<Value>, ApiError> {
    let clients = state.clients.clone();
    handle_action_with_meta(
        "doctor",
        "api",
        dispatch_meta_from_headers(
            &headers,
            auth.as_ref().map(|value| &value.0),
            peer.map(|Extension(ConnectInfo(addr))| addr),
        ),
        req,
        ACTIONS,
        move |action, params| async move {
            crate::dispatch::doctor::dispatch_with_clients_relay_and_auth(
                &clients,
                state.public_relay.clone(),
                crate::dispatch::doctor::AuthConfigSource::Authoritative(state.auth_config.clone()),
                &action,
                params,
                "api",
            )
            .await
        },
    )
    .await
}

/// `GET /v1/doctor/audit-full/stream` — SSE stream of `audit.full` results.
async fn stream_audit_full(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, ApiError> {
    const ACTION: &str = "audit.full";
    let request_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let start = std::time::Instant::now();

    info!(
        surface = "api",
        service = "doctor",
        action = ACTION,
        request_id = request_id.as_deref(),
        "dispatch start"
    );

    let (tx, rx) = tokio::sync::mpsc::channel::<crate::dispatch::doctor::Finding>(64);
    let clients = Arc::clone(&state.clients);
    let public_relay = state.public_relay.clone();
    let auth = state.auth_config.as_deref().cloned();

    let producer = tokio::spawn(async move {
        crate::dispatch::doctor::service::stream_audit_full_with_relay_and_auth(
            clients,
            public_relay,
            auth,
            tx,
        )
        .await;
    });

    info!(
        surface = "api",
        service = "doctor",
        action = ACTION,
        request_id = request_id.as_deref(),
        elapsed_ms = start.elapsed().as_millis(),
        "dispatch ok"
    );

    let opened_at = std::time::Instant::now();

    let event_stream = audit_event_stream(AuditEventStreamState {
        rx,
        request_id,
        opened_at,
        _producer: AbortTaskOnDrop(producer.abort_handle()),
    });

    Ok(Sse::new(event_stream).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NotifyOnDrop(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }

    #[tokio::test]
    async fn dropping_sse_stream_aborts_its_audit_producer() {
        let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let producer = tokio::spawn(async move {
            let _notify = NotifyOnDrop(Some(dropped_tx));
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        });
        started_rx.await.expect("producer must start");
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        let stream = audit_event_stream(AuditEventStreamState {
            rx,
            request_id: None,
            opened_at: std::time::Instant::now(),
            _producer: AbortTaskOnDrop(producer.abort_handle()),
        });

        drop(stream);

        tokio::time::timeout(std::time::Duration::from_millis(200), dropped_rx)
            .await
            .expect("dropped SSE stream must promptly abort its producer")
            .expect("producer drop notification must be delivered");
    }
}
