//! Bounded HTTPS delivery. No callback URL, body, or key is logged here.
use std::net::{IpAddr, SocketAddr, ToSocketAddrs as _};
use std::sync::Arc;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::future::BoxFuture;
use hmac::{Hmac, KeyInit as _, Mac as _};
use reqwest::{StatusCode, header};
use sha2::Sha256;
use subtle::ConstantTimeEq as _;
use tokio::sync::Semaphore;
use url::Url;

use super::super::codemode_notices::now_ms;
use super::types::{EventError, WebhookSecret, parse_callback};

const OPERATION_DEADLINE: Duration = Duration::from_secs(10);
const NETWORK_STEP_DEADLINE: Duration = Duration::from_secs(5);
const MAX_VERIFY_BYTES: usize = 8_192;
const MAX_PAYLOAD_BYTES: usize = 262_144;

pub(super) type OfferWitness = Box<dyn FnOnce() -> BoxFuture<'static, std::io::Result<()>> + Send>;

pub(super) struct WebhookAttempt {
    pub url: Url,
    pub subscription_id: String,
    pub event_id: String,
    pub body: Vec<u8>,
    pub key: WebhookSecret,
    pub previous_key: Option<(WebhookSecret, i64)>,
    /// Persist immediately before the connected HTTP transport reads the body.
    pub offer: Option<OfferWitness>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DeliveryOutcome {
    Received,
    Retry { after_ms: i64 },
    Stop,
    Gone,
}

pub(super) trait WebhookSender: Send + Sync {
    fn verify<'a>(
        &'a self,
        url: &'a Url,
        id: &'a str,
        key: &'a WebhookSecret,
    ) -> BoxFuture<'a, Result<(), EventError>>;
    fn send(&self, attempt: WebhookAttempt) -> BoxFuture<'_, DeliveryOutcome>;
}

pub(super) struct HttpsWebhookSender {
    resolver_slots: Arc<Semaphore>,
    #[cfg(test)]
    connector: Option<reqwest::Client>,
}

impl Default for HttpsWebhookSender {
    fn default() -> Self {
        Self {
            resolver_slots: Arc::new(Semaphore::new(8)),
            #[cfg(test)]
            connector: None,
        }
    }
}

impl HttpsWebhookSender {
    async fn client(&self, url: &Url) -> Result<reqwest::Client, EventError> {
        #[cfg(test)]
        if let Some(client) = &self.connector {
            return Ok(client.clone());
        }
        // Revalidate stored destinations and DNS on every attempt, including
        // verification. The HTTP library receives only the checked DNS answers.
        parse_callback(url.as_str())?;
        let mut builder = client_builder();
        let port = url
            .port_or_known_default()
            .ok_or(EventError::Callback("invalid_url"))?;
        match url.host() {
            Some(url::Host::Ipv4(ip)) => validate_addresses(&[SocketAddr::from((ip, port))])?,
            Some(url::Host::Ipv6(ip)) => validate_addresses(&[SocketAddr::from((ip, port))])?,
            _ => {}
        }
        if let Some(url::Host::Domain(domain)) = url.host() {
            let name = domain.to_owned();
            let permit = Arc::clone(&self.resolver_slots)
                .try_acquire_owned()
                .map_err(|_| EventError::Callback("busy"))?;
            // The blocking resolver retains its permit even if the waiter times
            // out, so stalled DNS cannot create an unbounded job queue.
            let addresses = tokio::time::timeout(
                NETWORK_STEP_DEADLINE,
                tokio::task::spawn_blocking(move || {
                    let _permit = permit;
                    (name.as_str(), port)
                        .to_socket_addrs()
                        .map(|addresses| addresses.take(65).collect::<Vec<_>>())
                }),
            )
            .await
            .map_err(|_| EventError::Callback("timeout"))?
            .map_err(|_| EventError::Callback("dns_failed"))?
            .map_err(|_| EventError::Callback("dns_failed"))?;
            validate_addresses(&addresses)?;
            builder = builder.dns_resolver(Arc::new(PinnedDns {
                domain: domain.to_owned(),
                addresses,
            }));
        }
        builder
            .build()
            .map_err(|_| EventError::Callback("transport_error"))
    }

    async fn post(&self, attempt: WebhookAttempt) -> Result<reqwest::Response, EventError> {
        if attempt.body.len() > MAX_PAYLOAD_BYTES {
            return Err(EventError::Callback("payload_too_large"));
        }
        let client = self.client(&attempt.url).await?;
        let timestamp = now_ms() / 1_000;
        let signature = sign(&attempt, timestamp);
        let mut signature = header::HeaderValue::from_str(&signature)
            .map_err(|_| EventError::Callback("transport_error"))?;
        signature.set_sensitive(true);
        let request = client
            .post(attempt.url)
            .header(header::CONTENT_TYPE, "application/json")
            .header("webhook-id", attempt.event_id)
            .header("webhook-timestamp", timestamp.to_string())
            .header("webhook-signature", signature)
            .header("x-mcp-subscription-id", attempt.subscription_id);
        let body = attempt.body;
        let request = if let Some(offer) = attempt.offer {
            let length = body.len();
            // Hyper polls the body on the connected request. A durable offer
            // precedes receiver processing, but DNS/TLS failures never set it.
            // If the generation or lease changed, abort before exposing bytes.
            let body = reqwest::Body::wrap_stream(futures::stream::once(async move {
                offer().await?;
                Ok::<_, std::io::Error>(body)
            }));
            request
                .header(header::CONTENT_LENGTH, length.to_string())
                .body(body)
        } else {
            request.body(body)
        };
        request.send().await.map_err(|error| {
            EventError::Callback(if error.is_timeout() {
                "timeout"
            } else {
                "transport_error"
            })
        })
    }
}

/// The callback client has no fallback DNS path. A different name must
/// never resolve to a destination outside the validated address set.
struct PinnedDns {
    domain: String,
    addresses: Vec<SocketAddr>,
}

impl reqwest::dns::Resolve for PinnedDns {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        if name.as_str().eq_ignore_ascii_case(&self.domain) {
            let addresses: reqwest::dns::Addrs = Box::new(self.addresses.clone().into_iter());
            Box::pin(async { Ok(addresses) })
        } else {
            Box::pin(async { Err(std::io::Error::other("unvalidated callback hostname").into()) })
        }
    }
}

fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .referer(false)
        .connect_timeout(NETWORK_STEP_DEADLINE)
        .timeout(OPERATION_DEADLINE)
}

fn validate_addresses(addresses: &[SocketAddr]) -> Result<(), EventError> {
    if addresses.is_empty() || addresses.len() > 64 {
        return Err(EventError::Callback("dns_failed"));
    }
    for address in addresses {
        let ip = address.ip();
        labby_primitives::ssrf::check_ip_not_private(ip, "MCP event callback")
            .map_err(|_| EventError::Callback("destination_blocked"))?;
        // Only ordinary global-unicast IPv6 or a validated IPv4 mapping.
        // Exclude transition/protocol ranges that can relay to a different IP.
        if let IpAddr::V6(ip) = ip
            && ip.to_ipv4_mapped().is_none()
        {
            let [first, second, ..] = ip.segments();
            if first & 0xe000 != 0x2000
                || (first == 0x2001 && second < 0x0200)
                || first & 0xfff0 == 0x3ff0
            {
                return Err(EventError::Callback("destination_blocked"));
            }
        }
    }
    Ok(())
}

fn sign_webhook(secret: &WebhookSecret, id: &str, time: i64, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.bytes()).expect("HMAC accepts variable keys");
    mac.update(format!("{id}.{time}.").as_bytes());
    mac.update(body);
    format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()))
}

fn sign(attempt: &WebhookAttempt, time: i64) -> String {
    let mut header = sign_webhook(&attempt.key, &attempt.event_id, time, &attempt.body);
    if let Some((previous, until)) = &attempt.previous_key
        && now_ms() < *until
    {
        header.push(' ');
        header.push_str(&sign_webhook(
            previous,
            &attempt.event_id,
            time,
            &attempt.body,
        ));
    }
    header
}

impl WebhookSender for HttpsWebhookSender {
    fn verify<'a>(
        &'a self,
        url: &'a Url,
        id: &'a str,
        key: &'a WebhookSecret,
    ) -> BoxFuture<'a, Result<(), EventError>> {
        Box::pin(async move {
            tokio::time::timeout(OPERATION_DEADLINE, async {
                let mut bytes = [0; 32];
                getrandom::fill(&mut bytes)
                    .map_err(|_| EventError::Callback("verification_unavailable"))?;
                let challenge = hex::encode(bytes);
                let body = serde_json::to_vec(&serde_json::json!({
                    "type":"verification", "challenge":challenge
                }))
                .map_err(|_| EventError::Invalid)?;
                let mut response = self
                    .post(WebhookAttempt {
                        url: url.clone(),
                        subscription_id: id.to_owned(),
                        event_id: format!("msg_verification_{}", ulid::Ulid::new()),
                        body,
                        key: key.clone(),
                        previous_key: None,
                        offer: None,
                    })
                    .await?;
                if !response.status().is_success() {
                    return Err(EventError::Callback("challenge_failed"));
                }
                let mut response_body = Vec::new();
                while let Some(chunk) = response.chunk().await.map_err(|error| {
                    EventError::Callback(if error.is_timeout() {
                        "timeout"
                    } else {
                        "challenge_failed"
                    })
                })? {
                    if chunk.len() > MAX_VERIFY_BYTES - response_body.len() {
                        return Err(EventError::Callback("response_too_large"));
                    }
                    response_body.extend_from_slice(&chunk);
                }
                #[derive(serde::Deserialize)]
                struct Echo {
                    challenge: String,
                }
                let echo: Echo = serde_json::from_slice(&response_body)
                    .map_err(|_| EventError::Callback("challenge_failed"))?;
                if !bool::from(echo.challenge.as_bytes().ct_eq(challenge.as_bytes())) {
                    return Err(EventError::Callback("challenge_failed"));
                }
                Ok(())
            })
            .await
            .map_err(|_| EventError::Callback("timeout"))?
        })
    }

    fn send(&self, attempt: WebhookAttempt) -> BoxFuture<'_, DeliveryOutcome> {
        Box::pin(async move {
            match tokio::time::timeout(OPERATION_DEADLINE, self.post(attempt)).await {
                Ok(Ok(response)) => {
                    let status = response.status();
                    if status.is_success() {
                        DeliveryOutcome::Received
                    } else if status == StatusCode::GONE {
                        DeliveryOutcome::Gone
                    } else if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
                        let after_ms = response
                            .headers()
                            .get(header::RETRY_AFTER)
                            .and_then(|value| value.to_str().ok())
                            .map_or(0, retry_after_ms);
                        DeliveryOutcome::Retry { after_ms }
                    } else {
                        DeliveryOutcome::Stop
                    }
                }
                Ok(Err(EventError::Callback(
                    "invalid_url" | "destination_blocked" | "payload_too_large",
                ))) => DeliveryOutcome::Stop,
                _ => DeliveryOutcome::Retry { after_ms: 0 },
            }
        })
    }
}

fn retry_after_ms(value: &str) -> i64 {
    if let Ok(seconds) = value.parse::<u64>() {
        return seconds.min(300) as i64 * 1_000;
    }
    // HTTP-date, as used by Retry-After. A bad date means normal backoff.
    let Some(date) = value.strip_suffix(" GMT") else {
        return 0;
    };
    jiff::Timestamp::strptime("%a, %d %b %Y %H:%M:%S %z", format!("{date} +0000")).map_or(
        0,
        |time| {
            time.as_millisecond()
                .saturating_sub(now_ms())
                .clamp(0, 300_000)
        },
    )
}

#[cfg(test)]
pub(super) mod tests;
