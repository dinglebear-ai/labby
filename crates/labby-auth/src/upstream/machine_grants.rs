//! SDK-backed unattended grants and expiry-aware encrypted credential storage.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::*;
#[path = "machine_enterprise.rs"]
mod enterprise_grant;
#[path = "machine_grant_store.rs"]
mod grant_store;
#[path = "machine_workload.rs"]
mod workload_grant;
use grant_store::{GrantStore, RenewingGrantStore};
use labby_runtime::gateway_config::{
    UpstreamClientJwtAlgorithm, UpstreamMachineAuthentication, UpstreamOauthMode,
};
use oauth2::TokenResponse as _;
use rmcp::transport::auth::{
    AuthError, ClientCredentialsConfig, CredentialStore, JwtSigningAlgorithm, OAuthHttpClient,
    OAuthHttpClientFuture, OAuthHttpRequest, OAuthTokenResponse, StoredCredentials,
    VendorExtraTokenFields,
};

fn secret(name: &str) -> Result<String, OauthError> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            OauthError::NeedsReauth(format!(
                "configure the unattended OAuth environment secret '{name}'"
            ))
        })
}

fn sdk_error(error: OauthError) -> AuthError {
    match error {
        OauthError::NeedsReauth(_) => AuthError::AuthorizationRequired,
        other => AuthError::CredentialStoreError(format!(
            "unattended OAuth grant failed ({})",
            other.kind()
        )),
    }
}

/// Observe definitive grant rejection without retaining provider bodies or secrets.
struct GrantHttpClient {
    inner: TrustedOriginOAuthHttpClient,
    rejected: AtomicBool,
    failure: std::sync::Mutex<Option<OAuthEgressKind>>,
    invalid_expiry: AtomicBool,
}
impl OAuthHttpClient for GrantHttpClient {
    fn execute(&self, request: OAuthHttpRequest) -> OAuthHttpClientFuture<'_> {
        Box::pin(async move {
            let post = request.request.method() == oauth2::http::Method::POST;
            let response = match self.inner.execute(request).await {
                Ok(response) => response,
                Err(error) => {
                    *self
                        .failure
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                        Some(crate::upstream::http_client::oauth_http_error_class(&error));
                    return Err(error);
                }
            };
            if post && response.status().is_success() {
                if let Ok(body) = serde_json::from_slice::<serde_json::Value>(response.body()) {
                    if body.get("access_token").is_some()
                        && body
                            .get("expires_in")
                            .and_then(serde_json::Value::as_u64)
                            .is_none_or(|expiry| expiry == 0)
                    {
                        self.invalid_expiry.store(true, Ordering::Release);
                    }
                }
            }
            if !response.status().is_success() {
                *self
                    .failure
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                    Some(OAuthEgressKind::UpstreamError);
            }
            if post
                && !response.status().is_success()
                && serde_json::from_slice::<serde_json::Value>(response.body())
                    .ok()
                    .and_then(|body| {
                        body.get("error")
                            .and_then(|error| error.as_str())
                            .map(str::to_owned)
                    })
                    .is_some_and(|error| error == "invalid_grant")
            {
                self.rejected.store(true, Ordering::Release);
            }
            Ok(response)
        })
    }
}

impl UpstreamOauthManager {
    #[cfg(test)]
    pub(in crate::upstream) fn unattended_grant_running(&self, subject: &str) -> bool {
        self.locks.is_running(&self.upstream.name, subject)
    }

    #[cfg(test)]
    pub(in crate::upstream) fn unattended_coordination_owners(&self) -> usize {
        Arc::strong_count(&self.locks)
    }

    /// Stable mode name for status and authorization surfaces.
    #[must_use]
    pub fn authorization_mode_label(&self) -> &'static str {
        match self.upstream.oauth.as_ref().map(|oauth| &oauth.mode) {
            Some(UpstreamOauthMode::ClientCredentials { .. }) => "client_credentials",
            Some(UpstreamOauthMode::EnterpriseManaged { .. }) => "enterprise_managed",
            Some(UpstreamOauthMode::WorkloadIdentityFederation { .. }) => {
                "workload_identity_federation"
            }
            _ => "authorization_code_pkce",
        }
    }

    /// Whether this configuration obtains grants without a browser interaction.
    #[must_use]
    pub fn is_unattended(&self) -> bool {
        self.authorization_mode_label() != "authorization_code_pkce"
    }

    fn unattended_binding(&self) -> Result<String, OauthError> {
        serde_json::to_string(&(&self.upstream.url, self.oauth_config()?))
            .map(|config| crate::util::fingerprint(&config))
            .map_err(|_| OauthError::Internal("serialize unattended grant configuration".into()))
    }

    pub(super) async fn unattended_authorization_manager(
        &self,
        subject: &str,
    ) -> Result<AuthorizationManager, OauthError> {
        // Bootstrap before publishing an AuthClient; later SDK reads renew through
        // the same store, including clients retained across access-token expiry.
        self.unattended_credentials(subject, 30).await?;
        let mut manager = authorization_manager_for_upstream(self.upstream_url()?.as_str()).await?;
        let metadata = self.get_or_discover_metadata(&mut manager, false).await?;
        manager.set_metadata(metadata);
        manager.set_credential_store(RenewingGrantStore {
            manager: self.clone(),
            subject: subject.to_owned(),
        });
        Ok(manager)
    }

    pub(super) async fn unattended_credentials(
        &self,
        subject: &str,
        renewal_window: u64,
    ) -> Result<(StoredCredentials, bool), OauthError> {
        let policy = self.grant_publication_policy.as_ref().ok_or_else(|| {
            OauthError::Internal(
                "unattended grants require the host credential publication policy".into(),
            )
        })?;
        if !policy.epoch.is_current() {
            return Err(OauthError::NeedsReauth(
                "unattended credential authority changed".into(),
            ));
        }
        let worker = self.clone();
        let identity = subject.to_owned();
        let changed = Arc::new(AtomicBool::new(false));
        let changed_owner = Arc::clone(&changed);
        let (executed, result) = self
            .locks
            .run_shared_owned(&self.upstream.name, subject, move || async move {
                let result = tokio::time::timeout(
                    Duration::from_secs(90),
                    worker.unattended_credentials_owned(&identity, renewal_window),
                )
                .await
                .unwrap_or_else(|_| {
                    Err(OauthError::Egress {
                        kind: OAuthEgressKind::Timeout,
                        message: "unattended OAuth grant exceeded its deadline".into(),
                    })
                });
                let policy = worker.grant_publication_policy.as_ref().ok_or_else(|| {
                    OauthError::Internal("unattended grant publication policy is missing".into())
                })?;
                if !policy.epoch.is_current() {
                    return Err(OauthError::NeedsReauth(
                        "unattended credential authority changed".into(),
                    ));
                }
                let _publication = policy.barrier.try_read().map_err(|_| {
                    OauthError::NeedsReauth("unattended credential authority is changing".into())
                })?;
                if !policy.epoch.is_current() {
                    return Err(OauthError::NeedsReauth(
                        "unattended credential authority changed".into(),
                    ));
                }
                match result {
                    Ok((_, renewed)) => {
                        worker
                            .refresh_failures
                            .clear(&worker.upstream.name, &identity);
                        changed_owner.store(renewed, Ordering::Release);
                        Ok(())
                    }
                    Err(error) => {
                        worker.refresh_failures.record_failure(
                            &worker.upstream.name,
                            &identity,
                            &error,
                        );
                        Err(error)
                    }
                }
            })
            .await;
        if !policy.epoch.is_current() {
            return Err(OauthError::NeedsReauth(
                "unattended credential authority changed".into(),
            ));
        }
        // Credential use belongs to the same authority as the completed grant.
        // This reader begins after provider work and guards only local state.
        // Renewal can run inside a checked RPC that already holds a reader.
        // A queued writer must reject this publication instead of deadlocking.
        let _publication = policy.barrier.try_read().map_err(|_| {
            OauthError::NeedsReauth("unattended credential authority is changing".into())
        })?;
        if !policy.epoch.is_current() {
            return Err(OauthError::NeedsReauth(
                "unattended credential authority changed".into(),
            ));
        }
        result?;
        let store = SqliteCredentialStore::new(
            self.sqlite.clone(),
            self.key.clone(),
            &self.upstream.name,
            subject,
        )
        .with_grant_binding(Some(self.unattended_binding()?));
        let credentials = store.load().await.map_err(map_auth_error)?.ok_or_else(|| {
            OauthError::NeedsReauth("unattended credential changed before use".into())
        })?;
        let now = now_unix()? as u64;
        let valid = credentials
            .token_received_at
            .zip(
                credentials
                    .token_response
                    .as_ref()
                    .and_then(|token| token.expires_in()),
            )
            .is_some_and(|(received, expires)| received.saturating_add(expires.as_secs()) > now);
        if !valid {
            return Err(OauthError::Egress {
                kind: OAuthEgressKind::UpstreamError,
                message: "unattended access token expired before use".into(),
            });
        }
        Ok((credentials, executed && changed.load(Ordering::Acquire)))
    }

    async fn unattended_credentials_owned(
        &self,
        subject: &str,
        renewal_window: u64,
    ) -> Result<(StoredCredentials, bool), OauthError> {
        if subject.trim().is_empty() {
            return Err(OauthError::NeedsReauth(
                "unattended grants require an identified caller".into(),
            ));
        }
        self.upstream
            .validate()
            .map_err(|_| OauthError::Internal("invalid unattended OAuth configuration".into()))?;
        let mut store = SqliteCredentialStore::new(
            self.sqlite.clone(),
            self.key.clone(),
            &self.upstream.name,
            subject,
        )
        .with_grant_binding(Some(self.unattended_binding()?))
        .with_grant_publication_policy(self.grant_publication_policy.clone().ok_or_else(|| {
            OauthError::Internal("unattended grant publication policy is missing".into())
        })?);
        if let Some(handler) = &self.refresh_rejection_handler {
            store = store.with_refresh_rejection_handler(Arc::clone(handler));
        }
        let store = Arc::new(store);
        // Same identity guard used by ordinary OAuth refreshes; no nested guard.
        let _guard = store
            .acquire_refresh_guard()
            .await
            .map_err(map_auth_error)?;
        let current = store.load().await.map_err(map_auth_error)?;
        let now = now_unix()? as u64;
        let fresh = current.as_ref().is_some_and(|credentials| {
            let expires = credentials
                .token_response
                .as_ref()
                .and_then(|token| token.expires_in());
            credentials
                .token_received_at
                .zip(expires)
                .is_some_and(|(received, expires)| {
                    received.saturating_add(expires.as_secs())
                        > now.saturating_add(renewal_window.min(expires.as_secs() / 10))
                })
        });
        if fresh {
            return Ok((current.expect("fresh credentials exist"), false));
        }
        if let Some(error) = self
            .refresh_failures
            .recent_error(&self.upstream.name, subject)
        {
            return Err(error);
        }
        let http = Arc::new(GrantHttpClient {
            inner: TrustedOriginOAuthHttpClient::new(self.upstream_url()?.as_str())?,
            rejected: AtomicBool::new(false),
            failure: std::sync::Mutex::new(None),
            invalid_expiry: AtomicBool::new(false),
        });
        let result = self
            .exchange_unattended_grant(Arc::clone(&store), Arc::clone(&http))
            .await;
        if let Err(error) = result {
            if http.invalid_expiry.load(Ordering::Acquire) {
                return Err(OauthError::Egress {
                    kind: OAuthEgressKind::UpstreamError,
                    message: "unattended OAuth token response requires positive expires_in".into(),
                });
            }
            if let Some(current) = current.as_ref()
                && http.rejected.load(Ordering::Acquire)
            {
                store
                    .on_refresh_token_rejected(current)
                    .await
                    .map_err(map_auth_error)?;
                return Err(OauthError::NeedsReauth(
                    "the provider rejected the unattended grant; update its configured credential"
                        .into(),
                ));
            }
            if let Some(kind) = *http
                .failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                return Err(OauthError::Egress {
                    kind,
                    message: "unattended OAuth request failed".into(),
                });
            }
            return Err(error);
        }
        let credentials = store.load().await.map_err(map_auth_error)?.ok_or_else(|| {
            OauthError::NeedsReauth("unattended credential changed during grant persistence".into())
        })?;
        Ok((credentials, true))
    }

    async fn exchange_unattended_grant(
        &self,
        store: Arc<SqliteCredentialStore>,
        http: Arc<GrantHttpClient>,
    ) -> Result<(), OauthError> {
        let oauth = self.oauth_config()?;
        let UpstreamOauthRegistration::Preregistered {
            client_id,
            client_secret_env,
        } = &oauth.registration
        else {
            return Err(OauthError::Internal(
                "unattended grants require preregistered client metadata".into(),
            ));
        };
        let resource = self.upstream_url()?;
        let mut manager =
            AuthorizationManager::new_with_oauth_http_client(resource.as_str(), http.clone())
                .await
                .map_err(|_| {
                    OauthError::Internal("create unattended authorization manager".into())
                })?;
        let metadata = self.get_or_discover_metadata(&mut manager, false).await?;
        let scopes = self.effective_scopes()?;
        manager.set_metadata(metadata.clone());
        match &oauth.mode {
            UpstreamOauthMode::ClientCredentials { authentication } => {
                let config = match authentication {
                    UpstreamMachineAuthentication::ClientSecret => {
                        ClientCredentialsConfig::ClientSecret {
                            client_id: client_id.clone(),
                            client_secret: secret(client_secret_env.as_deref().ok_or_else(
                                || OauthError::Internal("client_secret_env is required".into()),
                            )?)?,
                            scopes,
                            resource: Some(resource.to_string()),
                        }
                    }
                    UpstreamMachineAuthentication::PrivateKeyJwt {
                        signing_key_env,
                        algorithm,
                        token_endpoint_audience,
                    } => ClientCredentialsConfig::PrivateKeyJwt {
                        client_id: client_id.clone(),
                        signing_key: secret(signing_key_env)?.into_bytes(),
                        signing_algorithm: match algorithm {
                            UpstreamClientJwtAlgorithm::RS256 => JwtSigningAlgorithm::RS256,
                            UpstreamClientJwtAlgorithm::RS384 => JwtSigningAlgorithm::RS384,
                            UpstreamClientJwtAlgorithm::RS512 => JwtSigningAlgorithm::RS512,
                            UpstreamClientJwtAlgorithm::ES256 => JwtSigningAlgorithm::ES256,
                            UpstreamClientJwtAlgorithm::ES384 => JwtSigningAlgorithm::ES384,
                        },
                        token_endpoint_audience: token_endpoint_audience.clone(),
                        scopes,
                        resource: Some(resource.to_string()),
                    },
                };
                manager.validate_client_credentials_metadata(&config).map_err(|_| OauthError::Internal("authorization metadata does not approve the configured client-credentials authentication".into()))?;
                manager.configure_client_credentials(&config).map_err(|_| {
                    OauthError::Internal("configure unattended client credentials".into())
                })?;
                manager.set_credential_store(GrantStore(store));
                manager.exchange_client_credentials(&config).await.map_err(|_| OauthError::NeedsReauth("client-credentials exchange failed; check the configured registration and secret".into()))?;
            }
            UpstreamOauthMode::EnterpriseManaged { .. } => {
                self.exchange_enterprise_grant(&metadata, store.as_ref(), http.as_ref())
                    .await?;
            }
            UpstreamOauthMode::WorkloadIdentityFederation { .. } => {
                self.exchange_workload_grant(&metadata, store.as_ref(), http.as_ref())
                    .await?;
            }
            UpstreamOauthMode::AuthorizationCodePkce => {
                return Err(OauthError::Internal(
                    "interactive authorization is not an unattended grant".into(),
                ));
            }
        }
        Ok(())
    }
}
