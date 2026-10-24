//! Bounded Team credential binding used by disposable daemon fixtures.
use super::sanitize;
use std::time::Duration;

pub(super) async fn bind(
    base_url: &str,
    credential: &str,
    team_id: &str,
    upstream: &str,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())?;
    tokio::time::timeout_at(deadline, async {
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(format!(
                    "team credential binding for {upstream} exceeded deadline"
                ));
            }
            let mut response = client
                .post(format!("{base_url}/v1/access/admin"))
                .bearer_auth(credential)
                .header("content-type", "application/json")
                .json(&serde_json::json!({
                    "action": "access.gateway_credential.bind",
                    "params": {
                        "team_id": team_id,
                        "upstream_name": upstream,
                        "binding_id": format!("{upstream}-binding"),
                    },
                }))
                .send()
                .await
                .map_err(|error| error.to_string())?;
            if response.status().is_success() {
                return Ok(());
            }
            let status = response.status();
            // Stop receiving diagnostics at the cap. The shared deadline also
            // covers stalled headers and a peer that never finishes its body.
            let mut bytes = Vec::with_capacity(16 * 1024);
            let body = loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        let remaining = (16 * 1024_usize).saturating_sub(bytes.len());
                        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                        if bytes.len() == 16 * 1024 {
                            break sanitize(&String::from_utf8_lossy(&bytes));
                        }
                    }
                    Ok(None) => break sanitize(&String::from_utf8_lossy(&bytes)),
                    Err(error) => break format!("<response body unavailable: {error}>"),
                }
            };
            if status != reqwest::StatusCode::SERVICE_UNAVAILABLE {
                return Err(format!(
                    "team credential binding for {upstream} failed: {status}: {body}"
                ));
            }
            tokio::time::sleep_until(
                deadline.min(tokio::time::Instant::now() + Duration::from_millis(50)),
            )
            .await;
        }
    })
    .await
    .map_err(|_| format!("team credential binding for {upstream} exceeded deadline"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn stalled_binding(response: &'static [u8]) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket.write_all(response).await.unwrap();
            std::future::pending::<()>().await;
        });
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            bind(
                &format!("http://{address}"),
                "fixture-canary",
                "team",
                "upstream",
                tokio::time::Instant::now() + Duration::from_millis(50),
            ),
        )
        .await;
        peer.abort();
        drop(peer.await);
        assert!(result.is_ok(), "fixture escaped its aggregate deadline");
        assert!(result.unwrap().unwrap_err().contains("deadline"));
    }

    #[tokio::test]
    async fn team_binding_bounds_stalled_headers() {
        stalled_binding(b"").await;
    }

    #[tokio::test]
    async fn team_binding_bounds_stalled_error_body() {
        stalled_binding(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 100\r\n\r\nx").await;
    }
    #[tokio::test]
    async fn team_binding_caps_diagnostics_before_reading_whole_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            assert!(socket.read(&mut request).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 1000000\r\n\r\n")
                .await
                .unwrap();
            socket.write_all(&vec![b'x'; 17 * 1024]).await.unwrap();
            std::future::pending::<()>().await;
        });
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            bind(
                &format!("http://{address}"),
                "fixture-canary",
                "team",
                "upstream",
                tokio::time::Instant::now() + Duration::from_secs(2),
            ),
        )
        .await;
        peer.abort();
        drop(peer.await);
        let error = result
            .expect("must stop reading at diagnostic cap")
            .unwrap_err();
        assert!(error.contains("400 Bad Request"));
        assert!(error.len() <= 16 * 1024 + 200);
    }

    #[tokio::test]
    async fn team_binding_retries_unavailable_then_succeeds() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            for status in ["503 Service Unavailable", "200 OK"] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            bind(
                &format!("http://{address}"),
                "fixture-canary",
                "team",
                "upstream",
                tokio::time::Instant::now() + Duration::from_secs(1),
            ),
        )
        .await;
        peer.abort();
        drop(peer.await);
        result
            .expect("binding stays bounded")
            .expect("retry succeeds");
    }
}
