//! rmcp credential-store adapter for Labby's central Google credential broker.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use oauth2::{AccessToken, RefreshToken, Scope, TokenResponse as _, basic::BasicTokenType};
use rmcp::transport::auth::{
    AuthError, CredentialRefreshGuard, CredentialStore, OAuthTokenResponse, StoredCredentials,
    VendorExtraTokenFields,
};
use rmcp_client as rmcp;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tracing::warn;

use crate::google::{GoogleProvider, merge_google_scopes};
use crate::sqlite::SqliteStore;
use crate::types::{GoogleProviderCredentialRow, GoogleProviderCredentialUpdate};
use crate::upstream::types::OauthError;
use crate::util::fingerprint;

const GOOGLE_ISSUER: &str = "https://accounts.google.com";
const REFRESH_OPERATION_LOCK_STRIPES: usize = 2_048;

static REFRESH_OPERATION_LOCKS: OnceLock<[Arc<Mutex<()>>; REFRESH_OPERATION_LOCK_STRIPES]> =
    OnceLock::new();

/// Return the process-wide refresh-operation mutex for one stable credential.
///
/// This lock is deliberately distinct from `google_refresh::lock`. rmcp holds
/// it across load, token exchange, and save, while `save` acquires the shorter
/// persistence lock. Using the same mutex for both would deadlock during save.
/// SQLite CAS and revocation fencing reject stale cross-process persistence,
/// but do not serialize rotating-token exchanges across processes.
/// Fixed stripes keep memory bounded without changing an active identity's
/// mutex when capacity changes. Hash collisions serialize unrelated refreshes;
/// they never share credential data or change store/provider/subject checks.
pub(super) fn refresh_operation_lock(credential_identity: &str) -> Arc<Mutex<()>> {
    let locks =
        REFRESH_OPERATION_LOCKS.get_or_init(|| std::array::from_fn(|_| Arc::new(Mutex::new(()))));
    let digest = Sha256::digest(credential_identity.as_bytes());
    let stripe =
        usize::from(u16::from_be_bytes([digest[0], digest[1]])) % REFRESH_OPERATION_LOCK_STRIPES;
    Arc::clone(&locks[stripe])
}

#[derive(Clone)]
pub struct GoogleProviderCredentialStore {
    store: SqliteStore,
    provider: Arc<GoogleProvider>,
    account: Option<String>,
    expected_client_id: String,
    required_scopes: Vec<String>,
    authorization_fence_epoch: Arc<AtomicI64>,
    refresh_attempt: Arc<std::sync::Mutex<Option<(String, i64)>>>,
}

impl std::fmt::Debug for GoogleProviderCredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoogleProviderCredentialStore")
            .field("account", &self.account.as_ref().map(|_| "<redacted>"))
            .field("expected_client_id", &self.expected_client_id)
            .field("required_scopes", &self.required_scopes)
            .finish_non_exhaustive()
    }
}

impl GoogleProviderCredentialStore {
    pub fn new(
        store: SqliteStore,
        provider: Arc<GoogleProvider>,
        account: Option<String>,
        expected_client_id: String,
        required_scopes: Vec<String>,
    ) -> Self {
        Self {
            store,
            provider,
            account: account
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            expected_client_id,
            required_scopes: normalize_scopes(required_scopes),
            authorization_fence_epoch: Arc::new(AtomicI64::new(-1)),
            refresh_attempt: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    #[cfg(test)]
    fn refresh_attempt(&self) -> Option<(String, i64)> {
        self.refresh_attempt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(super) fn take_refresh_attempt(&self) -> Option<(String, i64)> {
        self.refresh_attempt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    pub async fn credential_row(&self) -> Result<Option<GoogleProviderCredentialRow>, OauthError> {
        if self.account.is_none()
            && self
                .store
                .count_google_provider_credentials()
                .await
                .map_err(|error| OauthError::Internal(error.to_string()))?
                > 1
        {
            return Err(OauthError::AccountAmbiguous(
                "configure oauth.credential.account as a Google sub or verified email".to_string(),
            ));
        }
        self.store
            .find_google_provider_credential_by_selector(self.account.as_deref())
            .await
            .map_err(|error| match error {
                crate::error::AuthError::Config(message) => OauthError::AccountAmbiguous(message),
                other => OauthError::Internal(other.to_string()),
            })
    }

    /// Validate whether an authorization may create or upgrade the selected credential.
    ///
    /// Missing scopes and legacy rows are allowed because authorization repairs them.
    /// A credential already bound to another OAuth client is never silently rebound.
    pub async fn authorization_preflight(&self) -> Result<(), OauthError> {
        let Some(row) = self.credential_row().await? else {
            return Ok(());
        };
        if !row.client_id.is_empty() && row.client_id != self.expected_client_id {
            return Err(OauthError::ClientMismatch(
                "shared Google credential belongs to a different OAuth client".to_string(),
            ));
        }
        Ok(())
    }

    pub async fn validated_credential_row(
        &self,
    ) -> Result<Option<GoogleProviderCredentialRow>, OauthError> {
        let Some(row) = self.credential_row().await? else {
            return Ok(None);
        };
        if row.client_id.is_empty() {
            return Err(OauthError::NeedsReauth(
                "shared Google credential predates client binding; authorize this upstream once to bind it"
                    .to_string(),
            ));
        }
        if row.client_id != self.expected_client_id {
            return Err(OauthError::ClientMismatch(
                "shared Google credential belongs to a different OAuth client".to_string(),
            ));
        }
        let missing_scopes = missing_scopes(&self.required_scopes, &row.granted_scopes);
        if !missing_scopes.is_empty() {
            return Err(OauthError::ScopeUpgradeRequired { missing_scopes });
        }
        Ok(Some(row))
    }

    #[must_use]
    pub fn required_scopes(&self) -> &[String] {
        &self.required_scopes
    }

    async fn load_row_for_rmcp(&self) -> Result<Option<GoogleProviderCredentialRow>, AuthError> {
        let row = self.credential_row().await.map_err(|error| match error {
            OauthError::AccountAmbiguous(_) => AuthError::AuthorizationRequired,
            other => AuthError::InternalError(other.to_string()),
        })?;
        let Some(row) = row else {
            return Ok(None);
        };
        // A v7 row has no client binding. Return no stored rmcp credentials so
        // begin_authorization can bind it through a new Google code exchange.
        if row.client_id.is_empty() {
            return Ok(None);
        }
        if row.client_id != self.expected_client_id {
            return Err(AuthError::AuthorizationRequired);
        }
        // Missing scopes must not block begin_authorization. Normal authenticated
        // client construction validates the scope subset before using the token.
        Ok(Some(row))
    }

    async fn identity_for_save(
        &self,
        token: &OAuthTokenResponse,
        existing: Option<&GoogleProviderCredentialRow>,
    ) -> Result<(String, Option<String>), AuthError> {
        let id_token = token
            .extra_fields()
            .0
            .get("id_token")
            .and_then(serde_json::Value::as_str);
        if let Some(id_token) = id_token {
            let identity = self
                .provider
                .verify_identity(id_token)
                .await
                .map_err(|error| AuthError::InternalError(error.to_string()))?;
            if identity.email_verified != Some(true) {
                tracing::warn!(
                    subject_id = %fingerprint(&identity.subject),
                    kind = "oauth_needs_reauth",
                    "shared Google credential callback did not include a verified email claim"
                );
                return Err(AuthError::AuthorizationRequired);
            }
            if let Some(existing) = existing
                && existing.subject != identity.subject
            {
                tracing::warn!(
                    expected_subject_id = %fingerprint(&existing.subject),
                    returned_subject_id = %fingerprint(&identity.subject),
                    kind = "oauth_needs_reauth",
                    "shared Google credential callback returned a different account"
                );
                return Err(AuthError::AuthorizationRequired);
            }
            if let Some(selector) = self.account.as_deref() {
                let selector_matches = selector == identity.subject
                    || identity
                        .email
                        .as_deref()
                        .is_some_and(|email| email.eq_ignore_ascii_case(selector));
                if !selector_matches {
                    return Err(AuthError::AuthorizationRequired);
                }
            }
            return Ok((identity.subject, identity.email));
        }

        existing
            .map(|row| (row.subject.clone(), row.email.clone()))
            .ok_or(AuthError::AuthorizationRequired)
    }
}

impl CredentialStore for GoogleProviderCredentialStore {
    fn load<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<StoredCredentials>, AuthError>> + Send + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            *self
                .refresh_attempt
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            let fence_epoch = self
                .store
                .google_provider_fence_epoch()
                .await
                .map_err(|error| AuthError::InternalError(error.to_string()))?;
            self.authorization_fence_epoch
                .store(fence_epoch, Ordering::Release);
            let Some(row) = self.load_row_for_rmcp().await? else {
                return Ok(None);
            };
            *self
                .refresh_attempt
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((row.subject.clone(), row.generation));
            let token_received_at = row.token_received_at.unwrap_or(0).max(0) as u64;
            let expires_in = row
                .access_token_expires_at
                .unwrap_or(0)
                .saturating_sub(row.token_received_at.unwrap_or(0))
                .max(0) as u64;
            let access_token = row
                .access_token
                .unwrap_or_else(|| "expired-central-google-access-token".to_string());
            let mut token = OAuthTokenResponse::new(
                AccessToken::new(access_token),
                BasicTokenType::Bearer,
                VendorExtraTokenFields::default(),
            );
            token.set_refresh_token(Some(RefreshToken::new(row.refresh_token)));
            let expires = Duration::from_secs(expires_in);
            token.set_expires_in(Some(&expires));
            token.set_scopes(Some(
                row.granted_scopes.iter().cloned().map(Scope::new).collect(),
            ));
            Ok(Some(
                StoredCredentials::new(
                    row.client_id,
                    Some(token),
                    row.granted_scopes,
                    Some(token_received_at),
                )
                .with_issuer(row.issuer),
            ))
        })
    }

    fn save<'life0, 'async_trait>(
        &'life0 self,
        credentials: StoredCredentials,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            // Bind persistence to the generation that supplied the token used
            // for this exchange. Reloading below is only for merge inputs; it
            // must never advance the CAS generation past this attempt.
            let attempted = self.take_refresh_attempt();
            if credentials.client_id != self.expected_client_id {
                return Err(AuthError::AuthorizationRequired);
            }
            let token = credentials
                .token_response
                .as_ref()
                .ok_or(AuthError::AuthorizationRequired)?;
            let existing = self
                .credential_row()
                .await
                .map_err(|error| AuthError::InternalError(error.to_string()))?;
            if existing.as_ref().is_some_and(|row| {
                !row.client_id.is_empty() && row.client_id != self.expected_client_id
            }) {
                return Err(AuthError::AuthorizationRequired);
            }
            let (subject, email) = self.identity_for_save(token, existing.as_ref()).await?;
            if attempted
                .as_ref()
                .is_some_and(|(attempted_subject, _)| attempted_subject != &subject)
            {
                return Err(AuthError::AuthorizationRequired);
            }
            let _provider_guard = crate::google_refresh::lock(&subject).lock_owned().await;
            let observed_revocation_epoch = self.authorization_fence_epoch.load(Ordering::Acquire);
            if observed_revocation_epoch < 0 {
                return Err(AuthError::AuthorizationRequired);
            }
            let existing = self
                .store
                .find_google_provider_credential(&subject)
                .await
                .map_err(|error| AuthError::InternalError(error.to_string()))?;
            let refresh_token = token
                .refresh_token()
                .map(|value| value.secret().to_string())
                .or_else(|| existing.as_ref().map(|row| row.refresh_token.clone()))
                .ok_or(AuthError::AuthorizationRequired)?;
            let token_received_at = credentials
                .token_received_at
                .map(|value| value as i64)
                .unwrap_or_else(crate::util::now_unix);
            let expires_in = token
                .expires_in()
                .map(|value| value.as_secs())
                .unwrap_or(3600);
            let access_token_expires_at =
                token_received_at.saturating_add(i64::try_from(expires_in).unwrap_or(i64::MAX));
            let granted_scopes = merge_google_scopes(
                existing
                    .as_ref()
                    .map(|row| row.granted_scopes.as_slice())
                    .unwrap_or_default(),
                &credentials.granted_scopes,
            );
            let scope_upgraded = existing
                .as_ref()
                .is_none_or(|row| !missing_scopes(&granted_scopes, &row.granted_scopes).is_empty());
            let issuer = credentials
                .issuer
                .clone()
                .or_else(|| Some(GOOGLE_ISSUER.to_string()));
            let update = GoogleProviderCredentialUpdate {
                subject,
                email,
                client_id: credentials.client_id,
                granted_scopes,
                access_token: token.access_token().secret().to_string(),
                refresh_token,
                token_received_at,
                access_token_expires_at,
                issuer,
                refreshed: existing.is_some() && !scope_upgraded,
                scope_upgraded,
            };
            let update_subject = update.subject.clone();
            let observed_generation = attempted
                .as_ref()
                .map(|(_, generation)| *generation)
                .or_else(|| existing.as_ref().map(|row| row.generation));
            let persisted = if let Some(generation) = observed_generation {
                self.store
                    .replace_google_provider_token_bundle_if_generation(update, generation)
                    .await
            } else {
                self.store
                    .insert_google_provider_token_bundle_if_absent(
                        update,
                        observed_revocation_epoch,
                    )
                    .await
            }
            .map_err(|error| AuthError::InternalError(error.to_string()))?;
            if persisted {
                return Ok(());
            }
            let replacement_present = self
                .store
                .has_google_provider_credential_for_subject(&update_subject)
                .await
                .map_err(|error| AuthError::InternalError(error.to_string()))?;
            warn!(
                subject_id = %fingerprint(&update_subject),
                observed_provider_generation = ?observed_generation,
                replacement_provider_credential_present = replacement_present,
                "upstream google credential save discarded stale token bundle after generation changed"
            );
            Err(AuthError::AuthorizationRequired)
        })
    }

    fn acquire_refresh_guard<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<Option<CredentialRefreshGuard>, AuthError>>
                + Send
                + 'async_trait,
        >,
    >
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let credential_identity = self
                .credential_row()
                .await
                .map_err(|error| AuthError::CredentialStoreError(error.to_string()))?
                .map(|row| row.subject)
                .or_else(|| self.account.clone())
                .ok_or_else(|| {
                    AuthError::CredentialStoreError(
                        "cannot coordinate an unbound Google credential identity".to_string(),
                    )
                })?;
            let guard = refresh_operation_lock(&format!("google:{credential_identity}"))
                .lock_owned()
                .await;
            Ok(Some(CredentialRefreshGuard::new(guard)))
        })
    }

    fn clear<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuthError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        // rmcp may clear its store when metadata changes. A shared provider
        // credential is protected from that per-upstream lifecycle operation.
        Box::pin(async { Ok(()) })
    }
}

fn normalize_scopes(scopes: Vec<String>) -> Vec<String> {
    merge_google_scopes(&[], &scopes)
}

#[must_use]
pub fn missing_scopes(required: &[String], granted: &[String]) -> Vec<String> {
    let granted: std::collections::HashSet<&str> = granted.iter().map(String::as_str).collect();
    required
        .iter()
        .filter(|scope| !granted.contains(scope.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_operation_identity_survives_capacity_recovery() {
        let mut retained: Vec<_> = (0..REFRESH_OPERATION_LOCK_STRIPES)
            .map(|index| refresh_operation_lock(&format!("capacity-fixture:{index}")))
            .collect();
        let first = refresh_operation_lock("capacity-fixture:overflow");
        let guard = Arc::clone(&first).try_lock_owned().unwrap();

        // Release a formerly occupied registry slot while the overflow caller
        // still owns its guard. Another lookup must retain the same lock.
        drop(retained.pop());
        let second = refresh_operation_lock("capacity-fixture:overflow");
        assert!(
            Arc::clone(&second).try_lock_owned().is_err(),
            "capacity recovery must not split one credential across two locks"
        );
        assert!(Arc::ptr_eq(&first, &second));
        drop(guard);
        assert!(second.try_lock_owned().is_ok());
    }

    #[test]
    fn refresh_operation_hash_collisions_only_serialize_callers() {
        let mut seen = std::collections::HashMap::new();
        let (first, second) = (0..=REFRESH_OPERATION_LOCK_STRIPES)
            .find_map(|index| {
                let lock = refresh_operation_lock(&format!("collision-fixture:{index}"));
                seen.insert(Arc::as_ptr(&lock), Arc::clone(&lock))
                    .map(|previous| (previous, lock))
            })
            .expect("more identities than stripes must include a collision");
        let guard = first.try_lock_owned().unwrap();
        assert!(Arc::clone(&second).try_lock_owned().is_err());
        drop(guard);
        assert!(second.try_lock_owned().is_ok());
    }

    async fn test_store() -> SqliteStore {
        let path = tempfile::tempdir().unwrap().keep().join("auth.db");
        SqliteStore::open_with_key(
            path,
            Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                "google-store-test-key",
            )),
        )
        .await
        .unwrap()
    }

    fn provider() -> Arc<GoogleProvider> {
        Arc::new(
            GoogleProvider::new(
                "google-client".to_string(),
                "google-secret".to_string(),
                url::Url::parse("https://lab.example.com/oauth/google/callback").unwrap(),
            )
            .unwrap(),
        )
    }

    async fn insert_bundle(store: &SqliteStore, scopes: Vec<String>) {
        let now = crate::util::now_unix();
        store
            .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
                subject: "google-subject".to_string(),
                email: Some("admin@example.com".to_string()),
                client_id: "google-client".to_string(),
                granted_scopes: scopes,
                access_token: "access-token".to_string(),
                refresh_token: "refresh-token".to_string(),
                token_received_at: now,
                access_token_expires_at: now + 3600,
                issuer: Some(GOOGLE_ISSUER.to_string()),
                refreshed: false,
                scope_upgraded: true,
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn credential_store_save_started_before_revoke_cannot_recreate_credential() {
        let store = test_store().await;
        insert_bundle(&store, vec!["openid".to_string()]).await;
        let adapter = GoogleProviderCredentialStore::new(
            store.clone(),
            provider(),
            Some("google-subject".to_string()),
            "google-client".to_string(),
            vec!["openid".to_string()],
        );
        assert!(CredentialStore::load(&adapter).await.unwrap().is_some());
        let generation = store
            .find_google_provider_credential("google-subject")
            .await
            .unwrap()
            .unwrap()
            .generation;
        assert!(
            store
                .invalidate_google_provider_credential("google-subject", generation)
                .await
                .unwrap()
                .invalidated
        );

        let mut token = OAuthTokenResponse::new(
            AccessToken::new("late-access".to_string()),
            BasicTokenType::Bearer,
            VendorExtraTokenFields::default(),
        );
        token.set_refresh_token(Some(RefreshToken::new("late-refresh".to_string())));
        let credentials = StoredCredentials::new(
            "google-client".to_string(),
            Some(token),
            vec!["openid".to_string()],
            Some(crate::util::now_unix().max(0) as u64),
        );
        assert!(CredentialStore::save(&adapter, credentials).await.is_err());
        assert!(
            store
                .find_google_provider_credential("google-subject")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn failed_refresh_attempt_generation_cannot_invalidate_fresh_replacement() {
        let store = test_store().await;
        insert_bundle(&store, vec!["openid".to_string()]).await;
        let adapter = GoogleProviderCredentialStore::new(
            store.clone(),
            provider(),
            Some("google-subject".to_string()),
            "google-client".to_string(),
            vec!["openid".to_string()],
        );
        assert!(CredentialStore::load(&adapter).await.unwrap().is_some());
        let (attempted_subject, attempted_generation) =
            adapter.refresh_attempt().expect("attempt generation");

        let now = crate::util::now_unix();
        assert!(
            store
                .replace_google_provider_token_bundle_if_generation(
                    GoogleProviderCredentialUpdate {
                        subject: "google-subject".to_string(),
                        email: Some("admin@example.com".to_string()),
                        client_id: "google-client".to_string(),
                        granted_scopes: vec!["openid".to_string()],
                        access_token: "replacement-access".to_string(),
                        refresh_token: "replacement-refresh".to_string(),
                        token_received_at: now,
                        access_token_expires_at: now + 3600,
                        issuer: Some(GOOGLE_ISSUER.to_string()),
                        refreshed: true,
                        scope_upgraded: false,
                    },
                    attempted_generation,
                )
                .await
                .unwrap()
        );

        let invalidation = store
            .invalidate_google_provider_credential(&attempted_subject, attempted_generation)
            .await
            .unwrap();
        assert!(!invalidation.invalidated);
        let credential = store
            .find_google_provider_credential("google-subject")
            .await
            .unwrap()
            .expect("fresh replacement survives");
        assert_eq!(credential.refresh_token, "replacement-refresh");
    }

    #[tokio::test]
    async fn refresh_guard_does_not_deadlock_credential_save() {
        let store = test_store().await;
        insert_bundle(&store, vec!["openid".to_string()]).await;
        let adapter = GoogleProviderCredentialStore::new(
            store,
            provider(),
            Some("google-subject".to_string()),
            "google-client".to_string(),
            vec!["openid".to_string()],
        );
        assert!(CredentialStore::load(&adapter).await.unwrap().is_some());
        let guard = CredentialStore::acquire_refresh_guard(&adapter)
            .await
            .unwrap()
            .expect("Google credentials coordinate refresh operations");
        let mut token = OAuthTokenResponse::new(
            AccessToken::new("refreshed-access".to_string()),
            BasicTokenType::Bearer,
            VendorExtraTokenFields::default(),
        );
        token.set_refresh_token(Some(RefreshToken::new("refreshed-token".to_string())));
        let credentials = StoredCredentials::new(
            "google-client".to_string(),
            Some(token),
            vec!["openid".to_string()],
            Some(crate::util::now_unix().max(0) as u64),
        );

        tokio::time::timeout(
            Duration::from_secs(2),
            CredentialStore::save(&adapter, credentials),
        )
        .await
        .expect("refresh save must not reacquire its operation guard")
        .unwrap();
        drop(guard);
    }

    #[tokio::test]
    async fn refresh_guards_serialize_same_identity_but_not_distinct_stripes() {
        let first_store = test_store().await;
        insert_bundle(&first_store, vec!["openid".to_string()]).await;
        let first = GoogleProviderCredentialStore::new(
            first_store.clone(),
            provider(),
            Some("google-subject".to_string()),
            "google-client".to_string(),
            vec!["openid".to_string()],
        );
        let same = first.clone();

        let other_store = test_store().await;
        let now = crate::util::now_unix();
        other_store
            .upsert_google_provider_token_bundle(GoogleProviderCredentialUpdate {
                subject: "other-google-subject".to_string(),
                email: Some("other@example.com".to_string()),
                client_id: "google-client".to_string(),
                granted_scopes: vec!["openid".to_string()],
                access_token: "other-access".to_string(),
                refresh_token: "other-refresh".to_string(),
                token_received_at: now,
                access_token_expires_at: now + 3600,
                issuer: Some(GOOGLE_ISSUER.to_string()),
                refreshed: false,
                scope_upgraded: true,
            })
            .await
            .unwrap();
        let other = GoogleProviderCredentialStore::new(
            other_store,
            provider(),
            Some("other-google-subject".to_string()),
            "google-client".to_string(),
            vec!["openid".to_string()],
        );

        let first_guard = CredentialStore::acquire_refresh_guard(&first)
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                CredentialStore::acquire_refresh_guard(&same),
            )
            .await
            .is_err(),
            "the same credential identity must wait for the active refresh"
        );
        let other_guard = tokio::time::timeout(
            Duration::from_secs(2),
            CredentialStore::acquire_refresh_guard(&other),
        )
        .await
        .expect("credentials on distinct stripes must not be serialized")
        .unwrap()
        .unwrap();
        drop(other_guard);
        drop(first_guard);
        let _released_guard = tokio::time::timeout(
            Duration::from_secs(2),
            CredentialStore::acquire_refresh_guard(&same),
        )
        .await
        .expect("same-identity waiter proceeds after release")
        .unwrap()
        .expect("Google refresh guard");
    }

    #[tokio::test]
    async fn refresh_guard_reports_unbound_identity_as_store_error() {
        let adapter = GoogleProviderCredentialStore::new(
            test_store().await,
            provider(),
            None,
            "google-client".to_string(),
            vec!["openid".to_string()],
        );
        assert!(matches!(
            CredentialStore::acquire_refresh_guard(&adapter).await,
            Err(AuthError::CredentialStoreError(message))
                if message == "cannot coordinate an unbound Google credential identity"
        ));
    }

    #[test]
    fn missing_scopes_returns_only_the_required_difference() {
        assert_eq!(
            missing_scopes(
                &["openid".to_string(), "calendar".to_string()],
                &["profile".to_string(), "openid".to_string()],
            ),
            vec!["calendar"]
        );
    }

    #[tokio::test]
    async fn validated_row_rejects_missing_scopes_and_client_mismatch() {
        let store = test_store().await;
        insert_bundle(&store, vec!["openid".to_string()]).await;
        let missing = GoogleProviderCredentialStore::new(
            store.clone(),
            provider(),
            Some("admin@example.com".to_string()),
            "google-client".to_string(),
            vec!["openid".to_string(), "calendar".to_string()],
        );
        assert!(matches!(
            missing.validated_credential_row().await,
            Err(OauthError::ScopeUpgradeRequired { missing_scopes })
                if missing_scopes == vec!["calendar"]
        ));
        missing
            .authorization_preflight()
            .await
            .expect("missing scopes must remain repairable by incremental authorization");

        let wrong_client = GoogleProviderCredentialStore::new(
            store,
            provider(),
            Some("google-subject".to_string()),
            "different-client".to_string(),
            vec!["openid".to_string()],
        );
        assert!(matches!(
            wrong_client.validated_credential_row().await,
            Err(OauthError::ClientMismatch(_))
        ));
        assert!(matches!(
            wrong_client.authorization_preflight().await,
            Err(OauthError::ClientMismatch(_))
        ));
    }

    #[tokio::test]
    async fn rmcp_clear_does_not_delete_a_shared_provider_credential() {
        let store = test_store().await;
        insert_bundle(&store, vec!["openid".to_string()]).await;
        let adapter = GoogleProviderCredentialStore::new(
            store.clone(),
            provider(),
            None,
            "google-client".to_string(),
            vec!["openid".to_string()],
        );
        CredentialStore::clear(&adapter).await.unwrap();
        assert!(
            store
                .find_google_provider_credential("google-subject")
                .await
                .unwrap()
                .is_some()
        );
    }
}
