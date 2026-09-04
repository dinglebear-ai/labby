//! Opaque live browser authority. Session timestamps confer no recent-auth proof.
use crate::sqlite::SqliteStore;
use crate::types::BrowserSessionRow;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::{Digest as _, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use subtle::ConstantTimeEq as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthorityError {
    #[error("verified browser authority required")]
    Denied,
    #[error("browser authority changed")]
    Changed,
    #[error("browser authority is unavailable")]
    Unavailable,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PermissionState {
    pub epoch: String,
    pub scopes: Vec<String>,
}
pub type PolicyFuture<'a> =
    Pin<Box<dyn Future<Output = Result<PermissionState, AuthorityError>> + Send + 'a>>;
/// Inject only the server's live policy, never request-supplied permissions.
pub trait BrowserPolicy: Send + Sync {
    fn current<'a>(&'a self, session: &'a BrowserSessionRow) -> PolicyFuture<'a>;
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AuthorityBinding {
    actor: [u8; 32],
    session: [u8; 32],
    authority: [u8; 32],
}
impl std::fmt::Debug for AuthorityBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthorityBinding(<opaque>)")
    }
}
impl AuthorityBinding {
    pub fn matches(&self, other: &Self) -> bool {
        bool::from(self.authority.ct_eq(&other.authority))
    }
}

#[derive(Clone)]
pub struct BrowserAuthority {
    store: SqliteStore,
    session: BrowserSessionRow,
    source: String,
    policy: Arc<dyn BrowserPolicy>,
    binding: AuthorityBinding,
    permissions: PermissionState,
    google: bool,
}
impl std::fmt::Debug for BrowserAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowserAuthority(<redacted>)")
    }
}
pub struct VerifiedBrowserGrant {
    permissions: PermissionState,
}
impl VerifiedBrowserGrant {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.permissions.scopes.iter().any(|item| item == scope)
    }
}

impl BrowserAuthority {
    pub async fn verify(
        store: SqliteStore,
        session_id: &str,
        source: &str,
        policy: Arc<dyn BrowserPolicy>,
    ) -> Result<Self, AuthorityError> {
        if session_id.is_empty() || session_id.len() > 1024 {
            return Err(AuthorityError::Denied);
        }
        let row = store
            .find_browser_session(session_id)
            .await
            .map_err(|_| AuthorityError::Unavailable)?
            .ok_or(AuthorityError::Denied)?;
        let permissions = policy.current(&row).await?;
        Self::from_checked(store, row, source.to_owned(), policy, permissions)
    }
    fn from_checked(
        store: SqliteStore,
        session: BrowserSessionRow,
        source: String,
        policy: Arc<dyn BrowserPolicy>,
        mut permissions: PermissionState,
    ) -> Result<Self, AuthorityError> {
        if source.is_empty()
            || source.len() > 2048
            || session.subject.is_empty()
            || session.subject.len() > 1024
            || session.csrf_token.is_empty()
            || session.csrf_token.len() > 1024
            || permissions.epoch.is_empty()
            || permissions.epoch.len() > 256
            || permissions.scopes.len() > 64
            || permissions
                .scopes
                .iter()
                .any(|scope| scope.is_empty() || scope.len() > 256)
        {
            return Err(AuthorityError::Denied);
        }
        permissions.scopes.sort();
        permissions.scopes.dedup();
        let row = serde_json::to_vec(&session).map_err(|_| AuthorityError::Unavailable)?;
        if row.len() > 16 * 1024 {
            return Err(AuthorityError::Denied);
        }
        let actor = keyed_fingerprint(
            "browser.actor.v1",
            &[source.as_bytes(), session.subject.as_bytes()],
        )?;
        let session_key = keyed_fingerprint("browser.session.v1", &[&actor, &row])?;
        let scopes =
            serde_json::to_vec(&permissions.scopes).map_err(|_| AuthorityError::Unavailable)?;
        let authority = keyed_fingerprint(
            "browser.authority.v1",
            &[&session_key, permissions.epoch.as_bytes(), &scopes],
        )?;
        Ok(Self {
            store,
            session,
            source,
            policy,
            binding: AuthorityBinding {
                actor,
                session: session_key,
                authority,
            },
            permissions,
            google: false,
        })
    }
    /// Call immediately before every protected cache read or mutation commit.
    pub async fn revalidate(&self) -> Result<VerifiedBrowserGrant, AuthorityError> {
        let fresh = Self::verify(
            self.store.clone(),
            &self.session.session_id,
            &self.source,
            self.policy.clone(),
        )
        .await?;
        if !self.binding.matches(&fresh.binding) {
            return Err(AuthorityError::Changed);
        }
        Ok(VerifiedBrowserGrant {
            permissions: fresh.permissions,
        })
    }
    pub fn actor_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.binding.actor)
    }
    pub fn binding(&self) -> AuthorityBinding {
        self.binding
    }
    pub(crate) fn session_snapshot(&self) -> BrowserSessionRow {
        self.session.clone()
    }
    pub(crate) fn proof_parts(&self) -> ([[u8; 32]; 3], i64) {
        // Durable owner key is private store metadata, never a public capability.
        // Unlike the permission/session MACs, rate accounting survives restart.
        let mut owner = Sha256::new();
        owner.update((self.source.len() as u64).to_be_bytes());
        owner.update(self.source.as_bytes());
        owner.update(self.session.subject.as_bytes());
        (
            [
                owner.finalize().into(),
                self.binding.session,
                self.binding.authority,
            ],
            self.session.expires_at,
        )
    }
    /// Public epoch is one-way and process-bound; it is not a session cookie.
    pub fn public_epoch(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.binding.authority)
    }
    pub fn identity_provider(&self) -> Option<&'static str> {
        self.google.then_some("google")
    }

    #[cfg(feature = "http-axum")]
    pub(crate) async fn from_google(
        state: Arc<crate::state::AuthState>,
        session: BrowserSessionRow,
        scopes: Vec<String>,
    ) -> Result<Self, AuthorityError> {
        let source = format!(
            "google:{}",
            state
                .config
                .public_url
                .as_ref()
                .ok_or(AuthorityError::Unavailable)?
        );
        let store = state.store.clone();
        let policy: Arc<dyn BrowserPolicy> = Arc::new(GooglePolicy { state, scopes });
        let permissions = policy.current(&session).await?;
        let mut authority = Self::from_checked(store, session, source, policy, permissions)?;
        authority.google = true;
        Ok(authority)
    }
    #[cfg(feature = "http-axum")]
    pub(crate) fn from_project(
        store: SqliteStore,
        session: BrowserSessionRow,
        revalidator: Arc<dyn crate::middleware::ProjectSessionRevalidator>,
        grant: &labby_primitives::product_credential::BoundAccessGrant,
    ) -> Result<Self, AuthorityError> {
        let source = format!("project:{}:{}", grant.installation_id, grant.issuer);
        Self::from_checked(
            store,
            session,
            source,
            Arc::new(ProjectPolicy(revalidator)),
            project_permissions(grant)?,
        )
    }
}

pub(crate) fn keyed_fingerprint(label: &str, parts: &[&[u8]]) -> Result<[u8; 32], AuthorityError> {
    static KEY: OnceLock<Result<[u8; 32], AuthorityError>> = OnceLock::new();
    let key = KEY
        .get_or_init(|| {
            let mut key = [0; 32];
            getrandom::fill(&mut key).map_err(|_| AuthorityError::Unavailable)?;
            Ok(key)
        })
        .as_ref()
        .map_err(|error| *error)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| AuthorityError::Unavailable)?;
    mac.update(label.as_bytes());
    for part in parts {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part);
    }
    Ok(mac.finalize().into_bytes().into())
}

#[cfg(feature = "http-axum")]
struct GooglePolicy {
    state: Arc<crate::state::AuthState>,
    scopes: Vec<String>,
}
#[cfg(feature = "http-axum")]
impl BrowserPolicy for GooglePolicy {
    fn current<'a>(&'a self, session: &'a BrowserSessionRow) -> PolicyFuture<'a> {
        Box::pin(async move {
            if session.project_binding.is_some() {
                return Err(AuthorityError::Denied);
            }
            let epoch = self
                .state
                .store
                .google_provider_revocation_epoch(&session.subject)
                .await
                .map_err(|_| AuthorityError::Unavailable)?;
            Ok(PermissionState {
                epoch: epoch.to_string(),
                scopes: self.scopes.clone(),
            })
        })
    }
}
#[cfg(feature = "http-axum")]
struct ProjectPolicy(Arc<dyn crate::middleware::ProjectSessionRevalidator>);
#[cfg(feature = "http-axum")]
impl BrowserPolicy for ProjectPolicy {
    fn current<'a>(&'a self, session: &'a BrowserSessionRow) -> PolicyFuture<'a> {
        Box::pin(async move {
            let binding = session
                .project_binding
                .as_ref()
                .ok_or(AuthorityError::Denied)?;
            let grant = self
                .0
                .revalidate(binding)
                .await
                .map_err(|error| match error {
                    crate::middleware::ProjectSessionRevalidationError::Denied => {
                        AuthorityError::Denied
                    }
                    crate::middleware::ProjectSessionRevalidationError::Unavailable => {
                        AuthorityError::Unavailable
                    }
                })?;
            if *binding != crate::types::ProjectSessionBinding::from(&grant) {
                return Err(AuthorityError::Changed);
            }
            project_permissions(&grant)
        })
    }
}
#[cfg(feature = "http-axum")]
fn project_permissions(
    grant: &labby_primitives::product_credential::BoundAccessGrant,
) -> Result<PermissionState, AuthorityError> {
    let encoded = serde_json::to_vec(&(
        crate::types::ProjectSessionBinding::from(grant),
        grant.requires_admin,
        grant.destructive,
    ))
    .map_err(|_| AuthorityError::Unavailable)?;
    let epoch =
        URL_SAFE_NO_PAD.encode(keyed_fingerprint("browser.project.policy.v1", &[&encoded])?);
    Ok(PermissionState {
        epoch,
        scopes: grant.scopes.clone(),
    })
}
