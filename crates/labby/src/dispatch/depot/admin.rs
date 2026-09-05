//! Provider lifecycle policy and durable commit orchestration.
use super::manager::{Candidate, Manager};
use super::store::{Outcome as StoreOutcome, Pair, Store, StoreError};
use crate::config::depot::{AuthMode, ProviderView, canonical_endpoint};
use labby_auth::browser_authority::BrowserAuthority;
use labby_auth::reauth::{Outcome as ProofOutcome, ProofError, ProofHandle, Proofs, Purpose};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

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
}
impl Admin {
    pub fn new(manager: Arc<Manager>, store: Arc<Store>, proofs: Proofs) -> Self {
        Self {
            manager,
            store,
            proofs,
        }
    }

    pub async fn commit_candidate(
        &self,
        authority: &BrowserAuthority,
        proof: String,
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
        let proof = ProofHandle::parse(proof).map_err(|_| AdminError::FreshAuth)?;
        let reservation = self
            .proofs
            .reserve(&proof, authority, &purpose)
            .await
            .map_err(map_proof)?;
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
        self.proofs
            .finalize(&reservation, ProofOutcome::Committed)
            .await
            .map_err(map_proof)?;
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
