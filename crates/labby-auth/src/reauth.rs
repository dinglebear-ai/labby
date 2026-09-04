//! Short-lived action proofs. The product's durable operation journal remains
//! authoritative for committed outcomes after proof expiry or process restart.
use crate::browser_authority::{
    AuthorityBinding, AuthorityError, BrowserAuthority, keyed_fingerprint,
};
use crate::sqlite::SqliteStore;
use crate::util::now_unix;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProofError {
    #[error("recent authentication is required")]
    Required,
    #[error("recent authentication is unsupported")]
    Unsupported,
    #[error("recent authentication proof expired")]
    Expired,
    #[error("recent authentication proof does not match this operation")]
    Replayed,
    #[error("current browser authority is required")]
    Denied,
    #[error("recent authentication is unavailable")]
    Unavailable,
    #[error("recent authentication rate limit reached")]
    RateLimited,
    #[error("recent authentication proof capacity reached")]
    Capacity,
    #[error("recent authentication purpose is invalid")]
    InvalidPurpose,
}
impl ProofError {
    pub const fn kind(self) -> &'static str {
        match self {
            Self::Required => "recent_auth_required",
            Self::Unsupported => "recent_auth_unsupported",
            Self::Expired => "recent_auth_expired",
            Self::Replayed => "recent_auth_replayed",
            Self::Denied => "auth_failed",
            Self::Unavailable => "recent_auth_unavailable",
            Self::RateLimited => "rate_limited",
            Self::Capacity => "recent_auth_capacity",
            Self::InvalidPurpose => "validation_failed",
        }
    }
}
impl From<AuthorityError> for ProofError {
    fn from(error: AuthorityError) -> Self {
        match error {
            AuthorityError::Unavailable => Self::Unavailable,
            _ => Self::Denied,
        }
    }
}

/// Only a verified provider callback inside this crate can construct an event.
/// A caller cannot manufacture freshness from a cookie or a client timestamp.
/// ```compile_fail
/// let event = labby_auth::reauth::TrustedAuthEvent { authenticated_at: 123 };
/// ```
pub struct TrustedAuthEvent {
    pub(crate) binding: AuthorityBinding,
    pub(crate) authenticated_at: i64,
}

impl TrustedAuthEvent {
    pub(crate) fn from_google(
        authority: &BrowserAuthority,
        evidence: &crate::google::GoogleFreshAuth,
    ) -> Result<Self, ProofError> {
        if authority.identity_provider() != Some("google")
            || authority.session_snapshot().subject != evidence.subject()
        {
            return Err(ProofError::Denied);
        }
        Ok(Self {
            binding: authority.binding(),
            authenticated_at: evidence.authenticated_at(),
        })
    }
}

pub struct Purpose {
    pub(crate) digest: [u8; 32],
    pub(crate) operation: String,
    scope: String,
}
impl std::fmt::Debug for Purpose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Purpose(<bound>)")
    }
}
impl Purpose {
    pub fn new(
        action: &str,
        resource: &str,
        version: &str,
        operation: &str,
        scope: &str,
        payload: &Value,
    ) -> Result<Self, ProofError> {
        if [action, resource, version, operation, scope]
            .iter()
            .any(|value| value.is_empty() || value.len() > 128)
        {
            return Err(ProofError::InvalidPurpose);
        }
        let payload = canonical(payload, 0, &mut 65_536)?;
        let encoded = serde_json::to_vec(&(action, resource, version, operation, scope, payload))
            .map_err(|_| ProofError::InvalidPurpose)?;
        if encoded.len() > 65_536 {
            return Err(ProofError::InvalidPurpose);
        }
        Ok(Self {
            digest: keyed_fingerprint("reauth.purpose.v1", &[&encoded])?,
            operation: operation.to_owned(),
            scope: scope.to_owned(),
        })
    }

    pub(crate) fn stored_parts(&self) -> ([u8; 32], &str, &str) {
        (self.digest, &self.operation, &self.scope)
    }

    pub(crate) fn from_stored(
        digest: [u8; 32],
        operation: String,
        scope: String,
    ) -> Result<Self, ProofError> {
        if operation.is_empty() || operation.len() > 128 || scope.is_empty() || scope.len() > 128 {
            return Err(ProofError::InvalidPurpose);
        }
        Ok(Self {
            digest,
            operation,
            scope,
        })
    }
}

#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct ProofHandle(String);
impl std::fmt::Debug for ProofHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProofHandle(<redacted>)")
    }
}
impl ProofHandle {
    pub fn parse(value: String) -> Result<Self, ProofError> {
        if value.len() != 43 {
            return Err(ProofError::Expired);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(&value)
            .map_err(|_| ProofError::Expired)?;
        if bytes.len() != 32 || URL_SAFE_NO_PAD.encode(bytes) != value {
            return Err(ProofError::Expired);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedProof {
    pub proof: ProofHandle,
    pub expires_at: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Committed,
    Aborted,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationState {
    Reserved,
    Finalized(Outcome),
}
#[derive(Clone)]
pub(crate) struct ProofBinding {
    pub session_snapshot: crate::types::BrowserSessionRow,
    pub hash: [u8; 32],
    pub actor: [u8; 32],
    pub session: [u8; 32],
    pub authority: [u8; 32],
    pub purpose: [u8; 32],
    pub operation: String,
}
pub struct Reservation {
    binding: ProofBinding,
    state: ReservationState,
}
impl Reservation {
    pub fn state(&self) -> ReservationState {
        self.state
    }
}
#[derive(Clone)]
pub struct Proofs {
    store: SqliteStore,
}
impl Proofs {
    pub fn new(store: SqliteStore) -> Self {
        Self { store }
    }
    pub async fn issue(
        &self,
        authority: &BrowserAuthority,
        event: &TrustedAuthEvent,
        purpose: &Purpose,
    ) -> Result<IssuedProof, ProofError> {
        check_scope(authority, purpose).await?;
        let now = now_unix();
        if !event.binding.matches(&authority.binding())
            || event.authenticated_at > now
            || event.authenticated_at <= now - 300
        {
            return Err(ProofError::Required);
        }
        let (_, session_expiry) = authority.proof_parts();
        let expires_at = (now + 120)
            .min(event.authenticated_at + 300)
            .min(session_expiry);
        let proof =
            ProofHandle(crate::util::random_token(32).map_err(|_| ProofError::Unavailable)?);
        self.store
            .reauth_insert(
                binding(&proof, authority, purpose),
                event.authenticated_at,
                expires_at,
            )
            .await?;
        Ok(IssuedProof { proof, expires_at })
    }
    /// Repeated reservation is allowed only for the identical logical operation.
    pub async fn reserve(
        &self,
        proof: &ProofHandle,
        authority: &BrowserAuthority,
        purpose: &Purpose,
    ) -> Result<Reservation, ProofError> {
        check_scope(authority, purpose).await?;
        let binding = binding(proof, authority, purpose);
        let state = self.store.reauth_reserve(binding.clone()).await?;
        Ok(Reservation { binding, state })
    }
    /// Called after durable commit/abort. Expiry cannot falsely undo a commit.
    pub async fn finalize(
        &self,
        reservation: &Reservation,
        outcome: Outcome,
    ) -> Result<(), ProofError> {
        self.store
            .reauth_finalize(reservation.binding.clone(), outcome)
            .await
    }
}
async fn check_scope(authority: &BrowserAuthority, purpose: &Purpose) -> Result<(), ProofError> {
    if !authority.revalidate().await?.has_scope(&purpose.scope) {
        return Err(ProofError::Denied);
    }
    Ok(())
}
fn binding(proof: &ProofHandle, authority: &BrowserAuthority, purpose: &Purpose) -> ProofBinding {
    let session_snapshot = authority.session_snapshot();
    let ([actor, session, authority], _) = authority.proof_parts();
    ProofBinding {
        session_snapshot,
        hash: Sha256::digest(proof.0.as_bytes()).into(),
        actor,
        session,
        authority,
        purpose: purpose.digest,
        operation: purpose.operation.clone(),
    }
}
fn canonical(value: &Value, depth: usize, budget: &mut usize) -> Result<Value, ProofError> {
    if depth > 64 {
        return Err(ProofError::InvalidPurpose);
    }
    *budget = budget.checked_sub(24).ok_or(ProofError::InvalidPurpose)?;
    match value {
        Value::Object(object) => {
            let sorted: std::collections::BTreeMap<_, _> = object.iter().collect();
            let mut output = serde_json::Map::new();
            for (key, value) in sorted {
                *budget = budget
                    .checked_sub(key.len())
                    .ok_or(ProofError::InvalidPurpose)?;
                output.insert(key.clone(), canonical(value, depth + 1, budget)?);
            }
            Ok(Value::Object(output))
        }
        Value::Array(array) => array
            .iter()
            .map(|value| canonical(value, depth + 1, budget))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        Value::String(string) => {
            *budget = budget
                .checked_sub(string.len())
                .ok_or(ProofError::InvalidPurpose)?;
            Ok(value.clone())
        }
        _ => Ok(value.clone()),
    }
}
