use std::{env, time::Duration};

use reqwest::{Client, StatusCode, Url};
use serde_json::{Value, json};

use crate::dispatch::error::ToolError;

pub(crate) const BASE_URL_ENV: &str = "LABBY_PHOENIX_OPENAI_BASE_URL";
pub(crate) const API_KEY_ENV: &str = "LABBY_PHOENIX_OPENAI_API_KEY";
const REQUEST_TIMEOUT: Duration = Duration::from_mins(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;

#[derive(Clone)]
pub(crate) struct OpenAiBackend {
    http: Client,
    base_url: Url,
    api_key: Option<String>,
}

impl OpenAiBackend {
    pub(crate) fn from_env() -> Option<Self> {
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
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path().trim_end_matches('/'));
            base_url.set_path(&path);
        }
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
            api_key: api_key.filter(|value| !value.trim().is_empty()),
        })
    }

    #[cfg(test)]
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
        self.send_json(
            reqwest::Method::POST,
            "sessions",
            Some(json!({"id": session_id, "url": "https://chatgpt.com/"})),
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn close_session(&self, session_id: &str) -> Result<(), ToolError> {
        let path = format!("sessions/{}/close", encode_path_segment(session_id));
        self.send_json(reqwest::Method::POST, &path, Some(json!({})))
            .await?;
        Ok(())
    }

    pub(crate) async fn rename_session(
        &self,
        session_id: &str,
        title: &str,
    ) -> Result<(), ToolError> {
        let path = format!("sessions/{}/title", encode_path_segment(session_id));
        self.send_json(reqwest::Method::PUT, &path, Some(json!({"title": title})))
            .await?;
        Ok(())
    }

    pub(crate) async fn cancel_session(&self, session_id: &str) -> Result<(), ToolError> {
        let path = format!("sessions/{}/cancel", encode_path_segment(session_id));
        self.send_json(reqwest::Method::POST, &path, Some(json!({})))
            .await?;
        Ok(())
    }

    pub(crate) async fn chat(
        &self,
        session_id: &str,
        model: &str,
        input: &str,
    ) -> Result<String, ToolError> {
        let value = self
            .send_json(
                reqwest::Method::POST,
                "chat/completions",
                Some(json!({
                    "model": model,
                    "stream": false,
                    "messages": [{"role": "user", "content": input}],
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

    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ToolError> {
        let url = self
            .base_url
            .join(path)
            .map_err(|_| protocol("failed to construct OpenAI-compatible endpoint URL"))?;
        let mut request = self.http.request(method, url.clone());
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
        let bytes = response.bytes().await.map_err(|error| {
            unavailable(format!("Phoenix provider response read failed: {error}"))
        })?;
        if !status.is_success() {
            return Err(provider_http_error(status, &bytes));
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| protocol(format!("Phoenix provider returned invalid JSON: {error}")))
    }
}

fn encode_path_segment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn invalid_endpoint() -> ToolError {
    ToolError::InvalidParam {
        message: format!(
            "{BASE_URL_ENV} must be an absolute http(s) URL without query or fragment"
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
        assert!(OpenAiBackend::from_url("file:///tmp/provider", None).is_err());
        assert!(OpenAiBackend::from_url("https://user:secret@example.test/v1", None).is_err());
        assert!(OpenAiBackend::from_url("https://example.test/v1?token=secret", None).is_err());
    }
}
