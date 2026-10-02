//! Same-user Unix control adapter. Never mount these routes on a TCP router.
use crate::dispatch::tailcat::{PairingRequest, manager::Manager};
use axum::{
    Router,
    body::Bytes,
    http::StatusCode,
    response::{IntoResponse as _, Response},
    routing::{get, post},
};
use base64::Engine as _;
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepare {
    origin: String,
    peer: String,
    upstream: String,
    source_credential: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Approve {
    id: String,
    nonce: String,
    origin: String,
    peer: String,
    upstream: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stop {
    id: String,
}

pub(crate) struct ControlListener {
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl ControlListener {
    pub(crate) async fn start(path: PathBuf, manager: Arc<Manager>) -> anyhow::Result<Self> {
        let config = crate::unix_listener::private_control_config(path)?;
        let listener = crate::unix_listener::bind(&config).await?;
        let cancel = CancellationToken::new();
        let task = super::serve_owned(listener, router(manager), cancel.clone(), 16 * 1024);
        Ok(Self { cancel, task })
    }
}
impl Drop for ControlListener {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

fn denied() -> Response {
    StatusCode::UNAUTHORIZED.into_response()
}
fn invalid() -> Response {
    StatusCode::BAD_REQUEST.into_response()
}
fn router(manager: Arc<Manager>) -> Router {
    let preparing = manager.clone();
    let approving = manager.clone();
    let stopping = manager.clone();
    Router::new()
        .route("/prepare", post(move |body: Bytes| {
            let manager = preparing.clone();
            async move {
                let Ok(input) = serde_json::from_slice::<Prepare>(&body) else { return invalid() };
                let Ok(source) = labby_primitives::product_credential::ProductCredential::parse(&input.source_credential) else { return denied() };
                let request = PairingRequest { origin: input.origin, peer: input.peer, upstream: input.upstream };
                let Ok(prepared) = manager.prepare(request, source).await else { return denied() };
                axum::Json(serde_json::json!({
                    "id": prepared.id,
                    "nonce": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(prepared.nonce),
                    "origin": prepared.origin, "peer": prepared.peer, "upstream": prepared.upstream,
                })).into_response()
            }
        }))
        .route("/approve", post(move |body: Bytes| {
            let manager = approving.clone();
            async move {
                let Ok(input) = serde_json::from_slice::<Approve>(&body) else { return invalid() };
                let Ok(nonce) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(input.nonce) else { return invalid() };
                let Ok(nonce) = <[u8;32]>::try_from(nonce) else { return invalid() };
                let request = PairingRequest { origin: input.origin, peer: input.peer, upstream: input.upstream };
                let Ok((id, delivery)) = manager.approve(&input.id, &nonce, request).await else { return denied() };
                axum::Json(delivery_packet(id, delivery)).into_response()
            }
        }))
        .route("/status", get(move || {
            let manager = manager.clone();
            async move {
                let Ok(sessions) = manager.status() else { return denied() };
                axum::Json(serde_json::json!({"sessions": sessions.into_iter().map(|(id, state)|
                    serde_json::json!({"id":id, "state":state})).collect::<Vec<_>>()})).into_response()
            }
        }))
        .route("/stop", post(move |body: Bytes| {
            let manager = stopping.clone();
            async move {
                let Ok(input) = serde_json::from_slice::<Stop>(&body) else { return invalid() };
                match manager.stop(&input.id).await { Ok(()) => StatusCode::NO_CONTENT.into_response(), Err(_) => denied() }
            }
        }))
}

fn delivery_packet(
    id: String,
    delivery: crate::dispatch::tailcat::SessionDelivery,
) -> serde_json::Value {
    serde_json::json!({ "version": 1, "id": id, "address": delivery.address,
        "port": delivery.port, "peer": delivery.peer, "upstream": delivery.upstream, "grant": delivery.envelope, "origin": delivery.origin,
        "generation": delivery.generation, "expiresAt": delivery.expires_at.saturating_mul(1000),
        "derpMapURL": delivery.derp_map_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[test]
    fn delivery_packet_matches_browser_units_and_relay_contract() {
        let packet = delivery_packet(
            "session".into(),
            crate::dispatch::tailcat::SessionDelivery {
                address: "100.64.0.1".into(),
                port: 1,
                peer: format!("nodekey:{}", "a".repeat(64)),
                upstream: "microsandbox".into(),
                envelope: "sealed".into(),
                origin: "https://depot.example".into(),
                generation: "generation".into(),
                expires_at: 1_800_000_000,
                derp_map_url: "https://relay.example/map".into(),
            },
        );
        assert_eq!(packet["version"], 1);
        assert_eq!(packet["expiresAt"], 1_800_000_000_000u64);
        assert_eq!(packet["derpMapURL"], "https://relay.example/map");
        assert!(packet.get("privateKey").is_none());
        assert!(packet.get("source_credential").is_none());
    }

    async fn request(path: &std::path::Path, wire: &[u8]) -> String {
        let mut stream = tokio::net::UnixStream::connect(path).await.unwrap();
        stream.write_all(wire).await.unwrap();
        let mut bytes = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            stream.take(4096).read_to_end(&mut bytes),
        )
        .await
        .unwrap()
        .unwrap();
        String::from_utf8(bytes).unwrap()
    }
    #[tokio::test]
    async fn control_socket_is_private_bounded_and_removes_only_its_own_inode() {
        let (directory, manager) = crate::dispatch::tailcat::manager::tests::fixture().await;
        let path = directory.path().join("control.sock");
        let listener = ControlListener::start(path.clone(), Arc::new(manager))
            .await
            .unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let status = request(&path, b"GET /status HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        assert!(status.starts_with("HTTP/1.1 200"));
        assert!(status.ends_with("{\"sessions\":[]}"));
        let operator = request(
            &path,
            b"GET /v1/gateway HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(operator.starts_with("HTTP/1.1 404"));
        let body = b"{\"secret-in-unknown-field\":true}";
        let wire = format!(
            "POST /prepare HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            std::str::from_utf8(body).unwrap()
        );
        let invalid = request(&path, wire.as_bytes()).await;
        assert!(invalid.starts_with("HTTP/1.1 400"));
        assert!(!invalid.contains("secret-in-unknown-field"));
        drop(listener);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while path.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn kernel_uid_mismatch_rejects_control_connection() {
        let directory = tempfile::Builder::new()
            .prefix("tailcat-control-")
            .tempdir_in(if cfg!(target_os = "macos") {
                PathBuf::from("/private/tmp")
            } else {
                std::env::temp_dir().canonicalize().unwrap()
            })
            .unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("control.sock");
        let mut config = crate::unix_listener::private_control_config(path.clone()).unwrap();
        config.peer_policy.uid = Some(nix::unistd::Uid::effective().as_raw() ^ 1);
        let listener = crate::unix_listener::bind(&config).await.unwrap();
        let router = Router::new().route("/status", get(|| async { "should-not-run" }));
        let cancel = CancellationToken::new();
        let task = super::super::serve_owned(listener, router, cancel.clone(), 16384);
        let mut stream = tokio::net::UnixStream::connect(&path).await.unwrap();
        let _written = stream
            .write_all(b"GET /status HTTP/1.1\r\nHost: localhost\r\nX-UID: 0\r\n\r\n")
            .await;
        let mut bytes = [0; 1024];
        let read = tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut bytes))
            .await
            .unwrap();
        assert!(matches!(read, Ok(0) | Err(_)));
        cancel.cancel();
        task.abort();
        let _joined = task.await;
    }
}
