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
}

impl Identity {
    pub fn parse(value: Value) -> Result<Self, ProviderError> {
        let parsed: Self = serde_json::from_value(value)
            .map_err(|_| ProviderError::Failed(Failure::Incompatible))?;
        if parsed.contract_version != "depot.discovery/v1"
            || !parsed.snapshot_continuations
            || !(1..=200).contains(&parsed.max_page_size)
        {
            return Err(ProviderError::Failed(Failure::Incompatible));
        }
        Ok(parsed)
    }
    pub fn same_authority(&self, other: &Self) -> bool {
        self.deployment_id == other.deployment_id
            && self.deployment_epoch == other.deployment_epoch
            && self.authority_epoch == other.authority_epoch
    }
}

#[derive(PartialEq, Eq)]
struct RuntimeKey {
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
                    Secret::bearer(&view.endpoint, value)
                        .map(Some)
                        .map_err(|_| Failure::Configuration)
                }),
        };
        let client = secret.and_then(|secret| {
            NetworkClient::new(&view.endpoint, secret, policy.clone())
                .map_err(|_| Failure::Configuration)
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
        self.check(manual)?;
        let mut identity = self
            .identity
            .try_lock()
            .map_err(|_| ProviderError::Pending)?;
        if !manual && let Some(identity) = identity.as_ref() {
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
            .and_then(Identity::parse);
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
