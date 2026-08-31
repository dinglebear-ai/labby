//! Bounded server-side transport for the optional Depot control plane.

use std::{env, sync::Arc, time::Duration};

use reqwest::{Client, StatusCode, Url};
use serde::Serialize;
use serde_json::{Value, json};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct DepotClient {
    http: Client,
    base_url: Option<Url>,
    token: Option<Arc<str>>,
    enabled: bool,
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
    Unavailable,
    ResponseTooLarge,
    InvalidResponse,
}

impl DepotClient {
    #[must_use]
    pub fn from_env() -> Self {
        let enabled = env::var("LABBY_DEPOT_ENABLED").is_ok_and(|value| value == "1");
        let base_url = env::var("LABBY_DEPOT_URL")
            .ok()
            .and_then(|value| Url::parse(&format!("{}/", value.trim_end_matches('/'))).ok());
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
        }
    }

    #[must_use]
    pub fn status(&self) -> DepotStatus {
        DepotStatus {
            configured: self.base_url.is_some() && self.token.is_some(),
            enabled: self.enabled,
            mutation_authority: self.enabled && self.base_url.is_some() && self.token.is_some(),
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

    pub async fn upload(
        &self,
        upload_id: &str,
        bytes: Vec<u8>,
        actor: &str,
    ) -> Result<Value, DepotError> {
        if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 || !upload_id.starts_with("upl_") {
            return Err(DepotError::UnsupportedOperation);
        }
        if !self.enabled {
            return Err(DepotError::Disabled);
        }
        let base = self.base_url.as_ref().ok_or(DepotError::Unconfigured)?;
        let token = self.token.as_ref().ok_or(DepotError::Unconfigured)?;
        let url = base
            .join(&format!("uploads/{upload_id}"))
            .map_err(|_| DepotError::Unconfigured)?;
        let response = self
            .http
            .put(url)
            .bearer_auth(token.as_ref())
            .header("content-type", "application/octet-stream")
            .header("x-labby-actor", actor)
            .body(bytes)
            .send()
            .await
            .map_err(|_| DepotError::Unavailable)?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|_| DepotError::InvalidResponse)?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(DepotError::Upstream(status, body))
        }
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
        let mut response = request.send().await.map_err(|_| DepotError::Unavailable)?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(DepotError::ResponseTooLarge);
        }
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| DepotError::Unavailable)?
        {
            if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(DepotError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| DepotError::InvalidResponse)?;
        if status.is_success() {
            Ok(value)
        } else {
            Err(DepotError::Upstream(status, value))
        }
    }
}

fn allowed_operation(operation: &str) -> bool {
    matches!(
        operation,
        "depot.artifacts.list"
            | "depot.artifacts.get"
            | "depot.artifacts.intake_candidate"
            | "depot.artifacts.follow"
            | "depot.artifacts.fork"
            | "depot.artifacts.set_publication"
            | "depot.artifacts.set_license"
            | "depot.uploads.create"
            | "depot.uploads.get"
            | "depot.uploads.delete"
            | "depot.ingest.start"
            | "depot.ingest.list"
            | "depot.ingest.get"
            | "depot.ingest.cancel"
            | "depot.ingest.retry"
    )
}

pub fn error_body(error: &DepotError) -> Value {
    match error {
        DepotError::Upstream(status, body) => {
            json!({"error":"depot_rejected","status":status.as_u16(),"detail":body})
        }
        DepotError::Disabled => json!({"error":"depot_disabled"}),
        DepotError::Unconfigured => json!({"error":"depot_unconfigured"}),
        DepotError::UnsupportedOperation => json!({"error":"unsupported_operation"}),
        DepotError::Unavailable => json!({"error":"depot_unavailable"}),
        DepotError::ResponseTooLarge => json!({"error":"depot_response_too_large"}),
        DepotError::InvalidResponse => json!({"error":"invalid_depot_response"}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_allowlist_excludes_generic_and_destructive_unknowns() {
        assert!(allowed_operation("depot.artifacts.list"));
        assert!(allowed_operation("depot.ingest.start"));
        assert!(!allowed_operation("depot.artifacts.delete"));
        assert!(!allowed_operation("depot.admin.execute"));
    }

    #[test]
    fn transport_errors_do_not_disclose_credentials_or_urls() {
        let body = error_body(&DepotError::Unavailable).to_string();
        assert_eq!(body, r#"{"error":"depot_unavailable"}"#);
        assert!(!body.contains("token"));
    }
}
