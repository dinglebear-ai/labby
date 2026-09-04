//! Bounded server-side transport for the optional Depot control plane.

use std::{env, sync::Arc, time::Duration};

use reqwest::{Client, StatusCode, Url};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const QUEUE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_INTERACTIVE_REQUESTS: usize = 16;

#[derive(Clone)]
pub struct DepotClient {
    http: Client,
    base_url: Option<Url>,
    token: Option<Arc<str>>,
    enabled: bool,
    interactive: Arc<Semaphore>,
    queue_timeout: Duration,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepotStatus {
    pub configured: bool,
    pub enabled: bool,
    pub mutation_authority: bool,
    pub max_response_bytes: usize,
}

#[derive(Debug)]
pub enum DepotError {
    Disabled,
    Unconfigured,
    UnsupportedOperation,
    Upstream(StatusCode, Value),
    QueueTimeout,
    Unavailable(TransportFailure),
    ResponseTooLarge,
    InvalidResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportFailure {
    Connect,
    Timeout,
    Request,
    ResponseBody,
}

impl TransportFailure {
    const fn category(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Timeout => "timeout",
            Self::Request => "request",
            Self::ResponseBody => "response_body",
        }
    }
}

impl DepotClient {
    #[must_use]
    pub fn from_env() -> Self {
        let enabled = env::var("LABBY_DEPOT_ENABLED").is_ok_and(|value| value == "1");
        let base_url =
            env::var("LABBY_DEPOT_URL")
                .ok()
                .and_then(|value| match parse_base_url(&value) {
                    Ok(url) => Some(url),
                    Err(_) => {
                        tracing::warn!(
                            category = "invalid_base_url",
                            variable = "LABBY_DEPOT_URL",
                            "Depot configuration rejected"
                        );
                        None
                    }
                });
        let token = env::var("LABBY_DEPOT_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(Arc::from);
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Depot HTTP client configuration is valid");
        Self {
            http,
            base_url,
            token,
            enabled,
            interactive: Arc::new(Semaphore::new(MAX_INTERACTIVE_REQUESTS)),
            queue_timeout: QUEUE_TIMEOUT,
        }
    }

    #[must_use]
    pub fn status(&self) -> DepotStatus {
        DepotStatus {
            configured: self.base_url.is_some() && self.token.is_some(),
            enabled: self.enabled,
            // A configured shared service credential is read-only. Mutation
            // authority requires negotiated actor/epoch/capability support,
            // which this first compatibility slice does not yet implement.
            mutation_authority: false,
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }

    pub async fn session(&self, actor: &str) -> Result<Value, DepotError> {
        self.request(reqwest::Method::GET, "api/session", None, actor)
            .await
    }

    pub async fn operations(&self, actor: &str) -> Result<Value, DepotError> {
        self.request(reqwest::Method::GET, "api/operations", None, actor)
            .await
    }

    pub async fn call(
        &self,
        operation: &str,
        params: Value,
        actor: &str,
    ) -> Result<Value, DepotError> {
        if !allowed_operation(operation) {
            return Err(DepotError::UnsupportedOperation);
        }
        self.request(
            reqwest::Method::POST,
            &format!("api/operations/{operation}"),
            Some(params),
            actor,
        )
        .await
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        actor: &str,
    ) -> Result<Value, DepotError> {
        if !self.enabled {
            return Err(DepotError::Disabled);
        }
        let _permit = tokio::time::timeout(self.queue_timeout, self.interactive.acquire())
            .await
            .map_err(|_| {
                tracing::warn!(category = "queue_timeout", "Depot request rejected");
                DepotError::QueueTimeout
            })?
            .map_err(|_| DepotError::Unavailable(TransportFailure::Request))?;
        let base = self.base_url.as_ref().ok_or(DepotError::Unconfigured)?;
        let token = self.token.as_ref().ok_or(DepotError::Unconfigured)?;
        let url = base.join(path).map_err(|_| DepotError::Unconfigured)?;
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(token.as_ref())
            .header("accept", "application/json")
            .header("x-labby-actor", actor);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| {
            let category = if error.is_timeout() {
                TransportFailure::Timeout
            } else if error.is_connect() {
                TransportFailure::Connect
            } else {
                TransportFailure::Request
            };
            tracing::warn!(category = category.category(), "Depot transport failed");
            DepotError::Unavailable(category)
        })?;
        decode_response(response).await
    }
}

fn parse_base_url(value: &str) -> Result<Url, ()> {
    let url = Url::parse(&format!("{}/", value.trim_end_matches('/'))).map_err(|_| ())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(());
    }
    Ok(url)
}

async fn decode_response(mut response: reqwest::Response) -> Result<Value, DepotError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(DepotError::ResponseTooLarge);
    }
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        tracing::warn!(category = "response_body", "Depot transport failed");
        DepotError::Unavailable(TransportFailure::ResponseBody)
    })? {
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(DepotError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| DepotError::InvalidResponse)?;
    if status.is_success() {
        Ok(value)
    } else {
        Err(DepotError::Upstream(status, value))
    }
}

fn allowed_operation(operation: &str) -> bool {
    matches!(operation, "depot.artifacts.list" | "depot.artifacts.get")
}

pub fn error_body(error: &DepotError) -> Value {
    match error {
        DepotError::Upstream(status, body) => {
            json!({"error":"depot_rejected","status":status.as_u16(),"detail":body})
        }
        DepotError::Disabled => json!({"error":"depot_disabled"}),
        DepotError::Unconfigured => json!({"error":"depot_unconfigured"}),
        DepotError::UnsupportedOperation => json!({"error":"unsupported_operation"}),
        DepotError::QueueTimeout => json!({"error":"depot_busy"}),
        DepotError::Unavailable(_) => json!({"error":"depot_unavailable"}),
        DepotError::ResponseTooLarge => json!({"error":"depot_response_too_large"}),
        DepotError::InvalidResponse => json!({"error":"invalid_depot_response"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client(base_url: Url, permits: usize, queue_timeout: Duration) -> DepotClient {
        drop(rustls::crypto::ring::default_provider().install_default());
        DepotClient {
            http: Client::builder()
                // Loopback refusal can outlast 250 ms on Windows. This fixture
                // verifies connect classification, not the request timeout.
                .timeout(Duration::from_secs(5))
                .no_proxy()
                .build()
                .unwrap(),
            base_url: Some(base_url),
            token: Some(Arc::from("test-token")),
            enabled: true,
            interactive: Arc::new(Semaphore::new(permits)),
            queue_timeout,
        }
    }

    #[test]
    fn operation_allowlist_excludes_generic_and_destructive_unknowns() {
        assert!(allowed_operation("depot.artifacts.list"));
        assert!(allowed_operation("depot.artifacts.get"));
        assert!(!allowed_operation("depot.ingest.start"));
        assert!(!allowed_operation("depot.artifacts.delete"));
        assert!(!allowed_operation("depot.admin.execute"));
    }

    #[test]
    fn transport_errors_do_not_disclose_credentials_or_urls() {
        let body = error_body(&DepotError::Unavailable(TransportFailure::Connect)).to_string();
        assert_eq!(body, r#"{"error":"depot_unavailable"}"#);
        assert!(!body.contains("token"));
    }

    #[test]
    fn depot_url_requires_an_http_origin() {
        assert!(parse_base_url("https://depot.example.test").is_ok());
        assert!(parse_base_url("https://user:password@depot.example.test").is_err());
        assert!(parse_base_url("https://depot.example.test?token=secret").is_err());
        assert!(parse_base_url("file:///tmp/depot-token").is_err());
        assert!(parse_base_url("not a url").is_err());
    }

    #[tokio::test]
    async fn interactive_queue_wait_is_bounded() {
        let client = test_client(
            Url::parse("http://127.0.0.1:9/").unwrap(),
            1,
            Duration::from_millis(20),
        );
        let _held = client.interactive.acquire().await.unwrap();

        let error = client.session("actor").await.unwrap_err();
        assert!(matches!(error, DepotError::QueueTimeout));
    }

    #[tokio::test]
    async fn connection_failure_retains_sanitized_category() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client = test_client(
            Url::parse(&format!("http://{address}/")).unwrap(),
            1,
            Duration::from_secs(2),
        );

        let error = client.session("actor").await.unwrap_err();
        assert!(
            matches!(error, DepotError::Unavailable(TransportFailure::Connect)),
            "expected a connection failure, got {error:?}"
        );
        assert_eq!(error_body(&error), json!({"error":"depot_unavailable"}));
    }
}
