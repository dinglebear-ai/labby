use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::Mutex;
#[cfg(feature = "http-axum")]
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};
use url::Url;

use crate::config::{AuthConfig, AuthMode};
use crate::error::AuthError;
use crate::google::GoogleProvider;
use crate::jwt::SigningKeys;
use crate::resource_registry::ResourceRegistry;
use crate::sqlite::SqliteStore;
#[cfg(feature = "http-axum")]
use crate::types::RegisteredClient;

const RATE_LIMIT_RETRY_AFTER_MS: u64 = 60_000;
const RATE_LIMIT_MAX_IP_BUCKETS: usize = 4_096;
const RATE_LIMIT_IDLE_TTL_SECS: u64 = 10 * 60;
/// Bound concurrent untrusted metadata fetches while per-document locks
/// coalesce duplicate requests without serializing independent documents.
#[cfg(feature = "http-axum")]
const REMOTE_FETCH_MAX_CONCURRENT: usize = 16;
const EXPIRED_RECORD_CLEANUP_INTERVAL: Duration = Duration::from_mins(5);
const EXPIRED_RECORD_CLEANUP_BATCH: u32 = 256;

struct CleanupTask {
    abort: tokio::task::AbortHandle,
}

impl Drop for CleanupTask {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

fn spawn_expired_record_cleanup(
    store: SqliteStore,
    interval: Duration,
    batch_limit: u32,
) -> CleanupTask {
    let task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let started = Instant::now();
            match store.cleanup_expired_bounded(batch_limit).await {
                Ok(0) => debug!(
                    crate_name = "labby-auth",
                    phase = "auth.cleanup_expired.finish",
                    deleted_rows = 0_u64,
                    batch_limit,
                    elapsed_ms = started.elapsed().as_millis(),
                    "bounded expired auth record cleanup completed"
                ),
                Ok(deleted_rows) => info!(
                    crate_name = "labby-auth",
                    phase = "auth.cleanup_expired.finish",
                    deleted_rows,
                    batch_limit,
                    elapsed_ms = started.elapsed().as_millis(),
                    "bounded expired auth record cleanup completed"
                ),
                Err(error) => warn!(
                    crate_name = "labby-auth",
                    phase = "auth.cleanup_expired.error",
                    kind = error.kind(),
                    batch_limit,
                    elapsed_ms = started.elapsed().as_millis(),
                    "bounded expired auth record cleanup failed"
                ),
            }
        }
    });
    CleanupTask {
        abort: task.abort_handle(),
    }
}

/// Per-request parameters for rate-limiting. Each bucket is independent.
struct RateLimiterInner {
    /// Tokens available in the bucket.
    tokens: f64,
    /// Maximum tokens, equal to the full per-minute burst allowance.
    max_tokens: f64,
    /// Refill rate in tokens per second.
    refill_rate: f64,
    /// Last refill time.
    last_refill: Instant,
}

impl RateLimiterInner {
    fn new(requests_per_minute: u32) -> Self {
        let rate = requests_per_minute as f64 / 60.0;
        let max_tokens = requests_per_minute.max(1) as f64;
        Self {
            tokens: max_tokens,
            max_tokens,
            refill_rate: rate,
            last_refill: Instant::now(),
        }
    }

    fn try_acquire(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.max_tokens);
        self.last_refill = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Per-IP token-bucket rate limiter.
///
/// Uses a `DashMap` of `tokio::sync::Mutex<RateLimiterInner>` so:
/// - different IPs can be checked concurrently without serializing on a global lock
///   (lab-77y5.10 — one IP cannot exhaust the global bucket),
/// - the per-bucket lock is a `tokio::sync::Mutex` so contention does not park a
///   Tokio worker thread (lab-77y5.9).
///
/// Cheap to clone (all state is behind `Arc`).
#[derive(Clone)]
struct PerIpRateLimiter {
    requests_per_minute: u32,
    max_buckets: usize,
    idle_ttl_secs: u64,
    started_at: Instant,
    buckets: Arc<DashMap<IpAddr, Arc<RateLimitBucket>>>,
    maintenance_lock: Arc<std::sync::Mutex<()>>,
}

struct RateLimitBucket {
    limiter: Mutex<RateLimiterInner>,
    last_seen_secs: AtomicU64,
}

impl PerIpRateLimiter {
    fn new(requests_per_minute: u32) -> Self {
        Self::new_with_limits(
            requests_per_minute,
            RATE_LIMIT_MAX_IP_BUCKETS,
            RATE_LIMIT_IDLE_TTL_SECS,
        )
    }

    fn new_with_limits(requests_per_minute: u32, max_buckets: usize, idle_ttl_secs: u64) -> Self {
        Self {
            requests_per_minute,
            max_buckets: max_buckets.max(1),
            idle_ttl_secs: idle_ttl_secs.max(1),
            started_at: Instant::now(),
            buckets: Arc::new(DashMap::new()),
            maintenance_lock: Arc::new(std::sync::Mutex::new(())),
        }
    }

    /// Try to consume one token for `ip`. Returns `true` if allowed.
    async fn try_acquire(&self, ip: IpAddr) -> bool {
        self.try_acquire_at(ip, self.started_at.elapsed().as_secs())
            .await
    }

    async fn try_acquire_at(&self, ip: IpAddr, now_secs: u64) -> bool {
        // Fast path: bucket already exists.
        if let Some(bucket_ref) = self.buckets.get(&ip) {
            let bucket = Arc::clone(bucket_ref.value());
            drop(bucket_ref);
            bucket.last_seen_secs.store(now_secs, Ordering::Relaxed);
            return bucket.limiter.lock().await.try_acquire();
        }

        // Serialize the slow path so concurrent new addresses cannot exceed
        // the configured cap. Values are Arc-backed so no DashMap shard guard
        // is held while awaiting the per-bucket Tokio mutex.
        let bucket = {
            let _maintenance = self
                .maintenance_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(bucket) = self.buckets.get(&ip) {
                Arc::clone(bucket.value())
            } else {
                self.evict_stale_and_lru(now_secs);
                let bucket = Arc::new(RateLimitBucket {
                    limiter: Mutex::new(RateLimiterInner::new(self.requests_per_minute)),
                    last_seen_secs: AtomicU64::new(now_secs),
                });
                self.buckets.insert(ip, Arc::clone(&bucket));
                bucket
            }
        };
        bucket.last_seen_secs.store(now_secs, Ordering::Relaxed);
        bucket.limiter.lock().await.try_acquire()
    }

    fn evict_stale_and_lru(&self, now_secs: u64) {
        let stale_before = now_secs.saturating_sub(self.idle_ttl_secs);
        const EVICTION_SCAN_LIMIT: usize = 64;
        let stale = self
            .buckets
            .iter()
            .take(EVICTION_SCAN_LIMIT)
            .filter(|entry| entry.last_seen_secs.load(Ordering::Relaxed) < stale_before)
            .map(|entry| *entry.key())
            .collect::<Vec<_>>();
        for key in stale {
            self.buckets.remove(&key);
        }
        while self.buckets.len() >= self.max_buckets {
            let oldest = self
                .buckets
                .iter()
                .take(EVICTION_SCAN_LIMIT)
                .min_by_key(|entry| entry.last_seen_secs.load(Ordering::Relaxed))
                .map(|entry| *entry.key());
            let Some(oldest) = oldest else { break };
            self.buckets.remove(&oldest);
        }
    }

    #[cfg(test)]
    fn bucket_count(&self) -> usize {
        self.buckets.len()
    }

    #[cfg(test)]
    fn has_bucket(&self, ip: IpAddr) -> bool {
        self.buckets.contains_key(&ip)
    }
}

#[derive(Clone)]
pub struct AuthState {
    pub config: Arc<AuthConfig>,
    pub store: SqliteStore,
    pub signing_keys: Arc<SigningKeys>,
    pub google: Arc<GoogleProvider>,
    _cleanup_task: Arc<CleanupTask>,
    resource_registry: ResourceRegistry,
    authorize_limiter: PerIpRateLimiter,
    register_limiter: PerIpRateLimiter,
    token_limiter: PerIpRateLimiter,
    #[cfg(feature = "http-axum")]
    pub(crate) cimd_cache: Arc<DashMap<String, (RegisteredClient, i64)>>,
    /// Short-lived failures for untrusted CIMD URLs. This prevents a caller
    /// from repeatedly turning the same invalid document into outbound I/O.
    #[cfg(feature = "http-axum")]
    pub(crate) cimd_negative_cache: Arc<DashMap<String, i64>>,
    #[cfg(feature = "http-axum")]
    pub(crate) cimd_cache_maintenance: Arc<std::sync::Mutex<()>>,
    #[cfg(feature = "http-axum")]
    pub(crate) jwks_cache: Arc<DashMap<String, (jsonwebtoken::jwk::JwkSet, i64)>>,
    /// Short-lived failures and unknown-key results, keyed by JWKS document
    /// rather than attacker-controlled `kid` values.
    #[cfg(feature = "http-axum")]
    pub(crate) jwks_negative_cache: Arc<DashMap<String, i64>>,
    #[cfg(feature = "http-axum")]
    pub(crate) jwks_cache_maintenance: Arc<std::sync::Mutex<()>>,
    #[cfg(feature = "http-axum")]
    pub(crate) google_refresh_failures: Arc<DashMap<String, (bool, String, Instant)>>,
    #[cfg(feature = "http-axum")]
    pub(crate) google_refresh_flights:
        Arc<DashMap<String, Arc<crate::google_refresh::GoogleRefreshFlight>>>,
    /// Per-document single-flight locks. Never hold one global lock across
    /// remote I/O: an unrelated slow metadata endpoint must not block OAuth.
    #[cfg(feature = "http-axum")]
    pub(crate) remote_fetch_locks: Arc<DashMap<String, Arc<Mutex<()>>>>,
    /// Serializes bounded maintenance of the attacker-keyed single-flight map.
    #[cfg(feature = "http-axum")]
    pub(crate) remote_fetch_lock_maintenance: Arc<std::sync::Mutex<()>>,
    /// Global concurrency cap for untrusted remote metadata fetches. This is a
    /// semaphore, not a mutex: unrelated documents may still fetch in parallel.
    #[cfg(feature = "http-axum")]
    pub(crate) remote_fetch_permits: Arc<Semaphore>,
}

impl AuthState {
    pub async fn new(config: AuthConfig) -> Result<Self, AuthError> {
        Self::new_with_resource_registry(config, ResourceRegistry::new()).await
    }

    pub async fn new_with_resource_registry(
        config: AuthConfig,
        resource_registry: ResourceRegistry,
    ) -> Result<Self, AuthError> {
        if !matches!(config.mode, AuthMode::OAuth) {
            return Err(AuthError::Config(format!(
                "AuthState requires {prefix}_AUTH_MODE=oauth",
                prefix = config.env_prefix
            )));
        }
        if config.token_encryption_key.is_none() {
            return Err(AuthError::Config(format!(
                "{prefix}_TOKEN_ENCRYPTION_KEY is required when {prefix}_AUTH_MODE=oauth; \
                 Google provider credentials must be encrypted at rest",
                prefix = config.env_prefix
            )));
        }

        let public_url = config.public_url.clone().ok_or_else(|| {
            AuthError::Config(format!(
                "{prefix}_PUBLIC_URL is required when {prefix}_AUTH_MODE=oauth",
                prefix = config.env_prefix
            ))
        })?;
        let redirect_uri = config.google.callback_url.clone().unwrap_or_else(|| {
            build_google_redirect_uri(&public_url, &config.google.callback_path)
        });
        let store = SqliteStore::open_with_key(
            config.sqlite_path.clone(),
            config.token_encryption_key.clone(),
        )
        .await?;
        let signing_keys = SigningKeys::load_or_create(&config.key_path)?;
        let mut google = GoogleProvider::new(
            config.google.client_id.clone(),
            config.google.client_secret.clone(),
            redirect_uri,
        )?;
        google.scopes.clone_from(&config.google.scopes);
        let sqlite_path_id = crate::util::fingerprint(&config.sqlite_path.to_string_lossy());
        let key_path_id = crate::util::fingerprint(&config.key_path.to_string_lossy());
        info!(
            crate_name = "labby-auth",
            env_prefix = %config.env_prefix,
            auth_mode = "oauth",
            public_url_id = %crate::util::fingerprint(public_url.as_str()),
            google_redirect_uri_id = %crate::util::fingerprint(google.redirect_uri.as_str()),
            sqlite_path_id = %sqlite_path_id,
            key_path_id = %key_path_id,
            google_scope_count = config.google.scopes.len(),
            "auth state initialized"
        );

        let authorize_limiter = PerIpRateLimiter::new(config.authorize_requests_per_minute);
        let register_limiter = PerIpRateLimiter::new(config.register_requests_per_minute);
        let token_limiter = PerIpRateLimiter::new(config.token_requests_per_minute);
        let cleanup_task = Arc::new(spawn_expired_record_cleanup(
            store.clone(),
            EXPIRED_RECORD_CLEANUP_INTERVAL,
            EXPIRED_RECORD_CLEANUP_BATCH,
        ));
        Ok(Self {
            config: Arc::new(config),
            store,
            signing_keys: Arc::new(signing_keys),
            google: Arc::new(google),
            _cleanup_task: cleanup_task,
            resource_registry,
            authorize_limiter,
            register_limiter,
            token_limiter,
            #[cfg(feature = "http-axum")]
            cimd_cache: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            google_refresh_failures: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            google_refresh_flights: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            cimd_negative_cache: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            cimd_cache_maintenance: Arc::new(std::sync::Mutex::new(())),
            #[cfg(feature = "http-axum")]
            jwks_cache: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            jwks_negative_cache: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            jwks_cache_maintenance: Arc::new(std::sync::Mutex::new(())),
            #[cfg(feature = "http-axum")]
            remote_fetch_locks: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            remote_fetch_lock_maintenance: Arc::new(std::sync::Mutex::new(())),
            #[cfg(feature = "http-axum")]
            remote_fetch_permits: Arc::new(Semaphore::new(REMOTE_FETCH_MAX_CONCURRENT)),
        })
    }

    /// Replace the extra OAuth resource audiences accepted by `/authorize` and `/token`.
    ///
    /// The canonical `{LAB_PUBLIC_URL}/mcp` resource is always accepted; callers use this
    /// to publish Gateway-managed protected MCP resources such as
    /// `https://mcp.example.com/syslog` or `https://syslog.example.com/mcp`.
    pub fn set_allowed_resource_urls(&self, resources: impl IntoIterator<Item = String>) {
        self.set_allowed_resource_scopes(
            resources
                .into_iter()
                .map(|resource| (resource, self.config.scopes_supported.to_vec())),
        );
    }

    /// Replace the extra OAuth resource audiences and the scopes each resource accepts.
    pub fn set_allowed_resource_scopes(
        &self,
        resources: impl IntoIterator<Item = (String, Vec<String>)>,
    ) {
        if let Err(error) = self.replace_configured_resource_scopes(resources) {
            debug!(%error, "ignored invalid configured OAuth protected resource scopes");
        }
    }

    pub fn replace_configured_resource_scopes(
        &self,
        resources: impl IntoIterator<Item = (String, Vec<String>)>,
    ) -> Result<(), crate::resource_registry::ResourceRegistryError> {
        self.resource_registry
            .replace_configured_resource_scopes(resources)
    }

    #[must_use]
    pub fn resource_registry(&self) -> ResourceRegistry {
        self.resource_registry.clone()
    }

    pub fn is_allowed_resource_url(&self, resource: &str) -> bool {
        self.resource_registry
            .effective_resource_scopes(resource)
            .is_some()
    }

    pub fn allowed_resource_scopes(&self, resource: &str) -> Option<Vec<String>> {
        self.resource_registry.effective_resource_scopes(resource)
    }

    /// Rate-limit guard for `/authorize` and `/browser_login` endpoints.
    ///
    /// Keyed per remote IP so one client cannot exhaust the global bucket
    /// (lab-77y5.10). Uses `tokio::sync::Mutex` internally so contention does
    /// not park a Tokio worker thread (lab-77y5.9).
    pub async fn check_authorize_rate_limit(&self, ip: IpAddr) -> Result<(), AuthError> {
        if self.authorize_limiter.try_acquire(ip).await {
            Ok(())
        } else {
            Err(AuthError::RateLimited {
                message: "authorize rate limit exceeded".to_string(),
                retry_after_ms: RATE_LIMIT_RETRY_AFTER_MS,
            })
        }
    }

    /// Rate-limit guard for `/register` endpoint.
    ///
    /// Keyed per remote IP — see `check_authorize_rate_limit` for the rationale.
    pub async fn check_register_rate_limit(&self, ip: IpAddr) -> Result<(), AuthError> {
        if self.register_limiter.try_acquire(ip).await {
            Ok(())
        } else {
            Err(AuthError::RateLimited {
                message: "register rate limit exceeded".to_string(),
                retry_after_ms: RATE_LIMIT_RETRY_AFTER_MS,
            })
        }
    }

    /// Rate-limit guard for the credential-bearing `/token` endpoint.
    pub async fn check_token_rate_limit(&self, ip: IpAddr) -> Result<(), AuthError> {
        if self.token_limiter.try_acquire(ip).await {
            Ok(())
        } else {
            Err(AuthError::RateLimited {
                message: "token rate limit exceeded".to_string(),
                retry_after_ms: RATE_LIMIT_RETRY_AFTER_MS,
            })
        }
    }

    /// Returns the merged email allowlist: admin first, then all `allowed_users` rows,
    /// deduplicating case-insensitively so admin is never counted twice.
    ///
    /// This is the single source of truth used in both OAuth callback branches. A DB
    /// error is surfaced as [`AuthError::Storage`] (fail-closed — server fault, not
    /// user fault).
    ///
    /// Never log the returned emails directly — pass them only to
    /// `check_email_allowlist`, which uses `fingerprint()` for safe diagnostics.
    pub async fn resolve_allowed_emails(&self) -> Result<Vec<String>, AuthError> {
        let mut emails = vec![self.config.admin_email.clone()];
        for row in self.store.list_allowed_users().await? {
            if !row.email.eq_ignore_ascii_case(&self.config.admin_email) {
                emails.push(row.email);
            }
        }
        Ok(emails)
    }

    /// Rejects new OAuth state rows when the pending count exceeds `max_pending_oauth_states`.
    pub async fn ensure_pending_oauth_state_capacity(&self) -> Result<(), AuthError> {
        let count = self.store.count_pending_oauth_states().await?;
        if count >= self.config.max_pending_oauth_states {
            return Err(AuthError::RateLimited {
                message: "too many pending authorization requests".to_string(),
                retry_after_ms: 5_000,
            });
        }
        Ok(())
    }

    /// Consume a signed assertion identifier once in the durable auth store.
    pub async fn consume_assertion_jti(
        &self,
        issuer: &str,
        jti: &str,
        issued_at: i64,
        expires_at: i64,
    ) -> Result<bool, AuthError> {
        self.store
            .consume_assertion_jti(issuer, jti, issued_at, expires_at, crate::util::now_unix())
            .await
    }

    #[cfg(test)]
    pub fn for_tests(
        config: AuthConfig,
        store: SqliteStore,
        signing_keys: SigningKeys,
        google: GoogleProvider,
    ) -> Self {
        let authorize_limiter = PerIpRateLimiter::new(config.authorize_requests_per_minute);
        let register_limiter = PerIpRateLimiter::new(config.register_requests_per_minute);
        let token_limiter = PerIpRateLimiter::new(config.token_requests_per_minute);
        let cleanup_task = Arc::new(spawn_expired_record_cleanup(
            store.clone(),
            EXPIRED_RECORD_CLEANUP_INTERVAL,
            EXPIRED_RECORD_CLEANUP_BATCH,
        ));
        Self {
            config: Arc::new(config),
            store,
            signing_keys: Arc::new(signing_keys),
            google: Arc::new(google),
            _cleanup_task: cleanup_task,
            resource_registry: ResourceRegistry::new(),
            authorize_limiter,
            register_limiter,
            token_limiter,
            #[cfg(feature = "http-axum")]
            cimd_cache: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            google_refresh_failures: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            google_refresh_flights: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            cimd_negative_cache: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            cimd_cache_maintenance: Arc::new(std::sync::Mutex::new(())),
            #[cfg(feature = "http-axum")]
            jwks_cache: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            jwks_negative_cache: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            jwks_cache_maintenance: Arc::new(std::sync::Mutex::new(())),
            #[cfg(feature = "http-axum")]
            remote_fetch_locks: Arc::new(DashMap::new()),
            #[cfg(feature = "http-axum")]
            remote_fetch_lock_maintenance: Arc::new(std::sync::Mutex::new(())),
            #[cfg(feature = "http-axum")]
            remote_fetch_permits: Arc::new(Semaphore::new(REMOTE_FETCH_MAX_CONCURRENT)),
        }
    }
}

fn build_google_redirect_uri(public_url: &Url, callback_path: &str) -> Url {
    let mut redirect_uri = public_url.clone();
    let base_path = redirect_uri.path().trim_end_matches('/');
    let callback_path = callback_path.trim_start_matches('/');
    let next_path = if base_path.is_empty() {
        format!("/{callback_path}")
    } else {
        format!("{base_path}/{callback_path}")
    };

    redirect_uri.set_path(&next_path);
    redirect_uri.set_query(None);
    redirect_uri.set_fragment(None);
    redirect_uri
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::tempdir;

    use super::*;
    use crate::config::GoogleConfig;
    use crate::util::now_unix;

    #[tokio::test]
    async fn cloned_auth_states_share_resource_registry() {
        let state = resolve_state("admin@example.com").await;
        let clone = state.clone();
        state
            .resource_registry()
            .create_resource_lease(
                "https://proxy.example:53147/mcp",
                ["mcp:read"],
                Duration::from_mins(1),
                "clone-test",
            )
            .unwrap();

        assert_eq!(clone.resource_registry().lease_count(), 1);
    }

    #[cfg(feature = "http-axum")]
    #[tokio::test(flavor = "current_thread")]
    async fn auth_state_initialization_logs_redact_configuration_metadata() {
        let _tracing_lock = crate::test_support::TRACING_TEST_LOCK.lock().await;
        let buf = crate::test_support::global_tracing_buffer();
        let directory = tempdir().unwrap();
        let public_url_sentinel = "sentinel-public.example";
        let redirect_sentinel = "sentinel-google-redirect.example";
        let scope_sentinel = "sentinel-state-scope-secret";
        let sqlite_sentinel = "sentinel-auth-storage.db";
        let key_sentinel = "sentinel-auth-signing.pem";
        let mut config = crate::authorize::tests::test_auth_config();
        config.public_url =
            Some(Url::parse(&format!("https://{public_url_sentinel}/gateway")).unwrap());
        config.google.callback_url =
            Some(Url::parse(&format!("https://{redirect_sentinel}/callback")).unwrap());
        config.google.scopes = vec![scope_sentinel.to_string()];
        config.sqlite_path = directory.path().join(sqlite_sentinel);
        config.key_path = directory.path().join(key_sentinel);

        let state = AuthState::new(config).await.expect("auth state");
        drop(state);

        let logs = crate::test_support::captured_logs(buf);
        for sentinel in [
            public_url_sentinel,
            redirect_sentinel,
            scope_sentinel,
            sqlite_sentinel,
            key_sentinel,
        ] {
            assert!(
                !logs.contains(sentinel),
                "auth configuration metadata leaked into logs: {sentinel}\n{logs}"
            );
        }
        assert!(logs.contains("google_scope_count"), "{logs}");
        assert!(logs.contains("sqlite_path_id"), "{logs}");
        assert!(logs.contains("key_path_id"), "{logs}");
    }

    #[tokio::test]
    async fn per_ip_rate_limiter_evicts_idle_and_lru_buckets_under_address_churn() {
        let limiter = PerIpRateLimiter::new_with_limits(60, 3, 10);
        for octet in 1..=3 {
            assert!(
                limiter
                    .try_acquire_at(IpAddr::from([192, 0, 2, octet]), u64::from(octet))
                    .await
            );
        }
        // Refresh .1 so .2 is the least-recently-used live entry.
        assert!(
            limiter
                .try_acquire_at(IpAddr::from([192, 0, 2, 1]), 4)
                .await
        );
        assert!(
            limiter
                .try_acquire_at(IpAddr::from([192, 0, 2, 4]), 5)
                .await
        );
        assert_eq!(limiter.bucket_count(), 3);
        assert!(limiter.has_bucket(IpAddr::from([192, 0, 2, 1])));
        assert!(!limiter.has_bucket(IpAddr::from([192, 0, 2, 2])));

        // At t=20 all prior entries exceed the 10-second idle TTL. A new
        // address must prune them rather than letting the map grow forever.
        assert!(
            limiter
                .try_acquire_at(IpAddr::from([198, 51, 100, 1]), 20)
                .await
        );
        assert_eq!(limiter.bucket_count(), 1);
    }

    #[tokio::test]
    #[allow(clippy::panic)]
    async fn per_ip_rate_limiter_recovers_poisoned_maintenance_lock() {
        let limiter = PerIpRateLimiter::new_with_limits(60, 3, 10);
        let lock = Arc::clone(&limiter.maintenance_lock);
        drop(std::panic::catch_unwind(move || {
            let _guard = lock.lock().expect("initial maintenance lock");
            panic!("poison maintenance lock");
        }));

        assert!(
            limiter
                .try_acquire_at(IpAddr::from([192, 0, 2, 9]), 1)
                .await
        );
    }

    #[tokio::test(start_paused = true)]
    async fn periodic_cleanup_removes_expired_records_after_interval() {
        let directory = tempdir().unwrap();
        let store = SqliteStore::open_with_key(
            directory.path().join("auth.db"),
            Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                "periodic-cleanup-test-key",
            )),
        )
        .await
        .unwrap();
        let now = now_unix();
        store
            .save_upstream_oauth_state(crate::types::UpstreamOauthStateRow {
                upstream_name: "expired-upstream".to_string(),
                subject: "expired-subject".to_string(),
                csrf_token: "expired-csrf".to_string(),
                pkce_verifier: "expired-verifier".to_string(),
                expected_issuer: None,
                require_issuer: false,
                requested_scopes: Vec::new(),
                created_at: now - 20,
                expires_at: now - 1,
            })
            .await
            .unwrap();

        let task = spawn_expired_record_cleanup(store.clone(), Duration::from_secs(30), 1);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::task::yield_now().await;

        assert!(
            store
                .find_upstream_oauth_state_owner("expired-csrf", now)
                .await
                .unwrap()
                .is_none()
        );
        drop(task);
    }

    /// Builds a minimal `AuthState` for unit-testing `resolve_allowed_emails`.
    async fn resolve_state(admin_email: &str) -> AuthState {
        let dir = tempdir().expect("tempdir");
        AuthState::new(AuthConfig {
            mode: AuthMode::OAuth,
            public_url: Some(Url::parse("https://lab.example.com").expect("url")),
            sqlite_path: dir.path().join("auth.db"),
            key_path: dir.path().join("auth.pem"),
            bootstrap_secret: None,
            allowed_client_redirect_uris: Vec::new(),
            admin_email: admin_email.to_string(),
            google: GoogleConfig {
                client_id: "client-id".to_string(),
                client_secret: "client-secret".to_string(),
                callback_url: None,
                callback_path: "/auth/google/callback".to_string(),
                scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
            },
            token_encryption_key: Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                "state-test-google-provider-key",
            )),
            access_token_ttl: Duration::from_hours(1),
            refresh_token_ttl: Duration::from_hours(1),
            auth_code_ttl: Duration::from_mins(5),
            register_requests_per_minute: 10,
            authorize_requests_per_minute: 20,
            max_pending_oauth_states: 1024,
            ..AuthConfig::default()
        })
        .await
        .expect("auth state")
    }

    #[cfg(feature = "http-axum")]
    #[tokio::test]
    async fn auth_state_refuses_oauth_without_provider_credential_encryption() {
        let mut config = crate::authorize::tests::test_auth_config();
        config.token_encryption_key = None;
        let error = AuthState::new(config)
            .await
            .err()
            .expect("OAuth without provider encryption must fail closed");
        assert!(error.to_string().contains("TOKEN_ENCRYPTION_KEY"));
        assert!(error.to_string().contains("encrypted at rest"));
    }

    #[tokio::test]
    async fn resolve_allowed_emails_returns_admin_when_table_is_empty() {
        let state = resolve_state("admin@example.com").await;
        let emails = state.resolve_allowed_emails().await.unwrap();
        assert_eq!(emails, vec!["admin@example.com"]);
    }

    #[tokio::test]
    async fn resolve_allowed_emails_includes_db_rows_after_admin() {
        let state = resolve_state("admin@example.com").await;
        state
            .store
            .add_allowed_user("alice@example.com", "admin", now_unix())
            .await
            .unwrap();
        state
            .store
            .add_allowed_user("bob@example.com", "admin", now_unix() + 1)
            .await
            .unwrap();
        let emails = state.resolve_allowed_emails().await.unwrap();
        // Admin is always first; DB rows follow in created_at ASC order.
        assert_eq!(
            emails,
            vec!["admin@example.com", "alice@example.com", "bob@example.com"]
        );
    }

    #[tokio::test]
    async fn resolve_allowed_emails_deduplicates_admin_present_in_db() {
        let state = resolve_state("admin@example.com").await;
        // add_allowed_user lowercases; admin_email may differ in case → still deduped.
        state
            .store
            .add_allowed_user("Admin@Example.COM", "self", now_unix())
            .await
            .unwrap();
        state
            .store
            .add_allowed_user("other@example.com", "admin", now_unix() + 1)
            .await
            .unwrap();
        let emails = state.resolve_allowed_emails().await.unwrap();
        // "admin@example.com" from DB is deduped; "other@example.com" remains.
        assert_eq!(emails, vec!["admin@example.com", "other@example.com"]);
    }

    #[tokio::test]
    async fn auth_state_preserves_public_url_path_prefix_in_google_redirect_uri() {
        let temp = tempdir().expect("tempdir");
        let state = AuthState::new(AuthConfig {
            mode: AuthMode::OAuth,
            public_url: Some(Url::parse("https://lab.example.com/gateway").expect("public url")),
            sqlite_path: temp.path().join("auth.db"),
            key_path: temp.path().join("auth.pem"),
            bootstrap_secret: None,
            allowed_client_redirect_uris: Vec::new(),
            admin_email: "admin@example.com".to_string(),
            google: GoogleConfig {
                client_id: "client-id".to_string(),
                client_secret: "client-secret".to_string(),
                callback_url: None,
                callback_path: "/auth/google/callback".to_string(),
                scopes: vec![
                    "openid".to_string(),
                    "email".to_string(),
                    "profile".to_string(),
                ],
            },
            token_encryption_key: Some(crate::at_rest::TokenEncryptionKey::from_passphrase(
                "state-test-google-provider-key",
            )),
            access_token_ttl: Duration::from_hours(1),
            refresh_token_ttl: Duration::from_hours(1),
            auth_code_ttl: Duration::from_mins(5),
            register_requests_per_minute: 10,
            authorize_requests_per_minute: 20,
            max_pending_oauth_states: 1024,
            ..AuthConfig::default()
        })
        .await
        .expect("auth state");

        assert_eq!(
            state.google.redirect_uri.as_str(),
            "https://lab.example.com/gateway/auth/google/callback"
        );
    }

    #[tokio::test]
    async fn auth_state_uses_explicit_google_callback_url_without_changing_public_url() {
        let temp = tempdir().expect("tempdir");
        let config = AuthConfig::from_sources(
            [
                ("LAB_AUTH_MODE", "oauth"),
                ("LAB_PUBLIC_URL", "https://issuer.example.com"),
                (
                    "LAB_GOOGLE_CALLBACK_URL",
                    "https://app.example.com/auth/google/callback",
                ),
                ("LAB_GOOGLE_CLIENT_ID", "client-id"),
                ("LAB_GOOGLE_CLIENT_SECRET", "client-secret"),
                ("LAB_AUTH_ADMIN_EMAIL", "admin@example.com"),
                (
                    "LAB_TOKEN_ENCRYPTION_KEY",
                    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
                ),
                (
                    "LAB_AUTH_SQLITE_PATH",
                    temp.path().join("auth.db").to_str().expect("sqlite path"),
                ),
                (
                    "LAB_AUTH_KEY_PATH",
                    temp.path().join("auth.pem").to_str().expect("key path"),
                ),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string())),
        )
        .expect("auth config");
        let state = AuthState::new(config).await.expect("auth state");

        assert_eq!(
            state.config.public_url.as_ref().map(Url::as_str),
            Some("https://issuer.example.com/")
        );
        assert_eq!(
            state.google.redirect_uri.as_str(),
            "https://app.example.com/auth/google/callback"
        );
    }
}
