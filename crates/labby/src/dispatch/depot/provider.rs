//! Immutable connection incarnation and explicit discovery contract adapter.
use super::health::{Failure, Health, Provenance};
use super::network::{NetworkClient, NetworkError, NetworkPolicy, Operation, Secret};
use super::scheduler::{Admission, ProviderAdmission};
use crate::config::depot::{AuthMode, OpaqueEpoch, ProviderView};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub(super) const SOURCE_ORIGINS: &[&str] = &["mcp-registry", "acp-registry", "ard"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("Depot work is pending")]
    Pending,
    #[error("Depot provider changed")]
    Stale,
    #[error("Depot provider is disabled")]
    Disabled,
    #[error("Depot provider request failed")]
    Failed(Failure),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    contract_version: String,
    pub deployment_id: OpaqueEpoch,
    pub deployment_epoch: OpaqueEpoch,
    pub authority_epoch: OpaqueEpoch,
    pub listing_epoch: OpaqueEpoch,
    snapshot_continuations: bool,
    pub max_page_size: u16,
    #[serde(skip)]
    new_feed_supported: bool,
    #[serde(skip)]
    source_origins: Vec<String>,
}

impl Identity {
    pub fn parse(value: Value) -> Result<Self, ProviderError> {
        let source_origins = source_origin_capability(&value);
        let new_feed_supported = value
            .get("feeds")
            .and_then(|feeds| feeds.get("new"))
            .is_some_and(|feed| {
                feed.get("rankingVersion").and_then(Value::as_str) == Some("new/v1")
                    && feed.get("windowSeconds").and_then(Value::as_u64) == Some(604_800)
                    && feed.get("acceptsAsOf").and_then(Value::as_bool) == Some(true)
            });
        let mut parsed: Self = serde_json::from_value(value)
            .map_err(|_| ProviderError::Failed(Failure::Incompatible))?;
        if parsed.contract_version != "depot.discovery/v1"
            || !parsed.snapshot_continuations
            || !(1..=200).contains(&parsed.max_page_size)
        {
            return Err(ProviderError::Failed(Failure::Incompatible));
        }
        parsed.new_feed_supported = new_feed_supported;
        parsed.source_origins = source_origins;
        Ok(parsed)
    }

    pub fn supports_new_feed(&self) -> bool {
        self.new_feed_supported
    }
    pub fn supports_source_origin(&self, origin: &str) -> bool {
        self.source_origins.iter().any(|value| value == origin)
    }
    pub fn same_authority(&self, other: &Self) -> bool {
        self.deployment_id == other.deployment_id
            && self.deployment_epoch == other.deployment_epoch
            && self.authority_epoch == other.authority_epoch
    }
}

/// Malformed optional capabilities do not disable ordinary discovery, but must
/// never authorize sending a filter that an older provider might ignore.
fn source_origin_capability(value: &Value) -> Vec<String> {
    let Some(filter) = value
        .get("filters")
        .and_then(|filters| filters.get("sourceOrigin"))
    else {
        return Vec::new();
    };
    if filter.get("version").and_then(Value::as_str) != Some("source-origin/v1") {
        return Vec::new();
    }
    let Some(values) = filter.get("values").and_then(Value::as_array) else {
        return Vec::new();
    };
    if values.is_empty() || values.len() > SOURCE_ORIGINS.len() {
        return Vec::new();
    }
    let mut origins = Vec::new();
    for value in values {
        let Some(origin) = value
            .as_str()
            .filter(|origin| SOURCE_ORIGINS.contains(origin))
        else {
            return Vec::new();
        };
        if origins.iter().any(|value| value == origin) {
            return Vec::new();
        }
        origins.push(origin.to_owned());
    }
    origins
}

#[derive(PartialEq, Eq)]
struct RuntimeKey {
    provider_id: String,
    host_managed: bool,
    read_project_id: Option<String>,
    endpoint: String,
    enabled: bool,
    auth: AuthMode,
    reference: Option<String>,
    credential: Option<[u8; 32]>,
    policy: NetworkPolicy,
}
impl RuntimeKey {
    fn new(view: &ProviderView, token: Option<&str>, policy: &NetworkPolicy) -> Self {
        Self {
            provider_id: view.id.clone(),
            host_managed: view.host_managed,
            read_project_id: view.read_project_id.clone(),
            endpoint: crate::config::depot::canonical_endpoint(&view.endpoint)
                .map_or_else(|_| view.endpoint.clone(), |url| url.to_string()),
            enabled: view.enabled,
            auth: view.auth_mode,
            reference: view.bearer_token_env.clone(),
            credential: token.map(|value| Sha256::digest(value.as_bytes()).into()),
            policy: policy.clone(),
        }
    }
}

pub struct Reply {
    pub identity: Identity,
    pub result: Value,
}

pub struct ProviderRuntime {
    incarnation: String,
    key: RuntimeKey,
    client: std::sync::Mutex<Option<Arc<NetworkClient>>>,
    cancellation: CancellationToken,
    pub health: Health,
    admission: ProviderAdmission,
    identity: Mutex<Option<Identity>>,
}

impl ProviderRuntime {
    pub fn new(view: &ProviderView, token: Option<&str>, policy: NetworkPolicy) -> Self {
        let secret = match view.auth_mode {
            AuthMode::Anonymous => Ok(None),
            AuthMode::Bearer => token
                .filter(|value| !value.trim().is_empty())
                .ok_or(Failure::Configuration)
                .and_then(|value| {
                    let bound = if view.host_managed {
                        Secret::local_bearer(&view.id, &view.endpoint, value)
                    } else {
                        Secret::bearer(&view.endpoint, value)
                    };
                    bound.map(Some).map_err(|_| Failure::Configuration)
                }),
        };
        let client = secret.and_then(|secret| {
            let client = if view.host_managed {
                secret
                    .ok_or(NetworkError::CredentialBinding)
                    .and_then(|secret| NetworkClient::local(&view.id, &view.endpoint, secret))
            } else {
                NetworkClient::new(&view.endpoint, secret, policy.clone())
            };
            client.map_err(|_| Failure::Configuration)
        });
        Self {
            incarnation: uuid::Uuid::new_v4().to_string(),
            key: RuntimeKey::new(view, token, &policy),
            client: std::sync::Mutex::new(client.ok().map(Arc::new)),
            cancellation: CancellationToken::new(),
            health: Health::default(),
            admission: ProviderAdmission::default(),
            identity: Mutex::new(None),
        }
    }
    pub fn matches(
        &self,
        view: &ProviderView,
        token: Option<&str>,
        policy: &NetworkPolicy,
    ) -> bool {
        self.key == RuntimeKey::new(view, token, policy)
    }
    pub fn incarnation(&self) -> &str {
        &self.incarnation
    }
    pub fn cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
    /// Last qualified filter observation; never blocks or performs upstream work.
    /// `None` means unknown (including refresh contention), unlike a qualified
    /// empty list, which means no supported origins were advertised.
    pub fn source_origins_snapshot(&self) -> Option<Vec<String>> {
        if self.cancelled() {
            return None;
        }
        self.identity
            .try_lock()
            .ok()?
            .as_ref()
            .map(|identity| identity.source_origins.clone())
    }
    pub fn cancel(&self) {
        self.cancellation.cancel();
        self.client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    pub async fn qualify(
        &self,
        admission: &Admission,
        manual: bool,
    ) -> Result<Identity, ProviderError> {
        self.qualify_inner(admission, manual, false).await
    }

    /// Refresh buffered-read authority without bypassing automatic retry policy.
    pub async fn revalidate(&self, admission: &Admission) -> Result<Identity, ProviderError> {
        self.qualify_inner(admission, false, true).await
    }

    async fn qualify_inner(
        &self,
        admission: &Admission,
        manual: bool,
        refresh: bool,
    ) -> Result<Identity, ProviderError> {
        self.check(manual)?;
        let mut identity = self
            .identity
            .try_lock()
            .map_err(|_| ProviderError::Pending)?;
        if !manual
            && !refresh
            && let Some(identity) = identity.as_ref()
        {
            return Ok(identity.clone());
        }
        let provenance = if manual {
            Provenance::Probe
        } else {
            Provenance::Qualification
        };
        let result = self
            .request(Operation::Identity, None, admission)
            .await
            .map_err(|error| match error {
                ProviderError::Failed(Failure::NotFound | Failure::SnapshotChanged) => {
                    ProviderError::Failed(Failure::Incompatible)
                }
                other => other,
            })
            .and_then(Identity::parse)
            .and_then(|current| {
                if refresh
                    && identity
                        .as_ref()
                        .is_some_and(|previous| !previous.same_authority(&current))
                {
                    Err(ProviderError::Failed(Failure::SnapshotChanged))
                } else {
                    Ok(current)
                }
            });
        self.observe(&result, provenance)?;
        if let Ok(qualified) = &result {
            *identity = Some(qualified.clone());
        } else {
            *identity = None;
        }
        result
    }

    pub async fn call(
        &self,
        operation: Operation,
        body: Value,
        admission: &Admission,
    ) -> Result<Reply, ProviderError> {
        let expected = self.qualify(admission, false).await?;
        let provenance = match operation {
            Operation::List => Provenance::List,
            Operation::Get => Provenance::Get,
            Operation::Identity => Provenance::Qualification,
        };
        let result = self
            .request(operation, Some(body), admission)
            .await
            .and_then(|mut value| {
                let result = value
                    .get_mut("result")
                    .map(Value::take)
                    .ok_or(ProviderError::Failed(Failure::Incompatible))?;
                let identity = Identity::parse(value)?;
                if !expected.same_authority(&identity) {
                    return Err(ProviderError::Failed(Failure::SnapshotChanged));
                }
                Ok(Reply { identity, result })
            });
        self.observe(&result, provenance)?;
        if matches!(result, Err(ProviderError::Failed(Failure::SnapshotChanged))) {
            *self.identity.lock().await = None;
        }
        result
    }

    fn check(&self, manual: bool) -> Result<(), ProviderError> {
        if self.cancelled() {
            return Err(ProviderError::Stale);
        }
        if !self.key.enabled {
            return Err(ProviderError::Disabled);
        }
        self.health.admit(manual)
    }
    async fn request(
        &self,
        operation: Operation,
        body: Option<Value>,
        admission: &Admission,
    ) -> Result<Value, ProviderError> {
        if self.cancelled() {
            return Err(ProviderError::Stale);
        }
        let client = self
            .client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(ProviderError::Failed(Failure::Configuration))?;
        let _permit = admission
            .try_call(&self.admission)
            .map_err(|_| ProviderError::Pending)?;
        tokio::select! {
            biased;
            () = self.cancellation.cancelled() => Err(ProviderError::Stale),
            result = client.call(operation, body, admission.deadline()) => result.map_err(network_failure),
        }
    }
    fn observe<T>(
        &self,
        result: &Result<T, ProviderError>,
        provenance: Provenance,
    ) -> Result<(), ProviderError> {
        if self.cancelled() {
            return Err(ProviderError::Stale);
        }
        match result {
            Ok(_) => self.health.record(Ok(()), provenance),
            Err(ProviderError::Failed(failure)) => self.health.record(Err(*failure), provenance),
            _ => {}
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn retains_test_client(&self) -> bool {
        self.client.lock().unwrap().is_some()
    }
    #[cfg(test)]
    pub(super) fn from_test_client(client: NetworkClient) -> Self {
        let view = crate::config::depot::DepotPreferences::default()
            .resolve(&Default::default())
            .providers
            .remove(0);
        let mut runtime = Self::new(&view, None, NetworkPolicy::default());
        *runtime.client.get_mut().unwrap() = Some(Arc::new(client));
        runtime
    }
}

fn network_failure(error: NetworkError) -> ProviderError {
    ProviderError::Failed(match error {
        NetworkError::Status(401 | 403) => Failure::Unauthorized,
        NetworkError::Status(404) => Failure::NotFound,
        NetworkError::Status(409) => Failure::SnapshotChanged,
        NetworkError::InvalidEndpoint | NetworkError::Blocked | NetworkError::CredentialBinding => {
            Failure::Configuration
        }
        NetworkError::TooLarge
        | NetworkError::InvalidResponse
        | NetworkError::Status(400..=499) => Failure::Incompatible,
        _ => Failure::Transient,
    })
}

#[cfg(test)]
mod source_snapshot_tests {
    use super::*;
    use crate::config::depot::DepotPreferences;
    use crate::dispatch::depot::manager::{Manager, SecretSnapshot};
    use serde_json::json;

    #[test]
    fn source_origin_status_distinguishes_unknown_empty_and_contention_without_io() {
        let preferences: DepotPreferences = toml::from_str(
            r#"
public_enabled = false
[[providers]]
id = "team"
name = "Team"
endpoint = "https://example.invalid/depot"
enabled = true
auth_mode = "anonymous"
"#,
        )
        .unwrap();
        let manager = Manager::new(
            &preferences,
            SecretSnapshot::default(),
            NetworkPolicy::default(),
        );
        let runtime = manager.snapshot().providers["team"].runtime.clone();
        let assert_projection = |expected: Value| {
            for rows in [
                serde_json::to_value(manager.status()).unwrap(),
                serde_json::to_value(manager.admin_status("version")).unwrap(),
            ] {
                let row = rows
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["id"] == "team")
                    .unwrap();
                assert_eq!(row.get("sourceOrigins"), Some(&expected));
                // Reading capabilities must not qualify or mutate health.
                assert_eq!(row["health"]["state"], "unknown");
                assert_eq!(row["health"]["observedAt"], Value::Null);
            }
        };
        assert_projection(Value::Null);
        let mut document = json!({"contractVersion":"depot.discovery/v1","deploymentId":"deployment",
            "deploymentEpoch":"boot","authorityEpoch":"authority","listingEpoch":"listing",
            "snapshotContinuations":true,"maxPageSize":200});
        *runtime.identity.try_lock().unwrap() = Some(Identity::parse(document.clone()).unwrap());
        assert_projection(json!([]));
        document["filters"] =
            json!({"sourceOrigin":{"version":"source-origin/v1","values":["ard","mcp-registry"]}});
        *runtime.identity.try_lock().unwrap() = Some(Identity::parse(document).unwrap());
        assert_projection(json!(["ard", "mcp-registry"]));
        {
            let _refresh_guard = runtime.identity.try_lock().unwrap();
            assert_projection(Value::Null);
        }
        assert_projection(json!(["ard", "mcp-registry"]));
        runtime.cancel();
        assert_projection(Value::Null);
    }
}
