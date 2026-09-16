use std::{env, time::Duration};

use reqwest::{Client, StatusCode, Url};
use serde_json::{Value, json};

use crate::dispatch::error::ToolError;

pub(crate) const BASE_URL_ENV: &str = "LABBY_PHOENIX_OPENAI_BASE_URL";
pub(crate) const API_KEY_ENV: &str = "LABBY_PHOENIX_OPENAI_API_KEY";
const REQUEST_TIMEOUT: Duration = Duration::from_mins(5);
const SESSION_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;
// A 16 MiB UTF-8 completion can expand close to 6x when JSON-escaped. Keep
// provider responses bounded without rejecting the runtime's valid output range.
const MAX_SUCCESS_BODY_BYTES: usize = 100 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct OpenAiBackend {
    http: Client,
    base_url: Url,
    api_key: Option<String>,
}

/// One chat turn. Pinned Agent instructions travel as `System` and caller
/// input as `User`, so the provider's role boundary keeps the caller from
/// rewriting the revision inside a shared prompt.
#[derive(Clone, Copy, Debug)]
pub(crate) enum ChatMessage<'a> {
    System(&'a str),
    User(&'a str),
}

impl ChatMessage<'_> {
    fn to_value(self) -> Value {
        match self {
            Self::System(content) => json!({"role": "system", "content": content}),
            Self::User(content) => json!({"role": "user", "content": content}),
        }
    }
}

/// Process-wide provider base URL for unit tests. The crate forbids unsafe
/// code and `std::env::set_var` is unsafe in edition 2024, so tests that need
/// a resolvable harness digest pin the URL here instead of mutating the
/// environment. Nothing connects to it unless a test drives execution.
#[cfg(test)]
static TEST_BASE_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Pin the provider base URL for every later `from_env` in this process.
/// Building the HTTP client needs a process-level TLS provider, which the
/// test binary does not install on its own.
#[cfg(test)]
pub(crate) fn install_test_base_url(url: &str) {
    drop(rustls::crypto::ring::default_provider().install_default());
    drop(TEST_BASE_URL.set(url.to_owned()));
}

impl OpenAiBackend {
    pub(crate) fn from_env() -> Option<Self> {
        #[cfg(test)]
        if let Some(base_url) = TEST_BASE_URL.get() {
            return Self::from_url(base_url, None).ok();
        }
        let base_url = env::var(BASE_URL_ENV).ok()?;
        match Self::from_url(&base_url, env::var(API_KEY_ENV).ok()) {
            Ok(backend) => Some(backend),
            Err(error) => {
                tracing::warn!(
                    variable = BASE_URL_ENV,
                    error = %error,
                    "Phoenix OpenAI-compatible endpoint configuration rejected"
                );
                None
            }
        }
    }

    pub(crate) fn from_url(base_url: &str, api_key: Option<String>) -> Result<Self, ToolError> {
        let mut base_url = Url::parse(base_url).map_err(|_| invalid_endpoint())?;
        if !matches!(base_url.scheme(), "http" | "https")
            || !base_url.has_host()
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(invalid_endpoint());
        }
        let path = base_url.path().trim_end_matches('/');
        let normalized_path = if path.is_empty() {
            "/".to_owned()
        } else {
            format!("{path}/")
        };
        base_url.set_path(&normalized_path);
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|error| {
                unavailable(format!("failed to build Phoenix HTTP client: {error}"))
            })?;
        Ok(Self {
            http,
            base_url,
            api_key: api_key.and_then(|value| {
                let value = value.trim();
                (!value.is_empty()).then(|| value.to_owned())
            }),
        })
    }

    pub(crate) fn base_url(&self) -> &Url {
        &self.base_url
    }

    pub(crate) async fn models(&self) -> Result<Vec<Value>, ToolError> {
        let value = self.get_json("models").await?;
        value
            .get("data")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| protocol("OpenAI-compatible /models response did not contain data[]"))
    }

    pub(crate) async fn create_session(&self, session_id: &str) -> Result<(), ToolError> {
        self.send_session_json(
            reqwest::Method::POST,
            "sessions",
            Some(json!({"id": session_id, "url": "https://chatgpt.com/"})),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn close_session(&self, session_id: &str) -> Result<(), ToolError> {
        let path = format!("sessions/{}/close", encode_path_segment(session_id));
        self.send_session_json(reqwest::Method::POST, &path, Some(json!({})))
            .await?;
        Ok(())
    }

    pub(crate) async fn rename_session(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<(), ToolError> {
        let path = format!("sessions/{}/title", encode_path_segment(session_id));
        self.send_session_json(reqwest::Method::PUT, &path, Some(json!({"title": title})))
            .await?;
        Ok(())
    }

    pub(crate) async fn cancel_session(&self, session_id: &str) -> Result<(), ToolError> {
        let path = format!("sessions/{}/cancel", encode_path_segment(session_id));
        self.send_session_json(reqwest::Method::POST, &path, Some(json!({})))
            .await?;
        Ok(())
    }

    pub(crate) async fn chat(
        &self,
        session_id: &str,
        model: &str,
        messages: &[ChatMessage<'_>],
    ) -> Result<String, ToolError> {
        let messages = messages
            .iter()
            .map(|message| message.to_value())
            .collect::<Vec<_>>();
        let value = self
            .send_json(
                reqwest::Method::POST,
                "chat/completions",
                Some(json!({
                    "model": model,
                    "stream": false,
                    "messages": messages,
                    "gateway": {"session_id": session_id}
                })),
            )
            .await?;
        value
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| protocol("OpenAI-compatible completion response did not contain choices[0].message.content"))
    }

    async fn get_json(&self, path: &str) -> Result<Value, ToolError> {
        self.send_json(reqwest::Method::GET, path, None).await
    }

    async fn send_session_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ToolError> {
        self.send_json_with_timeout(method, path, body, SESSION_REQUEST_TIMEOUT)
            .await
    }

    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ToolError> {
        self.send_json_with_timeout(method, path, body, REQUEST_TIMEOUT)
            .await
    }

    async fn send_json_with_timeout(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        request_timeout: Duration,
    ) -> Result<Value, ToolError> {
        let url = self
            .base_url
            .join(path)
            .map_err(|_| protocol("failed to construct OpenAI-compatible endpoint URL"))?;
        let mut request = self
            .http
            .request(method, url.clone())
            .timeout(request_timeout);
        if let Some(api_key) = self.api_key.as_deref() {
            request = request.bearer_auth(api_key);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| {
            unavailable(format!("Phoenix provider request to {url} failed: {error}"))
        })?;
        let status = response.status();
        let bytes = if status.is_success() {
            read_body_limited(response, MAX_SUCCESS_BODY_BYTES, true).await?
        } else {
            read_body_limited(response, MAX_ERROR_BODY_BYTES, false).await?
        };
        if !status.is_success() {
            return Err(provider_http_error(status, &bytes));
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| protocol(format!("Phoenix provider returned invalid JSON: {error}")))
    }
}

async fn read_body_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
    reject_overflow: bool,
) -> Result<Vec<u8>, ToolError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| unavailable(format!("Phoenix provider response read failed: {error}")))?
    {
        let remaining = max_bytes.saturating_sub(bytes.len());
        if chunk.len() > remaining {
            if reject_overflow {
                return Err(protocol(
                    "Phoenix provider response exceeded the configured size bound",
                ));
            }
            bytes.extend_from_slice(&chunk[..remaining]);
            break;
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() == max_bytes && !reject_overflow {
            break;
        }
    }
    Ok(bytes)
}

fn encode_path_segment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn invalid_endpoint() -> ToolError {
    ToolError::InvalidParam {
        message: format!(
            "{BASE_URL_ENV} must be an absolute http(s) URL without credentials, query, or fragment"
        ),
        param: BASE_URL_ENV.into(),
    }
}

fn provider_http_error(status: StatusCode, body: &[u8]) -> ToolError {
    let body = &body[..body.len().min(MAX_ERROR_BODY_BYTES)];
    let detail = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_owned());
    unavailable(format!(
        "Phoenix provider returned HTTP {}{}",
        status.as_u16(),
        if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        }
    ))
}

fn protocol(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "protocol_error".into(),
        message: message.into(),
    }
}

fn unavailable(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "unavailable".into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_normalizes_openai_base_urls() {
        drop(rustls::crypto::ring::default_provider().install_default());
        let backend = OpenAiBackend::from_url("http://127.0.0.1:43871/v1", None).unwrap();
        assert_eq!(backend.base_url().as_str(), "http://127.0.0.1:43871/v1/");
        let normalized = OpenAiBackend::from_url("https://example.test/v1///", None).unwrap();
        assert_eq!(normalized.base_url().as_str(), "https://example.test/v1/");
        assert!(OpenAiBackend::from_url("file:///tmp/provider", None).is_err());
        assert!(OpenAiBackend::from_url("https://user:secret@example.test/v1", None).is_err());
        assert!(OpenAiBackend::from_url("https://example.test/v1?token=secret", None).is_err());
    }

    #[tokio::test]
    async fn provider_session_cleanup_uses_cancel_and_close_routes() {
        use wiremock::{
            Mock, MockServer, ResponseTemplate,
            matchers::{method, path},
        };

        drop(rustls::crypto::ring::default_provider().install_default());
        let server = MockServer::start().await;
        for route in [
            "/v1/sessions/session-1/cancel",
            "/v1/sessions/session-1/close",
        ] {
            Mock::given(method("POST"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
                .expect(1)
                .mount(&server)
                .await;
        }
        let backend = OpenAiBackend::from_url(&format!("{}/v1", server.uri()), None).unwrap();

        backend.cancel_session("session-1").await.unwrap();
        backend.close_session("session-1").await.unwrap();

        server.verify().await;
    }
}
