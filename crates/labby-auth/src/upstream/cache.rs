//! Per-`(upstream, subject)` `AuthClient` cache.
//!
//! Each entry binds one MCP upstream and one lab subject to a single
//! `AuthClient<reqwest::Client>` so tokens are never shared between users.
//! Entries are built lazily on first use via the upstream's
//! [`UpstreamOauthManager`], cached by `(upstream_name, subject)`, and
//! invalidated when the upstream's OAuth registration changes (e.g.
//! `client_id` rotation) or when the upstream is removed from config at
//! reload time.
//!
//! The cache is injected into both `GatewayManager` (for lifecycle and
//! eviction during config reload) and `UpstreamPool` (for per-request
//! lookup from MCP handlers). Extracting it avoids a circular dependency:
//! the pool does not need a reference to the gateway and the gateway does
//! not need to know how the pool uses the clients.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use dashmap::DashMap;
use labby_runtime::gateway_config::{UpstreamConfig, UpstreamOauthRegistration};
use rmcp::transport::AuthClient;
use rmcp::transport::streamable_http_client::StreamableHttpClient;
use rmcp_client as rmcp;
use tokio::sync::{Mutex, RwLock};

use crate::upstream::manager::UpstreamOauthManager;
use crate::upstream::types::OauthError;

/// Callback used by a host surface that can drive an interactive OAuth flow.
///
/// The reusable auth crate does not know how to open a browser or receive a
/// redirect. A host (for example Labby's stdio transport) can install this
/// hook to turn a `NeedsReauth` result into a completed authorization flow and
/// then let the cache retry the original connection once.
pub type OauthReauthHandler = Arc<dyn Fn(String, String) -> OauthReauthFuture + Send + Sync>;

/// Future returned by [`OauthReauthHandler`].
pub type OauthReauthFuture = Pin<Box<dyn Future<Output = Result<(), OauthError>> + Send>>;

/// A cached `AuthClient` plus the OAuth-registration fingerprint it was
/// built from. When the current config's fingerprint differs, the entry
/// is evicted and rebuilt so a stale `client_id` never signs a request.
pub struct CachedAuthClient {
    pub client: Arc<AuthClient<reqwest::Client>>,
    fingerprint: String,
}

/// Per-`(upstream, subject)` `AuthClient` cache.
///
/// Cheap to clone (all state is behind `Arc`). Safe to share between the
/// gateway manager and the upstream pool.
#[derive(Clone)]
pub struct OauthClientCache {
    /// Cached clients keyed by `(upstream_name, subject)`.
    clients: Arc<DashMap<(String, String), Arc<CachedAuthClient>>>,
    /// Per-upstream OAuth managers, owned by the gateway manager and
    /// shared in by `Arc` so the cache can call `build_auth_client`.
    managers: Arc<DashMap<String, UpstreamOauthManager>>,
    /// Per-`(upstream, subject)` build lock so concurrent first-request
    /// tasks don't issue duplicate token exchanges against the AS.
    build_locks: Arc<DashMap<(String, String), Arc<Mutex<()>>>>,
    build_lock_overflow: Arc<Mutex<()>>,
    build_lock_maintenance: Arc<std::sync::Mutex<()>>,
    client_maintenance: Arc<std::sync::Mutex<()>>,
    evicted_clients: Arc<std::sync::Mutex<VecDeque<(String, String)>>>,
    client_capacity: usize,
    /// Process-wide credential lifecycle barrier shared with every upstream
    /// pool built from this cache. Connection builders take a read guard for
    /// their complete build-and-publish path; revocation takes the write guard.
    invalidation_barrier: Arc<RwLock<()>>,
    /// Monotonic credential lifecycle generation. Builders snapshot this
    /// before I/O and must re-check it under the short publication reader.
    lifecycle_epoch: Arc<AtomicU64>,
    /// Optional host-owned interactive reauthorization hook.
    reauth_handler: Option<OauthReauthHandler>,
}

const MAX_BUILD_LOCKS: usize = 2_048;

impl OauthClientCache {
    /// Create a new cache backed by the gateway's OAuth manager map.
    #[must_use]
    pub fn new(managers: Arc<DashMap<String, UpstreamOauthManager>>) -> Self {
        Self {
            clients: Arc::new(DashMap::new()),
            managers,
            build_locks: Arc::new(DashMap::new()),
            build_lock_overflow: Arc::new(Mutex::new(())),
            build_lock_maintenance: Arc::new(std::sync::Mutex::new(())),
            client_maintenance: Arc::new(std::sync::Mutex::new(())),
            evicted_clients: Arc::new(std::sync::Mutex::new(VecDeque::new())),
            client_capacity: MAX_BUILD_LOCKS,
            invalidation_barrier: Arc::new(RwLock::new(())),
            lifecycle_epoch: Arc::new(AtomicU64::new(0)),
            reauth_handler: None,
        }
    }

    /// Override the number of ready clients retained by this cache.
    ///
    /// Hosts with a smaller connection budget may use this to keep the auth
    /// client and transport registries under the same bound.
    #[must_use]
    pub fn with_client_capacity(mut self, capacity: usize) -> Self {
        self.client_capacity = capacity.max(1);
        self
    }

    /// Publish a host-prepared client through the normal bounded cache path.
    ///
    /// This is useful when a host completes authorization outside the lazy
    /// builder but still needs identical lifecycle and capacity semantics.
    pub fn publish_prebuilt(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        client: Arc<AuthClient<reqwest::Client>>,
    ) -> Result<(), OauthError> {
        let fingerprint = registration_fingerprint(config, None)?;
        self.insert_cached_client(
            (config.name.clone(), subject.to_string()),
            Arc::new(CachedAuthClient {
                client,
                fingerprint,
            }),
        );
        Ok(())
    }

    /// Number of ready clients currently retained.
    #[must_use]
    pub fn ready_client_count(&self) -> usize {
        self.clients.len()
    }

    /// Whether a ready client remains reusable for this subject.
    #[must_use]
    pub fn contains_ready_client(&self, upstream: &str, subject: &str) -> bool {
        self.clients
            .contains_key(&(upstream.to_string(), subject.to_string()))
    }

    /// Shared lifecycle barrier used by gateway pools and credential mutation.
    #[must_use]
    pub fn invalidation_barrier(&self) -> Arc<RwLock<()>> {
        Arc::clone(&self.invalidation_barrier)
    }

    /// Snapshot the credential lifecycle generation before starting I/O.
    #[must_use]
    pub fn lifecycle_epoch(&self) -> u64 {
        self.lifecycle_epoch.load(Ordering::Acquire)
    }

    /// Advance the generation while holding the lifecycle write barrier.
    pub fn advance_lifecycle_epoch(&self) -> u64 {
        self.lifecycle_epoch.fetch_add(1, Ordering::AcqRel) + 1
    }

    async fn ensure_epoch_current(&self, expected: u64) -> Result<(), OauthError> {
        let _publication = self.invalidation_barrier.read().await;
        if self.lifecycle_epoch() == expected {
            Ok(())
        } else {
            Err(OauthError::NeedsReauth(
                "credentials changed while the OAuth client was being built".to_string(),
            ))
        }
    }

    /// Install a host-owned interactive reauthorization hook.
    ///
    /// The hook is invoked only after the normal credential load/refresh path
    /// returns [`OauthError::NeedsReauth`]. It runs while the per-key build
    /// lock is held, so concurrent first requests share one browser flow.
    #[must_use]
    pub fn with_reauth_handler(mut self, handler: OauthReauthHandler) -> Self {
        self.reauth_handler = Some(handler);
        self
    }

    /// Return a cached `AuthClient<reqwest::Client>` for `(upstream, subject)`,
    /// building one on first use.
    ///
    /// Kept for callers that need a shared `Arc<AuthClient<reqwest::Client>>`
    /// (e.g. status-check endpoints).  The MCP connection path uses
    /// `get_or_build_capped` instead so the `BodyCappedHttpClient` cap applies.
    ///
    /// If a cached entry exists but was built from a different OAuth
    /// registration than the current `config`, the entry is evicted and
    /// rebuilt so stale `client_id`s never sign requests.
    ///
    /// For `Dynamic` upstreams the fingerprint includes the stored
    /// `client_id` (fetched from SQLite via the upstream manager) so a
    /// re-registration cycle evicts the cached `AuthClient` (lab-77y5.13).
    ///
    /// Concurrent first-request callers for the same key are serialised
    /// by a per-key mutex so only one token exchange runs.
    #[allow(dead_code)]
    pub async fn get_or_build(
        &self,
        config: &UpstreamConfig,
        subject: &str,
    ) -> Result<Arc<AuthClient<reqwest::Client>>, OauthError> {
        let lifecycle_epoch = self.lifecycle_epoch();
        // For Dynamic upstreams, include the stored client_id in the
        // fingerprint so a re-registration is detected (lab-77y5.13).
        let dynamic_client_id: Option<String> = if config
            .oauth
            .as_ref()
            .is_some_and(|o| matches!(o.registration, UpstreamOauthRegistration::Dynamic))
        {
            self.managers
                .get(&config.name)
                .map(|r| r.clone())
                .ok_or_else(|| {
                    OauthError::Internal(format!(
                        "no oauth manager registered for upstream '{}'",
                        config.name
                    ))
                })?
                .stored_dynamic_client_id(subject)
                .await?
        } else {
            None
        };

        let reauth_handler = self.reauth_handler.clone();
        let upstream_name = config.name.clone();
        let subject_owned = subject.to_string();
        let client = self
            .get_or_insert_with(
                config,
                subject,
                dynamic_client_id.as_deref(),
                || async move {
                    let manager = self
                        .managers
                        .get(&upstream_name)
                        .map(|r| r.clone())
                        .ok_or_else(|| {
                            OauthError::Internal(format!(
                                "no oauth manager registered for upstream '{}'",
                                upstream_name
                            ))
                        })?;
                    let auth_client = match manager.build_auth_client(&subject_owned).await {
                        Err(OauthError::NeedsReauth(reason)) => {
                            let Some(handler) = reauth_handler else {
                                return Err(OauthError::NeedsReauth(reason));
                            };
                            handler(upstream_name.clone(), subject_owned.clone()).await?;
                            manager.build_auth_client(&subject_owned).await?
                        }
                        result => result?,
                    };
                    Ok(Arc::new(auth_client))
                },
            )
            .await?;
        if let Err(error) = self.ensure_epoch_current(lifecycle_epoch).await {
            self.evict_subject(&config.name, subject);
            return Err(error);
        }
        Ok(client)
    }

    /// Build an `AuthClient<C>` wrapping the supplied HTTP client and return it
    /// WITHOUT caching it.
    ///
    /// This is the P-H4 entry point: callers that manage their own connection
    /// cache (e.g. `UpstreamPool::acquire_or_connect_subject` via
    /// `SubjectScopedConnection`) pass a pre-built `BodyCappedHttpClient` so
    /// the OAuth path gets the same streaming response-size cap as the
    /// non-OAuth path.  The caller caches the resulting `AuthClient` at the
    /// pool level, so there is no double-caching here.
    pub async fn get_or_build_capped<C>(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        http_client: C,
    ) -> Result<AuthClient<C>, OauthError>
    where
        C: StreamableHttpClient + Clone,
    {
        let lifecycle_epoch = self.lifecycle_epoch();
        // The capped path does not retain the resulting AuthClient, but it
        // still shares this single-flight gate with `get_or_build`. Without
        // the gate, two cold connections could both observe a revoked refresh
        // token and open duplicate interactive browser flows.
        let key = (config.name.clone(), subject.to_string());
        let lock = self.build_lock(&key);
        let _guard = lock.lock().await;

        let manager = self
            .managers
            .get(&config.name)
            .map(|r| r.clone())
            .ok_or_else(|| {
                OauthError::Internal(format!(
                    "no oauth manager registered for upstream '{}'",
                    config.name
                ))
            })?;
        let Some(handler) = self.reauth_handler.clone() else {
            let client = manager.build_auth_client_with(subject, http_client).await?;
            self.ensure_epoch_current(lifecycle_epoch).await?;
            return Ok(client);
        };

        let retry_client = http_client.clone();
        let client = match manager.build_auth_client_with(subject, http_client).await {
            Err(OauthError::NeedsReauth(_)) => {
                handler(config.name.clone(), subject.to_string()).await?;
                manager.build_auth_client_with(subject, retry_client).await
            }
            result => result,
        }?;
        self.ensure_epoch_current(lifecycle_epoch).await?;
        Ok(client)
    }

    #[allow(dead_code)]
    async fn get_or_insert_with<F, Fut>(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        // For `Dynamic` upstreams: the stored `client_id` to fold into the
        // fingerprint. `None` for non-dynamic upstreams.
        dynamic_client_id: Option<&str>,
        builder: F,
    ) -> Result<Arc<AuthClient<reqwest::Client>>, OauthError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Arc<AuthClient<reqwest::Client>>, OauthError>>,
    {
        let fingerprint = registration_fingerprint(config, dynamic_client_id)?;
        let key = (config.name.clone(), subject.to_string());
        let lock = self.build_lock(&key);

        if let Some(entry) = self.clients.get(&key)
            && entry.fingerprint == fingerprint
        {
            return Ok(Arc::clone(&entry.client));
        }

        let _guard = lock.lock().await;

        // Re-check after acquiring the lock: another caller may have built
        // the entry while we were waiting.
        if let Some(entry) = self.clients.get(&key)
            && entry.fingerprint == fingerprint
        {
            return Ok(Arc::clone(&entry.client));
        }

        let lifecycle_epoch = self.lifecycle_epoch();
        let arc_client = builder().await?;
        let _publication = self.invalidation_barrier.read().await;
        if self.lifecycle_epoch() != lifecycle_epoch {
            return Err(OauthError::NeedsReauth(
                "credentials changed while the OAuth client was being built".to_string(),
            ));
        }

        self.insert_cached_client(
            key,
            Arc::new(CachedAuthClient {
                client: Arc::clone(&arc_client),
                fingerprint,
            }),
        );

        Ok(arc_client)
    }

    fn insert_cached_client(&self, key: (String, String), client: Arc<CachedAuthClient>) {
        let _maintenance = self
            .client_maintenance
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.clients.contains_key(&key) && self.clients.len() >= self.client_capacity {
            let evicted = { self.clients.iter().next().map(|entry| entry.key().clone()) };
            if let Some(evicted) = evicted {
                self.clients.remove(&evicted);
                let mut pending = self
                    .evicted_clients
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if pending.len() >= self.client_capacity {
                    pending.pop_front();
                }
                pending.push_back(evicted);
            }
        }
        self.clients.insert(key, client);
    }

    /// Drain cache-capacity victims so the gateway can evict the matching live
    /// subject connection from its separately-owned transport registry.
    pub fn take_capacity_evictions(&self) -> Vec<(String, String)> {
        self.evicted_clients
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect()
    }

    /// Evict the entry for a single `(upstream, subject)` pair.
    ///
    /// Used by API handlers when credentials are cleared or when a refresh
    /// fails terminally and the next request must reauthenticate.
    pub fn evict_subject(&self, upstream: &str, subject: &str) {
        let key = (upstream.to_string(), subject.to_string());
        self.clients.remove(&key);
        // build_locks is intentionally NOT evicted: it serializes concurrent
        // builders for the same (upstream, subject) key. Removing it creates a
        // race window where two concurrent callers both see no cached client,
        // both drop the lock guard, and then both start building in parallel.
    }

    /// Evict every entry for `upstream`.
    ///
    /// Used at config reload when an upstream is removed or its OAuth
    /// registration changes, and when the whole server shuts down the
    /// upstream's sessions.
    pub fn evict_upstream(&self, upstream: &str) {
        self.clients.retain(|(name, _), _| name != upstream);
        // build_locks intentionally preserved — see comment in evict_subject.
    }

    /// Evict all cached OAuth clients.
    ///
    /// Used when a shared provider credential is explicitly revoked because the
    /// credential may back several upstream names. Build locks are preserved.
    pub fn evict_all(&self) {
        self.clients.clear();
    }

    /// Evict every entry whose upstream is not in `known`.
    ///
    /// Used at config reload to drop cached clients for upstreams that no
    /// longer exist in config.
    pub fn evict_upstreams_not_in(&self, known: &std::collections::HashSet<&str>) {
        self.clients
            .retain(|(name, _), _| known.contains(name.as_str()));
    }

    fn build_lock(&self, incoming: &(String, String)) -> Arc<Mutex<()>> {
        let _maintenance = self
            .build_lock_maintenance
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = self.build_locks.get(incoming) {
            return existing.value().clone();
        }
        if self.build_locks.len() < MAX_BUILD_LOCKS {
            return self
                .build_locks
                .entry(incoming.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone();
        }
        let idle = self
            .build_locks
            .iter()
            .find(|entry| Arc::strong_count(entry.value()) == 1)
            .map(|entry| entry.key().clone());
        if let Some(idle) = idle {
            self.build_locks.remove(&idle);
            self.build_locks
                .entry(incoming.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        } else {
            Arc::clone(&self.build_lock_overflow)
        }
    }

    /// Number of cached clients. Intended for tests and observability.
    #[allow(dead_code)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.clients.len()
    }

    /// True when the cache holds no clients.
    #[allow(dead_code)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }
}

/// Compute a stable fingerprint of the OAuth registration.
///
/// When the fingerprint changes, the cached `AuthClient` is discarded.
/// `Preregistered` changes when `client_id` rotates; `ClientMetadataDocument`
/// changes when its URL moves; `Dynamic` includes the stored per-subject
/// `client_id` (lab-77y5.13) so a re-registration cycle evicts the stale entry.
#[allow(dead_code)]
fn registration_fingerprint(
    config: &UpstreamConfig,
    dynamic_client_id: Option<&str>,
) -> Result<String, OauthError> {
    let oauth = config
        .oauth
        .as_ref()
        .ok_or_else(|| OauthError::Internal("upstream has no oauth config".to_string()))?;

    Ok(match &oauth.registration {
        UpstreamOauthRegistration::Preregistered { client_id, .. } => {
            format!("preregistered:{client_id}")
        }
        UpstreamOauthRegistration::ClientMetadataDocument { url } => {
            format!("client_metadata_document:{url}")
        }
        UpstreamOauthRegistration::Dynamic => {
            // Include the stored client_id so a re-registration evicts the
            // stale cached AuthClient (lab-77y5.13). Fall back to "none" when
            // no client_id has been persisted yet (first-time registration
            // in-flight) so the initial build is not blocked.
            format!("dynamic:{}", dynamic_client_id.unwrap_or("none"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::gateway_config::{UpstreamOauthConfig, UpstreamOauthMode};
    use rmcp_client::transport::AuthorizationManager;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn insert_test_client(
        cache: &OauthClientCache,
        upstream: &str,
        subject: &str,
        fingerprint: &str,
        client: Arc<AuthClient<reqwest::Client>>,
    ) {
        cache.clients.insert(
            (upstream.to_string(), subject.to_string()),
            Arc::new(CachedAuthClient {
                client,
                fingerprint: fingerprint.to_string(),
            }),
        );
    }

    fn cfg(name: &str, client_id: &str) -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: name.to_string(),
            url: Some(format!("https://{name}.example/mcp")),
            transport: None,
            socket_path: None,
            headers: Default::default(),
            command: None,
            args: vec![],
            bearer_token_env: None,
            env: std::collections::BTreeMap::new(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: false,
            expose_skills: None,
            code_mode_hint: None,
            oauth: Some(UpstreamOauthConfig {
                mode: UpstreamOauthMode::AuthorizationCodePkce,
                registration: UpstreamOauthRegistration::Preregistered {
                    client_id: client_id.to_string(),
                    client_secret_env: None,
                },
                scopes: None,
                credential: Default::default(),
                prefer_client_metadata_document: None,
            }),
            imported_from: None,
            priority: 1.0,
        }
    }

    #[test]
    fn active_build_lock_references_cannot_grow_registry_past_cap() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        let held = (0..MAX_BUILD_LOCKS)
            .map(|index| cache.build_lock(&("upstream".to_string(), format!("subject-{index}"))))
            .collect::<Vec<_>>();
        let overflow_one = cache.build_lock(&("upstream".to_string(), "overflow-one".to_string()));
        let overflow_two = cache.build_lock(&("upstream".to_string(), "overflow-two".to_string()));
        assert_eq!(cache.build_locks.len(), MAX_BUILD_LOCKS);
        assert!(Arc::ptr_eq(&overflow_one, &overflow_two));
        drop(held);
    }

    #[tokio::test]
    async fn live_client_references_cannot_grow_cache_past_cap() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        let client = dummy_auth_client().await;
        let held = (0..=MAX_BUILD_LOCKS)
            .map(|index| {
                cache.insert_cached_client(
                    ("upstream".to_string(), format!("subject-{index}")),
                    Arc::new(CachedAuthClient {
                        client: Arc::clone(&client),
                        fingerprint: "same".to_string(),
                    }),
                );
                Arc::clone(&client)
            })
            .collect::<Vec<_>>();

        assert_eq!(cache.clients.len(), MAX_BUILD_LOCKS);
        assert_eq!(held.len(), MAX_BUILD_LOCKS + 1);
        let victims = cache.take_capacity_evictions();
        assert_eq!(victims.len(), 1);
        assert_eq!(victims[0].0, "upstream");
    }

    #[test]
    fn fingerprint_differs_on_client_id_change() {
        let a = registration_fingerprint(&cfg("acme", "id-1"), None).unwrap();
        let b = registration_fingerprint(&cfg("acme", "id-2"), None).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn fingerprint_stable_for_identical_config() {
        let a = registration_fingerprint(&cfg("acme", "id-1"), None).unwrap();
        let b = registration_fingerprint(&cfg("acme", "id-1"), None).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn empty_cache_is_empty() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    async fn dummy_auth_client() -> Arc<AuthClient<reqwest::Client>> {
        // See google.rs::GoogleProvider::new for why this call is needed
        // under "rustls-no-provider" -- idempotent, safe to ignore Err.
        drop(rustls::crypto::ring::default_provider().install_default());
        let manager = AuthorizationManager::new("http://localhost")
            .await
            .expect("authorization manager");
        Arc::new(AuthClient::new(reqwest::Client::new(), manager))
    }

    #[tokio::test]
    async fn cache_atomic_first_request_no_double_build() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        let config = cfg("acme", "id-1");
        let builds = Arc::new(AtomicUsize::new(0));

        let left = {
            let cache = cache.clone();
            let config = config.clone();
            let builds = Arc::clone(&builds);
            tokio::spawn(async move {
                cache
                    .get_or_insert_with(&config, "alice", None, || {
                        let builds = Arc::clone(&builds);
                        async move {
                            builds.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                            Ok(dummy_auth_client().await)
                        }
                    })
                    .await
                    .expect("left client")
            })
        };
        let right = {
            let cache = cache.clone();
            let config = config.clone();
            let builds = Arc::clone(&builds);
            tokio::spawn(async move {
                cache
                    .get_or_insert_with(&config, "alice", None, || {
                        let builds = Arc::clone(&builds);
                        async move {
                            builds.fetch_add(1, Ordering::SeqCst);
                            Ok(dummy_auth_client().await)
                        }
                    })
                    .await
                    .expect("right client")
            })
        };

        let left = left.await.expect("join left");
        let right = right.await.expect("join right");

        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert!(Arc::ptr_eq(&left, &right));
    }

    #[tokio::test]
    async fn reauth_handler_runs_on_missing_credentials() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sqlite = crate::sqlite::SqliteStore::open(dir.path().join("auth.db"))
            .await
            .expect("sqlite store");
        let key = crate::upstream::encryption::load_key(&base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            [0u8; 32],
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
        let mut config = cfg("acme", "id-1");
        config.url = Some(format!("{}/mcp", server.uri()));
        let managers = Arc::new(DashMap::new());
        managers.insert(
            config.name.clone(),
            UpstreamOauthManager::new(
                sqlite,
                key,
                config.clone(),
                "http://127.0.0.1:12345/auth/upstream/callback".to_string(),
            ),
        );
        let called = Arc::new(AtomicUsize::new(0));
        let called_for_handler = Arc::clone(&called);
        let handler: OauthReauthHandler = Arc::new(move |_, _| {
            called_for_handler.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {
                Err(OauthError::Internal(
                    "test reauthorization failure".to_string(),
                ))
            })
        });
        let cache = OauthClientCache::new(Arc::clone(&managers)).with_reauth_handler(handler);

        drop(rustls::crypto::ring::default_provider().install_default());
        let error = cache
            .get_or_build_capped(&config, "gateway", reqwest::Client::new())
            .await
            .expect_err("missing credentials should require reauth");

        assert!(
            matches!(error, OauthError::Internal(message) if message == "test reauthorization failure")
        );
        assert_eq!(called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn evict_all_removes_clients_for_every_google_upstream() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        insert_test_client(
            &cache,
            "google-calendar",
            "gateway",
            "preregistered:google-client",
            dummy_auth_client().await,
        );
        insert_test_client(
            &cache,
            "google-drive",
            "gateway",
            "preregistered:google-client",
            dummy_auth_client().await,
        );
        assert_eq!(cache.len(), 2);

        cache.evict_all();

        assert!(cache.is_empty());
    }

    #[tokio::test]
    async fn cache_refuses_stale_client_id_after_config_change() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        let old = cfg("acme", "id-1");
        let new = cfg("acme", "id-2");
        let old_fingerprint = registration_fingerprint(&old, None).expect("old fingerprint");
        insert_test_client(
            &cache,
            "acme",
            "alice",
            &old_fingerprint,
            dummy_auth_client().await,
        );

        let rebuilt = Arc::new(AtomicUsize::new(0));
        let client = cache
            .get_or_insert_with(&new, "alice", None, || {
                let rebuilt = Arc::clone(&rebuilt);
                async move {
                    rebuilt.fetch_add(1, Ordering::SeqCst);
                    Ok(dummy_auth_client().await)
                }
            })
            .await
            .expect("rebuilt client");

        assert_eq!(rebuilt.load(Ordering::SeqCst), 1);
        assert_eq!(cache.len(), 1);
        let stored = cache
            .clients
            .get(&(String::from("acme"), String::from("alice")))
            .expect("stored client");
        assert_eq!(
            stored.fingerprint,
            registration_fingerprint(&new, None).unwrap()
        );
        assert!(Arc::ptr_eq(&stored.client, &client));
    }

    #[tokio::test]
    async fn uncapped_builder_cannot_publish_after_lifecycle_epoch_changes() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        let config = cfg("epoch-race", "client");
        let started = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let building = cache.clone();
        let started_for_builder = Arc::clone(&started);
        let resume_for_builder = Arc::clone(&resume);
        let config_for_builder = config.clone();
        let task = tokio::spawn(async move {
            building
                .get_or_insert_with(&config_for_builder, "alice", None, || async move {
                    started_for_builder.notify_one();
                    resume_for_builder.notified().await;
                    Ok(dummy_auth_client().await)
                })
                .await
        });
        started.notified().await;
        let barrier = cache.invalidation_barrier();
        let writer = barrier.write_owned().await;
        cache.advance_lifecycle_epoch();
        drop(writer);
        resume.notify_one();

        assert!(matches!(
            task.await.unwrap(),
            Err(OauthError::NeedsReauth(_))
        ));
        assert!(cache.is_empty());
    }

    // End-to-end eviction tests live in the Task 4 Step 7 suite where a real
    // `UpstreamOauthManager` and credential store are set up; constructing an
    // `AuthClient` here requires an async network-touching call to
    // `AuthorizationManager::new`, which is inappropriate for a unit test.
}
