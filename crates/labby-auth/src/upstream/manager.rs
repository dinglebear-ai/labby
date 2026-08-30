//! Upstream OAuth lifecycle manager.
//!
//! `UpstreamOauthManager` orchestrates the full outbound `authorization_code` + PKCE
//! flow for one configured upstream MCP server.  It is per-upstream (constructed once
//! per `UpstreamConfig` that has an `oauth` block) and is `Clone` / `Send + Sync`.
//!
//! ## Subject
//!
//! All public methods take a `subject: &str` identifying the lab user initiating the
//! flow.  Credentials are stored and refreshed independently per `(upstream, subject)`.
//!
//! ## Two-phase authorization
//!
//! ```text
//! begin_authorization(subject)
//!   ↓  generates PKCE + CSRF, stores state in SQLite, returns redirect URL
//! browser → AS → callback
//!   ↓
//! complete_authorization_callback(subject, code, csrf)
//!   ↓  exchanges code, stores encrypted tokens in SQLite
//! build_auth_client(subject)
//!   ↓  loads stored credentials, proactively refreshes if stale
//! AuthClient<reqwest::Client>  → used by UpstreamPool for MCP calls
//! ```
//!
//! ## AS metadata caching
//!
//! Authorization server metadata is fetched once per upstream (not per-subject) and
//! cached to avoid an HTTP round-trip on every `build_auth_client` call.

use std::sync::Arc;

use labby_runtime::gateway_config::{
    UpstreamConfig, UpstreamOauthCredentialSource, UpstreamOauthRegistration,
};
use rmcp::transport::auth::{AuthorizationMetadata, OAuthClientConfig};
use rmcp::transport::streamable_http_client::StreamableHttpClient;
use rmcp::transport::{AuthClient, AuthorizationManager};
use rmcp_client as rmcp;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::RwLock;
use tracing::info;

use crate::google::GoogleProvider;
use crate::sqlite::SqliteStore;
use crate::types::UpstreamOauthCredentialRow;
use crate::upstream::encryption::EncryptionKey;
use crate::upstream::google_store::GoogleProviderCredentialStore;
use crate::upstream::http_client::{
    TrustedOriginOAuthHttpClient, authorization_manager_for_upstream,
};
use crate::upstream::refresh::{RefreshFailureCache, RefreshLocks};
use crate::upstream::store::{SqliteCredentialStore, SqliteStateStore};
mod discovery;

pub use discovery::discover_published_metadata;
use discovery::{
    DynamicClientRegistrationUse, extract_state_param, google_offline_access_url,
    is_known_split_endpoint_origin, url_origin,
};
#[cfg(test)]
use discovery::{
    ProtectedResourceMetadata, authorization_metadata_candidates, bounded_authorization_servers,
};

use crate::upstream::types::{
    BeginAuthorization, GoogleCredentialBrokerStatus, OAuthEgressKind, OauthError,
};

const TOKEN_EXPIRY_WARNING_SECS: i64 = 300;
const PROACTIVE_REFRESH_WINDOW_SECS: i64 = 30;
const MAX_AUTHORIZATION_SERVERS: usize = 8;
const OAUTH_METADATA_DISCOVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

#[derive(Clone, Copy)]
enum AuthClientTransport {
    Default,
    Supplied,
}

impl AuthClientTransport {
    const fn suffix(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Supplied => " (with_client)",
        }
    }
}

/// Upstream OAuth manager for a single upstream MCP server.
///
/// Cheap to clone — all mutable state is behind `Arc`.
#[derive(Clone)]
pub struct UpstreamOauthManager {
    sqlite: SqliteStore,
    key: EncryptionKey,
    upstream: UpstreamConfig,
    redirect_uri: Arc<String>,
    locks: Arc<RefreshLocks>,
    /// Tracks recent refresh failures so a known-dead credential fails fast
    /// instead of hitting the authorization server on every request.
    refresh_failures: Arc<RefreshFailureCache>,
    /// Cached AS metadata (fetched once per upstream, shared across subjects).
    metadata_cache: Arc<RwLock<Option<AuthorizationMetadata>>>,
}

impl UpstreamOauthManager {
    /// Create a new manager for `upstream`.
    ///
    /// `redirect_uri` is the absolute URL of the OAuth callback endpoint that will
    /// receive the authorization code (e.g.
    /// `https://lab.example/v1/upstream-oauth/{name}/callback`).
    pub fn new(
        sqlite: SqliteStore,
        key: EncryptionKey,
        upstream: UpstreamConfig,
        redirect_uri: String,
    ) -> Self {
        Self {
            sqlite,
            key,
            upstream,
            redirect_uri: Arc::new(redirect_uri),
            locks: Arc::new(RefreshLocks::new()),
            refresh_failures: Arc::new(RefreshFailureCache::new()),
            metadata_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// Return the `UpstreamConfig` this manager was constructed with.
    ///
    /// Used to persist transient (probe-created) managers back into the gateway
    /// LabConfig when authorization completes for the first time.
    pub fn upstream_config(&self) -> &UpstreamConfig {
        &self.upstream
    }

    #[must_use]
    pub fn credential_source_label(&self) -> &'static str {
        match self.upstream.oauth.as_ref().map(|oauth| &oauth.credential) {
            Some(UpstreamOauthCredentialSource::GoogleProvider { .. }) => "google_provider",
            _ => "dedicated",
        }
    }

    pub async fn google_credential_broker_status(
        &self,
    ) -> Result<Option<GoogleCredentialBrokerStatus>, OauthError> {
        let Some(oauth) = self.upstream.oauth.as_ref() else {
            return Ok(None);
        };
        let UpstreamOauthCredentialSource::GoogleProvider { account } = &oauth.credential else {
            return Ok(None);
        };
        let required_scopes = self.effective_scopes()?;
        let store = self.google_credential_store(required_scopes.clone())?;
        let row = store.credential_row().await?;
        let granted_scopes = row
            .as_ref()
            .map(|row| row.granted_scopes.clone())
            .unwrap_or_default();
        let missing_scopes =
            crate::upstream::google_store::missing_scopes(&required_scopes, &granted_scopes);
        Ok(Some(GoogleCredentialBrokerStatus {
            account_selector_configured: account
                .as_deref()
                .is_some_and(|selector| !selector.trim().is_empty()),
            provider_generation: row.as_ref().map(|row| row.generation),
            client_bound: row.as_ref().is_some_and(|row| {
                !row.client_id.is_empty()
                    && self
                        .oauth_config()
                        .ok()
                        .map(|config| match &config.registration {
                            UpstreamOauthRegistration::Preregistered { client_id, .. } => {
                                client_id == &row.client_id
                            }
                            UpstreamOauthRegistration::Dynamic
                            | UpstreamOauthRegistration::ClientMetadataDocument { .. } => false,
                        })
                        .unwrap_or(false)
            }),
            required_scopes,
            granted_scopes,
            missing_scopes,
        }))
    }

    /// Return `true` if persisted credentials exist for `subject`.
    ///
    /// Does not check whether the credentials are still valid.
    #[allow(dead_code)]
    pub async fn has_credentials(&self, subject: &str) -> Result<bool, OauthError> {
        if self.oauth_config()?.credential.is_google_provider() {
            return self
                .google_credential_store(self.effective_scopes()?)?
                .credential_row()
                .await
                .map(|row| row.is_some());
        }
        self.sqlite
            .find_upstream_oauth_credentials(&self.upstream.name, subject)
            .await
            .map(|opt| opt.is_some())
            .map_err(OauthError::Storage)
    }

    /// Begin the authorization flow.
    ///
    /// Discovers (or uses cached) AS metadata, registers or configures the OAuth
    /// client, generates a PKCE challenge, saves the pending state to SQLite, and
    /// returns the authorization URL to redirect the operator's browser to.
    ///
    /// Enforces S256 PKCE — returns `OauthError::UnsupportedMethod` if the AS does
    /// not advertise S256 in `code_challenge_methods_supported`.
    pub async fn begin_authorization(
        &self,
        subject: &str,
    ) -> Result<BeginAuthorization, OauthError> {
        let started = std::time::Instant::now();
        let scopes_owned = self.effective_scopes()?;
        if self.oauth_config()?.credential.is_google_provider() {
            self.google_credential_store(scopes_owned.clone())?
                .authorization_preflight()
                .await?;
        }
        let scopes: Vec<&str> = scopes_owned.iter().map(String::as_str).collect();
        let upstream_url = self.upstream_url()?;

        let mut manager = authorization_manager_for_upstream(upstream_url.as_str())
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = e.kind(),
                    error = %e,
                    "upstream oauth: failed to create authorization manager"
                );
                e
            })?;

        self.install_credential_store(&mut manager, subject, scopes_owned.clone())?;
        let state_store = SqliteStateStore::new(self.sqlite.clone(), &self.upstream.name, subject);
        manager.set_state_store(state_store);

        let metadata = self
            .get_or_discover_metadata(&mut manager)
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = e.kind(),
                    error = %e,
                    "upstream oauth: AS metadata discovery failed"
                );
                e
            })?;

        info!(
            upstream = %self.upstream.name,
            subject,
            issuer = metadata.issuer.as_deref().unwrap_or("<none>"),
            "upstream oauth: AS metadata ready"
        );

        self.verify_google_provider_issuer(&metadata)?;
        Self::verify_s256(&metadata.code_challenge_methods_supported).inspect_err(|e| {
            tracing::warn!(
                upstream = %self.upstream.name,
                subject,
                kind = e.kind(),
                "upstream oauth: S256 PKCE verification failed"
            );
        })?;
        manager.set_metadata(metadata);

        let client_cfg = self
            .resolve_client_config(
                &mut manager,
                subject,
                &scopes,
                DynamicClientRegistrationUse::BeginAuthorization,
            )
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = e.kind(),
                    error = %e,
                    "upstream oauth: client config resolution failed"
                );
                e
            })?;

        manager.configure_client(client_cfg).map_err(|e| {
            tracing::warn!(
                upstream = %self.upstream.name,
                subject,
                kind = "internal_error",
                error = %e,
                "upstream oauth: client configuration failed"
            );
            OauthError::Internal(format!("configure client: {e}"))
        })?;

        let authorization_url = manager.get_authorization_url(&scopes).await.map_err(|e| {
            tracing::warn!(
                upstream = %self.upstream.name,
                subject,
                kind = "internal_error",
                error = %e,
                "upstream oauth: authorization URL generation failed"
            );
            OauthError::Internal(format!("get authorization url: {e}"))
        })?;
        let authorization_url = google_offline_access_url(&authorization_url)?;

        let _csrf = extract_state_param(&authorization_url).ok_or_else(|| {
            tracing::warn!(
                upstream = %self.upstream.name,
                subject,
                kind = "internal_error",
                "upstream oauth: authorization URL missing state parameter"
            );
            OauthError::Internal("authorization url missing required state parameter".to_string())
        })?;

        info!(
            upstream = %self.upstream.name,
            subject,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth: authorization started"
        );

        Ok(BeginAuthorization { authorization_url })
    }

    /// Complete the authorization callback.
    ///
    /// Exchanges the authorization code for tokens and persists the encrypted
    /// credentials. Completion is reconstructed from persisted PKCE state rather
    /// than an in-memory pending map, so callbacks remain valid across restarts.
    pub async fn complete_authorization_callback(
        &self,
        subject: &str,
        code: &str,
        csrf_token: &str,
    ) -> Result<(), OauthError> {
        self.complete_authorization_callback_with_issuer(subject, code, csrf_token, None)
            .await
    }

    pub async fn complete_authorization_callback_with_issuer(
        &self,
        subject: &str,
        code: &str,
        csrf_token: &str,
        issuer: Option<&str>,
    ) -> Result<(), OauthError> {
        let started = std::time::Instant::now();

        let auth_manager = self
            .configured_authorization_manager(
                subject,
                DynamicClientRegistrationUse::CompleteAuthorization,
            )
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = e.kind(),
                    error = %e,
                    "upstream oauth: failed to build configured authorization manager for token exchange"
                );
                e
            })?;

        auth_manager
            .exchange_code_for_token_with_issuer(code, csrf_token, issuer)
            .await
            .map_err(|e| {
                let mapped = map_auth_error(e);
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = mapped.kind(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "upstream oauth: token exchange failed"
                );
                mapped
            })?;

        info!(
            upstream = %self.upstream.name,
            subject,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth: authorization completed, tokens stored"
        );

        // A fresh grant supersedes whatever was failing before -- don't make
        // the caller wait out the circuit-breaker cooldown after fixing it.
        self.refresh_failures.clear(&self.upstream.name, subject);

        Ok(())
    }

    /// Explicitly revoke the central Google provider credential selected by this upstream.
    ///
    /// This is intentionally separate from per-upstream `clear_credentials`: one
    /// provider credential can authorize several Google MCP servers and inbound Labby
    /// grants, so removal must be an explicit destructive action.
    pub async fn revoke_shared_google_credential(
        &self,
    ) -> Result<crate::types::GoogleProviderInvalidation, OauthError> {
        let oauth = self.oauth_config()?;
        let UpstreamOauthCredentialSource::GoogleProvider { account } = &oauth.credential else {
            return Err(OauthError::SharedCredentialProtected(
                "upstream does not use the central Google provider credential".to_string(),
            ));
        };
        let lock = self.acquire_refresh_lock("<explicit-revoke>").await?;
        let _guard = lock.lock().await;
        let invalidation = self
            .sqlite
            .revoke_google_provider_credential(account.as_deref())
            .await
            .map_err(OauthError::Storage)?;
        tracing::info!(
            upstream = %self.upstream.name,
            invalidated = invalidation.invalidated,
            revoked_refresh_tokens = invalidation.revoked_refresh_tokens,
            revoked_authorization_codes = invalidation.revoked_authorization_codes,
            "central Google provider credential explicitly revoked"
        );
        Ok(invalidation)
    }

    /// Delete all stored credentials for `subject` and evict any cached state.
    pub async fn clear_credentials(&self, subject: &str) -> Result<(), OauthError> {
        if self.oauth_config()?.credential.is_google_provider() {
            return Err(OauthError::SharedCredentialProtected(
                "use the explicit Google provider revoke action; per-upstream clear cannot remove a shared credential"
                    .to_string(),
            ));
        }
        self.refresh_failures.clear(&self.upstream.name, subject);
        self.sqlite
            .delete_upstream_oauth_credentials(&self.upstream.name, subject)
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = "internal_error",
                    error = %e,
                    "upstream oauth: failed to delete credentials from store"
                );
                OauthError::Storage(e)
            })?;

        self.sqlite
            .delete_dynamic_client_registration(&self.upstream.name, subject)
            .await
            .map_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    subject,
                    kind = "internal_error",
                    error = %e,
                    "upstream oauth: failed to delete dynamic client registration"
                );
                OauthError::Storage(e)
            })?;

        info!(
            upstream = %self.upstream.name,
            subject,
            "upstream oauth: credentials and dynamic registration cleared"
        );

        Ok(())
    }

    /// Return an `AuthClient` ready for use, proactively refreshing if near expiry.
    ///
    /// Creates a fresh `AuthorizationManager` backed by stored credentials.  Uses
    /// cached AS metadata to avoid an extra HTTP round-trip.
    ///
    /// Returns `OauthError::NeedsReauth` when no credentials are stored or the
    /// refresh token has been revoked.
    pub async fn build_auth_client(
        &self,
        subject: &str,
    ) -> Result<AuthClient<reqwest::Client>, OauthError> {
        let manager = self
            .prepare_stored_authorization_manager(subject, AuthClientTransport::Default)
            .await?;
        Ok(AuthClient::new(reqwest::Client::new(), manager))
    }

    /// Build an `AuthClient<C>` wrapping the supplied HTTP client (P-H4).
    ///
    /// Identical to `build_auth_client` except the caller provides the HTTP
    /// transport, enabling `BodyCappedHttpClient` or any other
    /// `StreamableHttpClient` to be used on the OAuth path.  The resulting
    /// client is NOT cached — callers that need caching must do so themselves.
    pub async fn build_auth_client_with<C>(
        &self,
        subject: &str,
        http_client: C,
    ) -> Result<AuthClient<C>, OauthError>
    where
        C: StreamableHttpClient,
    {
        let manager = self
            .prepare_stored_authorization_manager(subject, AuthClientTransport::Supplied)
            .await?;
        Ok(AuthClient::new(http_client, manager))
    }

    /// Prepare the canonical stored-credential authorization manager shared by
    /// every HTTP transport wrapper.
    async fn prepare_stored_authorization_manager(
        &self,
        subject: &str,
        transport: AuthClientTransport,
    ) -> Result<AuthorizationManager, OauthError> {
        let started = std::time::Instant::now();
        self.preflight_shared_google_credential().await?;

        let mut manager = self
            .configured_authorization_manager(
                subject,
                DynamicClientRegistrationUse::StoredCredentials,
            )
            .await
            .inspect_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    provider = %self.oauth_provider_label(),
                    subject,
                    scope = %self.oauth_scope_label(),
                    kind = e.kind(),
                    elapsed_ms = started.elapsed().as_millis(),
                    fallback = "reauthorization_required",
                    "upstream oauth: failed to build auth client manager{}",
                    transport.suffix()
                );
            })?;
        let initialized = manager.initialize_from_store().await.map_err(|e| {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                kind = "internal_error",
                elapsed_ms = started.elapsed().as_millis(),
                fallback = "reauthorization_required",
                "upstream oauth: failed to initialize auth client from credential store{}",
                transport.suffix()
            );
            OauthError::Internal(format!("initialize from store: {e}"))
        })?;

        if !initialized {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                kind = "oauth_needs_reauth",
                elapsed_ms = started.elapsed().as_millis(),
                fallback = "reauthorization_required",
                "upstream oauth: no stored credentials for auth client{}",
                transport.suffix()
            );
            return Err(OauthError::NeedsReauth(format!(
                "no stored credentials for upstream '{}' subject '{subject}'",
                self.upstream.name
            )));
        }

        self.reconfigure_client_after_store_init(&mut manager, subject)
            .await?;

        let credential_row = self.credential_row(subject).await?;
        let refresh_state = credential_row
            .as_ref()
            .and_then(|row| TokenRefreshState::from_row(row, now_unix().ok()?));
        let refresh_due = refresh_state
            .as_ref()
            .is_some_and(TokenRefreshState::refresh_due);
        if let Some(state) = refresh_state.as_ref() {
            self.log_expiring_token(subject, state, started.elapsed().as_millis());
            self.log_refresh_attempt(subject, state, started.elapsed().as_millis());
        }

        if refresh_due
            && let Some(recent_error) = self
                .refresh_failures
                .recent_error(&self.upstream.name, subject)
        {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                kind = "oauth_needs_reauth",
                elapsed_ms = started.elapsed().as_millis(),
                fallback = "reauthorization_required",
                "upstream oauth: token refresh skipped, recently failed{}",
                transport.suffix()
            );
            return Err(recent_error);
        }

        let access_result = if refresh_due {
            let (_, result) = self
                .locks
                .run_shared(&self.upstream.name, subject, || async {
                    match manager.get_access_token().await {
                        Ok(_) => Ok(()),
                        Err(error) => Err(self.map_refresh_error_and_maybe_invalidate(error).await),
                    }
                })
                .await;
            result
        } else {
            manager
                .get_access_token()
                .await
                .map(|_| ())
                .map_err(map_auth_error)
        };
        if let Err(mapped) = access_result {
            if refresh_due {
                self.refresh_failures
                    .record_failure(&self.upstream.name, subject, &mapped);
                tracing::warn!(
                    upstream = %self.upstream.name,
                    provider = %self.oauth_provider_label(),
                    subject,
                    scope = %self.oauth_scope_label(),
                    kind = mapped.kind(),
                    elapsed_ms = started.elapsed().as_millis(),
                    fallback = "reauthorization_required",
                    "upstream oauth: token refresh failed{}",
                    transport.suffix()
                );
            }
            return Err(mapped);
        }

        if refresh_due {
            manager.initialize_from_store().await.map_err(|error| {
                OauthError::Internal(format!("reload refreshed credential from store: {error}"))
            })?;
            self.reconfigure_client_after_store_init(&mut manager, subject)
                .await?;
        }

        self.refresh_failures.clear(&self.upstream.name, subject);
        if refresh_due {
            tracing::info!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                elapsed_ms = started.elapsed().as_millis(),
                fallback = "none",
                "upstream oauth: token refresh succeeded{}",
                transport.suffix()
            );
        }

        Ok(manager)
    }

    /// Force a refresh for stored credentials.
    ///
    /// `AuthorizationManager::get_access_token()` only refreshes inside rmcp's
    /// short refresh buffer. Status checks need an explicit refresh so UI state
    /// cannot report a stale credential row as connected.
    pub async fn refresh_auth_client_if_due(&self, subject: &str) -> Result<bool, OauthError> {
        let started = std::time::Instant::now();
        let lock = self.acquire_refresh_lock(subject).await?;
        let _guard = lock.lock().await;
        self.preflight_shared_google_credential().await?;

        // A status caller may have waited behind another status/request refresh.
        // Re-read inside the single-flight lock and do not replay the refresh
        // against a token that is no longer near expiry.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let still_due = self
            .credential_row(subject)
            .await?
            .is_some_and(|row| row.access_token_expires_at - now <= 300);
        if !still_due {
            tracing::debug!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                elapsed_ms = started.elapsed().as_millis(),
                coalesced = true,
                "upstream oauth: status refresh satisfied by concurrent caller"
            );
            return Ok(false);
        }
        if let Some(recent_error) = self
            .refresh_failures
            .recent_error(&self.upstream.name, subject)
        {
            tracing::debug!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                elapsed_ms = started.elapsed().as_millis(),
                cooldown = true,
                "upstream oauth: status refresh skipped during failure cooldown"
            );
            return Err(recent_error);
        }

        let mut manager = self
            .configured_authorization_manager(
                subject,
                DynamicClientRegistrationUse::StoredCredentials,
            )
            .await
            .inspect_err(|e| {
                tracing::warn!(
                    upstream = %self.upstream.name,
                    provider = %self.oauth_provider_label(),
                    subject,
                    scope = %self.oauth_scope_label(),
                    kind = e.kind(),
                    elapsed_ms = started.elapsed().as_millis(),
                    fallback = "reauthorization_required",
                    "upstream oauth: failed to build refresh manager"
                );
            })?;
        let initialized = manager.initialize_from_store().await.map_err(|e| {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                kind = "internal_error",
                elapsed_ms = started.elapsed().as_millis(),
                fallback = "reauthorization_required",
                "upstream oauth: failed to initialize refresh manager from credential store"
            );
            OauthError::Internal(format!("initialize from store: {e}"))
        })?;

        if !initialized {
            return Err(OauthError::NeedsReauth(format!(
                "no stored credentials for upstream '{}' subject '{subject}'",
                self.upstream.name
            )));
        }

        self.reconfigure_client_after_store_init(&mut manager, subject)
            .await?;

        let (executed, refresh_result) = self
            .locks
            .run_shared(&self.upstream.name, subject, || async {
                match manager.refresh_token().await {
                    Ok(_) => Ok(()),
                    Err(error) => Err(self.map_refresh_error_and_maybe_invalidate(error).await),
                }
            })
            .await;
        if let Err(mapped) = refresh_result {
            self.refresh_failures
                .record_failure(&self.upstream.name, subject, &mapped);
            return Err(mapped);
        }
        self.refresh_failures.clear(&self.upstream.name, subject);
        tracing::info!(
            upstream = %self.upstream.name,
            provider = %self.oauth_provider_label(),
            subject,
            scope = %self.oauth_scope_label(),
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth: status refresh succeeded"
        );
        Ok(executed)
    }

    pub async fn credential_row(
        &self,
        subject: &str,
    ) -> Result<Option<UpstreamOauthCredentialRow>, OauthError> {
        if self.oauth_config()?.credential.is_google_provider() {
            let row = self
                .google_credential_store(self.effective_scopes()?)?
                .credential_row()
                .await?;
            return row
                .map(|row| {
                    let granted_scopes_json = serde_json::to_string(&row.granted_scopes)
                        .map_err(|error| OauthError::Internal(error.to_string()))?;
                    Ok(UpstreamOauthCredentialRow {
                        upstream_name: self.upstream.name.clone(),
                        subject: subject.to_string(),
                        client_id: row.client_id,
                        granted_scopes_json,
                        token_blob: Vec::new(),
                        token_blob_nonce: Vec::new(),
                        token_received_at: row.token_received_at.unwrap_or(0),
                        access_token_expires_at: row.access_token_expires_at.unwrap_or(0),
                        refresh_token_present: true,
                    })
                })
                .transpose();
        }
        self.sqlite
            .find_upstream_oauth_credentials(&self.upstream.name, subject)
            .await
            .map_err(OauthError::Storage)
    }

    #[allow(dead_code)]
    pub async fn subject_for_state(&self, csrf_token: &str) -> Result<Option<String>, OauthError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| OauthError::Internal(format!("system clock error: {error}")))?
            .as_secs() as i64;
        self.sqlite
            .find_upstream_oauth_state_subject(&self.upstream.name, csrf_token, now)
            .await
            .map_err(OauthError::Storage)
    }

    /// Look up the stored dynamic `client_id` for `subject`, if any.
    ///
    /// Returns `None` when the upstream is not `Dynamic` or when no registration
    /// has been persisted yet. Used by `OauthClientCache` to include the
    /// per-subject `client_id` in the fingerprint so a re-registration is
    /// detected and the stale `AuthClient` is evicted (lab-77y5.13).
    pub async fn stored_dynamic_client_id(
        &self,
        subject: &str,
    ) -> Result<Option<String>, OauthError> {
        self.sqlite
            .find_dynamic_client_registration(&self.upstream.name, subject)
            .await
            .map_err(OauthError::Storage)
    }

    // ---- private helpers ----

    /// Restore the complete configured OAuth client after rmcp loads stored
    /// credentials. `initialize_from_store()` currently calls
    /// `configure_client_id()`, which drops confidential-client secrets. Every
    /// path that may refresh a token must therefore re-apply the resolved client
    /// config before asking rmcp for an access token.
    async fn reconfigure_client_after_store_init(
        &self,
        manager: &mut AuthorizationManager,
        subject: &str,
    ) -> Result<(), OauthError> {
        let scopes_owned = self.effective_scopes()?;
        let scopes: Vec<&str> = scopes_owned.iter().map(String::as_str).collect();
        let client_cfg = self
            .resolve_client_config(
                manager,
                subject,
                &scopes,
                DynamicClientRegistrationUse::StoredCredentials,
            )
            .await?;
        manager.configure_client(client_cfg).map_err(|error| {
            OauthError::Internal(format!(
                "re-configure client with credentials after store init: {error}"
            ))
        })
    }

    async fn configured_authorization_manager(
        &self,
        subject: &str,
        dynamic_registration_use: DynamicClientRegistrationUse,
    ) -> Result<AuthorizationManager, OauthError> {
        let upstream_url = self.upstream_url()?;
        let scopes_owned = self.effective_scopes()?;
        let scopes: Vec<&str> = scopes_owned.iter().map(String::as_str).collect();

        let mut manager = authorization_manager_for_upstream(upstream_url.as_str()).await?;

        self.install_credential_store(&mut manager, subject, scopes_owned.clone())?;
        let state_store = SqliteStateStore::new(self.sqlite.clone(), &self.upstream.name, subject);
        manager.set_state_store(state_store);

        let metadata = self.get_or_discover_metadata(&mut manager).await?;
        self.verify_google_provider_issuer(&metadata)?;
        Self::verify_s256(&metadata.code_challenge_methods_supported)?;
        manager.set_metadata(metadata);

        let client_cfg = self
            .resolve_client_config(&mut manager, subject, &scopes, dynamic_registration_use)
            .await?;
        manager
            .configure_client(client_cfg)
            .map_err(|e| OauthError::Internal(format!("configure client: {e}")))?;
        Ok(manager)
    }

    fn verify_google_provider_issuer(
        &self,
        metadata: &AuthorizationMetadata,
    ) -> Result<(), OauthError> {
        if !self.oauth_config()?.credential.is_google_provider() {
            return Ok(());
        }
        let issuer = metadata.issuer.as_deref().unwrap_or_default();
        if issuer != "https://accounts.google.com" {
            return Err(OauthError::IssuerMismatch(format!(
                "google_provider credential source requires issuer 'https://accounts.google.com', received '{issuer}'"
            )));
        }
        Ok(())
    }

    fn effective_scopes(&self) -> Result<Vec<String>, OauthError> {
        let oauth = self.oauth_config()?;
        let mut scopes = oauth.scopes.clone().unwrap_or_default();
        if oauth.credential.is_google_provider() {
            for scope in ["openid", "email", "profile"] {
                if !scopes.iter().any(|candidate| candidate == scope) {
                    scopes.push(scope.to_string());
                }
            }
        }
        scopes.sort();
        scopes.dedup();
        Ok(scopes)
    }

    fn google_credential_store(
        &self,
        required_scopes: Vec<String>,
    ) -> Result<GoogleProviderCredentialStore, OauthError> {
        let oauth = self.oauth_config()?;
        let UpstreamOauthCredentialSource::GoogleProvider { account } = &oauth.credential else {
            return Err(OauthError::Internal(
                "Google provider credential store requested for dedicated OAuth source".to_string(),
            ));
        };
        let UpstreamOauthRegistration::Preregistered {
            client_id,
            client_secret_env,
        } = &oauth.registration
        else {
            return Err(OauthError::ClientMismatch(
                "google_provider credentials require preregistered OAuth client metadata"
                    .to_string(),
            ));
        };
        let client_secret = client_secret_env
            .as_deref()
            .map(std::env::var)
            .transpose()
            .map_err(|error| {
                OauthError::Internal(format!(
                    "read Google OAuth client secret environment variable: {error}"
                ))
            })?
            .unwrap_or_default();
        let redirect_uri = url::Url::parse(self.redirect_uri.as_str()).map_err(|error| {
            OauthError::Internal(format!("parse upstream OAuth redirect URI: {error}"))
        })?;
        let provider = GoogleProvider::new(client_id.clone(), client_secret, redirect_uri)
            .map_err(|error| OauthError::Internal(error.to_string()))?;
        Ok(GoogleProviderCredentialStore::new(
            self.sqlite.clone(),
            Arc::new(provider),
            account.clone(),
            client_id.clone(),
            required_scopes,
        ))
    }

    fn install_credential_store(
        &self,
        manager: &mut AuthorizationManager,
        subject: &str,
        required_scopes: Vec<String>,
    ) -> Result<(), OauthError> {
        match &self.oauth_config()?.credential {
            UpstreamOauthCredentialSource::Dedicated => {
                manager.set_credential_store(SqliteCredentialStore::new(
                    self.sqlite.clone(),
                    self.key.clone(),
                    &self.upstream.name,
                    subject,
                ))
            }
            UpstreamOauthCredentialSource::GoogleProvider { .. } => {
                manager.set_credential_store(self.google_credential_store(required_scopes)?)
            }
        }
        Ok(())
    }

    async fn acquire_refresh_lock(
        &self,
        subject: &str,
    ) -> Result<Arc<tokio::sync::Mutex<()>>, OauthError> {
        match self.upstream.oauth.as_ref().map(|oauth| &oauth.credential) {
            Some(UpstreamOauthCredentialSource::GoogleProvider { account }) => {
                let store = self.google_credential_store(self.effective_scopes()?)?;
                let account_key = match store.credential_row().await? {
                    Some(row) => row.subject,
                    None => account
                        .clone()
                        .unwrap_or_else(|| "<unbound-google-provider>".to_string()),
                };
                Ok(crate::google_refresh::lock(&account_key))
            }
            _ => Ok(self.locks.acquire(&self.upstream.name, subject)),
        }
    }

    async fn preflight_shared_google_credential(&self) -> Result<(), OauthError> {
        if !self.oauth_config()?.credential.is_google_provider() {
            return Ok(());
        }
        let store = self.google_credential_store(self.effective_scopes()?)?;
        if store.validated_credential_row().await?.is_none() {
            return Err(OauthError::NeedsReauth(
                "no central Google provider credential is available".to_string(),
            ));
        }
        Ok(())
    }

    async fn map_refresh_error_and_maybe_invalidate(
        &self,
        error: rmcp::transport::AuthError,
    ) -> OauthError {
        let (terminal, mapped) = map_refresh_error(error);
        if terminal
            && self
                .oauth_config()
                .is_ok_and(|oauth| oauth.credential.is_google_provider())
        {
            let store = match self
                .google_credential_store(self.effective_scopes().unwrap_or_default())
            {
                Ok(store) => store,
                Err(build_error) => {
                    tracing::warn!(
                        upstream = %self.upstream.name,
                        kind = build_error.kind(),
                        "shared Google credential invalidation skipped because broker resolution failed"
                    );
                    return mapped;
                }
            };
            match store.credential_row().await {
                Ok(Some(row)) => {
                    match self
                        .sqlite
                        .invalidate_google_provider_credential(&row.subject, row.generation)
                        .await
                    {
                        Ok(invalidation) => tracing::warn!(
                            upstream = %self.upstream.name,
                            subject_id = %crate::util::fingerprint(&row.subject),
                            provider_generation = row.generation,
                            invalidated = invalidation.invalidated,
                            revoked_refresh_tokens = invalidation.revoked_refresh_tokens,
                            revoked_authorization_codes = invalidation.revoked_authorization_codes,
                            kind = mapped.kind(),
                            "terminal shared Google refresh failure invalidated provider credential"
                        ),
                        Err(invalidate_error) => tracing::warn!(
                            upstream = %self.upstream.name,
                            subject_id = %crate::util::fingerprint(&row.subject),
                            provider_generation = row.generation,
                            kind = "internal_error",
                            error = %invalidate_error,
                            "failed to invalidate terminal shared Google credential"
                        ),
                    }
                }
                Ok(None) => {}
                Err(resolve_error) => tracing::warn!(
                    upstream = %self.upstream.name,
                    kind = resolve_error.kind(),
                    "shared Google credential invalidation skipped because account resolution failed"
                ),
            }
        }
        mapped
    }

    fn oauth_config(
        &self,
    ) -> Result<&labby_runtime::gateway_config::UpstreamOauthConfig, OauthError> {
        self.upstream
            .oauth
            .as_ref()
            .ok_or_else(|| OauthError::Internal("upstream has no oauth config".to_string()))
    }

    fn oauth_scope_label(&self) -> String {
        self.upstream
            .oauth
            .as_ref()
            .and_then(|cfg| cfg.scopes.as_ref())
            .filter(|scopes| !scopes.is_empty())
            .map(|scopes| scopes.join(" "))
            .unwrap_or_else(|| "<none>".to_string())
    }

    fn oauth_provider_label(&self) -> String {
        self.upstream.name.clone()
    }

    fn log_expiring_token(&self, subject: &str, state: &TokenRefreshState, elapsed_ms: u128) {
        if state.seconds_until_expiry <= TOKEN_EXPIRY_WARNING_SECS {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                seconds_until_expiry = state.seconds_until_expiry,
                refresh_token_present = state.refresh_token_present,
                elapsed_ms,
                "upstream oauth: access token nearing expiry"
            );
        }
    }

    fn log_refresh_attempt(&self, subject: &str, state: &TokenRefreshState, elapsed_ms: u128) {
        if !state.refresh_due() {
            return;
        }

        if state.refresh_token_present {
            tracing::info!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                seconds_until_expiry = state.seconds_until_expiry,
                elapsed_ms,
                "upstream oauth: token refresh attempt"
            );
        } else {
            tracing::warn!(
                upstream = %self.upstream.name,
                provider = %self.oauth_provider_label(),
                subject,
                scope = %self.oauth_scope_label(),
                seconds_until_expiry = state.seconds_until_expiry,
                kind = "oauth_needs_reauth",
                elapsed_ms,
                fallback = "reauthorization_required",
                "upstream oauth: access token expired or near expiry without refresh token"
            );
        }
    }

    fn upstream_url(&self) -> Result<Arc<String>, OauthError> {
        let canonical = self
            .upstream
            .canonical_url()
            .ok_or_else(|| OauthError::Internal("upstream has no url".to_string()))?
            .map_err(|e| OauthError::Internal(format!("invalid upstream url: {e}")))?;
        Ok(Arc::new(canonical))
    }

    /// Fetch AS metadata, caching the result for subsequent calls.
    ///
    /// Enforces issuer binding per RFC 8414: `issuer` MUST be present and the
    /// `authorization_endpoint` + `token_endpoint` MUST share its origin. Rejects
    /// silent issuer drift between the first and subsequent discovery calls.
    ///
    /// Uses a single write-lock acquisition to avoid a TOCTOU race between a
    /// read-lock check and a subsequent write-lock cache update.
    async fn get_or_discover_metadata(
        &self,
        manager: &mut AuthorizationManager,
    ) -> Result<AuthorizationMetadata, OauthError> {
        let mut cache = self.metadata_cache.write().await;
        if let Some(meta) = cache.clone() {
            return Ok(meta);
        }

        let resolution = manager.resolve_metadata().await;
        let mut metadata = match resolution {
            Ok(resolution) if resolution.source.is_discovered() => resolution.metadata,
            resolution => match discover_published_metadata(self.upstream_url()?.as_str()).await? {
                Some(metadata) => metadata,
                None => {
                    let detail = resolution
                        .err()
                        .map(|error| error.to_string())
                        .unwrap_or_else(|| "no published OAuth metadata".to_string());
                    return Err(OauthError::Internal(format!("discover metadata: {detail}")));
                }
            },
        };

        if self.upstream.name == "swag" {
            metadata.authorization_endpoint = metadata
                .authorization_endpoint
                .replace("/mcp/authorize", "/authorize");
            metadata.token_endpoint = metadata.token_endpoint.replace("/mcp/token", "/token");
            if let Some(reg) = metadata.registration_endpoint.as_ref() {
                metadata.registration_endpoint = Some(reg.replace("/mcp/register", "/register"));
            }
        }

        self.verify_issuer_binding(&metadata)?;

        *cache = Some(metadata.clone());
        Ok(metadata)
    }

    /// RFC 8414 §3 issuer binding: `issuer` must be present, and every
    /// non-jwks endpoint origin (scheme + host + port) must match the
    /// issuer origin. This is stricter than a host-only check: it rejects
    /// endpoints served over a different scheme (e.g. http vs https) or
    /// on a different port, which a host-only comparison would miss.
    fn verify_issuer_binding(&self, metadata: &AuthorizationMetadata) -> Result<(), OauthError> {
        let issuer_raw = metadata.issuer.as_deref().ok_or_else(|| {
            OauthError::IssuerMismatch(format!(
                "AS metadata for upstream '{}' is missing required `issuer` claim",
                self.upstream.name
            ))
        })?;
        // Normalize the issuer: strip trailing slashes for a canonical form.
        let issuer_normalized = issuer_raw.trim_end_matches('/');
        let issuer_origin = url_origin(issuer_normalized).ok_or_else(|| {
            OauthError::IssuerMismatch(format!("issuer `{issuer_raw}` is not a valid URL"))
        })?;
        for (label, endpoint) in [
            (
                "authorization_endpoint",
                Some(metadata.authorization_endpoint.as_str()),
            ),
            ("token_endpoint", Some(metadata.token_endpoint.as_str())),
            (
                "registration_endpoint",
                metadata.registration_endpoint.as_deref(),
            ),
        ] {
            let Some(endpoint) = endpoint else { continue };
            let Some(origin) = url_origin(endpoint) else {
                return Err(OauthError::IssuerMismatch(format!(
                    "{label} `{endpoint}` is not a valid URL"
                )));
            };
            if origin != issuer_origin
                && !is_known_split_endpoint_origin(issuer_origin.as_str(), origin.as_str())
            {
                return Err(OauthError::IssuerMismatch(format!(
                    "{label} origin `{origin}` does not match issuer origin `{issuer_origin}`"
                )));
            }
        }
        Ok(())
    }

    fn verify_s256(methods: &Option<Vec<String>>) -> Result<(), OauthError> {
        match methods {
            Some(methods) if methods.iter().any(|m| m == "S256") => Ok(()),
            Some(methods) => Err(OauthError::UnsupportedMethod(format!(
                "AS does not advertise S256 PKCE; advertised methods: {methods:?}"
            ))),
            None => Err(OauthError::UnsupportedMethod(
                "AS did not advertise code_challenge_methods_supported; S256 is required"
                    .to_string(),
            )),
        }
    }

    async fn resolve_client_config(
        &self,
        manager: &mut AuthorizationManager,
        subject: &str,
        scopes: &[&str],
        dynamic_registration_use: DynamicClientRegistrationUse,
    ) -> Result<OAuthClientConfig, OauthError> {
        let oauth_cfg = self.oauth_config()?;
        match &oauth_cfg.registration {
            UpstreamOauthRegistration::Preregistered {
                client_id,
                client_secret_env,
            } => {
                let secret = match client_secret_env.as_deref() {
                    None => None,
                    Some(var) => {
                        let val = std::env::var(var).unwrap_or_default();
                        if val.is_empty() {
                            return Err(OauthError::Internal(format!(
                                "client_secret_env '{var}' is configured but env var '{var}' is not set or is empty"
                            )));
                        }
                        Some(val)
                    }
                };

                let mut cfg = OAuthClientConfig::new(client_id.clone(), self.redirect_uri.as_str());
                if let Some(s) = secret {
                    cfg = cfg.with_client_secret(s);
                }
                cfg = cfg.with_scopes(scopes.iter().map(|s| s.to_string()).collect());
                Ok(cfg)
            }
            UpstreamOauthRegistration::Dynamic => {
                // Dynamic registration (RFC 7591) has two different lifetimes:
                //   1. Stored credentials are durable and remain authoritative after
                //      a successful token exchange for normal MCP calls.
                //   2. The dynamic registration row is only pending state between
                //      begin_authorization and callback. It survives Lab restarts, but
                //      must not be reused to start a new flow because upstream AS state
                //      can be reset independently, leaving a stale client_id behind.

                match dynamic_registration_use {
                    DynamicClientRegistrationUse::StoredCredentials => {
                        if let Some(row) = self
                            .sqlite
                            .find_upstream_oauth_credentials(&self.upstream.name, subject)
                            .await
                            .map_err(OauthError::Storage)?
                        {
                            let mut cfg =
                                OAuthClientConfig::new(row.client_id, self.redirect_uri.as_str());
                            cfg = cfg.with_scopes(scopes.iter().map(|s| s.to_string()).collect());
                            return Ok(cfg);
                        }

                        return Err(OauthError::NeedsReauth(format!(
                            "no stored credentials for upstream '{}' subject '{subject}'",
                            self.upstream.name
                        )));
                    }
                    DynamicClientRegistrationUse::CompleteAuthorization => {
                        // Callback/token exchange path: use the client_id created
                        // by the begin_authorization call. This keeps callbacks
                        // valid across Lab process restarts and lets an explicit
                        // reauth flow replace stale stored credentials.
                        if let Some(client_id) = self
                            .sqlite
                            .find_dynamic_client_registration(&self.upstream.name, subject)
                            .await
                            .map_err(OauthError::Storage)?
                        {
                            let mut cfg =
                                OAuthClientConfig::new(client_id, self.redirect_uri.as_str());
                            cfg = cfg.with_scopes(scopes.iter().map(|s| s.to_string()).collect());
                            return Ok(cfg);
                        }

                        return Err(OauthError::NeedsReauth(format!(
                            "no dynamic client registration for upstream '{}' subject '{subject}'",
                            self.upstream.name
                        )));
                    }
                    DynamicClientRegistrationUse::BeginAuthorization => {}
                }

                // Beginning a new flow: register with the AS every time there are
                // no stored credentials. This self-heals when the upstream AS loses
                // its dynamic-client DB while Lab still has an old pending row.
                let cfg = manager
                    .register_client("lab", self.redirect_uri.as_str(), scopes)
                    .await
                    .map_err(|e| OauthError::Internal(format!("dynamic registration: {e}")))?;

                self.sqlite
                    .save_dynamic_client_registration(&self.upstream.name, subject, &cfg.client_id)
                    .await
                    .map_err(OauthError::Storage)?;

                // Read back the persisted value to use the DB-canonical client_id.
                let canonical_client_id = self
                    .sqlite
                    .find_dynamic_client_registration(&self.upstream.name, subject)
                    .await
                    .map_err(OauthError::Storage)?
                    .ok_or_else(|| {
                        OauthError::Internal(
                            "dynamic registration saved but read-back returned nothing".to_string(),
                        )
                    })?;

                let mut canonical_cfg =
                    OAuthClientConfig::new(canonical_client_id, self.redirect_uri.as_str());
                canonical_cfg =
                    canonical_cfg.with_scopes(scopes.iter().map(|s| s.to_string()).collect());
                Ok(canonical_cfg)
            }
            UpstreamOauthRegistration::ClientMetadataDocument { url } => {
                // Client ID Metadata Document (CIMD): the metadata-document URL
                // *is* the client identifier. No registration_endpoint call is
                // issued — the AS fetches the document itself when it first sees
                // the client_id. We construct the OAuth client locally.
                let parsed = url::Url::parse(url).map_err(|e| {
                    OauthError::Internal(format!("invalid client_metadata_document url: {e}"))
                })?;
                if parsed.scheme() != "https" {
                    return Err(OauthError::Internal(format!(
                        "client_metadata_document url must use https, got `{}`",
                        parsed.scheme()
                    )));
                }
                let cfg = OAuthClientConfig::new(url.clone(), self.redirect_uri.as_str())
                    .with_scopes(scopes.iter().map(|s| s.to_string()).collect());
                Ok(cfg)
            }
        }
    }
}

struct TokenRefreshState {
    seconds_until_expiry: i64,
    refresh_token_present: bool,
}

impl TokenRefreshState {
    fn from_row(row: &UpstreamOauthCredentialRow, now: i64) -> Option<Self> {
        if row.access_token_expires_at <= 0 {
            return None;
        }
        Some(Self {
            seconds_until_expiry: row.access_token_expires_at.saturating_sub(now),
            refresh_token_present: row.refresh_token_present,
        })
    }

    fn refresh_due(&self) -> bool {
        self.seconds_until_expiry <= PROACTIVE_REFRESH_WINDOW_SECS
    }
}

fn now_unix() -> Result<i64, OauthError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| OauthError::Internal(format!("system clock error: {error}")))
        .map(|duration| duration.as_secs() as i64)
}

fn map_refresh_error(error: rmcp::transport::AuthError) -> (bool, OauthError) {
    let terminal = match &error {
        rmcp::transport::AuthError::AuthorizationRequired
        | rmcp::transport::AuthError::TokenRefreshRejected(_) => true,
        rmcp::transport::AuthError::TokenRefreshFailed(message) => {
            let message = message.to_ascii_lowercase();
            message.contains("invalid_grant")
                || message.contains("invalid refresh token")
                || message.contains("refresh token has been revoked")
                || message.contains("refresh token expired")
        }
        _ => false,
    };
    if terminal {
        (true, map_auth_error(error))
    } else {
        (
            false,
            OauthError::Egress {
                kind: OAuthEgressKind::UpstreamError,
                message: error.to_string(),
            },
        )
    }
}

fn map_auth_error(e: rmcp::transport::AuthError) -> OauthError {
    match e {
        rmcp::transport::AuthError::AuthorizationRequired => {
            OauthError::NeedsReauth("authorization required".to_string())
        }
        rmcp::transport::AuthError::TokenExchangeFailed(msg) => OauthError::Internal(msg),
        rmcp::transport::AuthError::TokenRefreshFailed(msg) => {
            OauthError::NeedsReauth(format!("token refresh failed: {msg}"))
        }
        rmcp::transport::AuthError::TokenRefreshRejected(msg) => {
            OauthError::NeedsReauth(format!("refresh token rejected: {msg}"))
        }
        rmcp::transport::AuthError::AuthorizationServerMismatch {
            expected_issuer,
            received_issuer,
        } => OauthError::IssuerMismatch(format!(
            "expected authorization server issuer `{expected_issuer}`, received `{received_issuer}`"
        )),
        rmcp::transport::AuthError::AuthorizationServerMissingIssuer { expected_issuer } => {
            OauthError::IssuerMismatch(format!(
                "authorization response is missing required issuer `{expected_issuer}`"
            ))
        }
        other => OauthError::Internal(other.to_string()),
    }
}

#[cfg(test)]
mod url_tests {
    use super::{
        MAX_AUTHORIZATION_SERVERS, ProtectedResourceMetadata, UpstreamOauthManager,
        authorization_metadata_candidates, bounded_authorization_servers,
        discover_published_metadata, google_offline_access_url, map_auth_error, map_refresh_error,
    };
    use crate::upstream::types::OauthError;
    use labby_runtime::gateway_config::{
        UpstreamConfig, UpstreamOauthConfig, UpstreamOauthMode, UpstreamOauthRegistration,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn rejected_refresh_token_maps_to_reauthorization() {
        let error = map_auth_error(rmcp_client::transport::AuthError::TokenRefreshRejected(
            "invalid_grant".to_string(),
        ));
        assert!(matches!(error, OauthError::NeedsReauth(_)));
    }

    #[test]
    fn transient_refresh_failure_preserves_typed_egress_kind() {
        let (terminal, error) =
            map_refresh_error(rmcp_client::transport::AuthError::TokenRefreshFailed(
                "provider temporarily unavailable".to_string(),
            ));
        assert!(!terminal);
        assert!(matches!(error, OauthError::Egress { .. }));
        assert_eq!(error.kind(), "upstream_error");
    }

    #[test]
    fn callback_issuer_errors_preserve_issuer_mismatch_kind() {
        for error in [
            rmcp_client::transport::AuthError::AuthorizationServerMismatch {
                expected_issuer: "https://auth.example".to_string(),
                received_issuer: "https://evil.example".to_string(),
            },
            rmcp_client::transport::AuthError::AuthorizationServerMissingIssuer {
                expected_issuer: "https://auth.example".to_string(),
            },
        ] {
            assert!(matches!(
                map_auth_error(error),
                OauthError::IssuerMismatch(_)
            ));
        }
    }

    #[test]
    fn authorization_server_list_is_deduplicated_and_bounded() {
        let duplicate = ProtectedResourceMetadata {
            resource: None,
            authorization_server: Some("https://auth.example".to_string()),
            authorization_servers: Some(vec!["https://auth.example".to_string()]),
        };
        assert_eq!(bounded_authorization_servers(duplicate).unwrap().len(), 1);

        let excessive = ProtectedResourceMetadata {
            resource: None,
            authorization_server: None,
            authorization_servers: Some(
                (0..=MAX_AUTHORIZATION_SERVERS)
                    .map(|index| format!("https://auth-{index}.example"))
                    .collect(),
            ),
        };
        let error = bounded_authorization_servers(excessive).unwrap_err();
        assert_eq!(error.kind(), "validation_failed");
    }

    #[test]
    fn authorization_metadata_candidates_preserve_issuer_path_and_priority() {
        let issuer = url::Url::parse("https://auth.example/tenant").unwrap();
        let candidates = authorization_metadata_candidates(&issuer);
        assert_eq!(
            candidates.iter().map(url::Url::as_str).collect::<Vec<_>>(),
            vec![
                "https://auth.example/.well-known/oauth-authorization-server/tenant",
                "https://auth.example/.well-known/openid-configuration/tenant",
                "https://auth.example/tenant/.well-known/openid-configuration",
            ]
        );
    }

    #[tokio::test]
    async fn published_metadata_rejects_issuer_not_identical_to_selected_server() {
        let server = MockServer::start().await;
        let upstream = format!("{}/mcp", server.uri());
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "resource": upstream,
                "authorization_servers": [format!("{}/issuer", server.uri())]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server/issuer"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": format!("{}/issuer/", server.uri()),
                "authorization_endpoint": format!("{}/authorize", server.uri()),
                "token_endpoint": format!("{}/token", server.uri()),
                "code_challenge_methods_supported": ["S256"]
            })))
            .mount(&server)
            .await;

        let error = discover_published_metadata(&upstream).await.unwrap_err();
        assert!(matches!(error, OauthError::IssuerMismatch(_)));
    }

    #[tokio::test]
    async fn published_metadata_rejects_invalid_upstream_as_validation_error() {
        let error = discover_published_metadata("not a URL")
            .await
            .expect_err("invalid URL must fail before discovery");
        assert_eq!(error.kind(), "validation_failed");
        assert_eq!(error.http_status_code(), 400);
    }

    #[tokio::test]
    async fn malformed_authorization_server_does_not_block_later_valid_entry() {
        let server = MockServer::start().await;
        let upstream = format!("{}/mcp", server.uri());
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-protected-resource/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "resource": upstream,
                "authorization_servers": ["http://[", server.uri()]
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/.well-known/oauth-authorization-server"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": server.uri(),
                "authorization_endpoint": format!("{}/authorize", server.uri()),
                "token_endpoint": format!("{}/token", server.uri()),
                "code_challenge_methods_supported": ["S256"]
            })))
            .mount(&server)
            .await;

        let metadata = discover_published_metadata(&upstream)
            .await
            .expect("later valid authorization server must remain usable")
            .expect("metadata");
        assert_eq!(metadata.issuer.as_deref(), Some(server.uri().as_str()));
    }

    #[tokio::test]
    async fn default_and_supplied_transports_share_missing_credential_policy() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite = crate::sqlite::SqliteStore::open(dir.path().join("auth.db"))
            .await
            .expect("sqlite store");
        let key = crate::upstream::encryption::load_key(&base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            [0_u8; 32],
        ))
        .expect("encryption key");
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "issuer": format!("{}/mcp", server.uri()),
                "authorization_endpoint": format!("{}/authorize", server.uri()),
                "token_endpoint": format!("{}/token", server.uri()),
                "code_challenge_methods_supported": ["S256"]
            })))
            .mount(&server)
            .await;
        let manager = UpstreamOauthManager::new(
            sqlite,
            key,
            UpstreamConfig {
                enabled: true,
                name: "transport-parity".to_string(),
                url: Some(format!("{}/mcp", server.uri())),
                transport: None,
                socket_path: None,
                headers: Default::default(),
                command: None,
                args: vec![],
                bearer_token_env: None,
                env: Default::default(),
                proxy_resources: false,
                proxy_prompts: false,
                proxy_skills: false,
                expose_tools: None,
                expose_resources: None,
                expose_prompts: None,
                expose_skills: None,
                code_mode_hint: None,
                oauth: Some(UpstreamOauthConfig {
                    mode: UpstreamOauthMode::AuthorizationCodePkce,
                    registration: UpstreamOauthRegistration::Preregistered {
                        client_id: "client-id".to_string(),
                        client_secret_env: None,
                    },
                    scopes: None,
                    credential: Default::default(),
                    prefer_client_metadata_document: None,
                }),
                imported_from: None,
                priority: 1.0,
            },
            "http://127.0.0.1:12345/auth/upstream/callback".to_string(),
        );

        let default_error = manager
            .build_auth_client("alice")
            .await
            .expect_err("default transport must require credentials");
        let supplied_error = manager
            .build_auth_client_with("alice", reqwest::Client::new())
            .await
            .expect_err("supplied transport must require credentials");

        assert_eq!(default_error.kind(), "oauth_needs_reauth");
        assert_eq!(supplied_error.kind(), default_error.kind());
    }

    #[test]
    fn google_authorization_url_requests_offline_consent() {
        let url = "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&state=abc";
        let updated = google_offline_access_url(url).expect("url");
        let parsed = url::Url::parse(&updated).expect("updated url parses");
        let params: std::collections::HashMap<_, _> = parsed.query_pairs().collect();

        assert_eq!(
            params.get("access_type").map(|v| v.as_ref()),
            Some("offline")
        );
        assert_eq!(params.get("prompt").map(|v| v.as_ref()), Some("consent"));
        assert_eq!(
            params.get("include_granted_scopes").map(|v| v.as_ref()),
            Some("true")
        );
        assert_eq!(params.get("state").map(|v| v.as_ref()), Some("abc"));
    }

    #[test]
    fn non_google_authorization_url_is_unchanged() {
        let url = "https://auth.example.test/authorize?response_type=code&state=abc";
        let updated = google_offline_access_url(url).expect("url");
        assert_eq!(updated, url);
    }

    #[test]
    fn existing_google_authorization_params_are_preserved() {
        let url = "https://accounts.google.com/o/oauth2/v2/auth?access_type=online&prompt=select_account&include_granted_scopes=false";
        let updated = google_offline_access_url(url).expect("url");
        let parsed = url::Url::parse(&updated).expect("updated url parses");
        let params: std::collections::HashMap<_, _> = parsed.query_pairs().collect();

        assert_eq!(
            params.get("access_type").map(|v| v.as_ref()),
            Some("online")
        );
        assert_eq!(
            params.get("prompt").map(|v| v.as_ref()),
            Some("select_account")
        );
        assert_eq!(
            params.get("include_granted_scopes").map(|v| v.as_ref()),
            Some("false")
        );
    }

    #[test]
    fn pkce_validation_accepts_advertised_s256() {
        assert!(UpstreamOauthManager::verify_s256(&Some(vec!["S256".to_string()])).is_ok());
    }

    #[test]
    fn pkce_validation_rejects_advertised_non_s256_methods() {
        let error = UpstreamOauthManager::verify_s256(&Some(vec!["plain".to_string()]))
            .expect_err("plain PKCE must be refused");
        assert_eq!(error.kind(), "oauth_unsupported_method");
    }

    #[test]
    fn pkce_validation_rejects_missing_method_metadata() {
        let error = UpstreamOauthManager::verify_s256(&None)
            .expect_err("missing PKCE metadata must be refused");
        assert_eq!(error.kind(), "oauth_unsupported_method");
    }
}
