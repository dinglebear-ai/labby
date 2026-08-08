#[cfg(target_os = "linux")]
use std::ffi::OsString;
use std::io;
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStringExt as _;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use labby_runtime::gateway_config::UpstreamTransport;
use rmcp::model::{CallToolRequestParams, CallToolResponse};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use super::connect::connect_upstream;
use super::testsupport::test_upstream_config;

static ABSTRACT_SOCKET_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
struct RequestSignals {
    expected_host: Arc<AtomicBool>,
    bearer_header: Arc<AtomicBool>,
    custom_header: Arc<AtomicBool>,
    mcp_name_header: Arc<AtomicBool>,
    tool_call: Arc<AtomicBool>,
}

impl RequestSignals {
    fn new() -> Self {
        Self {
            expected_host: Arc::new(AtomicBool::new(false)),
            bearer_header: Arc::new(AtomicBool::new(false)),
            custom_header: Arc::new(AtomicBool::new(false)),
            mcp_name_header: Arc::new(AtomicBool::new(false)),
            tool_call: Arc::new(AtomicBool::new(false)),
        }
    }

    fn observe_headers(&self, headers: &str) {
        for line in headers.lines().skip(1) {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            if name.eq_ignore_ascii_case("host") && value == "local.internal" {
                self.expected_host.store(true, Ordering::SeqCst);
            } else if name.eq_ignore_ascii_case("authorization")
                && value.starts_with("Bearer ")
                && value.len() > "Bearer ".len()
            {
                self.bearer_header.store(true, Ordering::SeqCst);
            } else if name.eq_ignore_ascii_case("x-labby-test") && value == "present" {
                self.custom_header.store(true, Ordering::SeqCst);
            } else if name.eq_ignore_ascii_case("mcp-name") && value == "unix_echo" {
                self.mcp_name_header.store(true, Ordering::SeqCst);
            }
        }
    }

    fn assert_complete(&self) {
        assert!(self.expected_host.load(Ordering::SeqCst));
        assert!(self.bearer_header.load(Ordering::SeqCst));
        assert!(self.custom_header.load(Ordering::SeqCst));
        assert!(self.mcp_name_header.load(Ordering::SeqCst));
        assert!(self.tool_call.load(Ordering::SeqCst));
    }
}

fn header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn content_length(headers: &str) -> usize {
    headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0)
}

async fn write_response(
    stream: &mut UnixStream,
    status: &str,
    body: Option<Value>,
) -> io::Result<()> {
    let body = body.map_or_else(Vec::new, |value| serde_json::to_vec(&value).expect("JSON"));
    let content_type = if body.is_empty() {
        ""
    } else {
        "Content-Type: application/json\r\n"
    };
    let headers = format!(
        "HTTP/1.1 {status}\r\n{content_type}Content-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    stream.write_all(&body).await?;
    stream.flush().await
}

async fn handle_connection(mut stream: UnixStream, signals: RequestSignals) -> io::Result<()> {
    let mut pending = Vec::new();
    let mut scratch = [0_u8; 8192];

    loop {
        let headers_end = loop {
            if let Some(index) = header_end(&pending) {
                break index;
            }
            let read = stream.read(&mut scratch).await?;
            if read == 0 {
                return Ok(());
            }
            pending.extend_from_slice(&scratch[..read]);
        };

        let body_start = headers_end + 4;
        let headers = std::str::from_utf8(&pending[..headers_end])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
            .to_owned();
        let body_len = content_length(&headers);
        let total_len = body_start + body_len;
        while pending.len() < total_len {
            let read = stream.read(&mut scratch).await?;
            if read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "request body ended early",
                ));
            }
            pending.extend_from_slice(&scratch[..read]);
        }

        signals.observe_headers(&headers);
        let request_line = headers.lines().next().unwrap_or_default();
        assert_eq!(request_line, "POST /mcp HTTP/1.1");

        let request: Value = serde_json::from_slice(&pending[body_start..total_len])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        pending.drain(..total_len);

        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let id = request.get("id").cloned();
        if id.is_none() {
            write_response(&mut stream, "202 Accepted", None).await?;
            continue;
        }
        let id = id.expect("checked above");

        let response = match method {
            "server/discover" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "Method not found"}
            }),
            "initialize" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "unix-test", "version": "1.0.0"}
                }
            }),
            "tools/list" => json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "unix_echo",
                        "description": "Unix socket echo",
                        "inputSchema": {
                            "type": "object",
                            "additionalProperties": false
                        }
                    }]
                }
            }),
            "tools/call" => {
                signals.tool_call.store(true, Ordering::SeqCst);
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{"type": "text", "text": "unix-ok"}],
                        "isError": false
                    }
                })
            }
            other => json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("unexpected method: {other}")}
            }),
        };
        write_response(&mut stream, "200 OK", Some(response)).await?;
    }
}

async fn serve_unix_mcp(listener: UnixListener, signals: RequestSignals) -> io::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let connection_signals = signals.clone();
        tokio::spawn(async move {
            handle_connection(stream, connection_signals)
                .await
                .expect("Unix MCP connection should remain valid");
        });
    }
}

async fn exercise_unix_socket(socket_path: &str, listener: UnixListener) {
    assert!(
        std::env::var("HOME")
            .ok()
            .is_some_and(|home| !home.trim().is_empty()),
        "HOME is required for the bearer-token integration assertion"
    );

    let signals = RequestSignals::new();
    let server = tokio::spawn(serve_unix_mcp(listener, signals.clone()));

    let mut config = test_upstream_config();
    config.name = "unix-test".to_string();
    config.transport = Some(UpstreamTransport::UnixSocket);
    config.socket_path = Some(socket_path.to_string());
    config.url = Some("http://local.internal/mcp".to_string());
    config.bearer_token_env = Some("HOME".to_string());
    config
        .headers
        .insert("x-labby-test".to_string(), "present".to_string());
    config.validate().expect("valid Unix upstream config");

    let (connection, tools) = connect_upstream(&config, None, None, None, None)
        .await
        .expect("Unix socket upstream should connect");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "unix_echo");

    let response = connection
        .peer
        .call_tool_once(CallToolRequestParams::new("unix_echo"))
        .await
        .expect("Unix socket tool call should succeed");
    assert!(
        matches!(&response, CallToolResponse::Complete(_)),
        "expected a complete tool response, got {response:?}"
    );
    let CallToolResponse::Complete(result) = response else {
        return;
    };
    assert_eq!(
        result.content[0]
            .as_text()
            .expect("text result")
            .text
            .as_str(),
        "unix-ok"
    );

    signals.assert_complete();
    drop(connection);
    server.abort();
}

#[tokio::test]
async fn filesystem_unix_socket_upstream_preserves_http_behavior() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let socket_path = tempdir.path().join("mcp.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind filesystem Unix socket");
    exercise_unix_socket(socket_path.to_string_lossy().as_ref(), listener).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn abstract_unix_socket_upstream_discovers_and_calls_tool() {
    let sequence = ABSTRACT_SOCKET_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let socket_path = format!("@labby-uds-{}-{sequence}", std::process::id());
    let name = socket_path.as_bytes().strip_prefix(b"@").unwrap();
    let mut address = Vec::with_capacity(name.len() + 1);
    address.push(0);
    address.extend_from_slice(name);
    let listener = UnixListener::bind(PathBuf::from(OsString::from_vec(address)))
        .expect("bind abstract Unix socket");
    assert_eq!(
        listener.local_addr().unwrap().as_abstract_name(),
        Some(name)
    );
    exercise_unix_socket(&socket_path, listener).await;
}
