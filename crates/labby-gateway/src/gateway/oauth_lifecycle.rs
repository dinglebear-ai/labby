use std::sync::Arc;

use crate::gateway::manager::{GatewayManager, OauthStatusDiscoverySnapshot};
use crate::gateway::oauth::{UpstreamOauthConnectionState, UpstreamOauthStatusView};
use labby_auth::upstream::encryption::EncryptionKey;
use labby_auth::upstream::manager::UpstreamOauthManager;
use labby_auth::upstream::types::{BeginAuthorization, OauthError};
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::UpstreamConfig;

use crate::upstream::pool::OAuthSessionInvalidation;

const OAUTH_STATUS_DISCOVERY_FRESHNESS: std::time::Duration = std::time::Duration::from_secs(30);
const OAUTH_STATUS_DISCOVERY_FAILURE_COOLDOWN: std::time::Duration =
    std::time::Duration::from_mins(5);
const OAUTH_STATUS_DISCOVERY_CACHE_MAX: usize = 256;
const OAUTH_STATUS_EPOCH_MAX: usize = 4_096;

#[cfg(test)]
static STATUS_DISCOVERY_BARRIER: std::sync::OnceLock<
    std::sync::Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
fn install_status_discovery_barrier() -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    let barrier = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    *STATUS_DISCOVERY_BARRIER
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .expect("status barrier lock") = Some(barrier.clone());
    barrier
}

#[cfg(test)]
async fn wait_on_status_discovery_barrier() {
    let barrier = STATUS_DISCOVERY_BARRIER
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .expect("status barrier lock")
        .take();
    if let Some((entered, release)) = barrier {
        entered.notify_one();
        release.notified().await;
    }
}

fn oauth_status_discovery_is_fresh(snapshot: &OauthStatusDiscoverySnapshot) -> bool {
    snapshot.completed_at.elapsed()
        < if snapshot.tool_error.is_some() || snapshot.error.is_some() {
            OAUTH_STATUS_DISCOVERY_FAILURE_COOLDOWN
        } else {
            OAUTH_STATUS_DISCOVERY_FRESHNESS
        }
}

pub(crate) mod probe;
#[cfg(test)]
mod tests;

pub(super) fn tool_error_from_oauth(error: OauthError) -> ToolError {
    ToolError::Sdk {
        sdk_kind: error.kind().to_string(),
        message: error.to_string(),
    }
}

/// Decide whether to use RFC 7591 dynamic registration for an upstream.
///
/// The `prefer_client_metadata_document` field on `UpstreamOauthConfig` is the
/// authoritative control:
/// - `Some(true)`  → always use CIMD (Client ID Metadata Document), never dynamic
/// - `Some(false)` → always use dynamic registration when `supports_dynamic` is true
/// - `None` → legacy default: upstreams named `"swag"` use CIMD; all others
///   use dynamic registration when available.
///
/// The `"swag"` name check is intentionally kept as a **documented legacy default**
/// so existing deployments that omit the field continue to work. New upstreams
/// should set `prefer_client_metadata_document` explicitly.
pub(super) fn should_use_dynamic_registration(
    upstream: &str,
    supports_dynamic: bool,
    prefer_cimd: Option<bool>,
) -> bool {
    if !supports_dynamic {
        return false;
    }
    match prefer_cimd {
        Some(true) => false,        // operator explicitly prefers CIMD
        Some(false) => true,        // operator explicitly prefers dynamic registration
        None => upstream != "swag", // legacy: "swag" uses CIMD by default
    }
}

impl GatewayManager {
    async fn promote_probe_oauth_config(
        &self,
        upstream: &str,
        oauth_config: labby_runtime::gateway_config::UpstreamOauthConfig,
    ) -> Result<bool, ToolError> {
        // Hold lease ownership through persistence and runtime reconciliation.
        // The sweeper cannot observe an expired transient lease between those
        // two state transitions and delete the newly durable manager.
        let mut leases = self.transient_oauth_managers.lock().await;
        let mutation_guard = self.acquire_config_mutation().await?;
        let mut cfg = self.load_config_for_mutation().await?;
        let Some(existing) = cfg.upstream.iter_mut().find(|entry| entry.name == upstream) else {
            return Ok(false);
        };
        if existing.oauth.is_none() {
            existing.oauth = Some(oauth_config);
            let promoted = cfg.clone();
            self.persist_config_owned(mutation_guard, cfg).await?;
            leases.remove(upstream);
            self.reconcile_upstream_oauth_managers(&promoted);
        } else {
            leases.remove(upstream);
            self.reconcile_upstream_oauth_managers(&cfg);
        }
        Ok(true)
    }

    async fn discard_transient_oauth_manager(&self, upstream: &str) {
        let removed = self
            .transient_oauth_managers
            .lock()
            .await
            .remove(upstream)
            .is_some();
        if removed {
            if let Some(managers) = &self.upstream_oauth_managers {
                managers.remove(upstream);
            }
            if let Some(cache) = &self.oauth_client_cache {
                cache.evict_upstream(upstream);
            }
        }
    }

    fn oauth_status_pool(
        &self,
        request_timeout: std::time::Duration,
        relay_timeout: std::time::Duration,
    ) -> (Arc<crate::upstream::pool::UpstreamPool>, bool) {
        match self.current_pool_sync() {
            Some(pool) => (pool, false),
            None => (
                Arc::new(self.new_base_pool(request_timeout, relay_timeout)),
                true,
            ),
        }
    }

    async fn invalidate_oauth_status_discovery(&self, upstream: &str, subject: Option<&str>) {
        use std::sync::atomic::Ordering;

        // Wait for already-published single-flight discoveries, while the
        // generation fences discoveries that race lock publication. This
        // prevents clear/revoke/callback invalidation from being followed by a
        // stale status snapshot inserted by older work.
        let locks: Vec<_> = self
            .oauth_status_discovery_locks
            .iter()
            .filter(|entry| {
                entry.key().0 == upstream && subject.is_none_or(|subject| entry.key().1 == subject)
            })
            .map(|entry| entry.value().clone())
            .collect();
        let mut _guards = Vec::with_capacity(locks.len());
        for lock in locks {
            _guards.push(lock.lock_owned().await);
        }
        let epoch = self.oauth_status_next_epoch.fetch_add(1, Ordering::AcqRel);
        match subject {
            Some(subject) => {
                self.oauth_status_subject_epochs
                    .insert((upstream.to_string(), subject.to_string()), epoch);
            }
            None => {
                self.oauth_status_upstream_epochs
                    .insert(upstream.to_string(), epoch);
                self.oauth_status_subject_epochs
                    .retain(|(name, _), _| name != upstream);
            }
        }
        while self.oauth_status_subject_epochs.len() > OAUTH_STATUS_EPOCH_MAX {
            let oldest = self
                .oauth_status_subject_epochs
                .iter()
                .min_by_key(|entry| *entry.value())
                .map(|entry| entry.key().clone());
            let Some(oldest) = oldest else { break };
            self.oauth_status_subject_epochs.remove(&oldest);
        }
        self.oauth_status_discovery_cache.lock().await.retain(
            |(cached_upstream, cached_subject), _| {
                cached_upstream != upstream
                    || subject.is_some_and(|subject| cached_subject != subject)
            },
        );
        drop(_guards);
    }

    async fn invalidate_shared_oauth_status_discovery(&self) {
        let upstreams = {
            let config = self.config.read().await;
            Self::google_provider_upstream_names(&config)
        };
        for upstream in upstreams {
            self.invalidate_oauth_status_discovery(&upstream, None)
                .await;
        }
    }

    async fn oauth_status_discovery(
        &self,
        upstream: &str,
        subject: &str,
        config: UpstreamConfig,
    ) -> OauthStatusDiscoverySnapshot {
        let key = (upstream.to_string(), subject.to_string());
        if let Some(snapshot) = self.oauth_status_discovery_cache.lock().await.get(&key)
            && oauth_status_discovery_is_fresh(snapshot)
        {
            tracing::debug!(
                service = "upstream_oauth",
                action = "status.discovery",
                upstream,
                cache_hit = true,
                failure_cooldown = snapshot.tool_error.is_some() || snapshot.error.is_some(),
                "upstream oauth status discovery reused cached result"
            );
            return snapshot.clone();
        }

        let lock = self
            .oauth_status_discovery_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;
        if let Some(snapshot) = self.oauth_status_discovery_cache.lock().await.get(&key)
            && oauth_status_discovery_is_fresh(snapshot)
        {
            return snapshot.clone();
        }

        let started = std::time::Instant::now();
        let discovery_epoch = self.oauth_status_epoch(upstream, subject);
        let (request_timeout, relay_timeout) = {
            let cfg = self.config.read().await;
            (cfg.upstream_request_timeout(), cfg.upstream_relay_timeout())
        };
        // Status participates in the manager-owned connection lifecycle. When
        // a live pool is published, refresh that exact pool so status observes
        // the same connection incarnation as routed traffic. A cold manager
        // uses an explicitly ephemeral probe pool and drains it before return.
        let (pool, ephemeral_pool) = self.oauth_status_pool(request_timeout, relay_timeout);
        let permit = tokio::time::timeout(
            request_timeout,
            self.oauth_status_connect_bulkhead.clone().acquire_owned(),
        )
        .await;
        let discovery = match permit {
            Ok(Ok(_permit)) => pool.subject_scoped_upstream_summary(&config, subject).await,
            Ok(Err(_)) => Err("OAuth status connection bulkhead is closed".to_string()),
            Err(_) => Err("OAuth status connection queue timed out".to_string()),
        };
        #[cfg(test)]
        wait_on_status_discovery_barrier().await;
        let mut snapshot = OauthStatusDiscoverySnapshot {
            completed_at: tokio::time::Instant::now(),
            summary: discovery.as_ref().ok().copied(),
            tool_error: discovery.err(),
            error: None,
        };
        if ephemeral_pool {
            pool.drain_for_swap("oauth.status.ephemeral").await;
        }
        if self.oauth_status_epoch(upstream, subject) != discovery_epoch {
            snapshot.summary = None;
            snapshot.tool_error = Some(
                "OAuth credentials changed while status discovery was running; retry status"
                    .to_string(),
            );
            snapshot.error = None;
            if Arc::strong_count(&lock) <= 2 {
                self.oauth_status_discovery_locks.remove(&key);
            }
            return snapshot;
        }
        let mut cache = self.oauth_status_discovery_cache.lock().await;
        if cache.len() >= OAUTH_STATUS_DISCOVERY_CACHE_MAX && !cache.contains_key(&key) {
            if let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, value)| value.completed_at)
                .map(|(key, _)| key.clone())
            {
                cache.remove(&oldest);
            }
        }
        cache.insert(key.clone(), snapshot.clone());
        drop(cache);
        if Arc::strong_count(&lock) <= 2 {
            self.oauth_status_discovery_locks.remove(&key);
        }
        tracing::debug!(
            service = "upstream_oauth",
            action = "status.discovery",
            upstream,
            cache_hit = false,
            elapsed_ms = started.elapsed().as_millis(),
            failed = snapshot.tool_error.is_some() || snapshot.error.is_some(),
            "upstream oauth status discovery completed"
        );
        snapshot
    }

    fn oauth_status_epoch(&self, upstream: &str, subject: &str) -> (u64, u64) {
        let upstream_epoch = self
            .oauth_status_upstream_epochs
            .get(upstream)
            .map_or(0, |entry| *entry);
        let subject_epoch = self
            .oauth_status_subject_epochs
            .get(&(upstream.to_string(), subject.to_string()))
            .map_or(0, |entry| *entry);
        (upstream_epoch, subject_epoch)
    }

    fn is_routable_oauth_upstream(upstream: &UpstreamConfig) -> bool {
        upstream.enabled && upstream.priority > 0.0 && upstream.oauth.is_some()
    }

    fn google_provider_upstream_names(
        config: &labby_runtime::gateway_config::GatewayConfig,
    ) -> Vec<String> {
        config
            .upstream
            .iter()
            .filter(|upstream| {
                upstream
                    .oauth
                    .as_ref()
                    .is_some_and(|oauth| oauth.credential.is_google_provider())
            })
            .map(|upstream| upstream.name.clone())
            .collect()
    }

    pub async fn oauth_upstream_configs(&self) -> Vec<UpstreamConfig> {
        self.config
            .read()
            .await
            .upstream
            .iter()
            .filter(|upstream| Self::is_routable_oauth_upstream(upstream))
            .cloned()
            .collect()
    }

    pub async fn oauth_upstream_config(&self, upstream_name: &str) -> Option<UpstreamConfig> {
        self.config
            .read()
            .await
            .upstream
            .iter()
            .find(|upstream| {
                upstream.name == upstream_name && Self::is_routable_oauth_upstream(upstream)
            })
            .cloned()
    }

    pub async fn probe_upstream_oauth(
        &self,
        url: &str,
    ) -> Result<crate::gateway::oauth::ProbeResult, ToolError> {
        self.probe_upstream_oauth_for_upstream(url, None).await
    }

    pub async fn probe_upstream_oauth_for_upstream(
        &self,
        url: &str,
        upstream_name: Option<&str>,
    ) -> Result<crate::gateway::oauth::ProbeResult, ToolError> {
        probe::run(self, url, upstream_name).await
    }

    pub fn oauth_sqlite(&self) -> Option<labby_auth::sqlite::SqliteStore> {
        self.oauth_sqlite.clone()
    }

    pub fn oauth_redirect_uri(&self) -> Option<String> {
        self.oauth_redirect_uri.as_deref().map(|s| s.to_string())
    }

    async fn invalidate_subject_oauth_runtime(
        &self,
        upstream: &str,
        subject: &str,
        reason: &'static str,
    ) -> OAuthSessionInvalidation {
        let oauth_barrier = self
            .oauth_client_cache
            .as_ref()
            .map(|cache| cache.invalidation_barrier());
        let _guard = match oauth_barrier {
            Some(barrier) => Some(barrier.write_owned().await),
            None => None,
        };
        let invalidated = match self.current_pool_sync() {
            Some(pool) => {
                pool.invalidate_oauth_subject_sessions_guarded(upstream, subject, reason)
                    .await
            }
            None => {
                if let Some(cache) = &self.oauth_client_cache {
                    cache.evict_subject(upstream, subject);
                }
                OAuthSessionInvalidation::default()
            }
        };
        tracing::info!(
            service = "upstream_oauth",
            action = "session.invalidate",
            upstream,
            reason,
            generic_connections = invalidated.generic_connections,
            subject_connections = invalidated.subject_connections,
            relay_connections = invalidated.relay_connections,
            task_routes = invalidated.task_routes,
            invalidated_total = invalidated.total(),
            "upstream OAuth subject runtime invalidated"
        );
        invalidated
    }

    async fn invalidate_shared_oauth_runtime(
        &self,
        upstream: &str,
        reason: &'static str,
    ) -> OAuthSessionInvalidation {
        let config = self.config.read().await;
        let shared_upstreams = Self::google_provider_upstream_names(&config);
        drop(config);
        let oauth_barrier = self
            .oauth_client_cache
            .as_ref()
            .map(|cache| cache.invalidation_barrier());
        let _guard = match oauth_barrier {
            Some(barrier) => Some(barrier.write_owned().await),
            None => None,
        };
        let invalidated = match self.current_pool_sync() {
            Some(pool) => {
                pool.invalidate_oauth_upstream_sessions_guarded(&shared_upstreams, reason)
                    .await
            }
            None => {
                if let Some(cache) = &self.oauth_client_cache {
                    for name in &shared_upstreams {
                        cache.evict_upstream(name);
                    }
                }
                OAuthSessionInvalidation::default()
            }
        };
        tracing::info!(
            service = "upstream_oauth",
            action = "session.invalidate",
            upstream,
            reason,
            affected_upstreams = shared_upstreams.len(),
            generic_connections = invalidated.generic_connections,
            subject_connections = invalidated.subject_connections,
            relay_connections = invalidated.relay_connections,
            task_routes = invalidated.task_routes,
            invalidated_total = invalidated.total(),
            "shared upstream OAuth runtime invalidated"
        );
        invalidated
    }

    /// Fence every shared-Google upstream runtime for one revoked subject.
    ///
    /// The caller must durably revoke the provider credential first. This
    /// method then holds the shared lifecycle barrier while evicting the OAuth
    /// client cache and closing subject, generic, relay, and task-retained
    /// peers, so a successful administrative revocation cannot republish a
    /// client built from the old credential.
    pub async fn google_provider_lifecycle_write_guard(
        &self,
    ) -> Option<tokio::sync::OwnedRwLockWriteGuard<()>> {
        match &self.oauth_client_cache {
            Some(cache) => Some(cache.invalidation_barrier().write_owned().await),
            None => None,
        }
    }

    /// Drain one subject while the caller holds the lifecycle write barrier.
    pub async fn invalidate_google_provider_subject_runtime_guarded(
        &self,
        subject: &str,
        reason: &'static str,
    ) -> OAuthSessionInvalidation {
        let shared_upstreams = {
            let config = self.config.read().await;
            Self::google_provider_upstream_names(&config)
        };
        let mut total = OAuthSessionInvalidation::default();
        if let Some(pool) = self.current_pool_sync() {
            for upstream in &shared_upstreams {
                let invalidated = pool
                    .invalidate_oauth_subject_sessions_guarded(upstream, subject, reason)
                    .await;
                total.generic_connections += invalidated.generic_connections;
                total.subject_connections += invalidated.subject_connections;
                total.relay_connections += invalidated.relay_connections;
                total.task_routes += invalidated.task_routes;
            }
        } else if let Some(cache) = &self.oauth_client_cache {
            for upstream in &shared_upstreams {
                cache.evict_subject(upstream, subject);
            }
        }
        tracing::info!(
            service = "upstream_oauth",
            action = "allowlist_subject.invalidate",
            reason,
            affected_upstreams = shared_upstreams.len(),
            generic_connections = total.generic_connections,
            subject_connections = total.subject_connections,
            relay_connections = total.relay_connections,
            task_routes = total.task_routes,
            invalidated_total = total.total(),
            "allowlist subject OAuth runtime invalidated"
        );
        total
    }

    /// Look up the `UpstreamOauthManager` for `upstream` and return it, or
    /// emit a structured warning and return a `not_found` error.
    ///
    /// This is the single shared preamble used by `begin_upstream_authorization`,
    /// `complete_upstream_authorization_callback`, `upstream_oauth_status`, and
    /// `clear_upstream_credentials`. Extracted to avoid repeating the same 12-line
    /// pattern four times (Q-M5).
    fn require_oauth_manager(
        &self,
        upstream: &str,
        action: &'static str,
    ) -> Result<UpstreamOauthManager, ToolError> {
        self.upstream_oauth_manager(upstream).ok_or_else(|| {
            let kind = if action == "callback" {
                "oauth_probe_expired"
            } else {
                "not_found"
            };
            tracing::warn!(
                service = "upstream_oauth",
                action,
                upstream,
                kind,
                "upstream oauth {action}: upstream not found or has no oauth config"
            );
            ToolError::Sdk {
                sdk_kind: kind.to_string(),
                message: if action == "callback" {
                    format!("OAuth probe for upstream '{upstream}' expired; probe again and retry authorization")
                } else {
                    format!("upstream '{upstream}' not found or has no oauth config")
                },
            }
        })
    }

    pub async fn begin_upstream_authorization(
        &self,
        upstream: &str,
        subject: &str,
    ) -> Result<BeginAuthorization, ToolError> {
        let started = std::time::Instant::now();
        let manager = self.require_oauth_manager(upstream, "start")?;

        let result = manager.begin_authorization(subject).await.map_err(|e| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "start",
                upstream,
                kind = e.kind(),
                elapsed_ms = started.elapsed().as_millis(),
                "upstream oauth start: begin authorization failed"
            );
            tool_error_from_oauth(e)
        })?;

        tracing::info!(
            service = "upstream_oauth",
            action = "start",
            upstream,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth start: authorization URL generated"
        );
        Ok(result)
    }

    pub async fn complete_upstream_authorization_callback(
        &self,
        upstream: &str,
        subject: &str,
        code: &str,
        state: &str,
    ) -> Result<(), ToolError> {
        let started = std::time::Instant::now();
        let manager = self.require_oauth_manager(upstream, "callback")?;

        if let Err(e) = manager
            .complete_authorization_callback(subject, code, state)
            .await
        {
            self.discard_transient_oauth_manager(upstream).await;
            tracing::warn!(
                service = "upstream_oauth",
                action = "callback",
                upstream,
                kind = e.kind(),
                elapsed_ms = started.elapsed().as_millis(),
                "upstream oauth callback: token exchange failed"
            );
            return Err(tool_error_from_oauth(e));
        }
        tracing::info!(
            service = "upstream_oauth",
            action = "callback",
            upstream,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth callback: tokens stored"
        );

        if manager.credential_source_label() == "google_provider" {
            self.invalidate_shared_oauth_runtime(upstream, "oauth.google_provider.replace")
                .await;
            self.invalidate_shared_oauth_status_discovery().await;
        } else {
            self.invalidate_subject_oauth_runtime(upstream, subject, "oauth.credentials.replace")
                .await;
            self.invalidate_oauth_status_discovery(upstream, Some(subject))
                .await;
        }

        if let Some(oauth_config) = manager.upstream_config().oauth.clone() {
            if !self
                .promote_probe_oauth_config(upstream, oauth_config)
                .await?
            {
                tracing::debug!(
                    service = "upstream_oauth",
                    action = "callback",
                    upstream = %upstream,
                    "upstream oauth callback: no matching gateway in config; skipping oauth persistence"
                );
            } else {
                tracing::info!(
                    service = "upstream_oauth",
                    action = "callback",
                    upstream = %upstream,
                    "upstream oauth callback: persisting oauth config for probe-created manager"
                );
            }
        }

        Ok(())
    }

    pub async fn upstream_oauth_status(
        &self,
        upstream: &str,
        subject: &str,
    ) -> Result<UpstreamOauthStatusView, ToolError> {
        let started = std::time::Instant::now();
        let manager = self.require_oauth_manager(upstream, "status")?;
        let credential_source = manager.credential_source_label().to_string();
        let google_credential_broker =
            manager
                .google_credential_broker_status()
                .await
                .map_err(|error| {
                    tracing::warn!(
                        service = "upstream_oauth",
                        action = "status",
                        upstream,
                        kind = error.kind(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "upstream oauth status: Google credential broker lookup failed"
                    );
                    tool_error_from_oauth(error)
                })?;
        let scope_upgrade_required = google_credential_broker
            .as_ref()
            .is_some_and(|status| !status.missing_scopes.is_empty());

        let mut row = manager.credential_row(subject).await.map_err(|e| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "status",
                upstream,
                kind = e.kind(),
                elapsed_ms = started.elapsed().as_millis(),
                "upstream oauth status: credential lookup failed"
            );
            tool_error_from_oauth(e)
        })?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let mut refresh_attempted = false;
        let mut refreshed = false;
        let mut refresh_error_kind =
            scope_upgrade_required.then(|| "oauth_scope_upgrade_required".to_string());
        let mut refresh_error = scope_upgrade_required.then(|| {
            "the shared Google credential lacks one or more scopes required by this MCP server"
                .to_string()
        });

        if !scope_upgrade_required
            && row
                .as_ref()
                .is_some_and(|row| row.access_token_expires_at - now <= 300)
        {
            refresh_attempted = true;
            match manager.refresh_auth_client_if_due(subject).await {
                Ok(did_refresh) => {
                    refreshed = did_refresh;
                    if did_refresh {
                        self.invalidate_oauth_status_discovery(upstream, Some(subject))
                            .await;
                        self.invalidate_subject_oauth_runtime(
                            upstream,
                            subject,
                            "oauth.credentials.refresh",
                        )
                        .await;
                        // Fire-and-forget: rediscovery is a full reload that can
                        // outlive this request future's deadline; the detached
                        // task applies (and logs) on its own.
                        if let Err(error) = self
                            .reload_with_origin_detached(
                                Some("upstream-oauth.status.refresh"),
                                None,
                                std::time::Duration::ZERO,
                            )
                            .await
                        {
                            tracing::warn!(
                                service = "upstream_oauth",
                                action = "status",
                                upstream,
                                subject,
                                kind = error.kind(),
                                elapsed_ms = started.elapsed().as_millis(),
                                "upstream oauth status: refreshed token but gateway rediscovery failed"
                            );
                        }
                    }
                    row = manager.credential_row(subject).await.map_err(|e| {
                        tracing::warn!(
                            service = "upstream_oauth",
                            action = "status",
                            upstream,
                            kind = e.kind(),
                            elapsed_ms = started.elapsed().as_millis(),
                            "upstream oauth status: credential lookup after refresh failed"
                        );
                        tool_error_from_oauth(e)
                    })?;
                }
                Err(error) => {
                    refresh_error_kind = Some(error.kind().to_string());
                    refresh_error = Some(error.to_string());
                    tracing::warn!(
                        service = "upstream_oauth",
                        action = "status",
                        upstream,
                        subject,
                        kind = error.kind(),
                        elapsed_ms = started.elapsed().as_millis(),
                        "upstream oauth status: proactive refresh failed"
                    );
                }
            }
        }

        let (access_token_expires_at, seconds_until_expiry, refresh_token_present) = row
            .as_ref()
            .map(|row| {
                (
                    Some(row.access_token_expires_at),
                    Some(row.access_token_expires_at - now),
                    row.refresh_token_present,
                )
            })
            .unwrap_or((None, None, false));
        let expires_within_5m = seconds_until_expiry.is_some_and(|seconds| seconds <= 300);
        let mut state = if scope_upgrade_required {
            UpstreamOauthConnectionState::ScopeUpgradeRequired
        } else {
            match (
                row.is_some(),
                refresh_error_kind.is_some(),
                seconds_until_expiry,
            ) {
                (false, _, _) => UpstreamOauthConnectionState::Disconnected,
                (true, true, _) => UpstreamOauthConnectionState::RefreshFailed,
                (true, false, Some(seconds)) if seconds <= 0 => {
                    UpstreamOauthConnectionState::Expired
                }
                (true, false, Some(seconds)) if seconds <= 300 => {
                    UpstreamOauthConnectionState::Expiring
                }
                (true, false, _) => UpstreamOauthConnectionState::Connected,
            }
        };
        let authenticated = matches!(
            state,
            UpstreamOauthConnectionState::Connected | UpstreamOauthConnectionState::Expiring
        );
        let mut discovery_checked = false;
        let mut discovered_tool_count = 0;
        let mut exposed_tool_count = 0;
        let mut discovery_error = None;

        if authenticated {
            discovery_checked = true;
            let upstream_config = manager.upstream_config().clone();
            let discovery = self
                .oauth_status_discovery(upstream, subject, upstream_config)
                .await;
            if let Some(summary) = discovery.summary {
                discovered_tool_count = summary.discovered_tool_count;
                exposed_tool_count = summary.exposed_tool_count;
            }
            // Only a TOOL discovery failure marks the upstream as failed —
            // tool routing is gated solely on tool health (see
            // `UpstreamPool::healthy_tools`). A resources/prompts capability
            // probe that errors (e.g. an upstream whose endpoint returns HTTP
            // 400 for `resources/list` instead of a clean "unsupported" reply)
            // must NOT hide tools that discovered fine; surface it as a
            // non-fatal diagnostic and keep the connection authenticated.
            if let Some(error) = discovery.tool_error {
                discovery_error = Some(error);
                state = UpstreamOauthConnectionState::DiscoveryFailed;
            } else if let Some(error) = discovery.error {
                discovery_error = Some(error);
            }
        }
        let authenticated = matches!(
            state,
            UpstreamOauthConnectionState::Connected | UpstreamOauthConnectionState::Expiring
        );

        tracing::debug!(
            service = "upstream_oauth",
            action = "status",
            upstream,
            authenticated,
            expires_within_5m,
            refresh_attempted,
            refreshed,
            discovery_checked,
            discovered_tool_count,
            exposed_tool_count,
            state = ?state,
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth status: checked"
        );
        Ok(UpstreamOauthStatusView {
            authenticated,
            upstream: upstream.to_string(),
            credential_source,
            google_credential_broker,
            expires_within_5m,
            state,
            access_token_expires_at,
            seconds_until_expiry,
            refresh_token_present,
            refresh_attempted,
            refreshed,
            refresh_error_kind,
            refresh_error,
            discovery_checked,
            discovered_tool_count,
            exposed_tool_count,
            discovery_error,
        })
    }

    pub async fn revoke_google_provider_credential(
        &self,
        upstream: &str,
    ) -> Result<labby_auth::types::GoogleProviderInvalidation, ToolError> {
        let started = std::time::Instant::now();
        let manager = self.require_oauth_manager(upstream, "google_revoke")?;
        let invalidation = manager
            .revoke_shared_google_credential()
            .await
            .map_err(|error| {
                tracing::warn!(
                    service = "upstream_oauth",
                    action = "google_revoke",
                    upstream,
                    kind = error.kind(),
                    elapsed_ms = started.elapsed().as_millis(),
                    "Google provider credential revoke failed"
                );
                tool_error_from_oauth(error)
            })?;
        let sessions = self
            .invalidate_shared_oauth_runtime(upstream, "oauth.google_provider.revoke")
            .await;
        self.invalidate_shared_oauth_status_discovery().await;
        tracing::info!(
            service = "upstream_oauth",
            action = "google_revoke",
            upstream,
            invalidated = invalidation.invalidated,
            revoked_refresh_tokens = invalidation.revoked_refresh_tokens,
            revoked_authorization_codes = invalidation.revoked_authorization_codes,
            subject_connections = sessions.subject_connections,
            generic_connections = sessions.generic_connections,
            relay_connections = sessions.relay_connections,
            task_routes = sessions.task_routes,
            invalidated_sessions = sessions.total(),
            elapsed_ms = started.elapsed().as_millis(),
            "Google provider credential revoked and OAuth client cache evicted"
        );
        Ok(invalidation)
    }

    pub async fn clear_upstream_credentials(
        &self,
        upstream: &str,
        subject: &str,
    ) -> Result<(), ToolError> {
        let started = std::time::Instant::now();
        let manager = self.require_oauth_manager(upstream, "clear")?;

        manager.clear_credentials(subject).await.map_err(|e| {
            tracing::warn!(
                service = "upstream_oauth",
                action = "clear",
                upstream,
                kind = e.kind(),
                elapsed_ms = started.elapsed().as_millis(),
                "upstream oauth clear: failed to clear credentials"
            );
            tool_error_from_oauth(e)
        })?;
        let sessions = self
            .invalidate_subject_oauth_runtime(upstream, subject, "oauth.credentials.clear")
            .await;
        self.invalidate_oauth_status_discovery(upstream, Some(subject))
            .await;
        tracing::info!(
            service = "upstream_oauth",
            action = "clear",
            upstream,
            subject_connections = sessions.subject_connections,
            generic_connections = sessions.generic_connections,
            relay_connections = sessions.relay_connections,
            task_routes = sessions.task_routes,
            invalidated_sessions = sessions.total(),
            elapsed_ms = started.elapsed().as_millis(),
            "upstream oauth clear: credentials cleared and client cache evicted"
        );
        Ok(())
    }
}

/// Unwrapped OAuth runtime resources, borrowed from `GatewayManager`.
///
/// Extracted by `require_oauth_runtime` and passed to probe helpers so they
/// don't have to repeat the 4-tuple `Option` destructuring themselves.
pub(super) struct OauthRuntime<'a> {
    pub managers: &'a dashmap::DashMap<String, UpstreamOauthManager>,
    pub sqlite: &'a labby_auth::sqlite::SqliteStore,
    pub key: &'a EncryptionKey,
    pub redirect_uri: &'a Arc<String>,
}

impl GatewayManager {
    /// Return the OAuth runtime resources, or a structured `not_configured` error.
    ///
    /// Centralises the 4-tuple match used inside `probe_upstream_oauth_for_upstream`
    /// so callers don't need to handle the wildcard arm inline.
    pub(super) fn require_oauth_runtime(&self) -> Result<OauthRuntime<'_>, ToolError> {
        match (
            self.upstream_oauth_managers.as_deref(),
            self.oauth_sqlite.as_ref(),
            self.oauth_key.as_ref(),
            self.oauth_redirect_uri.as_ref(),
        ) {
            (Some(managers), Some(sqlite), Some(key), Some(redirect_uri)) => Ok(OauthRuntime {
                managers,
                sqlite,
                key,
                redirect_uri,
            }),
            _ => {
                tracing::warn!(
                    service = "upstream_oauth",
                    action = "probe",
                    kind = "not_configured",
                    "upstream oauth probe: oauth resources not configured (LABBY_PUBLIC_URL + LABBY_OAUTH_ENCRYPTION_KEY required)"
                );
                Err(ToolError::Sdk {
                    sdk_kind: "not_configured".to_string(),
                    message: "upstream OAuth requires LABBY_PUBLIC_URL (https) and LABBY_OAUTH_ENCRYPTION_KEY to be set".to_string(),
                })
            }
        }
    }
}
