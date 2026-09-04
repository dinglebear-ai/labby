//! One composition owner for immutable provider topology and secret snapshots.
use super::health::HealthView;
use super::network::NetworkPolicy;
use super::provider::{ProviderError, ProviderRuntime};
use super::scheduler::Scheduler;
use crate::config::depot::{DepotPreferences, LegacyDepot, ProviderView, allowed_secret_reference};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

#[derive(Clone, Default)]
pub struct SecretSnapshot(Arc<BTreeMap<String, String>>);
impl std::fmt::Debug for SecretSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretSnapshot(<redacted>)")
    }
}
impl SecretSnapshot {
    /// Called once at the composition boundary after normal env precedence.
    /// Hot rotation supplies a new snapshot from sanctioned storage instead.
    pub fn capture(config: &DepotPreferences) -> Self {
        let mut keys = BTreeSet::from([
            "LABBY_DEPOT_URL",
            "LABBY_DEPOT_ENABLED",
            "LABBY_DEPOT_TOKEN",
        ]);
        for raw in config.providers.iter().take(16) {
            if let Some(key) = raw
                .get("bearer_token_env")
                .and_then(toml::Value::as_str)
                .filter(|key| allowed_secret_reference(key))
            {
                keys.insert(key);
            }
        }
        Self::from_values(
            keys.into_iter()
                .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
                .collect(),
        )
    }
    pub fn from_values(values: BTreeMap<String, String>) -> Self {
        Self(Arc::new(
            values
                .into_iter()
                .filter(|(key, value)| {
                    value.len() <= 8192
                        && (allowed_secret_reference(key)
                            || matches!(key.as_str(), "LABBY_DEPOT_URL" | "LABBY_DEPOT_ENABLED"))
                })
                .take(19)
                .collect(),
        ))
    }
    fn token(&self, view: &ProviderView) -> Option<&str> {
        view.bearer_token_env
            .as_ref()
            .and_then(|key| self.0.get(key))
            .map(String::as_str)
    }
    fn legacy(&self) -> LegacyDepot {
        LegacyDepot {
            url: self.0.get("LABBY_DEPOT_URL").cloned(),
            enabled: self
                .0
                .get("LABBY_DEPOT_ENABLED")
                .map(|value| matches!(value.as_str(), "1" | "true")),
            token_present: self
                .0
                .get("LABBY_DEPOT_TOKEN")
                .is_some_and(|value| !value.trim().is_empty()),
        }
    }
}

#[derive(Clone)]
pub struct Provider {
    pub view: ProviderView,
    pub runtime: Arc<ProviderRuntime>,
}
pub struct Topology {
    pub version: String,
    pub membership_epoch: String,
    pub providers: BTreeMap<String, Provider>,
    pub diagnostics: Vec<crate::config::depot::ConfigDiagnostic>,
}
pub struct Candidate {
    expected_version: String,
    topology: Arc<Topology>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatus {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub health: HealthView,
}

pub struct Manager {
    topology: RwLock<Arc<Topology>>,
    pub scheduler: Scheduler,
}
impl Default for Manager {
    fn default() -> Self {
        Self::new(
            &DepotPreferences::default(),
            SecretSnapshot::default(),
            NetworkPolicy::default(),
        )
    }
}
impl Manager {
    /// Pure construction. Qualification is lazy and never gates readiness.
    pub fn new(config: &DepotPreferences, secrets: SecretSnapshot, policy: NetworkPolicy) -> Self {
        Self {
            topology: RwLock::new(Arc::new(build(config, secrets, policy, None))),
            scheduler: Scheduler::default(),
        }
    }
    pub fn snapshot(&self) -> Arc<Topology> {
        self.topology
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
    /// Prebuild outside the host config lock; publication does no network I/O.
    pub fn prepare(
        &self,
        config: &DepotPreferences,
        secrets: SecretSnapshot,
        policy: NetworkPolicy,
    ) -> Candidate {
        let previous = self.snapshot();
        Candidate {
            expected_version: previous.version.clone(),
            topology: Arc::new(build(config, secrets, policy, Some(&previous))),
        }
    }
    pub fn publish(&self, candidate: Candidate) -> Result<(), ProviderError> {
        let mut current = self
            .topology
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if current.version != candidate.expected_version {
            return Err(ProviderError::Stale);
        }
        for (id, provider) in &current.providers {
            if !candidate
                .topology
                .providers
                .get(id)
                .is_some_and(|next| Arc::ptr_eq(&provider.runtime, &next.runtime))
            {
                provider.runtime.cancel();
            }
        }
        *current = candidate.topology;
        Ok(())
    }
    pub fn is_current(&self, id: &str, incarnation: &str) -> bool {
        self.snapshot().providers.get(id).is_some_and(|provider| {
            provider.runtime.incarnation() == incarnation && !provider.runtime.cancelled()
        })
    }
    pub fn status(&self) -> Vec<ProviderStatus> {
        self.snapshot()
            .providers
            .values()
            .map(|provider| ProviderStatus {
                id: provider.view.id.clone(),
                name: provider.view.name.clone(),
                enabled: provider.view.enabled,
                health: provider.runtime.health.view(),
            })
            .collect()
    }
}

fn build(
    config: &DepotPreferences,
    secrets: SecretSnapshot,
    policy: NetworkPolicy,
    previous: Option<&Topology>,
) -> Topology {
    let resolved = config.resolve(&secrets.legacy());
    let providers: BTreeMap<_, _> = resolved
        .providers
        .into_iter()
        .map(|view| {
            let token = secrets.token(&view);
            let runtime = previous
                .and_then(|old| old.providers.get(&view.id))
                .filter(|old| old.runtime.matches(&view, token, &policy))
                .map_or_else(
                    || Arc::new(ProviderRuntime::new(&view, token, policy.clone())),
                    |old| old.runtime.clone(),
                );
            (view.id.clone(), Provider { view, runtime })
        })
        .collect();
    let membership = |providers: &BTreeMap<String, Provider>| {
        providers
            .iter()
            .filter(|(_, provider)| provider.view.enabled)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    };
    let membership_epoch = previous
        .filter(|old| membership(&old.providers) == membership(&providers))
        .map_or_else(
            || uuid::Uuid::new_v4().to_string(),
            |old| old.membership_epoch.clone(),
        );
    Topology {
        version: uuid::Uuid::new_v4().to_string(),
        membership_epoch,
        providers,
        diagnostics: resolved.diagnostics,
    }
}

/// Host-file-only exact private address grants. Invalid policy fails closed.
pub fn host_policy(config: &DepotPreferences) -> Result<NetworkPolicy, &'static str> {
    let Some(raw) = config.extra.get("private_hosts") else {
        return Ok(NetworkPolicy::default());
    };
    let private_hosts: BTreeMap<String, BTreeSet<std::net::IpAddr>> = raw
        .clone()
        .try_into()
        .map_err(|_| "invalid Depot host policy")?;
    if private_hosts.len() > 16
        || private_hosts.iter().any(|(host, addresses)| {
            host.is_empty() || host.len() > 253 || addresses.is_empty() || addresses.len() > 32
        })
    {
        return Err("invalid Depot host policy");
    }
    Ok(NetworkPolicy {
        private_hosts,
        #[cfg(test)]
        allow_test_loopback: false,
    })
}
