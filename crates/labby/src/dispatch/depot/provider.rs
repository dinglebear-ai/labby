//! Immutable connection incarnation and explicit discovery contract adapter.
use super::health::{Failure, Health, Provenance};
use super::network::{NetworkClient, NetworkError, NetworkPolicy, Operation, Secret};
use super::scheduler::{Admission, ProviderAdmission};
use crate::config::depot::{AuthMode, OpaqueEpoch, ProviderView};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
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
    #[error("Depot provider does not support the requested kind filter")]
    UnsupportedKind,
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
    #[serde(default)]
    pub supported_kinds: Vec<String>,
}

impl Identity {
    pub fn parse(value: Value) -> Result<Self, ProviderError> {
        let parsed: Self = serde_json::from_value(value)
            .map_err(|_| ProviderError::Failed(Failure::Incompatible))?;
        if parsed.contract_version != "depot.discovery/v1"
            || !parsed.snapshot_continuations
            || !(1..=200).contains(&parsed.max_page_size)
            || parsed.supported_kinds.len() > 64
            || parsed
                .supported_kinds
                .iter()
                .any(|kind| kind.is_empty() || kind.len() > 64)
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
    provider_id: String,
    host_managed: bool,
    read_project_id: Option<String>,
    expected_deployment_id: Option<OpaqueEpoch>,
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
            expected_deployment_id: view.expected_deployment_id.clone(),
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
    identity_invalidated: AtomicBool,
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
            identity_invalidated: AtomicBool::new(false),
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
        // Sibling discovery buckets share qualification without consuming a
        // call permit while waiting. Keep the original receipt deadline and
        // incarnation cancellation; a waiter must never outlive either.
        let mut identity = tokio::select! {
            biased;
            () = self.cancellation.cancelled() => return Err(ProviderError::Stale),
            () = tokio::time::sleep_until(admission.deadline()) => return Err(ProviderError::Pending),
            identity = self.identity.lock() => identity,
        };
        // The preceding qualification may have recorded a sticky failure.
        self.check(manual)?;
        if self.identity_invalidated.swap(false, Ordering::AcqRel) {
            *identity = None;
        }
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
            .and_then(Identity::parse)
            .and_then(|identity| {
                if self
                    .key
                    .expected_deployment_id
                    .as_ref()
                    .is_some_and(|expected| expected != &identity.deployment_id)
                {
                    Err(ProviderError::Failed(Failure::Configuration))
                } else {
                    Ok(identity)
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
            Operation::SkillsSearch => Provenance::List,
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
            // Publish invalidation even when a concurrent probe owns the lock.
            // Cleanup must not extend this call beyond its admission deadline.
            self.identity_invalidated.store(true, Ordering::Release);
            if let Ok(mut identity) = self.identity.try_lock() {
                *identity = None;
            }
        }
        result
    }

    /// Execute Depot's bounded native Skill index search through this exact
    /// provider runtime. Qualification first preserves deployment binding;
    /// the configured anonymous/bearer credential remains server-held.
    pub async fn search_skills(
        &self,
        query: &str,
        limit: usize,
        admission: &Admission,
    ) -> Result<Value, ProviderError> {
        let _identity = self.qualify(admission, false).await?;
        let result = self
            .request(
                Operation::SkillsSearch,
                Some(serde_json::json!({"query": query, "limit": limit})),
                admission,
            )
            .await
            .and_then(|mut value| {
                value
                    .get_mut("result")
                    .map(Value::take)
                    .ok_or(ProviderError::Failed(Failure::Incompatible))
            });
        self.observe(&result, Provenance::List)?;
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
        NetworkError::Timeout => Failure::Timeout,
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
mod public_binding_tests {
    use super::*;

    #[tokio::test]
    async fn snapshot_change_does_not_wait_for_a_contended_identity_lock() {
        use super::super::{network_tests::tls_fixture, scheduler::Scheduler};
        let (client, mut received) =
            tls_fixture("HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\n\r\n".into()).await;
        let runtime = ProviderRuntime::from_test_client(client);
        *runtime.identity.lock().await = Some(Identity::parse(serde_json::json!({
            "contractVersion":"depot.discovery/v1", "deploymentId":"public",
            "deploymentEpoch":"deployment", "authorityEpoch":"authority", "listingEpoch":"listing",
            "snapshotContinuations":true, "maxPageSize":200
        })).unwrap());
        let scheduler = Scheduler::default();
        let admission = scheduler
            .admit(
                "reader",
                tokio::time::Instant::now() - std::time::Duration::from_millis(4800),
            )
            .await
            .unwrap();
        let mut call =
            Box::pin(runtime.call(Operation::List, serde_json::json!({"limit":1}), &admission));
        tokio::select! {
            biased;
            request = &mut received => { request.unwrap(); },
            _ = &mut call => panic!("request finished before contention was installed"),
        }
        let probe_lock = runtime.identity.lock().await;
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_millis(400), &mut call)
                .await
                .expect("snapshot invalidation must not wait behind a probe"),
            Err(ProviderError::Failed(Failure::SnapshotChanged))
        ));
        drop(probe_lock);
        drop(call);
        drop(admission);
        let fresh = scheduler
            .admit("reader", tokio::time::Instant::now())
            .await
            .unwrap();
        assert!(
            runtime.qualify(&fresh, false).await.is_err(),
            "the closed fixture must be contacted again instead of reusing stale cached qualification"
        );
    }

    #[test]
    fn network_timeout_keeps_its_failure_classification() {
        let ProviderError::Failed(failure) = network_failure(NetworkError::Timeout) else {
            panic!("a network timeout must remain a provider failure");
        };
        assert_eq!(serde_json::to_value(failure).unwrap(), "timeout");
    }

    #[tokio::test]
    async fn queued_qualification_stops_when_the_runtime_is_cancelled() {
        use super::super::{network_tests::tls_fixture, scheduler::Scheduler};
        let (client, mut received) = tls_fixture(String::new()).await;
        let runtime = ProviderRuntime::from_test_client(client);
        let _identity = runtime.identity.lock().await;
        let scheduler = Scheduler::default();
        let admission = scheduler
            .admit("actor", tokio::time::Instant::now())
            .await
            .unwrap();
        let mut queued = Box::pin(runtime.qualify(&admission, false));
        assert!(futures::poll!(queued.as_mut()).is_pending());
        runtime.cancel();
        assert_eq!(queued.await, Err(ProviderError::Stale));
        assert!(matches!(
            received.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn queued_qualification_keeps_the_original_admission_deadline() {
        use super::super::{network_tests::tls_fixture, scheduler::Scheduler};
        let (client, mut received) = tls_fixture(String::new()).await;
        let runtime = ProviderRuntime::from_test_client(client);
        let _identity = runtime.identity.lock().await;
        let scheduler = Scheduler::default();
        let admission = scheduler
            .admit(
                "actor",
                tokio::time::Instant::now() - std::time::Duration::from_millis(4900),
            )
            .await
            .unwrap();
        let mut queued = Box::pin(runtime.qualify(&admission, false));
        assert!(futures::poll!(queued.as_mut()).is_pending());
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), queued)
                .await
                .unwrap(),
            Err(ProviderError::Pending)
        );
        assert!(matches!(
            received.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(
            runtime.health.view().state,
            super::super::health::HealthState::Unknown
        );
    }

    #[tokio::test]
    async fn qualified_deployment_must_match_the_host_binding() {
        use super::super::{network_tests::tls_fixture, scheduler::Scheduler};
        for expected in ["catalog", "wrong-catalog"] {
            let body = serde_json::json!({"contractVersion":"depot.discovery/v1","deploymentId":"catalog","deploymentEpoch":"boot","authorityEpoch":"read","listingEpoch":"1","snapshotContinuations":true,"maxPageSize":200}).to_string();
            let (client, _) = tls_fixture(format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            ))
            .await;
            let mut runtime = ProviderRuntime::from_test_client(client);
            runtime.key.expected_deployment_id = Some(expected.to_owned().try_into().unwrap());
            let scheduler = Scheduler::default();
            let admission = scheduler
                .admit("verified-actor", tokio::time::Instant::now())
                .await
                .unwrap();
            assert_eq!(
                runtime.qualify(&admission, false).await.is_ok(),
                expected == "catalog"
            );
        }
    }
}
