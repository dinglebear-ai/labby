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

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use labby_runtime::gateway_config::{UpstreamConfig, UpstreamOauthRegistration};
use rmcp::transport::AuthClient;
use rmcp::transport::streamable_http_client::StreamableHttpClient;
use rmcp_client as rmcp;
use tokio::sync::{Mutex, RwLock};

use crate::upstream::manager::UpstreamOauthManager;
use crate::upstream::types::OauthError;

const MAX_BUILD_LOCKS: usize = 4_096;
const MAX_CREDENTIAL_GENERATIONS: usize = 4_096;

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
    credential_generations: Arc<DashMap<(String, String), u64>>,
    next_credential_generation: Arc<AtomicU64>,
    /// Process-wide credential lifecycle barrier shared with every upstream
    /// pool built from this cache. Connection builders take a read guard for
    /// their complete build-and-publish path; revocation takes the write guard.
    invalidation_barrier: Arc<RwLock<()>>,
    /// Optional host-owned interactive reauthorization hook.
    reauth_handler: Option<OauthReauthHandler>,
}

impl OauthClientCache {
    fn prune_idle_build_locks(&self) {
        if self.build_locks.len() >= MAX_BUILD_LOCKS {
            self.build_locks
                .retain(|_, lock| Arc::strong_count(lock) > 1);
        }
    }
    /// Create a new cache backed by the gateway's OAuth manager map.
    #[must_use]
    pub fn new(managers: Arc<DashMap<String, UpstreamOauthManager>>) -> Self {
        Self {
            clients: Arc::new(DashMap::new()),
            managers,
            build_locks: Arc::new(DashMap::new()),
            credential_generations: Arc::new(DashMap::new()),
            next_credential_generation: Arc::new(AtomicU64::new(1)),
            invalidation_barrier: Arc::new(RwLock::new(())),
            reauth_handler: None,
        }
    }

    /// Shared lifecycle barrier used by gateway pools and credential mutation.
    #[must_use]
    pub fn invalidation_barrier(&self) -> Arc<RwLock<()>> {
        Arc::clone(&self.invalidation_barrier)
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
        let _lifecycle = self.invalidation_barrier.read().await;
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
        self.get_or_insert_with(
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
        .await
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
        self.get_or_build_capped_with_generation(config, subject, http_client)
            .await
            .map(|(client, _)| client)
    }

    /// Build a capped client and bind it to the current credential generation.
    pub async fn get_or_build_capped_with_generation<C>(
        &self,
        config: &UpstreamConfig,
        subject: &str,
        http_client: C,
    ) -> Result<(AuthClient<C>, u64), OauthError>
    where
        C: StreamableHttpClient + Clone,
    {
        // The capped path does not retain the resulting AuthClient, but it
        // still shares this single-flight gate with `get_or_build`. Without
        // the gate, two cold connections could both observe a revoked refresh
        // token and open duplicate interactive browser flows.
        let key = (config.name.clone(), subject.to_string());
        self.prune_idle_build_locks();
        let lock = self
            .build_locks
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
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
            return Ok((client, self.credential_generation(&config.name, subject)));
        };

        let retry_client = http_client.clone();
        match manager.build_auth_client_with(subject, http_client).await {
            Err(OauthError::NeedsReauth(_)) => {
                handler(config.name.clone(), subject.to_string()).await?;
                let client = manager
                    .build_auth_client_with(subject, retry_client)
                    .await?;
                Ok((client, self.credential_generation(&config.name, subject)))
            }
            result => {
                let client = result?;
                Ok((client, self.credential_generation(&config.name, subject)))
            }
        }
    }

    /// Current monotonic credential generation for one authorization context.
    #[must_use]
    pub fn credential_generation(&self, upstream: &str, subject: &str) -> u64 {
        let generation = *self
            .credential_generations
            .entry((upstream.to_string(), subject.to_string()))
            .or_insert_with(|| {
                self.next_credential_generation
                    .fetch_add(1, Ordering::AcqRel)
            });
        self.prune_credential_generations();
        generation
    }

    fn prune_credential_generations(&self) {
        while self.credential_generations.len() > MAX_CREDENTIAL_GENERATIONS {
            let oldest = self
                .credential_generations
                .iter()
                .min_by_key(|entry| *entry.value())
                .map(|entry| entry.key().clone());
            let Some(oldest) = oldest else { break };
            self.credential_generations.remove(&oldest);
        }
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

        if let Some(entry) = self.clients.get(&key)
            && entry.fingerprint == fingerprint
        {
            return Ok(Arc::clone(&entry.client));
        }

        self.prune_idle_build_locks();
        let lock = self
            .build_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Re-check after acquiring the lock: another caller may have built
        // the entry while we were waiting.
        if let Some(entry) = self.clients.get(&key)
            && entry.fingerprint == fingerprint
        {
            return Ok(Arc::clone(&entry.client));
        }

        let arc_client = builder().await?;

        self.clients.insert(
            key,
            Arc::new(CachedAuthClient {
                client: Arc::clone(&arc_client),
                fingerprint,
            }),
        );

        Ok(arc_client)
    }

    /// Evict the entry for a single `(upstream, subject)` pair.
    ///
    /// Used by API handlers when credentials are cleared or when a refresh
    /// fails terminally and the next request must reauthenticate.
    pub fn evict_subject(&self, upstream: &str, subject: &str) {
        let key = (upstream.to_string(), subject.to_string());
        self.credential_generations.remove(&key);
        self.clients.remove(&key);
        self.build_locks
            .remove_if(&key, |_, lock| Arc::strong_count(lock) == 1);
    }

    /// Evict every entry for `upstream`.
    ///
    /// Used at config reload when an upstream is removed or its OAuth
    /// registration changes, and when the whole server shuts down the
    /// upstream's sessions.
    pub fn evict_upstream(&self, upstream: &str) {
        self.credential_generations
            .retain(|(name, _), _| name != upstream);
        self.clients.retain(|(name, _), _| name != upstream);
        self.build_locks
            .retain(|(name, _), lock| name != upstream || Arc::strong_count(lock) > 1);
    }

    /// Evict all cached OAuth clients.
    ///
    /// Used when a shared provider credential is explicitly revoked because the
    /// credential may back several upstream names. Build locks are preserved.
    pub fn evict_all(&self) {
        self.credential_generations.clear();
        self.clients.clear();
        self.build_locks
            .retain(|_, lock| Arc::strong_count(lock) > 1);
    }

    /// Evict every entry whose upstream is not in `known`.
    ///
    /// Used at config reload to drop cached clients for upstreams that no
    /// longer exist in config.
    pub fn evict_upstreams_not_in(&self, known: &std::collections::HashSet<&str>) {
        self.credential_generations
            .retain(|(name, _), _| known.contains(name.as_str()));
        self.clients
            .retain(|(name, _), _| known.contains(name.as_str()));
        self.build_locks
            .retain(|(name, _), lock| known.contains(name.as_str()) || Arc::strong_count(lock) > 1);
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

    #[cfg(any(test, debug_assertions))]
    pub fn build_lock_count_for_tests(&self) -> usize {
        self.build_locks.len()
    }

    #[cfg(any(test, debug_assertions))]
    pub fn credential_generation_count_for_tests(&self) -> usize {
        self.credential_generations.len()
    }

    /// Insert a pre-built `AuthClient` directly into the cache.
    ///
    /// Test-only seam: available in `labby-auth`'s own tests and downstream
    /// debug test builds. It is intentionally not gated by a Cargo feature so
    /// `--all-features --release` cannot expose it in production artifacts.
    #[cfg(any(test, debug_assertions))]
    pub fn insert_for_tests(
        &self,
        upstream: &str,
        subject: &str,
        fingerprint: &str,
        client: Arc<AuthClient<reqwest::Client>>,
    ) {
        self.clients.insert(
            (upstream.to_string(), subject.to_string()),
            Arc::new(CachedAuthClient {
                client,
                fingerprint: fingerprint.to_string(),
            }),
        );
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

    #[test]
    fn idle_build_locks_are_bounded_but_active_locks_are_preserved() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        let active = Arc::new(Mutex::new(()));
        cache.build_locks.insert(
            ("hot".to_string(), "subject".to_string()),
            Arc::clone(&active),
        );
        for index in 0..100_000 {
            cache.build_locks.insert(
                ("upstream".to_string(), format!("subject-{index}")),
                Arc::new(Mutex::new(())),
            );
            cache.prune_idle_build_locks();
        }
        assert!(cache.build_lock_count_for_tests() <= MAX_BUILD_LOCKS + 1);
        assert!(
            cache
                .build_locks
                .contains_key(&("hot".to_string(), "subject".to_string()))
        );
    }

    #[test]
    fn lifecycle_eviction_removes_idle_build_locks_after_barrier() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        cache.build_locks.insert(
            ("removed".to_string(), "subject".to_string()),
            Arc::new(Mutex::new(())),
        );
        cache.evict_upstream("removed");
        assert_eq!(cache.build_lock_count_for_tests(), 0);
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
                "issuer": server.uri(),
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
        cache.insert_for_tests(
            "google-calendar",
            "gateway",
            "preregistered:google-client",
            dummy_auth_client().await,
        );
        cache.insert_for_tests(
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
        cache.insert_for_tests("acme", "alice", &old_fingerprint, dummy_auth_client().await);

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

    #[test]
    fn credential_generations_advance_on_scoped_and_broad_eviction() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        let alice_1 = cache.credential_generation("acme", "alice");
        let bob_1 = cache.credential_generation("acme", "bob");
        cache.evict_subject("acme", "alice");
        let alice_2 = cache.credential_generation("acme", "alice");
        assert!(alice_2 > alice_1);
        assert_eq!(cache.credential_generation("acme", "bob"), bob_1);
        cache.evict_upstream("acme");
        let alice_3 = cache.credential_generation("acme", "alice");
        let bob_2 = cache.credential_generation("acme", "bob");
        assert!(alice_3 > alice_2);
        assert!(bob_2 > bob_1);
        cache.evict_all();
        assert!(cache.credential_generation("acme", "alice") > alice_3);
        assert!(cache.credential_generation("acme", "bob") > bob_2);
    }

    #[test]
    fn credential_generations_are_bounded_and_removed_with_unknown_upstreams() {
        let cache = OauthClientCache::new(Arc::new(DashMap::new()));
        for index in 0..100_000 {
            let _ = cache.credential_generation("acme", &format!("subject-{index}"));
        }
        assert!(cache.credential_generation_count_for_tests() <= MAX_CREDENTIAL_GENERATIONS);
        let _ = cache.credential_generation("removed", "subject");
        cache.evict_upstreams_not_in(&std::collections::HashSet::from(["acme"]));
        assert!(
            !cache
                .credential_generations
                .contains_key(&("removed".to_string(), "subject".to_string()))
        );
    }

    // End-to-end eviction tests live in the Task 4 Step 7 suite where a real
    // `UpstreamOauthManager` and credential store are set up; constructing an
    // `AuthClient` here requires an async network-touching call to
    // `AuthorizationManager::new`, which is inappropriate for a unit test.
}
