//! Bounded server-side transport for the optional Depot control plane.

pub mod admin;
#[cfg(test)]
mod admin_tests;
pub mod cursor;
#[cfg(test)]
mod cursor_tests;
pub mod discovery;
#[cfg(test)]
mod discovery_tests;
pub mod health;
pub mod manager;
#[cfg(test)]
mod manager_tests;
pub mod network;
#[cfg(test)]
mod network_tests;
pub mod operations;
pub mod provider;
pub mod scheduler;
#[cfg(test)]
mod scheduler_tests;
pub mod store;
#[cfg(test)]
mod store_tests;

use std::{collections::HashMap, env, future::Future, sync::Arc, time::Duration};

use labby_auth::depot_delegation::{
    BrowserDepotAuthorization, DepotDelegationScope, DepotDelegationSubject, DepotDelegationTarget,
};
use labby_auth::jwt::SigningKeys;
use labby_primitives::product_credential::BoundAccessGrant;
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Semaphore};

const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const QUEUE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_INTERACTIVE_REQUESTS: usize = 16;
const COMPATIBILITY_SCHEMA_VERSION: &str = "labby.depot-compatibility/v1";

#[derive(Clone)]
pub struct DepotClient {
    http: Client,
    base_url: Option<Url>,
    token: Option<Arc<str>>,
    delegation: Option<Arc<DepotDelegationSigner>>,
    enabled: bool,
    interactive: Arc<Semaphore>,
    destructive_requests: Arc<Mutex<HashMap<String, DestructiveRequest>>>,
    operation_catalogs: Arc<Mutex<HashMap<String, OperationCatalogSnapshot>>>,
    queue_timeout: Duration,
}

#[derive(Clone)]
struct DepotDelegationSigner {
    keys: Arc<SigningKeys>,
    target: DepotDelegationTarget,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DepotStatus {
    pub configured: bool,
    pub enabled: bool,
    pub authority: DepotAuthority,
    pub max_response_bytes: usize,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DepotAuthority {
    Unknown,
    Read,
    Write,
}

#[derive(Clone, Debug)]
enum DestructiveRequest {
    Pending([u8; 32], tokio::time::Instant),
    Complete([u8; 32], Value, tokio::time::Instant),
    Indeterminate([u8; 32], tokio::time::Instant),
}

impl DestructiveRequest {
    fn observed_at(&self) -> tokio::time::Instant {
        match self {
            Self::Pending(_, at) | Self::Complete(_, _, at) | Self::Indeterminate(_, at) => *at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationPolicy {
    pub read_only: bool,
    pub destructive: bool,
}

struct OperationCatalogSnapshot {
    observed_at: tokio::time::Instant,
    policies: HashMap<String, OperationPolicy>,
}

#[derive(Debug)]
pub enum DepotError {
    Disabled,
    Unconfigured,
    UnsupportedOperation,
    InvalidCatalog,
    DestructiveIntentRequired,
    IdempotencyConflict,
    OutcomeIndeterminate,
    Upstream(StatusCode, Value),
    QueueTimeout,
    Unavailable(TransportFailure),
    ResponseTooLarge,
    InvalidResponse,
    DelegationUnavailable,
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
    /// Inert compatibility facade until the browser adapters receive the manager.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            http: Client::new(),
            base_url: None,
            token: None,
            delegation: None,
            enabled: false,
            interactive: Arc::new(Semaphore::new(MAX_INTERACTIVE_REQUESTS)),
            destructive_requests: Arc::new(Mutex::new(HashMap::new())),
            operation_catalogs: Arc::new(Mutex::new(HashMap::new())),
            queue_timeout: QUEUE_TIMEOUT,
        }
    }
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
            delegation: None,
            enabled,
            interactive: Arc::new(Semaphore::new(MAX_INTERACTIVE_REQUESTS)),
            destructive_requests: Arc::new(Mutex::new(HashMap::new())),
            operation_catalogs: Arc::new(Mutex::new(HashMap::new())),
            queue_timeout: QUEUE_TIMEOUT,
        }
    }

    /// Attach the server-owned Depot authority mapping. Incomplete mappings
    /// leave delegation disabled so mutation requests fail closed.
    #[must_use]
    pub fn with_delegation_from_env(
        mut self,
        keys: Arc<SigningKeys>,
        issuer: Option<&Url>,
    ) -> Self {
        let target = issuer.and_then(|issuer| {
            Some(DepotDelegationTarget {
                issuer: issuer.as_str().trim_end_matches('/').to_owned(),
                audience: required_env("LABBY_DEPOT_DELEGATION_AUDIENCE")?,
                deployment_id: required_env("LABBY_DEPOT_DELEGATION_DEPLOYMENT_ID")?,
                account_id: required_env("LABBY_DEPOT_DELEGATION_ACCOUNT_ID")?,
                tenant_id: required_env("LABBY_DEPOT_DELEGATION_TENANT_ID")?,
                team_id: env::var("LABBY_DEPOT_DELEGATION_TEAM_ID")
                    .ok()
                    .filter(|value| !value.trim().is_empty()),
            })
        });
        self.delegation = target.map(|target| Arc::new(DepotDelegationSigner { keys, target }));
        self
    }

    #[must_use]
    pub fn status(&self) -> DepotStatus {
        let configured = self.base_url.is_some() && self.token.is_some();
        DepotStatus {
            configured,
            enabled: self.enabled,
            // Configuration proves only that Labby can attempt a request. Depot
            // remains authoritative for token scopes, so local state must not
            // claim write authority that Depot has not attested.
            authority: DepotAuthority::Unknown,
            max_response_bytes: MAX_RESPONSE_BYTES,
        }
    }

    #[must_use]
    pub fn publishing_configured(&self) -> bool {
        self.enabled && self.base_url.is_some() && self.delegation.is_some()
    }

    #[cfg(test)]
    pub(crate) fn with_test_delegation(
        mut self,
        keys: Arc<SigningKeys>,
        target: DepotDelegationTarget,
    ) -> Self {
        self.delegation = Some(Arc::new(DepotDelegationSigner { keys, target }));
        self
    }

    #[cfg(test)]
    pub(crate) fn for_test(base_url: Url, token: &str) -> Self {
        drop(rustls::crypto::ring::default_provider().install_default());
        Self {
            http: Client::builder().no_proxy().build().unwrap(),
            base_url: Some(base_url),
            token: Some(Arc::from(token)),
            delegation: None,
            enabled: true,
            interactive: Arc::new(Semaphore::new(MAX_INTERACTIVE_REQUESTS)),
            destructive_requests: Arc::new(Mutex::new(HashMap::new())),
            operation_catalogs: Arc::new(Mutex::new(HashMap::new())),
            queue_timeout: QUEUE_TIMEOUT,
        }
    }

    pub async fn session(&self, actor: &str) -> Result<Value, DepotError> {
        self.request(reqwest::Method::GET, "api/session", None, actor)
            .await
    }

    pub async fn status_for_actor(&self, actor: &str) -> DepotStatus {
        let mut status = self.status();
        if let Ok(catalog) = self.operations(actor).await
            && let Ok(catalog) = serde_json::from_value::<OperationCatalog>(catalog)
        {
            status.authority = if catalog
                .operations
                .iter()
                .any(|operation| !operation.annotations.read_only_hint)
            {
                DepotAuthority::Write
            } else {
                DepotAuthority::Read
            };
        }
        status
    }

    pub async fn operations(&self, actor: &str) -> Result<Value, DepotError> {
        let mut value = self
            .request(reqwest::Method::GET, "api/operations", None, actor)
            .await?;
        let policies = parse_operation_catalog(&value)?;
        project_operation_groups(&mut value)?;
        self.operation_catalogs.lock().await.insert(
            actor.to_owned(),
            OperationCatalogSnapshot {
                observed_at: tokio::time::Instant::now(),
                policies,
            },
        );
        Ok(value)
    }

    /// Resolve an operation solely from Depot's current actor-filtered catalog.
    pub async fn operation_policy(
        &self,
        operation: &str,
        actor: &str,
    ) -> Result<OperationPolicy, DepotError> {
        if !valid_operation_name(operation) {
            return Err(DepotError::UnsupportedOperation);
        }
        let catalogs = self.operation_catalogs.lock().await;
        let catalog = catalogs.get(actor).ok_or(DepotError::InvalidCatalog)?;
        if catalog.observed_at.elapsed() > Duration::from_mins(5) {
            return Err(DepotError::InvalidCatalog);
        }
        catalog
            .policies
            .get(operation)
            .copied()
            .ok_or(DepotError::UnsupportedOperation)
    }

    pub async fn call(
        &self,
        operation: &str,
        params: Value,
        actor: &str,
        policy: OperationPolicy,
        idempotency_key: Option<&str>,
    ) -> Result<Value, DepotError> {
        self.call_with_grant(operation, params, actor, policy, idempotency_key, None)
            .await
    }

    pub async fn call_with_grant(
        &self,
        operation: &str,
        params: Value,
        actor: &str,
        policy: OperationPolicy,
        idempotency_key: Option<&str>,
        grant: Option<&BoundAccessGrant>,
    ) -> Result<Value, DepotError> {
        self.call_with_subject(
            operation,
            params,
            actor,
            policy,
            idempotency_key,
            grant.map(Into::into),
        )
        .await
    }

    async fn call_with_subject(
        &self,
        operation: &str,
        params: Value,
        actor: &str,
        policy: OperationPolicy,
        idempotency_key: Option<&str>,
        subject: Option<DepotDelegationSubject<'_>>,
    ) -> Result<Value, DepotError> {
        if policy.destructive {
            let key = idempotency_key
                .filter(|key| valid_idempotency_key(key))
                .ok_or(DepotError::DestructiveIntentRequired)?;
            return self
                .call_destructive(operation, params, actor, key, subject)
                .await;
        }
        self.call_upstream(operation, params, actor, None, policy, subject)
            .await
    }

    async fn call_destructive(
        &self,
        operation: &str,
        params: Value,
        actor: &str,
        idempotency_key: &str,
        subject: Option<DepotDelegationSubject<'_>>,
    ) -> Result<Value, DepotError> {
        let digest: [u8; 32] = Sha256::digest(
            serde_json::to_vec(&json!({"actor":actor,"operation":operation,"params":params}))
                .map_err(|_| DepotError::InvalidResponse)?,
        )
        .into();
        {
            let mut requests = self.destructive_requests.lock().await;
            match requests.get(idempotency_key) {
                Some(DestructiveRequest::Complete(existing, value, _)) if existing == &digest => {
                    return Ok(value.clone());
                }
                Some(DestructiveRequest::Pending(existing, _)) if existing == &digest => {
                    return Err(DepotError::OutcomeIndeterminate);
                }
                Some(DestructiveRequest::Indeterminate(existing, _)) if existing == &digest => {
                    return Err(DepotError::OutcomeIndeterminate);
                }
                Some(_) => return Err(DepotError::IdempotencyConflict),
                None => {
                    let now = tokio::time::Instant::now();
                    if requests.len() >= 1024 {
                        requests.retain(|_, state| {
                            now.duration_since(state.observed_at()) < Duration::from_hours(24)
                        });
                    }
                    if requests.len() >= 1024 {
                        let evict = requests.iter().find_map(|(key, state)| {
                            (!matches!(state, DestructiveRequest::Pending(_, _)))
                                .then(|| key.clone())
                        });
                        if let Some(evict) = evict {
                            requests.remove(&evict);
                        } else {
                            return Err(DepotError::QueueTimeout);
                        }
                    }
                    requests.insert(
                        idempotency_key.to_owned(),
                        DestructiveRequest::Pending(digest, now),
                    );
                }
            }
        }
        let result = self
            .call_upstream(
                operation,
                params,
                actor,
                Some(idempotency_key),
                OperationPolicy {
                    read_only: false,
                    destructive: true,
                },
                subject,
            )
            .await;
        let mut requests = self.destructive_requests.lock().await;
        match &result {
            Ok(value) => {
                requests.insert(
                    idempotency_key.to_owned(),
                    DestructiveRequest::Complete(
                        digest,
                        value.clone(),
                        tokio::time::Instant::now(),
                    ),
                );
            }
            Err(DepotError::Upstream(_, _)) => {
                requests.remove(idempotency_key);
            }
            Err(_) => {
                requests.insert(
                    idempotency_key.to_owned(),
                    DestructiveRequest::Indeterminate(digest, tokio::time::Instant::now()),
                );
            }
        }
        result
    }

    async fn call_upstream(
        &self,
        operation: &str,
        params: Value,
        actor: &str,
        idempotency_key: Option<&str>,
        policy: OperationPolicy,
        subject: Option<DepotDelegationSubject<'_>>,
    ) -> Result<Value, DepotError> {
        self.request_with_idempotency(
            reqwest::Method::POST,
            &format!("api/operations/{operation}"),
            Some(params),
            actor,
            idempotency_key,
            !policy.read_only,
            subject,
        )
        .await
        .and_then(compatibility_envelope)
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        actor: &str,
    ) -> Result<Value, DepotError> {
        self.request_with_idempotency(method, path, body, actor, None, false, None)
            .await
    }

    async fn request_with_idempotency(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        actor: &str,
        idempotency_key: Option<&str>,
        requires_delegation: bool,
        delegation_subject: Option<DepotDelegationSubject<'_>>,
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
        let delegated_token = if requires_delegation {
            let operation = path
                .strip_prefix("api/operations/")
                .ok_or(DepotError::UnsupportedOperation)?;
            let params = body.as_ref().ok_or(DepotError::UnsupportedOperation)?;
            Some(self.delegation_token(delegation_subject, operation, params)?)
        } else {
            None
        };
        let token = delegated_token
            .as_deref()
            .or(self.token.as_deref())
            .ok_or(DepotError::Unconfigured)?;
        let url = base.join(path).map_err(|_| DepotError::Unconfigured)?;
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(token)
            .header("accept", "application/json");
        if delegated_token.is_none() {
            request = request.header("x-labby-actor", actor);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(key) = idempotency_key {
            request = request.header("idempotency-key", key);
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

    fn delegation_token(
        &self,
        subject: Option<DepotDelegationSubject<'_>>,
        operation: &str,
        params: &Value,
    ) -> Result<String, DepotError> {
        let subject = subject.ok_or(DepotError::DelegationUnavailable)?;
        let signer = self
            .delegation
            .as_ref()
            .ok_or(DepotError::DelegationUnavailable)?;
        let token = match subject {
            DepotDelegationSubject::ProductCredential(grant) => signer.keys.issue_depot_delegation(
                &signer.target,
                grant,
                DepotDelegationScope::Write,
                operation,
                params,
                30,
            ),
            DepotDelegationSubject::Browser(authorization) => {
                signer.keys.issue_browser_depot_delegation(
                    &signer.target,
                    authorization,
                    DepotDelegationScope::Write,
                    operation,
                    params,
                    30,
                )
            }
        };
        token.map_err(|_| DepotError::DelegationUnavailable)
    }

    /// Stream bytes into a principal-bound Depot upload slot. This path never
    /// falls back to the configured read bearer and never forwards an actor
    /// header; Depot derives ownership from the fresh delegation subject.
    pub async fn upload_with_grant(
        &self,
        upload_id: &str,
        body: reqwest::Body,
        content_length: Option<u64>,
        content_type: &str,
        grant: Option<&BoundAccessGrant>,
    ) -> Result<Value, DepotError> {
        self.upload_with_subject(
            upload_id,
            body,
            content_length,
            content_type,
            grant.map(Into::into),
        )
        .await
    }

    async fn upload_with_subject(
        &self,
        upload_id: &str,
        body: reqwest::Body,
        content_length: Option<u64>,
        content_type: &str,
        subject: Option<DepotDelegationSubject<'_>>,
    ) -> Result<Value, DepotError> {
        if !self.enabled {
            return Err(DepotError::Disabled);
        }
        if !valid_upload_id(upload_id) {
            return Err(DepotError::UnsupportedOperation);
        }
        let _permit = tokio::time::timeout(self.queue_timeout, self.interactive.acquire())
            .await
            .map_err(|_| DepotError::QueueTimeout)?
            .map_err(|_| DepotError::Unavailable(TransportFailure::Request))?;
        let base = self.base_url.as_ref().ok_or(DepotError::Unconfigured)?;
        let binding = json!({
            "contentLength": content_length,
            "contentType": content_type,
            "uploadId": upload_id,
        });
        let token = self.delegation_token(
            subject,
            super::depot_publish::UPLOAD_PUT_OPERATION,
            &binding,
        )?;
        let url = base
            .join(&format!("uploads/{upload_id}"))
            .map_err(|_| DepotError::Unconfigured)?;
        let mut request = self
            .http
            .put(url)
            .bearer_auth(token)
            .header("accept", "application/json")
            .header("content-type", content_type)
            .body(body);
        if let Some(content_length) = content_length {
            request = request.header("content-length", content_length);
        }
        let response = request.send().await.map_err(|error| {
            DepotError::Unavailable(if error.is_timeout() {
                TransportFailure::Timeout
            } else if error.is_connect() {
                TransportFailure::Connect
            } else {
                TransportFailure::Request
            })
        })?;
        decode_response(response).await
    }

    pub async fn publish_skill_archive(
        &self,
        filename: &str,
        archive: Vec<u8>,
        namespace: Option<&str>,
        grant: &BoundAccessGrant,
    ) -> Result<Value, DepotError> {
        self.publish_skill_archive_for_subject(filename, archive, namespace, grant.into())
            .await
    }

    pub async fn publish_skill_archive_for_browser_revalidated<F, Fut>(
        &self,
        filename: &str,
        archive: Vec<u8>,
        namespace: Option<&str>,
        authorize: F,
    ) -> Result<Value, DepotError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<BrowserDepotAuthorization, DepotError>>,
    {
        let create_authorization = authorize().await?;
        let created = self
            .call_with_subject(
                super::depot_publish::UPLOAD_CREATE_OPERATION,
                json!({"filename": filename}),
                &create_authorization.principal_id,
                OperationPolicy {
                    read_only: false,
                    destructive: false,
                },
                None,
                Some((&create_authorization).into()),
            )
            .await?;
        let upload_id = created
            .pointer("/result/upload/id")
            .or_else(|| created.pointer("/upload/id"))
            .or_else(|| created.get("id"))
            .and_then(Value::as_str)
            .filter(|id| valid_upload_id(id))
            .ok_or(DepotError::InvalidResponse)?
            .to_owned();
        let content_length =
            u64::try_from(archive.len()).map_err(|_| DepotError::InvalidResponse)?;

        let upload_authorization = authorize().await?;
        if upload_authorization != create_authorization {
            return Err(DepotError::DelegationUnavailable);
        }
        self.upload_with_subject(
            &upload_id,
            reqwest::Body::from(archive),
            Some(content_length),
            "application/octet-stream",
            Some((&upload_authorization).into()),
        )
        .await?;

        let mut arguments = serde_json::Map::new();
        arguments.insert("uploadId".into(), Value::String(upload_id));
        if let Some(namespace) = namespace {
            arguments.insert("namespace".into(), Value::String(namespace.to_owned()));
        }
        let ingest_authorization = authorize().await?;
        if ingest_authorization != create_authorization {
            return Err(DepotError::DelegationUnavailable);
        }
        self.call_with_subject(
            super::depot_publish::INGEST_START_OPERATION,
            json!({"kind":"archive","arguments":arguments}),
            &ingest_authorization.principal_id,
            OperationPolicy {
                read_only: false,
                destructive: false,
            },
            None,
            Some((&ingest_authorization).into()),
        )
        .await
    }

    async fn publish_skill_archive_for_subject(
        &self,
        filename: &str,
        archive: Vec<u8>,
        namespace: Option<&str>,
        subject: DepotDelegationSubject<'_>,
    ) -> Result<Value, DepotError> {
        let actor = subject.principal_id();
        let created = self
            .call_with_subject(
                super::depot_publish::UPLOAD_CREATE_OPERATION,
                json!({"filename": filename}),
                actor,
                OperationPolicy {
                    read_only: false,
                    destructive: false,
                },
                None,
                Some(subject),
            )
            .await?;
        let upload_id = created
            .pointer("/result/upload/id")
            .or_else(|| created.pointer("/upload/id"))
            .or_else(|| created.get("id"))
            .and_then(Value::as_str)
            .filter(|id| valid_upload_id(id))
            .ok_or(DepotError::InvalidResponse)?
            .to_owned();
        let content_length =
            u64::try_from(archive.len()).map_err(|_| DepotError::InvalidResponse)?;
        self.upload_with_subject(
            &upload_id,
            reqwest::Body::from(archive),
            Some(content_length),
            "application/octet-stream",
            Some(subject),
        )
        .await?;
        let mut arguments = serde_json::Map::new();
        arguments.insert("uploadId".into(), Value::String(upload_id));
        if let Some(namespace) = namespace {
            arguments.insert("namespace".into(), Value::String(namespace.to_owned()));
        }
        self.call_with_subject(
            super::depot_publish::INGEST_START_OPERATION,
            json!({"kind":"archive","arguments":arguments}),
            actor,
            OperationPolicy {
                read_only: false,
                destructive: false,
            },
            None,
            Some(subject),
        )
        .await
    }
}

fn compatibility_envelope(value: Value) -> Result<Value, DepotError> {
    let Value::Object(mut response) = value else {
        return Err(DepotError::InvalidResponse);
    };
    response.insert(
        "schemaVersion".to_string(),
        Value::String(COMPATIBILITY_SCHEMA_VERSION.to_string()),
    );
    response.insert("contractVersion".to_string(), Value::from(1));
    Ok(Value::Object(response))
}

#[derive(Deserialize)]
struct OperationCatalog {
    operations: Vec<CatalogOperation>,
}

#[derive(Deserialize)]
struct CatalogOperation {
    name: String,
    annotations: CatalogAnnotations,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogAnnotations {
    read_only_hint: bool,
    destructive_hint: bool,
}

#[cfg(test)]
fn parse_operation_policy(catalog: &Value, name: &str) -> Result<OperationPolicy, DepotError> {
    parse_operation_catalog(catalog)?
        .remove(name)
        .ok_or(DepotError::UnsupportedOperation)
}

fn parse_operation_catalog(
    catalog: &Value,
) -> Result<HashMap<String, OperationPolicy>, DepotError> {
    let catalog: OperationCatalog =
        serde_json::from_value(catalog.clone()).map_err(|_| DepotError::InvalidCatalog)?;
    let mut policies = HashMap::with_capacity(catalog.operations.len());
    for item in catalog.operations {
        if !valid_operation_name(&item.name)
            || item.annotations.read_only_hint && item.annotations.destructive_hint
        {
            return Err(DepotError::InvalidCatalog);
        }
        let policy = OperationPolicy {
            read_only: item.annotations.read_only_hint,
            destructive: item.annotations.destructive_hint,
        };
        if policies.insert(item.name, policy).is_some() {
            return Err(DepotError::InvalidCatalog);
        }
    }
    Ok(policies)
}

fn valid_operation_name(operation: &str) -> bool {
    !operation.is_empty()
        && operation.len() <= 256
        && operation
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_idempotency_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 160 && key.bytes().all(|byte| byte.is_ascii_graphic())
}

fn valid_upload_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn project_operation_groups(value: &mut Value) -> Result<(), DepotError> {
    let operations = value
        .get_mut("operations")
        .and_then(Value::as_array_mut)
        .ok_or(DepotError::InvalidCatalog)?;
    for operation in operations {
        let object = operation
            .as_object_mut()
            .ok_or(DepotError::InvalidCatalog)?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or(DepotError::InvalidCatalog)?;
        let group = if name.starts_with("depot.tokens.") {
            "access"
        } else if name.starts_with("depot.maintenance.")
            || name.starts_with("depot.ingest.")
            || name.starts_with("depot.uploads.")
            || name.starts_with("depot.system.")
        {
            "operations"
        } else {
            "catalog"
        };
        object.insert("group".to_owned(), Value::String(group.to_owned()));
    }
    Ok(())
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

pub fn error_body(error: &DepotError) -> Value {
    match error {
        DepotError::Upstream(status, _) => {
            json!({"error":"depot_rejected","status":status.as_u16()})
        }
        DepotError::Disabled => json!({"error":"depot_disabled"}),
        DepotError::Unconfigured => json!({"error":"depot_unconfigured"}),
        DepotError::UnsupportedOperation => json!({"error":"unsupported_operation"}),
        DepotError::InvalidCatalog => json!({"error":"invalid_depot_catalog"}),
        DepotError::DestructiveIntentRequired => json!({"error":"destructive_intent_required"}),
        DepotError::IdempotencyConflict => json!({"error":"idempotency_conflict"}),
        DepotError::OutcomeIndeterminate => {
            json!({"error":"outcome_indeterminate","recovery":{"action":"reconcile_before_retry"}})
        }
        DepotError::QueueTimeout => json!({"error":"depot_busy"}),
        DepotError::Unavailable(_) => json!({"error":"depot_unavailable"}),
        DepotError::ResponseTooLarge => json!({"error":"depot_response_too_large"}),
        DepotError::InvalidResponse => json!({"error":"invalid_depot_response"}),
        DepotError::DelegationUnavailable => json!({"error":"depot_delegation_unavailable"}),
    }
}

fn required_env(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Mutex as StdMutex,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;
    use base64::Engine as _;
    use wiremock::matchers::{method, path};
    use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

    struct FreshDelegationWithoutActorHeader;

    impl Match for FreshDelegationWithoutActorHeader {
        fn matches(&self, request: &Request) -> bool {
            request
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value.starts_with("Bearer eyJ")
                        && !value.contains("inbound-user-bearer")
                        && !value.contains("shared-write-bearer")
                })
                && !request.headers.contains_key("x-labby-actor")
        }
    }

    struct ExactDelegation {
        operation: &'static str,
        params: Value,
        seen_jtis: Arc<StdMutex<Vec<String>>>,
    }

    impl Match for ExactDelegation {
        fn matches(&self, request: &Request) -> bool {
            let Some(token) = request
                .headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
            else {
                return false;
            };
            let Some(payload) = token.split('.').nth(1) else {
                return false;
            };
            let Ok(payload) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload)
            else {
                return false;
            };
            let Ok(claims) = serde_json::from_slice::<Value>(&payload) else {
                return false;
            };
            let expected_digest =
                labby_auth::depot_delegation::depot_params_digest(&self.params).unwrap();
            let Some(jti) = claims.get("jti").and_then(Value::as_str) else {
                return false;
            };
            self.seen_jtis.lock().unwrap().push(jti.to_owned());
            claims["depot_operation"] == self.operation
                && claims["depot_params_sha256"] == expected_digest
                && !request.headers.contains_key("x-labby-actor")
        }
    }

    fn delegation_grant() -> BoundAccessGrant {
        BoundAccessGrant {
            installation_id: "labby-lime-prod".into(),
            issuer: "https://team-labby.example".into(),
            subject: "source-subject".into(),
            principal_id: "person-123".into(),
            organization_id: "organization-lime".into(),
            project_id: "project-skills".into(),
            loadout_id: "loadout".into(),
            loadout_generation: 1,
            assignment_generation: 1,
            catalog_generation: 1,
            route_id: "route".into(),
            route_generation: 1,
            membership_epoch: 2,
            organization_policy_epoch: 3,
            project_policy_epoch: 4,
            credential_id: "credential".into(),
            credential_generation: 1,
            scopes: vec!["lab:read".into()],
            resource: "https://team-labby.example/mcp".into(),
            audience: "labby".into(),
            expires_at: u64::try_from(labby_auth::util::now_unix() + 600).unwrap(),
            requires_admin: false,
            destructive: false,
        }
    }

    fn browser_authorization() -> BrowserDepotAuthorization {
        BrowserDepotAuthorization {
            installation_id: "labby-lime-prod".into(),
            principal_id: "person-123".into(),
            organization_id: "organization-lime".into(),
            project_id: "project-skills".into(),
            membership_epoch: 2,
            organization_policy_epoch: 3,
            project_policy_epoch: 4,
            expires_at: u64::try_from(labby_auth::util::now_unix() + 600).unwrap(),
        }
    }

    #[tokio::test]
    async fn static_bearer_is_never_used_for_a_write_operation() {
        let client = DepotClient::for_test(
            Url::parse("http://127.0.0.1:9/").unwrap(),
            "shared-write-bearer-must-not-be-forwarded",
        );
        let result = client
            .call(
                "depot.skills.publish",
                json!({"name":"example"}),
                "untrusted-actor-header",
                OperationPolicy {
                    read_only: false,
                    destructive: false,
                },
                None,
            )
            .await;
        assert!(matches!(result, Err(DepotError::DelegationUnavailable)));
    }

    #[tokio::test]
    async fn write_forwards_only_a_fresh_delegation_and_no_actor_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/operations/depot.skills.publish"))
            .and(FreshDelegationWithoutActorHeader)
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"result":{}})))
            .expect(1)
            .mount(&server)
            .await;

        let temp = tempfile::tempdir().unwrap();
        let keys =
            Arc::new(SigningKeys::load_or_create(&temp.path().join("delegation-key.der")).unwrap());
        let mut client =
            DepotClient::for_test(Url::parse(&server.uri()).unwrap(), "shared-write-bearer");
        client.delegation = Some(Arc::new(DepotDelegationSigner {
            keys,
            target: DepotDelegationTarget {
                issuer: "https://team-labby.example".into(),
                audience: "https://depot.example".into(),
                deployment_id: "depot-lime-prod".into(),
                account_id: "account-lime".into(),
                tenant_id: "tenant-lime".into(),
                team_id: Some("team-lime".into()),
            },
        }));

        client
            .call_with_grant(
                "depot.skills.publish",
                json!({"name":"example","bearer":"inbound-user-bearer"}),
                "forged-actor-header",
                OperationPolicy {
                    read_only: false,
                    destructive: false,
                },
                None,
                Some(&delegation_grant()),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn skill_archive_publish_reuses_three_request_workflow_for_both_authority_types() {
        let server = MockServer::start().await;
        let seen_jtis = Arc::new(StdMutex::new(Vec::new()));
        Mock::given(method("POST"))
            .and(path("/api/operations/depot.uploads.create"))
            .and(ExactDelegation {
                operation: "depot.uploads.create",
                params: json!({"filename":"skill.zip"}),
                seen_jtis: Arc::clone(&seen_jtis),
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "result": {"upload": {"id": "upload-123"}}
            })))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/uploads/upload-123"))
            .and(ExactDelegation {
                operation: "depot.uploads.put",
                params: json!({
                    "contentLength": 13,
                    "contentType": "application/octet-stream",
                    "uploadId": "upload-123"
                }),
                seen_jtis: Arc::clone(&seen_jtis),
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "upload": {"id": "upload-123", "status": "ready"}
            })))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/operations/depot.ingest.start"))
            .and(ExactDelegation {
                operation: "depot.ingest.start",
                params: json!({
                    "kind":"archive",
                    "arguments":{"namespace":"team","uploadId":"upload-123"}
                }),
                seen_jtis: Arc::clone(&seen_jtis),
            })
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "result": {"job": {"id": "job-123"}}
            })))
            .expect(2)
            .mount(&server)
            .await;

        let temp = tempfile::tempdir().unwrap();
        let keys =
            Arc::new(SigningKeys::load_or_create(&temp.path().join("delegation-key.der")).unwrap());
        let mut client = DepotClient::for_test(
            Url::parse(&server.uri()).unwrap(),
            "shared-write-bearer-must-not-be-forwarded",
        );
        client.delegation = Some(Arc::new(DepotDelegationSigner {
            keys,
            target: DepotDelegationTarget {
                issuer: "https://team-labby.example".into(),
                audience: "https://depot.example".into(),
                deployment_id: "depot-lime-prod".into(),
                account_id: "account-lime".into(),
                tenant_id: "tenant-lime".into(),
                team_id: Some("team-lime".into()),
            },
        }));

        let result = client
            .publish_skill_archive(
                "skill.zip",
                b"archive bytes".to_vec(),
                Some("team"),
                &delegation_grant(),
            )
            .await
            .unwrap();
        assert_eq!(result["result"]["job"]["id"], "job-123");
        let authorization_checks = Arc::new(AtomicUsize::new(0));
        let result = client
            .publish_skill_archive_for_browser_revalidated(
                "skill.zip",
                b"archive bytes".to_vec(),
                Some("team"),
                || {
                    authorization_checks.fetch_add(1, Ordering::SeqCst);
                    async { Ok(browser_authorization()) }
                },
            )
            .await
            .unwrap();
        assert_eq!(result["result"]["job"]["id"], "job-123");
        assert_eq!(authorization_checks.load(Ordering::SeqCst), 3);
        let seen_jtis = seen_jtis.lock().unwrap();
        assert_eq!(seen_jtis.len(), 6);
        assert_eq!(
            seen_jtis
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            6
        );
    }

    #[tokio::test]
    async fn browser_publish_stops_before_ingest_when_revalidation_is_denied() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/operations/depot.uploads.create"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "result": {"upload": {"id": "upload-123"}}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/uploads/upload-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "upload": {"id": "upload-123", "status": "ready"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/operations/depot.ingest.start"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let temp = tempfile::tempdir().unwrap();
        let keys =
            Arc::new(SigningKeys::load_or_create(&temp.path().join("delegation-key.der")).unwrap());
        let client = DepotClient::for_test(Url::parse(&server.uri()).unwrap(), "unused")
            .with_test_delegation(
                keys,
                DepotDelegationTarget {
                    issuer: "https://team-labby.example".into(),
                    audience: "https://depot.example".into(),
                    deployment_id: "depot-lime-prod".into(),
                    account_id: "account-lime".into(),
                    tenant_id: "tenant-lime".into(),
                    team_id: Some("team-lime".into()),
                },
            );
        let checks = Arc::new(AtomicUsize::new(0));
        let result = client
            .publish_skill_archive_for_browser_revalidated(
                "skill.zip",
                b"archive bytes".to_vec(),
                None,
                || {
                    let check = checks.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if check == 2 {
                            Err(DepotError::DelegationUnavailable)
                        } else {
                            Ok(browser_authorization())
                        }
                    }
                },
            )
            .await;

        assert!(matches!(result, Err(DepotError::DelegationUnavailable)));
        assert_eq!(checks.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn browser_publish_stops_before_upload_when_revalidation_is_denied() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/operations/depot.uploads.create"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "result": {"upload": {"id": "upload-123"}}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/uploads/upload-123"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let temp = tempfile::tempdir().unwrap();
        let keys =
            Arc::new(SigningKeys::load_or_create(&temp.path().join("delegation-key.der")).unwrap());
        let client = DepotClient::for_test(Url::parse(&server.uri()).unwrap(), "unused")
            .with_test_delegation(
                keys,
                DepotDelegationTarget {
                    issuer: "https://team-labby.example".into(),
                    audience: "https://depot.example".into(),
                    deployment_id: "depot-lime-prod".into(),
                    account_id: "account-lime".into(),
                    tenant_id: "tenant-lime".into(),
                    team_id: Some("team-lime".into()),
                },
            );
        let checks = Arc::new(AtomicUsize::new(0));
        let result = client
            .publish_skill_archive_for_browser_revalidated(
                "skill.zip",
                b"archive bytes".to_vec(),
                None,
                || {
                    let check = checks.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if check == 1 {
                            Err(DepotError::DelegationUnavailable)
                        } else {
                            Ok(browser_authorization())
                        }
                    }
                },
            )
            .await;

        assert!(matches!(result, Err(DepotError::DelegationUnavailable)));
        assert_eq!(checks.load(Ordering::SeqCst), 2);
    }

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
            delegation: None,
            enabled: true,
            interactive: Arc::new(Semaphore::new(permits)),
            destructive_requests: Arc::new(Mutex::new(HashMap::new())),
            operation_catalogs: Arc::new(Mutex::new(HashMap::new())),
            queue_timeout,
        }
    }

    #[test]
    fn operation_policy_comes_from_the_actor_filtered_catalog() {
        let catalog = json!({"operations":[
            {"name":"depot.new.read","annotations":{"readOnlyHint":true,"destructiveHint":false}},
            {"name":"depot.new.destroy","annotations":{"readOnlyHint":false,"destructiveHint":true}}
        ]});
        assert_eq!(
            parse_operation_policy(&catalog, "depot.new.read").unwrap(),
            OperationPolicy {
                read_only: true,
                destructive: false
            }
        );
        assert_eq!(
            parse_operation_policy(&catalog, "depot.new.destroy").unwrap(),
            OperationPolicy {
                read_only: false,
                destructive: true
            }
        );
        assert!(matches!(
            parse_operation_policy(&catalog, "depot.hidden"),
            Err(DepotError::UnsupportedOperation)
        ));
        assert!(matches!(
            parse_operation_policy(
                &json!({"operations":[{"name":"depot.bad","annotations":{}}]}),
                "depot.bad"
            ),
            Err(DepotError::InvalidCatalog)
        ));
    }

    #[test]
    fn operation_groups_are_projected_by_the_server() {
        let mut catalog = json!({"operations":[
            {"name":"depot.skills.list","annotations":{"readOnlyHint":true,"destructiveHint":false}},
            {"name":"depot.tokens.list","annotations":{"readOnlyHint":false,"destructiveHint":false}},
            {"name":"depot.maintenance.gc","annotations":{"readOnlyHint":false,"destructiveHint":true}}
        ]});
        project_operation_groups(&mut catalog).unwrap();
        assert_eq!(catalog["operations"][0]["group"], "catalog");
        assert_eq!(catalog["operations"][1]["group"], "access");
        assert_eq!(catalog["operations"][2]["group"], "operations");
    }

    #[test]
    fn configured_enabled_client_reports_authority_as_unknown() {
        let client = test_client(
            Url::parse("https://depot.invalid/").unwrap(),
            1,
            Duration::from_secs(1),
        );
        assert!(matches!(client.status().authority, DepotAuthority::Unknown));
        assert!(matches!(
            DepotClient::disabled().status().authority,
            DepotAuthority::Unknown
        ));
    }

    #[test]
    fn operation_results_are_wrapped_in_the_labby_compatibility_contract() {
        let response = compatibility_envelope(json!({
            "result": {
                "artifacts": [{
                    "lineage": {
                        "following": false,
                        "upstreamArtifactId": null
                    }
                }],
                "nextCursor": null,
                "total": 1
            }
        }))
        .unwrap();

        assert_eq!(response["schemaVersion"], COMPATIBILITY_SCHEMA_VERSION);
        assert_eq!(response["contractVersion"], 1);
        assert_eq!(response["result"]["total"], 1);
        assert!(response["result"]["nextCursor"].is_null());
        assert!(response["result"]["artifacts"][0]["lineage"]["upstreamArtifactId"].is_null());
    }

    #[test]
    fn non_object_operation_results_fail_closed() {
        assert!(matches!(
            compatibility_envelope(json!([])),
            Err(DepotError::InvalidResponse)
        ));
    }

    #[test]
    fn transport_errors_do_not_disclose_credentials_or_urls() {
        let body = error_body(&DepotError::Unavailable(TransportFailure::Connect)).to_string();
        assert_eq!(body, r#"{"error":"depot_unavailable"}"#);
        assert!(!body.contains("token"));
    }

    #[test]
    fn privileged_upstream_errors_are_redacted_at_the_rust_boundary() {
        let body = error_body(&DepotError::Upstream(
            StatusCode::FORBIDDEN,
            json!({"message":"token secret-token rejected at /private/path"}),
        ));
        assert_eq!(body, json!({"error":"depot_rejected","status":403}));
        assert!(!body.to_string().contains("secret-token"));
        assert!(!body.to_string().contains("/private/path"));
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
