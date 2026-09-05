//! Provider lifecycle policy and durable commit orchestration.
use super::manager::{Candidate, Manager, SecretSnapshot};
use super::network::NetworkPolicy;
use super::store::{Outcome as StoreOutcome, Pair, Store, StoreError};
use crate::config::depot::{
    AuthMode, DepotPreferences, ProviderConfig, ProviderView, canonical_endpoint, valid_provider_id,
};
use labby_auth::browser_authority::BrowserAuthority;
use labby_auth::reauth::{Outcome as ProofOutcome, ProofError, ProofHandle, Proofs, Purpose};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::str::FromStr as _;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutation {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub enabled: bool,
    pub auth_mode: AuthMode,
    pub credential: CredentialChange,
}

#[derive(Debug)]
pub struct BuiltMutation {
    pub pair: Pair,
    pub preferences: DepotPreferences,
    pub secrets: SecretSnapshot,
    pub view: ProviderView,
    pub needs_fresh_proof: bool,
}

pub fn build_upsert(
    config: &str,
    environment: &str,
    mutation: &Mutation,
) -> Result<BuiltMutation, AdminError> {
    if mutation.id == "public" {
        if mutation.name != "Public Depot"
            || mutation.endpoint != crate::config::depot::PUBLIC_ENDPOINT
            || mutation.auth_mode != AuthMode::Anonymous
            || mutation.credential != CredentialChange::Retain
        {
            return Err(AdminError::Invalid);
        }
        let mut document = if config.trim().is_empty() {
            toml_edit::DocumentMut::new()
        } else {
            toml_edit::DocumentMut::from_str(config).map_err(|_| AdminError::Invalid)?
        };
        let mut preferences = parse_preferences(config)?;
        preferences.public_enabled = mutation.enabled;
        replace_depot_table(&mut document, &preferences)?;
        return Ok(BuiltMutation {
            pair: Pair {
                config: document.to_string(),
                environment: environment.to_owned(),
            },
            preferences,
            secrets: SecretSnapshot::from_values(parse_environment(environment)?),
            view: ProviderView {
                id: "public".into(),
                name: "Public Depot".into(),
                endpoint: crate::config::depot::PUBLIC_ENDPOINT.into(),
                enabled: mutation.enabled,
                auth_mode: AuthMode::Anonymous,
                bearer_token_env: None,
            },
            needs_fresh_proof: false,
        });
    }
    if !valid_provider_id(&mutation.id) || matches!(mutation.id.as_str(), "all" | "legacy") {
        return Err(AdminError::Invalid);
    }
    let env_key = format!(
        "LABBY_DEPOT_PROVIDER_{}_TOKEN",
        mutation.id.replace('-', "_").to_ascii_uppercase()
    );
    let provider = ProviderConfig {
        id: mutation.id.clone(),
        name: mutation.name.clone(),
        endpoint: mutation.endpoint.clone(),
        enabled: mutation.enabled,
        auth_mode: mutation.auth_mode,
        bearer_token_env: (mutation.auth_mode == AuthMode::Bearer).then(|| env_key.clone()),
    };
    provider.validate().map_err(|_| AdminError::Invalid)?;
    let mut document = if config.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        toml_edit::DocumentMut::from_str(config).map_err(|_| AdminError::Invalid)?
    };
    let mut preferences = parse_preferences(config)?;
    let current = preferences
        .providers
        .iter()
        .position(|raw| raw.get("id").and_then(toml::Value::as_str) == Some(&mutation.id));
    let current_view = current
        .and_then(|index| {
            preferences.providers[index]
                .clone()
                .try_into::<ProviderConfig>()
                .ok()
        })
        .map(ProviderView::from);
    let policy = change_policy(
        current_view.as_ref(),
        &ProviderView::from(provider.clone()),
        &mutation.credential,
    )?;
    let raw = toml::Value::try_from(provider.clone()).map_err(|_| AdminError::Invalid)?;
    if let Some(index) = current {
        preferences.providers[index] = raw;
    } else {
        preferences.providers.push(raw);
    }
    replace_depot_table(&mut document, &preferences)?;
    let mut values = parse_environment(environment)?;
    match &mutation.credential {
        CredentialChange::Retain => {}
        CredentialChange::Replace(value)
            if !value.is_empty() && value.len() <= 8192 && !value.chars().any(char::is_control) =>
        {
            values.insert(env_key.clone(), value.clone());
        }
        CredentialChange::Replace(_) => return Err(AdminError::Invalid),
        CredentialChange::Clear => {
            values.remove(&env_key);
        }
    }
    let environment = render_environment(
        environment,
        &env_key,
        values.get(&env_key).map(String::as_str),
    );
    Ok(BuiltMutation {
        pair: Pair {
            config: document.to_string(),
            environment,
        },
        preferences,
        secrets: SecretSnapshot::from_values(values),
        view: ProviderView::from(provider),
        needs_fresh_proof: policy.needs_fresh_proof,
    })
}

pub fn build_remove(
    config: &str,
    environment: &str,
    provider_id: &str,
) -> Result<BuiltMutation, AdminError> {
    if !valid_provider_id(provider_id) || matches!(provider_id, "all" | "public" | "legacy") {
        return Err(AdminError::Invalid);
    }
    let mut document = if config.trim().is_empty() {
        toml_edit::DocumentMut::new()
    } else {
        toml_edit::DocumentMut::from_str(config).map_err(|_| AdminError::Invalid)?
    };
    let mut preferences = parse_preferences(config)?;
    let Some(index) = preferences
        .providers
        .iter()
        .position(|raw| raw.get("id").and_then(toml::Value::as_str) == Some(provider_id))
    else {
        return Err(AdminError::Invalid);
    };
    let removed = preferences
        .providers
        .remove(index)
        .try_into::<ProviderConfig>()
        .map_err(|_| AdminError::Invalid)?;
    preferences.tombstones.insert(provider_id.to_owned());
    replace_depot_table(&mut document, &preferences)?;
    let mut values = parse_environment(environment)?;
    if let Some(key) = &removed.bearer_token_env {
        values.remove(key);
    }
    let environment = removed.bearer_token_env.as_deref().map_or_else(
        || environment.to_owned(),
        |key| render_environment(environment, key, None),
    );
    let view = ProviderView::from(removed);
    Ok(BuiltMutation {
        pair: Pair {
            config: document.to_string(),
            environment,
        },
        preferences,
        secrets: SecretSnapshot::from_values(values),
        view,
        needs_fresh_proof: true,
    })
}

fn replace_depot_table(
    document: &mut toml_edit::DocumentMut,
    preferences: &DepotPreferences,
) -> Result<(), AdminError> {
    let raw = toml::to_string(preferences).map_err(|_| AdminError::Invalid)?;
    let depot = toml_edit::DocumentMut::from_str(&raw).map_err(|_| AdminError::Invalid)?;
    document["depot"] = toml_edit::Item::Table(depot.as_table().clone());
    Ok(())
}

fn parse_preferences(config: &str) -> Result<DepotPreferences, AdminError> {
    if config.trim().is_empty() {
        return Ok(DepotPreferences::default());
    }
    let root = toml::from_str::<toml::Value>(config).map_err(|_| AdminError::Invalid)?;
    root.get("depot")
        .cloned()
        .map(toml::Value::try_into)
        .transpose()
        .map_err(|_| AdminError::Invalid)
        .map(Option::unwrap_or_default)
}

fn parse_environment(raw: &str) -> Result<BTreeMap<String, String>, AdminError> {
    dotenvy::from_read_iter(raw.as_bytes())
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map_err(|_| AdminError::Invalid)
}

fn render_environment(raw: &str, key: &str, value: Option<&str>) -> String {
    let mut output = String::new();
    for line in raw.lines() {
        if line
            .trim_start()
            .split_once('=')
            .is_some_and(|(candidate, _)| candidate.trim() == key)
        {
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }
    if let Some(value) = value {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        output.push_str(&format!("{key}=\"{escaped}\"\n"));
    }
    output
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action", content = "value")]
pub enum CredentialChange {
    Retain,
    Replace(String),
    Clear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangePolicy {
    pub needs_fresh_proof: bool,
    pub needs_qualification: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum AdminError {
    #[error("provider change is invalid")]
    Invalid,
    #[error("fresh browser authentication is required")]
    FreshAuth,
    #[error("provider configuration changed")]
    Stale,
    #[error("provider configuration recovery is required")]
    Recovery,
}

pub fn change_policy(
    current: Option<&ProviderView>,
    next: &ProviderView,
    credential: &CredentialChange,
) -> Result<ChangePolicy, AdminError> {
    let endpoint_changed = current.is_some_and(|current| {
        canonical_endpoint(&current.endpoint).ok() != canonical_endpoint(&next.endpoint).ok()
    });
    if matches!(credential, CredentialChange::Retain) && endpoint_changed {
        return Err(AdminError::FreshAuth);
    }
    if next.auth_mode == AuthMode::Bearer
        && matches!(credential, CredentialChange::Clear)
        && next.enabled
    {
        return Err(AdminError::Invalid);
    }
    if current.is_none()
        && next.auth_mode == AuthMode::Bearer
        && !matches!(credential, CredentialChange::Replace(value) if !value.trim().is_empty())
    {
        return Err(AdminError::FreshAuth);
    }
    let authority_changed =
        current.is_some_and(|current| current.auth_mode != next.auth_mode || endpoint_changed);
    Ok(ChangePolicy {
        needs_fresh_proof: authority_changed || !matches!(credential, CredentialChange::Retain),
        needs_qualification: next.enabled,
    })
}

pub struct Admin {
    manager: Arc<Manager>,
    store: Arc<Store>,
    proofs: Proofs,
    policy: NetworkPolicy,
}
impl Admin {
    pub fn new(
        manager: Arc<Manager>,
        store: Arc<Store>,
        proofs: Proofs,
        policy: NetworkPolicy,
    ) -> Self {
        Self {
            manager,
            store,
            proofs,
            policy,
        }
    }

    pub async fn current_version(&self) -> Result<String, AdminError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || store.current_version())
            .await
            .map_err(|_| AdminError::Recovery)?
            .map_err(map_store)
    }

    pub async fn operation(&self, operation_id: &str) -> Result<StoreOutcome, AdminError> {
        let store = self.store.clone();
        let operation_id = operation_id.to_owned();
        tokio::task::spawn_blocking(move || store.recover())
            .await
            .map_err(|_| AdminError::Recovery)?
            .map_err(map_store)?
            .filter(|outcome| outcome.operation_id == operation_id)
            .ok_or(AdminError::Invalid)
    }

    pub async fn upsert(
        &self,
        authority: &BrowserAuthority,
        proof: Option<String>,
        expected_version: &str,
        operation_id: &str,
        mutation: &Mutation,
        payload: &Value,
    ) -> Result<StoreOutcome, AdminError> {
        let store = self.store.clone();
        let current = tokio::task::spawn_blocking(move || store.read_pair())
            .await
            .map_err(|_| AdminError::Recovery)?
            .map_err(map_store)?;
        let built = build_upsert(&current.config, &current.environment, mutation)?;
        let candidate =
            self.manager
                .prepare(&built.preferences, built.secrets, self.policy.clone());
        self.commit_candidate(
            authority,
            proof,
            built.needs_fresh_proof,
            "providers.upsert",
            &mutation.id,
            expected_version,
            operation_id,
            payload,
            built.pair,
            candidate,
        )
        .await
    }

    pub async fn probe(
        &self,
        mutation: &Mutation,
    ) -> Result<super::health::HealthView, AdminError> {
        let store = self.store.clone();
        let current = tokio::task::spawn_blocking(move || store.read_pair())
            .await
            .map_err(|_| AdminError::Recovery)?
            .map_err(map_store)?;
        let built = build_upsert(&current.config, &current.environment, mutation)?;
        let candidate =
            self.manager
                .prepare(&built.preferences, built.secrets, self.policy.clone());
        let runtime = Arc::clone(
            &candidate
                .provider(&mutation.id)
                .ok_or(AdminError::Invalid)?
                .runtime,
        );
        let admission = self
            .manager
            .scheduler
            .probe(tokio::time::Instant::now())
            .map_err(|_| AdminError::Recovery)?;
        drop(runtime.qualify(&admission, true).await);
        Ok(runtime.health.view())
    }

    pub async fn remove(
        &self,
        authority: &BrowserAuthority,
        proof: String,
        provider_id: &str,
        expected_version: &str,
        operation_id: &str,
        payload: &Value,
    ) -> Result<StoreOutcome, AdminError> {
        let store = self.store.clone();
        let current = tokio::task::spawn_blocking(move || store.read_pair())
            .await
            .map_err(|_| AdminError::Recovery)?
            .map_err(map_store)?;
        let built = build_remove(&current.config, &current.environment, provider_id)?;
        let candidate =
            self.manager
                .prepare(&built.preferences, built.secrets, self.policy.clone());
        self.commit_candidate(
            authority,
            Some(proof),
            true,
            "providers.remove",
            provider_id,
            expected_version,
            operation_id,
            payload,
            built.pair,
            candidate,
        )
        .await
    }

    pub async fn commit_candidate(
        &self,
        authority: &BrowserAuthority,
        proof: Option<String>,
        needs_fresh_proof: bool,
        action: &str,
        provider_id: &str,
        expected_version: &str,
        operation_id: &str,
        payload: &Value,
        pair: Pair,
        candidate: Candidate,
    ) -> Result<StoreOutcome, AdminError> {
        authority
            .revalidate()
            .await
            .map_err(|_| AdminError::FreshAuth)?;
        let purpose = Purpose::new(
            action,
            provider_id,
            expected_version,
            operation_id,
            "lab:admin",
            payload,
        )
        .map_err(|_| AdminError::FreshAuth)?;
        let reservation = if needs_fresh_proof {
            let proof = ProofHandle::parse(proof.ok_or(AdminError::FreshAuth)?)
                .map_err(|_| AdminError::FreshAuth)?;
            Some(
                self.proofs
                    .reserve(&proof, authority, &purpose)
                    .await
                    .map_err(map_proof)?,
            )
        } else {
            None
        };
        authority
            .revalidate()
            .await
            .map_err(|_| AdminError::FreshAuth)?;
        let store = self.store.clone();
        let expected = expected_version.to_owned();
        let operation = operation_id.to_owned();
        let outcome =
            tokio::task::spawn_blocking(move || store.commit(&operation, &expected, &pair))
                .await
                .map_err(|_| AdminError::Recovery)?
                .map_err(map_store)?;
        self.manager
            .publish(candidate)
            .map_err(|_| AdminError::Stale)?;
        if let Some(reservation) = reservation {
            self.proofs
                .finalize(&reservation, ProofOutcome::Committed)
                .await
                .map_err(map_proof)?;
        }
        Ok(outcome)
    }
}

fn map_proof(_: ProofError) -> AdminError {
    AdminError::FreshAuth
}
fn map_store(error: StoreError) -> AdminError {
    match error {
        StoreError::Stale => AdminError::Stale,
        StoreError::RecoveryRequired | StoreError::Durability | StoreError::Busy => {
            AdminError::Recovery
        }
        StoreError::Invalid => AdminError::Invalid,
    }
}
