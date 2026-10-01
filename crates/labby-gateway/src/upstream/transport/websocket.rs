use std::borrow::Cow;

use futures::{SinkExt, StreamExt};
use rmcp::service::{RawRxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::worker::{Worker, WorkerConfig, WorkerContext, WorkerQuitReason};
use rmcp::{RoleClient, transport::worker::WorkerTransport};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{Message, WebSocketConfig};
use tokio_tungstenite::tungstenite::{self};

const DEFAULT_MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;
const DEFAULT_MAX_FRAME_SIZE: usize = 128 * 1024;
const WEBSOCKET_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
enum FrameWriteFailure {
    #[error("websocket write cancelled")]
    Cancelled,
    #[error(transparent)]
    Failed(WebSocketTransportError),
}

async fn send_frame<S>(
    writer: &mut S,
    frame: Message,
    cancellation: &tokio_util::sync::CancellationToken,
) -> Result<(), FrameWriteFailure>
where
    S: futures::Sink<Message, Error = tungstenite::Error> + Unpin,
{
    tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(FrameWriteFailure::Cancelled),
        result = tokio::time::timeout(WEBSOCKET_WRITE_TIMEOUT, writer.send(frame)) => {
            result.map_err(|_| FrameWriteFailure::Failed(WebSocketTransportError::new("websocket write timed out")))?
                .map_err(|error| FrameWriteFailure::Failed(WebSocketTransportError::new(format!("websocket write failed: {error}"))))
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum WebSocketTransportError {
    #[error("{0}")]
    Message(String),
}

impl WebSocketTransportError {
    fn new(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

#[derive(Debug, Clone)]
pub struct WebSocketTransportConfig {
    pub url: String,
    pub authorization: Option<String>,
    pub max_message_size: usize,
    pub max_frame_size: usize,
}

impl WebSocketTransportConfig {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            authorization: None,
            max_message_size: DEFAULT_MAX_MESSAGE_SIZE,
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
        }
    }

    #[must_use]
    pub fn with_authorization(mut self, authorization: Option<String>) -> Self {
        self.authorization = authorization;
        self
    }

    #[must_use]
    pub fn with_max_message_size(mut self, max_message_size: usize) -> Self {
        self.max_message_size = max_message_size;
        // Peers may send a complete JSON response in one frame. Keeping the
        // smaller default frame cap would reject otherwise permitted Skills
        // resources before the capability-specific size check can run.
        self.max_frame_size = max_message_size;
        self
    }
}

#[derive(Debug)]
pub struct WebSocketClientWorker {
    config: WebSocketTransportConfig,
}

impl WebSocketClientWorker {
    #[must_use]
    pub fn new(config: WebSocketTransportConfig) -> Self {
        Self { config }
    }
}

impl Worker for WebSocketClientWorker {
    type Error = WebSocketTransportError;
    type Role = RoleClient;

    fn preserves_raw_responses() -> bool {
        true
    }

    fn err_closed() -> Self::Error {
        WebSocketTransportError::new("websocket transport is closed")
    }

    fn err_join(error: tokio::task::JoinError) -> Self::Error {
        WebSocketTransportError::new(format!("websocket transport task failed: {error}"))
    }

    fn config(&self) -> WorkerConfig {
        let mut config = WorkerConfig::default();
        config.name = Some("upstream-websocket-client".to_string());
        config.channel_buffer_capacity = 32;
        config
    }

    async fn run(
        self,
        mut context: WorkerContext<Self>,
    ) -> Result<(), WorkerQuitReason<Self::Error>> {
        let mut request = self
            .config
            .url
            .clone()
            .into_client_request()
            .map_err(|error| {
                WorkerQuitReason::fatal(
                    WebSocketTransportError::new(format!("invalid websocket request: {error}")),
                    "build websocket request",
                )
            })?;
        if let Some(authorization) = &self.config.authorization {
            let header =
                tungstenite::http::HeaderValue::from_str(authorization).map_err(|error| {
                    WorkerQuitReason::fatal(
                        WebSocketTransportError::new(format!(
                            "invalid websocket authorization header: {error}"
                        )),
                        "build websocket authorization header",
                    )
                })?;
            request
                .headers_mut()
                .insert(tungstenite::http::header::AUTHORIZATION, header);
        }

        let mut websocket_config = WebSocketConfig::default();
        websocket_config.max_message_size = Some(self.config.max_message_size);
        websocket_config.max_frame_size = Some(self.config.max_frame_size);
        websocket_config.accept_unmasked_frames = false;
        let (socket, _) = connect_async_with_config(request, Some(websocket_config), false)
            .await
            .map_err(|error| {
                WorkerQuitReason::fatal(
                    WebSocketTransportError::new(format!("websocket connect failed: {error}")),
                    "connect websocket upstream",
                )
            })?;
        let (mut writer, mut reader) = socket.split();
        let cancellation = context.cancellation_token.clone();

        loop {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    // Drop the socket: even a best-effort Close may block when
                    // the peer has stopped reading its outgoing frames.
                    return Err(WorkerQuitReason::Cancelled);
                }
                inbound = reader.next() => {
                    match inbound {
                        Some(Ok(Message::Text(text))) => {
                            let message = decode_server_message(text.as_str()).map_err(|error| {
                                WorkerQuitReason::fatal(error, "decode websocket frame")
                            })?;
                            tokio::select! {
                                biased;
                                () = cancellation.cancelled() => return Err(WorkerQuitReason::Cancelled),
                                result = context.send_to_handler(message) => result?,
                            }
                        }
                        Some(Ok(Message::Binary(_))) => {
                            return Err(WorkerQuitReason::fatal(
                                WebSocketTransportError::new("binary websocket frames are not supported"),
                                "decode websocket frame",
                            ));
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            send_frame(&mut writer, Message::Pong(payload), &cancellation).await.map_err(|error| {
                                match error {
                                    FrameWriteFailure::Cancelled => WorkerQuitReason::Cancelled,
                                    FrameWriteFailure::Failed(error) => WorkerQuitReason::fatal(error, "send websocket pong"),
                                }
                            })?;
                        }
                        Some(Ok(Message::Pong(_))) => {}
                        Some(Ok(Message::Frame(_))) => {}
                        Some(Ok(Message::Close(_))) | None => return Err(WorkerQuitReason::TransportClosed),
                        Some(Err(error)) => {
                            return Err(WorkerQuitReason::fatal(
                                WebSocketTransportError::new(format!("websocket receive failed: {error}")),
                                "receive websocket frame",
                            ));
                        }
                    }
                }
                outbound = context.recv_from_handler() => {
                    let outbound = outbound?;
                    let payload = encode_client_message(&outbound.message).map_err(|error| {
                        WorkerQuitReason::fatal(error, "encode websocket frame")
                    })?;
                    match send_frame(&mut writer, Message::Text(payload.into()), &cancellation).await {
                        Ok(()) => {
                            drop(outbound.responder.send(Ok(())));
                        }
                        Err(FrameWriteFailure::Cancelled) => {
                            drop(outbound.responder.send(Err(WebSocketTransportError::new("websocket write cancelled"))));
                            return Err(WorkerQuitReason::Cancelled);
                        }
                        Err(FrameWriteFailure::Failed(error)) => {
                            drop(outbound.responder.send(Err(error.clone())));
                            return Err(WorkerQuitReason::fatal(error, "send websocket frame"));
                        }
                    }
                }
            }
        }
    }
}

pub type WebSocketClientTransport = WorkerTransport<WebSocketClientWorker>;

pub fn connect(config: WebSocketTransportConfig) -> WebSocketClientTransport {
    WorkerTransport::spawn(WebSocketClientWorker::new(config))
}

pub fn parse_ws_url(raw: &str) -> Result<url::Url, WebSocketTransportError> {
    let url = url::Url::parse(raw.trim())
        .map_err(|error| WebSocketTransportError::new(format!("invalid websocket url: {error}")))?;
    match url.scheme() {
        "ws" | "wss" => Ok(url),
        scheme => Err(WebSocketTransportError::new(format!(
            "unsupported websocket url scheme: {scheme}"
        ))),
    }
}

pub fn encode_client_message(
    message: &TxJsonRpcMessage<RoleClient>,
) -> Result<String, WebSocketTransportError> {
    serde_json::to_string(message).map_err(|error| {
        WebSocketTransportError::new(format!("failed to encode json-rpc frame: {error}"))
    })
}

pub fn decode_server_message(
    payload: &str,
) -> Result<RawRxJsonRpcMessage<RoleClient>, WebSocketTransportError> {
    serde_json::from_str(payload).map_err(|error| {
        WebSocketTransportError::new(format!("failed to decode json-rpc frame: {error}"))
    })
}

// Re-export from net::backoff to keep existing callers within dispatch/upstream working.
pub use crate::net::backoff::{jitter_delay, reprobe_backoff};

pub fn log_context(reason: &'static str) -> Cow<'static, str> {
    Cow::Borrowed(reason)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{
        ClientRequest, DiscoverRequest, DiscoverRequestParams, ErrorCode, ErrorData, NumberOrString,
    };

    #[test]
    fn parse_ws_url_accepts_websocket_schemes() {
        assert!(parse_ws_url("ws://localhost:9000/mcp").is_ok());
        assert!(parse_ws_url("wss://example.com/socket").is_ok());
    }

    #[test]
    fn parse_ws_url_rejects_invalid_inputs() {
        assert!(parse_ws_url("localhost:9000").is_err());
        assert!(parse_ws_url("http://localhost:9000/mcp").is_err());
        assert!(parse_ws_url("ws://localhost:9000 bad").is_err());
    }

    #[test]
    fn configured_message_limit_can_cover_skills_wire_payloads() {
        let limit = 24 * 1024 * 1024;
        let config =
            WebSocketTransportConfig::new("wss://example.com/mcp").with_max_message_size(limit);
        assert_eq!(config.max_message_size, limit);
        assert_eq!(config.max_frame_size, limit);
    }

    #[tokio::test]
    async fn configured_limit_accepts_large_single_frames_and_rejects_oversized_frames() {
        use tokio_tungstenite::WebSocketStream;
        use tokio_tungstenite::tungstenite::protocol::Role;

        let limit = 256 * 1024;
        for size in [limit, limit + 1] {
            let transport =
                WebSocketTransportConfig::new("ws://localhost").with_max_message_size(limit);
            let mut config = WebSocketConfig::default();
            config.max_message_size = Some(transport.max_message_size);
            config.max_frame_size = Some(transport.max_frame_size);
            let (client_io, server_io) = tokio::io::duplex(4096);
            let mut client =
                WebSocketStream::from_raw_socket(client_io, Role::Client, Some(config)).await;
            let mut server = WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
            let sender =
                tokio::spawn(
                    async move { server.send(Message::Text("x".repeat(size).into())).await },
                );
            let received = client.next().await.expect("response frame");
            if size == limit {
                assert_eq!(received.unwrap().into_text().unwrap().len(), limit);
                sender.await.unwrap().unwrap();
            } else {
                assert!(matches!(received, Err(tungstenite::Error::Capacity(_))));
                drop(client);
                drop(sender.await.unwrap());
            }
        }
    }

    #[test]
    fn json_rpc_frame_codec_round_trips_requests_responses_and_errors() {
        let request = TxJsonRpcMessage::<RoleClient>::request(
            ClientRequest::DiscoverRequest(DiscoverRequest::new(DiscoverRequestParams {})),
            NumberOrString::Number(7),
        );
        let encoded_request = encode_client_message(&request).expect("encode request");
        let decoded_request: serde_json::Value =
            serde_json::from_str(&encoded_request).expect("decode request json");
        assert_eq!(decoded_request["jsonrpc"], "2.0");
        assert_eq!(decoded_request["id"], 7);

        let response = serde_json::to_string(&RawRxJsonRpcMessage::<RoleClient>::response(
            serde_json::json!({}),
            NumberOrString::Number(9),
        ))
        .expect("encode response");
        let decoded_response = decode_server_message(&response).expect("decode response");
        assert!(matches!(
            decoded_response,
            RawRxJsonRpcMessage::<RoleClient>::Response(_)
        ));

        let error = serde_json::to_string(&RawRxJsonRpcMessage::<RoleClient>::error(
            ErrorData::new(ErrorCode::METHOD_NOT_FOUND, "method not found", None),
            Some(NumberOrString::Number(11)),
        ))
        .expect("encode error");
        let decoded_error = decode_server_message(&error).expect("decode error");
        assert!(matches!(
            decoded_error,
            RawRxJsonRpcMessage::<RoleClient>::Error(_)
        ));
    }
    #[tokio::test]
    async fn nonreading_peer_does_not_block_worker_cancellation() {
        use rmcp::transport::Transport;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (writing, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            let mut byte = [0];
            socket.get_ref().peek(&mut byte).await.unwrap();
            writing.send(()).unwrap();
            drop(wait.await);
            drop(socket);
        });
        let mut transport = connect(WebSocketTransportConfig::new(format!("ws://{address}")));
        let message = serde_json::from_value::<TxJsonRpcMessage<RoleClient>>(serde_json::json!({
            "jsonrpc":"2.0", "id":1, "method":"test/backpressure",
            "params":{"payload":"x".repeat(16 * 1024 * 1024)}
        }))
        .unwrap();
        let sent = tokio::spawn(transport.send(message));
        ready.await.unwrap();
        // Keep the remote socket alive without consuming its buffered frame.
        let closed =
            tokio::time::timeout(std::time::Duration::from_millis(300), transport.close()).await;
        let _ = release.send(());
        server.await.unwrap();
        drop(sent.await.unwrap());
        assert!(
            closed.is_ok(),
            "worker must cancel while socket send is backpressured"
        );
    }
    #[tokio::test(start_paused = true)]
    async fn stalled_text_and_pong_writes_have_a_finite_deadline() {
        for frame in [
            Message::Text("payload".into()),
            Message::Pong(vec![1].into()),
        ] {
            let cancellation = tokio_util::sync::CancellationToken::new();
            let mut sink = Box::pin(futures::sink::unfold((), |(), _: Message| {
                futures::future::pending::<Result<(), tungstenite::Error>>()
            }));
            let before = tokio::time::Instant::now();
            let result = send_frame(&mut sink, frame, &cancellation).await;
            assert!(result.unwrap_err().to_string().contains("timed out"));
            assert_eq!(before.elapsed(), WEBSOCKET_WRITE_TIMEOUT);
        }
    }

    #[tokio::test]
    #[allow(
        clippy::panic,
        reason = "fixture fails if cancelled work reaches the sink"
    )]
    async fn already_cancelled_write_never_dispatches_a_frame() {
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();
        let mut sink = Box::pin(futures::sink::unfold((), |(), _: Message| async {
            panic!("cancelled write must not enter sink");
            #[allow(unreachable_code)]
            Ok::<(), tungstenite::Error>(())
        }));
        let result = send_frame(&mut sink, Message::Pong(vec![1].into()), &cancellation).await;
        assert!(result.unwrap_err().to_string().contains("cancelled"));
    }
    #[tokio::test]
    async fn unread_handler_queue_does_not_block_worker_cancellation() {
        use rmcp::transport::Transport;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (filled, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            // Exceed the worker's32-message inbound queue. Keep the connection
            // alive after filling it so peer closure cannot release the worker.
            for id in 0..128 {
                socket
                    .send(Message::Text(
                        format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{{}}}}"#).into(),
                    ))
                    .await
                    .unwrap();
            }
            filled.send(()).unwrap();
            drop(wait.await);
        });
        let mut transport = connect(WebSocketTransportConfig::new(format!("ws://{address}")));
        ready.await.unwrap();
        // Observe inbound delivery, then let the worker exhaust its local queue.
        assert!(transport.receive_raw().await.is_some());
        for _ in 0..128 {
            tokio::task::yield_now().await;
        }
        let closed =
            tokio::time::timeout(std::time::Duration::from_millis(300), transport.close()).await;
        let _ = release.send(());
        server.await.unwrap();
        assert!(
            closed.is_ok(),
            "worker must cancel while handler queue is full"
        );
    }
}
