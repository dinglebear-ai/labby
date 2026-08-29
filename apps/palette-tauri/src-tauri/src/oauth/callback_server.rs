//! Loopback HTTP listener that captures the OAuth `?code&state` redirect.
//! RFC 8252 §7.3 native-app pattern: bind loopback on an ephemeral port,
//! register that port as a loopback `redirect_uri`, then accept browser requests
//! until one carries the matching state. A non-matching request (favicon, a
//! racing local process with a wrong state) is answered and ignored — only a
//! state-matching code/error ends the loop — so a hostile local request cannot
//! abort a legitimate login.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinSet;

const MAX_REQUEST_BYTES: usize = 8192;
const MAX_REQUEST_TARGET_BYTES: usize = 4096;
const CONNECTION_READ_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CONCURRENT_CONNECTIONS: usize = 8;

const SUCCESS_PAGE: &str = "<!doctype html><html><body style=\"font-family:sans-serif;background:#07131c;color:#e6f4fb;\
     text-align:center;padding-top:4rem\"><h2>Signed in to Labby</h2>\
     <p>You can close this tab and return to the palette.</p></body></html>";

const ERROR_PAGE: &str = "<!doctype html><html><body style=\"font-family:sans-serif;background:#07131c;color:#e6f4fb;\
     text-align:center;padding-top:4rem\"><h2>Sign-in failed</h2>\
     <p>Authorization was denied or could not complete. Return to the palette and try again.</p></body></html>";

pub(crate) struct CallbackListener {
    listener: TcpListener,
    pub redirect_uri: String,
}

pub(crate) struct CallbackParams {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

/// Bind a loopback listener on an ephemeral port. The `redirect_uri` string is
/// fixed here and must be reused verbatim for `/register`, `/authorize`, and
/// `/token`.
pub(crate) async fn bind() -> Result<CallbackListener, String> {
    let listener = TcpListener::bind(("localhost", 0))
        .await
        .map_err(|err| format!("failed to bind loopback callback listener: {err}"))?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    // Chrome HTTPS upgrade modes can attempt TLS for IP-literal loopback URLs
    // (`https://127.0.0.1:...`), which fails against this intentionally-plain
    // native-app HTTP listener. `localhost` remains a loopback redirect URI but
    // is treated as a trustworthy local origin by browsers.
    let redirect_uri = format!("http://localhost:{port}/callback");
    Ok(CallbackListener {
        listener,
        redirect_uri,
    })
}

impl CallbackListener {
    /// Accept connections until one carries the OAuth redirect with the matching
    /// `state`, returning the authorization `code`. Times out after `timeout`.
    pub(crate) async fn await_code(
        &self,
        expected_state: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        tokio::time::timeout(timeout, self.accept_loop(expected_state))
            .await
            .map_err(|_| "timed out waiting for the OAuth redirect".to_string())?
    }

    async fn accept_loop(&self, expected_state: &str) -> Result<String, String> {
        let permits = std::sync::Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));
        let (result_tx, mut result_rx) = mpsc::channel(1);
        let mut handlers = JoinSet::new();
        loop {
            tokio::select! {
                result = result_rx.recv() => {
                    handlers.abort_all();
                    while handlers.join_next().await.is_some() {}
                    return result.expect("callback result sender remains alive");
                }
                accepted = async {
                    let permit = permits.clone().acquire_owned().await;
                    let accepted = self.listener.accept().await;
                    (accepted, permit)
                } => {
                    let (accepted, permit) = accepted;
                    let permit = permit.map_err(|_| "OAuth callback handler pool closed".to_string())?;
                    let (socket, _) = accepted.map_err(|err| err.to_string())?;
                    let expected_state = expected_state.to_string();
                    let result_tx = result_tx.clone();
                    handlers.spawn(async move {
                        let _permit = permit;
                        handle_connection(socket, &expected_state, result_tx).await;
                    });
                }
                Some(_) = handlers.join_next(), if !handlers.is_empty() => {}
            }
        }
    }
}

async fn handle_connection(
    mut socket: TcpStream,
    expected_state: &str,
    result_tx: mpsc::Sender<Result<String, String>>,
) {
    let Some(target) = read_request_target(&mut socket).await else {
        respond(&mut socket, "400 Bad Request", "Bad Request").await;
        return;
    };
    // Only requests to the registered callback path bearing OUR state
    // are the real callback. Anything else (favicon, a racing process
    // with a wrong/absent state) is answered and ignored so it cannot
    // abort the flow.
    let path = target.split('?').next().unwrap_or(&target);
    if path != "/callback" {
        respond(&mut socket, "404 Not Found", "Not Found").await;
        return;
    }
    let params = parse_callback_params(&target);
    if params.state.as_deref() != Some(expected_state) {
        respond(&mut socket, "404 Not Found", "Not Found").await;
        return;
    }
    if let Some(error) = params.error {
        respond(&mut socket, "400 Bad Request", ERROR_PAGE).await;
        let _ = result_tx
            .send(Err(format!("authorization was denied ({error})")))
            .await;
        return;
    }
    if let Some(code) = params.code {
        respond(&mut socket, "200 OK", SUCCESS_PAGE).await;
        let _ = result_tx.send(Ok(code)).await;
        return;
    }
    respond(&mut socket, "400 Bad Request", "Missing code").await;
}

async fn read_request_target(socket: &mut TcpStream) -> Option<String> {
    tokio::time::timeout(CONNECTION_READ_TIMEOUT, async {
        let mut request = Vec::with_capacity(1024);
        let mut chunk = [0_u8; 1024];
        loop {
            let n = socket.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            if request.len().saturating_add(n) > MAX_REQUEST_BYTES {
                return None;
            }
            request.extend_from_slice(&chunk[..n]);
            if request.windows(4).any(|window| window == b"\r\n\r\n")
                || request.windows(2).any(|window| window == b"\n\n")
            {
                let head = String::from_utf8_lossy(&request);
                let request_line = head.lines().next()?;
                return parse_request_target(request_line).map(str::to_string);
            }
        }
    })
    .await
    .ok()
    .flatten()
}

async fn respond(socket: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.flush().await;
    let _ = socket.shutdown().await;
}

/// Extract the request target (path + query) from an HTTP request line.
pub(crate) fn parse_request_target(request_line: &str) -> Option<&str> {
    let mut parts = request_line.split_whitespace();
    let _method = parts.next()?;
    let target = parts.next()?;
    (target.starts_with('/') && target.len() <= MAX_REQUEST_TARGET_BYTES).then_some(target)
}

/// Parse `code`/`state`/`error` from a `/callback?...` target.
pub(crate) fn parse_callback_params(target: &str) -> CallbackParams {
    let mut params = CallbackParams {
        code: None,
        state: None,
        error: None,
    };
    if let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) {
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "code" => params.code = Some(value.into_owned()),
                "state" => params.state = Some(value.into_owned()),
                "error" => params.error = Some(value.into_owned()),
                _ => {}
            }
        }
    }
    params
}

#[cfg(test)]
#[path = "callback_server_tests.rs"]
mod tests;
