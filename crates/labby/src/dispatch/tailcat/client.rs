//! Same-user control client with no TCP, proxy, or remote-target fallback.
use bytes::Bytes;
use http_body_util::{BodyExt as _, Full};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Copy)]
pub(crate) enum Action {
    Prepare,
    Approve,
    Status,
    Stop,
}
impl Action {
    fn path(self) -> &'static str {
        match self {
            Self::Prepare => "/prepare",
            Self::Approve => "/approve",
            Self::Status => "/status",
            Self::Stop => "/stop",
        }
    }
    fn uncertain(self) -> bool {
        matches!(self, Self::Approve | Self::Stop)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ControlError {
    #[error("native Tailcat control denied the request")]
    Denied,
    #[error("native Tailcat control is unavailable; start the configured hosted gateway")]
    Unavailable,
    #[error("invalid native Tailcat control message")]
    Protocol,
    #[error("native Tailcat operation may have started; inspect tailcat status before retrying")]
    Uncertain,
}

pub(crate) struct LocalClient {
    path: PathBuf,
}
pub(crate) fn credential_wire(path: &std::path::Path) -> Result<String, ControlError> {
    let bytes =
        crate::installation::secure_file::read_private(path).map_err(|_| ControlError::Denied)?;
    let wire = std::str::from_utf8(&bytes).map_err(|_| ControlError::Denied)?;
    labby_primitives::product_credential::ProductCredential::parse(wire)
        .map_err(|_| ControlError::Denied)?;
    Ok(wire.to_owned())
}
impl LocalClient {
    pub(crate) fn new(path: PathBuf) -> Result<Self, ControlError> {
        if !path.is_absolute() {
            return Err(ControlError::Protocol);
        }
        Ok(Self { path })
    }
    pub(crate) async fn call(
        &self,
        action: Action,
        body: Option<Value>,
    ) -> Result<Value, ControlError> {
        let bytes = match body {
            Some(value) => serde_json::to_vec(&value).map_err(|_| ControlError::Protocol)?,
            None => Vec::new(),
        };
        if bytes.len() > 16 * 1024 {
            return Err(ControlError::Protocol);
        }
        let result =
            tokio::time::timeout(Duration::from_secs(40), self.exchange(action, bytes)).await;
        match result {
            Ok(Err(ControlError::Unavailable | ControlError::Protocol)) | Err(_)
                if action.uncertain() =>
            {
                Err(ControlError::Uncertain)
            }
            Ok(result) => result,
            Err(_) => Err(ControlError::Unavailable),
        }
    }
    async fn exchange(&self, action: Action, body: Vec<u8>) -> Result<Value, ControlError> {
        let stream = tokio::net::UnixStream::connect(&self.path)
            .await
            .map_err(|_| ControlError::Unavailable)?;
        verify_peer(&stream, nix::unistd::Uid::effective().as_raw())?;
        let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
            .max_headers(32)
            .max_buf_size(16 * 1024)
            .handshake(TokioIo::new(stream))
            .await
            .map_err(|_| ControlError::Unavailable)?;
        let _driver = Driver(tokio::spawn(async move {
            let _result = connection.await;
        }));
        let request = hyper::Request::builder()
            .method(if matches!(action, Action::Status) {
                "GET"
            } else {
                "POST"
            })
            .uri(action.path())
            .header("Host", "localhost")
            .header("Content-Type", "application/json")
            .header("Connection", "close")
            .body(Full::new(Bytes::from(body)))
            .map_err(|_| ControlError::Protocol)?;
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| ControlError::Unavailable)?;
        if !response.status().is_success() {
            return Err(ControlError::Denied);
        }
        if response.status() == hyper::StatusCode::NO_CONTENT {
            return Ok(serde_json::json!({"stopped":true}));
        }
        let mut incoming = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = incoming.frame().await {
            let frame = frame.map_err(|_| ControlError::Unavailable)?;
            if let Some(data) = frame.data_ref() {
                if bytes.len().saturating_add(data.len()) > 16 * 1024 {
                    return Err(ControlError::Protocol);
                }
                bytes.extend_from_slice(data);
            }
        }
        serde_json::from_slice(&bytes).map_err(|_| ControlError::Protocol)
    }
}
fn verify_peer(stream: &tokio::net::UnixStream, uid: u32) -> Result<(), ControlError> {
    let peer = stream.peer_cred().map_err(|_| ControlError::Denied)?;
    if peer.uid() != uid {
        return Err(ControlError::Denied);
    }
    Ok(())
}
struct Driver(tokio::task::JoinHandle<()>);
impl Drop for Driver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn private_client_uses_kernel_identity_and_never_falls_back_to_tcp() {
        let (directory, manager) = super::super::manager::tests::fixture().await;
        let path = directory.path().join("control.sock");
        let _listener =
            crate::api::tailcat::ControlListener::start(path.clone(), std::sync::Arc::new(manager))
                .await
                .unwrap();
        let peer = tokio::net::UnixStream::connect(&path).await.unwrap();
        assert!(verify_peer(&peer, nix::unistd::Uid::effective().as_raw() ^ 1).is_err());
        let client = LocalClient::new(path).unwrap();
        assert_eq!(
            client.call(Action::Status, None).await.unwrap(),
            serde_json::json!({"sessions":[]})
        );
        let absent = LocalClient::new(directory.path().join("absent.sock")).unwrap();
        assert!(matches!(
            absent.call(Action::Status, None).await,
            Err(ControlError::Unavailable)
        ));
        assert!(LocalClient::new("relative.sock".into()).is_err());
    }
}
