//! Bounded ciphertext rendezvous; this capability never establishes host authority.
use anyhow::{Result, ensure};
use reqwest::{
    Client, Url,
    header::{HeaderMap, HeaderValue},
};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrowserRequest {
    pub version: u8,
    pub origin: String,
    pub peer: String,
    pub upstream: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prepared {
    id: String,
    request: BrowserRequest,
}

pub(crate) struct Exchange {
    client: Client,
    endpoint: Url,
    origin: String,
    id: String,
}

fn opaque(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}

impl Exchange {
    pub(crate) async fn new(origin: &str, id: &str, code: &str) -> Result<Self> {
        ensure!(
            opaque(id) && opaque(code),
            "invalid pairing exchange code or identifier"
        );
        let base = Url::parse(origin).map_err(|_| anyhow::anyhow!("invalid rendezvous origin"))?;
        ensure!(
            base.scheme() == "https"
                && base.username().is_empty()
                && base.password().is_none()
                && base.query().is_none()
                && base.fragment().is_none()
                && base.path() == "/",
            "rendezvous must be an HTTPS origin"
        );
        let host = base
            .host_str()
            .ok_or_else(|| anyhow::anyhow!("rendezvous host missing"))?;
        labby_primitives::ssrf::check_host_not_private(host)?;
        let port = base
            .port_or_known_default()
            .ok_or_else(|| anyhow::anyhow!("rendezvous port missing"))?;
        let peers: Vec<_> = tokio::time::timeout(
            Duration::from_secs(5),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| anyhow::anyhow!("rendezvous resolution timed out"))?
        .map_err(|_| anyhow::anyhow!("rendezvous host unavailable"))?
        .take(17)
        .collect();
        ensure!(
            !peers.is_empty() && peers.len() <= 16,
            "rendezvous address budget exceeded"
        );
        for peer in &peers {
            labby_primitives::ssrf::check_ip_not_private(peer.ip(), "Tailcat rendezvous")?;
        }
        let mut capability = HeaderValue::from_str(code)
            .map_err(|_| anyhow::anyhow!("invalid pairing exchange code"))?;
        capability.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert("X-Labby-Pairing-Code", capability);
        let client = Client::builder()
            .retry(reqwest::retry::never())
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .default_headers(headers)
            .resolve_to_addrs(host, &peers)
            .build()
            .map_err(|_| anyhow::anyhow!("rendezvous client unavailable"))?;
        let endpoint = base.join(&format!("ui/tailcat/pairings/{id}/exchange"))?;
        Ok(Self {
            client,
            endpoint,
            origin: base.origin().ascii_serialization(),
            id: id.into(),
        })
    }

    pub(crate) async fn fetch(&self) -> Result<BrowserRequest> {
        let mut response = self
            .client
            .get(self.endpoint.clone())
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("pairing request unavailable"))?;
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "pairing request denied or expired"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("pairing response unavailable"))?
        {
            ensure!(
                bytes.len().saturating_add(chunk.len()) <= 4096,
                "pairing response exceeds budget"
            );
            bytes.extend_from_slice(&chunk);
        }
        let prepared: Prepared = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid pairing response"))?;
        ensure!(
            prepared.id == self.id
                && prepared.request.version == 1
                && prepared.request.origin == self.origin,
            "pairing request origin or identifier mismatch"
        );
        Ok(prepared.request)
    }

    /// One attempt only. The caller retires the native session on any error.
    pub(crate) async fn deposit(&self, packet: &Value) -> Result<()> {
        validate_packet(packet)?;
        let bytes = serde_json::to_vec(packet)?;
        ensure!(
            bytes.len() <= 32 * 1024,
            "sealed delivery exceeds rendezvous budget"
        );
        let response = self
            .client
            .post(self.endpoint.clone())
            .header("Content-Type", "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| {
                anyhow::anyhow!("pairing delivery uncertain; native session must be stopped")
            })?;
        ensure!(
            response.status() == reqwest::StatusCode::NO_CONTENT,
            "pairing delivery denied; native session must be stopped"
        );
        Ok(())
    }
}

fn validate_packet(packet: &Value) -> Result<()> {
    use base64::Engine as _;
    let object = packet
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("invalid sealed delivery"))?;
    ensure!(
        object.len() == 3 && packet["version"] == 1,
        "invalid sealed delivery fields"
    );
    let sender = packet["sender"]
        .as_str()
        .and_then(|sender| sender.strip_prefix("nodekey:"))
        .ok_or_else(|| anyhow::anyhow!("invalid sealed sender"))?;
    ensure!(
        sender.len() == 64 && sender.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "invalid sealed sender"
    );
    let ciphertext = packet["ciphertext"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing sealed ciphertext"))?;
    ensure!(
        ciphertext.len() <= 24 * 1024,
        "sealed ciphertext exceeds budget"
    );
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(ciphertext)
        .map_err(|_| anyhow::anyhow!("invalid sealed ciphertext"))?;
    ensure!(
        (40..=16 * 1024 + 40).contains(&bytes.len()),
        "invalid sealed ciphertext size"
    );
    Ok(())
}

pub(crate) fn fingerprint(id: &str, request: &BrowserRequest) -> Result<String> {
    use sha2::{Digest as _, Sha256};
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&(
        "labby.tailcat.public-request/v1",
        id,
        &request.origin,
        &request.peer,
        &request.upstream,
    ))?)))
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn exchange_sends_header_capability_and_never_retries_uncertain_deposit() {
        use axum::{
            Router,
            http::{HeaderMap, StatusCode},
            routing::{get, post},
        };
        use base64::Engine as _;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let count = Arc::new(AtomicUsize::new(0));
        let counted = count.clone();
        let key = "a".repeat(43);
        let id = "b".repeat(43);
        let response_id = id.clone();
        let router = Router::new().route("/exchange", get(move |headers: HeaderMap| {
            let id = response_id.clone();
            async move {
                assert!(headers.get("x-labby-pairing-code").is_some());
                axum::Json(serde_json::json!({"id":id,"request":{"version":1,"origin":"https://depot.example","peer":format!("nodekey:{}", "c".repeat(64)),"upstream":"microsandbox"}}))
            }
        })).route("/exchange", post(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            async { StatusCode::INTERNAL_SERVER_ERROR }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let mut headers = HeaderMap::new();
        let mut capability = reqwest::header::HeaderValue::from_str(&key).unwrap();
        capability.set_sensitive(true);
        headers.insert("x-labby-pairing-code", capability);
        // Test-only HTTP injection; production construction requires pinned public HTTPS.
        let exchange = super::Exchange {
            client: reqwest::Client::builder()
                .no_proxy()
                .retry(reqwest::retry::never())
                .timeout(std::time::Duration::from_secs(2))
                .default_headers(headers)
                .build()
                .unwrap(),
            endpoint: format!("http://{address}/exchange").parse().unwrap(),
            origin: "https://depot.example".into(),
            id,
        };
        assert_eq!(exchange.fetch().await.unwrap().upstream, "microsandbox");
        let packet = serde_json::json!({"version":1,"sender":format!("nodekey:{}", "a".repeat(64)),"ciphertext":base64::engine::general_purpose::STANDARD.encode([0_u8;40])});
        assert!(exchange.deposit(&packet).await.is_err());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        task.abort();
        let _stopped = task.await;
    }
    #[test]
    fn publication_rejects_plaintext_and_additional_fields() {
        use base64::Engine as _;
        let packet = serde_json::json!({"version":1,"sender":format!("nodekey:{}", "a".repeat(64)),"ciphertext":base64::engine::general_purpose::STANDARD.encode([0_u8;40])});
        assert!(super::validate_packet(&packet).is_ok());
        let mut with_secret = packet;
        with_secret["grant"] = serde_json::json!("must not be published");
        assert!(super::validate_packet(&with_secret).is_err());
        assert!(super::validate_packet(&serde_json::json!({"grant":"plaintext"})).is_err());
    }
    #[tokio::test]
    async fn rejects_unsafe_origin_and_malformed_capability_before_network() {
        let key = "a".repeat(43);
        for origin in [
            "http://example.com",
            "https://example.com/?secret=value",
            "https://user:secret@example.com",
            "https://localhost",
            "https://127.0.0.1",
        ] {
            assert!(super::Exchange::new(origin, &key, &key).await.is_err());
        }
        assert!(
            super::Exchange::new("https://example.com", "invalid", &key)
                .await
                .is_err()
        );
    }
}
