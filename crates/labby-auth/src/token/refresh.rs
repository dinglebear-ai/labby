use super::*;

/// The local single-use claim starts only after subject-scoped provider
/// serialization. Keep enough headroom beyond Google's 30-second HTTP timeout
/// for response verification, durable broker persistence, JWT issuance, and
/// the final atomic local-token rotation.
const REFRESH_CLAIM_LEASE_SECONDS: i64 = 90;

#[cfg(test)]
static REFRESH_LOCK_WAITERS: std::sync::OnceLock<
    dashmap::DashMap<String, std::sync::Arc<std::sync::atomic::AtomicUsize>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
pub(super) fn refresh_lock_waiter_counter(
    subject: &str,
) -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
    REFRESH_LOCK_WAITERS
        .get_or_init(dashmap::DashMap::new)
        .entry(subject.to_string())
        .or_insert_with(|| std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)))
        .clone()
}
const REFRESH_CLAIM_RENEW_INTERVAL: Duration = Duration::from_secs(20);
const REFRESH_REPLAY_GRACE_SECONDS: i64 = 5 * 60;

pub(super) struct RefreshClaimLease {
    store: crate::sqlite::SqliteStore,
    refresh_token: String,
    claim_id: String,
    heartbeat: Option<tokio::task::JoinHandle<()>>,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    active: bool,
    #[cfg(test)]
    observer: Option<std::sync::Arc<RefreshClaimLeaseObserver>>,
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct RefreshClaimLeaseObserver {
    pub(super) cancellation_released: tokio::sync::Notify,
    pub(super) renewal_finished: tokio::sync::Notify,
    pub(super) explicit_release_started: tokio::sync::Notify,
    pub(super) explicit_release_continue: tokio::sync::Notify,
}

impl RefreshClaimLease {
    fn start(
        store: crate::sqlite::SqliteStore,
        refresh_token: String,
        claim_id: String,
        refresh_token_id: String,
    ) -> (Self, tokio::sync::oneshot::Receiver<AuthError>) {
        Self::start_with_timing(
            store,
            refresh_token,
            claim_id,
            refresh_token_id,
            REFRESH_CLAIM_LEASE_SECONDS,
            REFRESH_CLAIM_RENEW_INTERVAL,
        )
    }

    fn start_with_timing(
        store: crate::sqlite::SqliteStore,
        refresh_token: String,
        claim_id: String,
        refresh_token_id: String,
        lease_seconds: i64,
        renew_interval: Duration,
    ) -> (Self, tokio::sync::oneshot::Receiver<AuthError>) {
        Self::start_inner(
            store,
            refresh_token,
            claim_id,
            refresh_token_id,
            lease_seconds,
            renew_interval,
            None,
        )
    }

    #[cfg(test)]
    pub(super) fn start_with_timing_observed(
        store: crate::sqlite::SqliteStore,
        refresh_token: String,
        claim_id: String,
        refresh_token_id: String,
        lease_seconds: i64,
        renew_interval: Duration,
        observer: std::sync::Arc<RefreshClaimLeaseObserver>,
    ) -> (Self, tokio::sync::oneshot::Receiver<AuthError>) {
        Self::start_inner(
            store,
            refresh_token,
            claim_id,
            refresh_token_id,
            lease_seconds,
            renew_interval,
            Some(observer),
        )
    }

    fn start_inner(
        store: crate::sqlite::SqliteStore,
        refresh_token: String,
        claim_id: String,
        refresh_token_id: String,
        lease_seconds: i64,
        renew_interval: Duration,
        #[cfg(test)] observer: Option<std::sync::Arc<RefreshClaimLeaseObserver>>,
        #[cfg(not(test))] _observer: Option<()>,
    ) -> (Self, tokio::sync::oneshot::Receiver<AuthError>) {
        let heartbeat_store = store.clone();
        let heartbeat_token = refresh_token.clone();
        let heartbeat_claim_id = claim_id.clone();
        let heartbeat_token_id = refresh_token_id.clone();
        let (lost_tx, lost_rx) = tokio::sync::oneshot::channel();
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
        #[cfg(test)]
        let heartbeat_observer = observer.clone();
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut cancel_rx => {
                        let release = heartbeat_store
                            .release_refresh_claim(&heartbeat_token, &heartbeat_claim_id)
                            .await;
                        match release {
                            Ok(()) => debug!(
                                refresh_token_id = %heartbeat_token_id,
                                "oauth refresh_token claim released after request cancellation"
                            ),
                            Err(error) => warn!(
                                refresh_token_id = %heartbeat_token_id,
                                kind = error.kind(),
                                error = %error,
                                "oauth refresh_token claim release after cancellation failed"
                            ),
                        }
                        #[cfg(test)]
                        if let Some(observer) = heartbeat_observer.as_ref() {
                            observer.cancellation_released.notify_one();
                        }
                        return;
                    }
                    renewal = async {
                        tokio::time::sleep(renew_interval).await;
                        let expires_at = now_unix().saturating_add(lease_seconds);
                        heartbeat_store
                            .renew_refresh_claim(&heartbeat_token, &heartbeat_claim_id, expires_at)
                            .await
                    } => {
                        #[cfg(test)]
                        if let Some(observer) = heartbeat_observer.as_ref() {
                            observer.renewal_finished.notify_one();
                        }
                        match renewal {
                            Ok(true) => {
                                debug!(
                                    refresh_token_id = %heartbeat_token_id,
                                    claim_lease_seconds = lease_seconds,
                                    "oauth refresh_token claim lease renewed"
                                );
                            }
                            Ok(false) => {
                                let error = AuthError::InvalidGrant(
                                    "refresh token claim ownership was lost".to_string(),
                                );
                                warn!(
                                    refresh_token_id = %heartbeat_token_id,
                                    kind = error.kind(),
                                    "oauth refresh_token claim lease could not be renewed"
                                );
                                drop(lost_tx.send(error));
                                return;
                            }
                            Err(error) => {
                                warn!(
                                    refresh_token_id = %heartbeat_token_id,
                                    kind = error.kind(),
                                    error = %error,
                                    "oauth refresh_token claim lease renewal failed"
                                );
                                drop(lost_tx.send(error));
                                return;
                            }
                        }
                    }
                }
            }
        });
        (
            Self {
                store,
                refresh_token,
                claim_id,
                heartbeat: Some(heartbeat),
                cancel: Some(cancel_tx),
                active: true,
                #[cfg(test)]
                observer,
            },
            lost_rx,
        )
    }

    pub(super) fn disarm(&mut self) {
        self.active = false;
        self.cancel.take();
        if let Some(heartbeat) = self.heartbeat.take() {
            heartbeat.abort();
        }
    }

    pub(super) async fn release(mut self) -> Result<(), AuthError> {
        self.cancel.take();
        if let Some(heartbeat) = self.heartbeat.take() {
            heartbeat.abort();
        }
        let store = self.store.clone();
        let refresh_token = self.refresh_token.clone();
        let refresh_token_id = fingerprint(&refresh_token);
        let claim_id = self.claim_id.clone();
        #[cfg(test)]
        let observer = self.observer.clone();
        let cleanup = tokio::spawn(async move {
            #[cfg(test)]
            if let Some(observer) = observer.as_ref() {
                observer.explicit_release_started.notify_one();
                observer.explicit_release_continue.notified().await;
            }
            let result = store.release_refresh_claim(&refresh_token, &claim_id).await;
            match result.as_ref() {
                Ok(()) => debug!(
                    refresh_token_id = %refresh_token_id,
                    "oauth refresh_token claim released after completed request"
                ),
                Err(error) => warn!(
                    refresh_token_id = %refresh_token_id,
                    kind = error.kind(),
                    error = %error,
                    "oauth refresh_token claim release after completed request failed"
                ),
            }
            result
        });
        // From this point onward the owned task, rather than Drop, is solely
        // responsible for cleanup. Dropping this request future detaches the
        // task but cannot cancel the durable release.
        self.active = false;
        cleanup.await.map_err(|error| {
            AuthError::Storage(format!("refresh claim release task failed: {error}"))
        })?
    }
}

impl Drop for RefreshClaimLease {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
            // The heartbeat task owns the cancellation cleanup and must remain
            // detached long enough to release the durable claim.
            self.heartbeat.take();
        } else if let Some(heartbeat) = self.heartbeat.take() {
            heartbeat.abort();
        }
    }
}

pub(super) async fn refresh_token_grant(
    state: AuthState,
    request: TokenRequest,
) -> Result<TokenResponse, AuthError> {
    let requested_resource = request
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| crate::authorize::validate_resource(&state, Some(value)))
        .transpose()?;
    let client_id = require_field(request.client_id, "client_id")?;
    let refresh_token = require_field(request.refresh_token, "refresh_token")?;
    authenticate_oauth_client(
        &state,
        &client_id,
        request.client_secret.as_deref(),
        request.client_assertion_type.as_deref(),
        request.client_assertion.as_deref(),
    )
    .await?;
    if let Some(response) = cached_refresh_response(
        &state,
        &client_id,
        &refresh_token,
        requested_resource.as_deref(),
    )
    .await?
    {
        info!(
            grant_type = "refresh_token",
            client_id = %fingerprint(&client_id),
            refresh_token_id = %fingerprint(&refresh_token),
            "oauth refresh_token retry reused the prior rotated response"
        );
        return Ok(response);
    }
    let refresh_token_id = fingerprint(&refresh_token);
    debug!(
        grant_type = "refresh_token",
        client_id = %fingerprint(&client_id),
        refresh_token_id = %refresh_token_id,
        requested_resource_id = %requested_resource.as_deref().map(fingerprint).unwrap_or_else(|| "<refresh-token-resource>".to_string()),
        "oauth refresh_token grant received"
    );
    let refresh_subject = state
        .store
        .find_refresh_token(&refresh_token)
        .await?
        .map(|stored| stored.subject)
        .ok_or_else(|| {
            debug!(
                refresh_token_id = %refresh_token_id,
                client_id = %fingerprint(&client_id),
                "oauth token rejected: unknown or expired refresh token"
            );
            AuthError::InvalidGrant("unknown refresh_token".to_string())
        })?;
    let subject_id = fingerprint(&refresh_subject);
    let claim_id = random_token(18)?;
    let claim_expires_at = now_unix().saturating_add(REFRESH_CLAIM_LEASE_SECONDS);
    let (stored, lock_wait_ms) =
        claim_refresh_after_subject_lock(&state.store, &refresh_token, &claim_id, claim_expires_at)
            .await?;
    debug!(
        grant_type = "refresh_token",
        client_id = %fingerprint(&client_id),
        refresh_token_id = %refresh_token_id,
        subject_id = %subject_id,
        lock_wait_ms,
        claim_lease_seconds = REFRESH_CLAIM_LEASE_SECONDS,
        "oauth refresh_token grant acquired subject serialization before local claim"
    );
    let Some(stored) = stored else {
        // Another request may own the durable claim while its provider refresh
        // is still in flight. Join through the replay record it publishes
        // instead of immediately turning ordinary concurrency into
        // `invalid_grant`. This wait holds no mutex and therefore cannot form a
        // provider-I/O convoy.
        let replay_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(response) = cached_refresh_response(
                &state,
                &client_id,
                &refresh_token,
                requested_resource.as_deref(),
            )
            .await?
            {
                info!(
                    grant_type = "refresh_token",
                    client_id = %fingerprint(&client_id),
                    refresh_token_id = %refresh_token_id,
                    lock_wait_ms,
                    "oauth concurrent refresh reused the prior rotated response"
                );
                return Ok(response);
            }
            if tokio::time::Instant::now() >= replay_deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        debug!(
            refresh_token_id = %refresh_token_id,
            client_id = %fingerprint(&client_id),
            "oauth token rejected: unknown or expired refresh token"
        );
        return Err(AuthError::InvalidGrant("unknown refresh_token".to_string()));
    };
    let (mut claim_lease, claim_lost) = RefreshClaimLease::start(
        state.store.clone(),
        refresh_token.clone(),
        claim_id.clone(),
        refresh_token_id.clone(),
    );
    let operation = complete_claimed_refresh(
        &state,
        &client_id,
        &refresh_token,
        &claim_id,
        &refresh_token_id,
        requested_resource,
        stored,
    );
    tokio::pin!(operation);
    tokio::pin!(claim_lost);
    let result = tokio::select! {
        biased;
        result = &mut operation => result,
        lost = &mut claim_lost => Err(lost.unwrap_or_else(|_| {
            AuthError::Storage("refresh token claim heartbeat stopped unexpectedly".to_string())
        })),
    };
    if result.is_ok() {
        claim_lease.disarm();
    } else {
        if matches!(&result, Err(AuthError::OauthNeedsReauth(_))) {
            state.store.revoke_refresh_token(&refresh_token).await?;
        }
        claim_lease.release().await?;
    }
    result
}

pub(super) async fn claim_refresh_after_subject_lock(
    store: &crate::sqlite::SqliteStore,
    refresh_token: &str,
    claim_id: &str,
    claim_expires_at: i64,
) -> Result<(Option<RefreshTokenRow>, u128), AuthError> {
    let lock_wait_started = Instant::now();
    #[cfg(test)]
    refresh_lock_waiter_counter(refresh_token).fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let lock_wait_ms = lock_wait_started.elapsed().as_millis();
    let stored = store
        .claim_refresh_token(refresh_token, claim_id, claim_expires_at)
        .await?;
    Ok((stored, lock_wait_ms))
}

async fn refresh_google_provider_credential(
    state: &AuthState,
    subject: &str,
    refresh_token_id: &str,
) -> Result<GoogleExchange, AuthError> {
    let subject_id = fingerprint(subject);
    let mut credential = state
        .store
        .find_google_provider_credential(subject)
        .await?
        .ok_or_else(|| {
            warn!(
                refresh_token_id = %refresh_token_id,
                subject_id = %subject_id,
                kind = "oauth_needs_reauth",
                "oauth token rejected: no google provider credential exists for subject"
            );
            AuthError::OauthNeedsReauth(
                "google provider credential is unavailable; reauthorization required".to_string(),
            )
        })?;

    let allowed = state.resolve_allowed_emails().await?;
    if crate::authorize::check_email_allowlist(
        credential.email.as_deref(),
        credential.email.as_ref().map(|_| true),
        None,
        &allowed,
        &state.config.allowed_email_domains,
    )
    .is_err()
    {
        let invalidation = state
            .store
            .invalidate_google_provider_credential(subject, credential.generation)
            .await?;
        warn!(
            refresh_token_id = %refresh_token_id,
            subject_id = %subject_id,
            provider_credential_invalidated = invalidation.invalidated,
            revoked_refresh_tokens = invalidation.revoked_refresh_tokens,
            revoked_authorization_codes = invalidation.revoked_authorization_codes,
            kind = "oauth_needs_reauth",
            "oauth token rejected: provider subject is no longer authorized"
        );
        return Err(AuthError::OauthNeedsReauth(
            "provider subject is no longer authorized".to_string(),
        ));
    }

    let now = now_unix();
    if credential
        .access_token_expires_at
        .is_some_and(|expiry| expiry > now + 30)
        && let Some(access_token) = credential.access_token.clone()
    {
        return Ok(GoogleExchange {
            subject: credential.subject,
            email: credential.email,
            email_verified: Some(true),
            hosted_domain: None,
            access_token,
            refresh_token: None,
            expires_in: credential
                .access_token_expires_at
                .map(|expiry| expiry.saturating_sub(now) as u64),
            granted_scopes: credential.granted_scopes,
            id_token: None,
        });
    }

    if let Some(error) = crate::google_refresh::recent_transient_failure(state, subject) {
        return Err(error);
    }

    for attempt in 0..2 {
        match state
            .google
            .refresh(
                &credential.refresh_token,
                &credential.subject,
                credential.email.as_deref(),
            )
            .await
        {
            Ok(google) => {
                crate::google_refresh::clear_transient_failure(state, subject);
                if google.subject != subject {
                    let invalidation = state
                        .store
                        .invalidate_google_provider_credential(subject, credential.generation)
                        .await?;
                    warn!(
                        refresh_token_id = %refresh_token_id,
                        expected_subject_id = %subject_id,
                        returned_subject_id = %fingerprint(&google.subject),
                        provider_credential_invalidated = invalidation.invalidated,
                        kind = "auth_failed",
                        "oauth token rejected: google refresh returned a different subject"
                    );
                    return Err(AuthError::AuthFailed(
                        "google refresh returned a different account subject".to_string(),
                    ));
                }
                let next_provider_refresh_token = google
                    .refresh_token
                    .as_deref()
                    .unwrap_or(&credential.refresh_token);
                let granted_scopes =
                    merge_google_scopes(&credential.granted_scopes, &google.granted_scopes);
                let token_received_at = now_unix();
                let persisted = state
                    .store
                    .replace_google_provider_token_bundle_if_generation(
                        crate::types::GoogleProviderCredentialUpdate {
                            subject: google.subject.clone(),
                            email: google.email.clone(),
                            client_id: state.google.client_id.clone(),
                            granted_scopes: granted_scopes.clone(),
                            access_token: google.access_token.clone(),
                            refresh_token: next_provider_refresh_token.to_string(),
                            token_received_at,
                            access_token_expires_at: token_received_at.saturating_add(
                                i64::try_from(google.expires_in.unwrap_or(3600))
                                    .unwrap_or(i64::MAX),
                            ),
                            issuer: Some("https://accounts.google.com".to_string()),
                            refreshed: true,
                            scope_upgraded: false,
                        },
                        credential.generation,
                    )
                    .await?;
                if !persisted {
                    let replacement_present = state
                        .store
                        .has_google_provider_credential_for_subject(subject)
                        .await?;
                    warn!(
                        refresh_token_id = %refresh_token_id,
                        subject_id = %subject_id,
                        stale_provider_generation = credential.generation,
                        replacement_provider_credential_present = replacement_present,
                        kind = "oauth_needs_reauth",
                        "oauth provider refresh result discarded because a newer generation was persisted"
                    );
                    return Err(AuthError::OauthNeedsReauth(
                        "google provider credential changed during refresh; reauthorization required"
                            .to_string(),
                    ));
                }
                return Ok(google);
            }
            Err(AuthError::OauthNeedsReauth(message)) => {
                let invalidation = state
                    .store
                    .invalidate_google_provider_credential(subject, credential.generation)
                    .await?;
                if invalidation.invalidated {
                    warn!(
                        refresh_token_id = %refresh_token_id,
                        subject_id = %subject_id,
                        provider_generation = credential.generation,
                        revoked_refresh_tokens = invalidation.revoked_refresh_tokens,
                        revoked_authorization_codes = invalidation.revoked_authorization_codes,
                        kind = "oauth_needs_reauth",
                        "oauth provider credential invalidated after google rejected refresh"
                    );
                    return Err(AuthError::OauthNeedsReauth(message));
                }
                if attempt == 1 {
                    return Err(AuthError::OauthNeedsReauth(message));
                }
                let replacement = state
                    .store
                    .find_google_provider_credential(subject)
                    .await?
                    .ok_or_else(|| AuthError::OauthNeedsReauth(message.clone()))?;
                warn!(
                    refresh_token_id = %refresh_token_id,
                    subject_id = %subject_id,
                    stale_provider_generation = credential.generation,
                    replacement_provider_generation = replacement.generation,
                    "oauth provider credential changed during failed refresh; retrying newest generation"
                );
                credential = replacement;
            }
            Err(error) => {
                crate::google_refresh::record_transient_failure(state, subject, &error);
                return Err(error);
            }
        }
    }

    Err(AuthError::OauthNeedsReauth(
        "google provider credential could not be refreshed; reauthorization required".to_string(),
    ))
}

async fn complete_claimed_refresh(
    state: &AuthState,
    client_id: &str,
    refresh_token: &str,
    claim_id: &str,
    refresh_token_id: &str,
    requested_resource: Option<String>,
    stored: RefreshTokenRow,
) -> Result<TokenResponse, AuthError> {
    if stored.client_id != client_id {
        warn!(
            refresh_token_id = %refresh_token_id,
            requested_client_id = %fingerprint(client_id),
            stored_client_id = %fingerprint(&stored.client_id),
            "oauth token rejected: client_id does not match refresh token"
        );
        return Err(AuthError::InvalidGrant(
            "client_id does not match the refresh token".to_string(),
        ));
    }
    let stored_resource = if stored.resource.trim().is_empty() {
        crate::metadata::canonical_resource_url(state)
    } else {
        stored.resource.clone()
    };
    if let Some(requested_resource) = requested_resource
        && requested_resource != stored_resource
    {
        warn!(
            refresh_token_id = %refresh_token_id,
            requested_resource_id = %fingerprint(&requested_resource),
            stored_resource_id = %fingerprint(&stored_resource),
            "oauth token rejected: resource does not match refresh token"
        );
        return Err(AuthError::InvalidGrant(
            "resource does not match the refresh token".to_string(),
        ));
    }

    // Refresh the subject-scoped Google credential before consuming the local
    // token. An invalid_grant compare-and-deletes the exact provider
    // generation that failed and atomically revokes every dependent local
    // grant, so the next authorization is forced through fresh consent.
    let (_, google) = crate::google_refresh::run_shared(state, &stored.subject, || {
        refresh_google_provider_credential(state, &stored.subject, refresh_token_id)
    })
    .await;
    let google = google?;

    let now = now_unix();
    let refreshed_expires_at = expires_at(
        now,
        state.config.refresh_token_ttl,
        &format!("{}_AUTH_REFRESH_TOKEN_TTL_SECS", state.config.env_prefix),
    )?;
    // A refresh grant may preserve, but never expand, the authorization that
    // was bound to the original grant. Policy changes require a new consent
    // flow rather than silently elevating a legacy token.
    let refreshed_scope = stored.scope.clone();

    let replacement_refresh_token = random_token(24)?;
    let replacement = RefreshTokenRow {
        refresh_token: replacement_refresh_token.clone(),
        client_id: stored.client_id.clone(),
        subject: google.subject.clone(),
        resource: stored_resource.clone(),
        scope: refreshed_scope.clone(),
        provider_refresh_token: None,
        created_at: now,
        expires_at: refreshed_expires_at,
    };
    let response = build_token_response(
        state,
        stored.client_id.clone(),
        google.subject.clone(),
        stored_resource.clone(),
        refreshed_scope.clone(),
        Some(replacement_refresh_token),
        TokenIdentity::ExternalIssuer(crate::google::GOOGLE_ISSUER.to_string()),
    )?;
    let response_ttl = i64::try_from(response.expires_in).unwrap_or(i64::MAX);
    let replay_expires_at = now.saturating_add(REFRESH_REPLAY_GRACE_SECONDS.min(response_ttl));
    state
        .store
        .rotate_claimed_refresh_token(
            refresh_token,
            claim_id,
            replacement,
            &response,
            replay_expires_at,
        )
        .await?
        .ok_or_else(|| AuthError::InvalidGrant("refresh token was already used".to_string()))?;

    info!(
        grant_type = "refresh_token",
        client_id = %fingerprint(&stored.client_id),
        refresh_token_id = %refresh_token_id,
        subject_id = %fingerprint(&google.subject),
        resource_id = %fingerprint(&stored_resource),
        scope_id = %fingerprint(&refreshed_scope),
        "oauth refresh_token grant rotated local token and issued new access token"
    );

    Ok(response)
}
