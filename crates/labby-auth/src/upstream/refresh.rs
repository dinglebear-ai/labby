//! Single-flight refresh coordination for upstream OAuth clients.
//!
//! `RefreshLocks` prevents concurrent callers for the same `(upstream, subject)` pair
//! from issuing simultaneous token refresh requests against the authorization server.
//! One caller wins the lock and executes `get_access_token()` (which internally handles
//! proactive refresh); all others wait and then return the already-refreshed token.
//!
//! **Scope:** This module handles *proactive* refresh triggered before making an MCP call.
//! Reactive 401-retry logic is wired in Task 4 (`dispatch/gateway/`).
//!
//! ## rmcp refresh semantics
//!
//! `AuthorizationManager::get_access_token()` refreshes the token when fewer than 30 s
//! remain before expiry.  It does **not** react to 401 responses from the resource server.
//! A 401 with a locally-still-valid token requires an explicit `refresh_token()` call
//! followed by a retry — that is the Task 4 responsibility.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::Mutex;

use super::types::{OAuthEgressKind, OauthError};

/// Per-`(upstream_name, subject)` mutex pool.
///
/// Entries are created lazily on first access and are never removed (the number of
/// distinct `(upstream, subject)` pairs is bounded by the number of configured upstreams
/// times the number of users, which is small in a homelab context).
#[derive(Default)]
pub struct RefreshLocks {
    locks: DashMap<(String, String), Arc<Mutex<()>>>,
    flights: DashMap<(String, String), Arc<RefreshFlight>>,
    overflow: Arc<Mutex<()>>,
    maintenance: std::sync::Mutex<()>,
}

#[derive(Default)]
struct RefreshFlight {
    running: Mutex<bool>,
    completed: std::sync::Mutex<Option<Result<(), CachedRefreshFailure>>>,
    notify: tokio::sync::Notify,
}

const MAX_COORDINATION_ENTRIES: usize = 2_048;

impl RefreshLocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the mutex for `(upstream_name, subject)`, creating it if absent.
    pub fn acquire(&self, upstream_name: &str, subject: &str) -> Arc<Mutex<()>> {
        let key = (upstream_name.to_string(), subject.to_string());
        let _maintenance = self
            .maintenance
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = self.locks.get(&key) {
            return existing.value().clone();
        }
        if self.locks.len() >= MAX_COORDINATION_ENTRIES {
            let idle = self
                .locks
                .iter()
                .find(|entry| Arc::strong_count(entry.value()) == 1)
                .map(|entry| entry.key().clone());
            if let Some(idle) = idle {
                self.locks.remove(&idle);
            } else {
                return Arc::clone(&self.overflow);
            }
        }
        self.locks
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Join an in-flight provider refresh without holding a mutex across I/O.
    /// Returns `true` only to the caller that executed `operation`.
    pub async fn run_shared<F, Fut>(
        &self,
        upstream_name: &str,
        subject: &str,
        operation: F,
    ) -> (bool, Result<(), OauthError>)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), OauthError>>,
    {
        let key = (upstream_name.to_string(), subject.to_string());
        let flight = {
            let _maintenance = self
                .maintenance
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(existing) = self.flights.get(&key) {
                existing.value().clone()
            } else {
                if self.flights.len() >= MAX_COORDINATION_ENTRIES {
                    let idle = self
                        .flights
                        .iter()
                        .find(|entry| Arc::strong_count(entry.value()) == 1)
                        .map(|entry| entry.key().clone());
                    if let Some(idle) = idle {
                        self.flights.remove(&idle);
                    } else {
                        return (
                            true,
                            Err(OauthError::Internal(
                                "OAuth refresh coordination capacity exhausted".to_string(),
                            )),
                        );
                    }
                }
                self.flights
                    .entry(key)
                    .or_insert_with(|| Arc::new(RefreshFlight::default()))
                    .clone()
            }
        };
        let notified = flight.notify.notified();
        {
            let mut running = flight.running.lock().await;
            if !*running {
                *running = true;
                drop(running);
                let result = operation().await;
                let cached = result.as_ref().map(|_| ()).map_err(|error| {
                    CachedRefreshFailure::from_error(error).unwrap_or_else(|| {
                        CachedRefreshFailure::Egress {
                            kind: OAuthEgressKind::UpstreamError,
                            message: error.to_string(),
                        }
                    })
                });
                *flight
                    .completed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cached);
                *flight.running.lock().await = false;
                flight.notify.notify_waiters();
                return (true, result);
            }
        }
        notified.await;
        let result = flight
            .completed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or(Ok(()))
            .map_err(|error| error.to_error());
        (false, result)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.locks.len()
    }
}

/// How long a confirmed refresh failure suppresses further live retries for the
/// same `(upstream, subject)` pair. Chosen to be well short of any human patience
/// window (so a fix shows up promptly) while still cutting a dead credential's
/// call volume against the authorization server by roughly two orders of
/// magnitude versus retrying on every single request.
pub const REFRESH_FAILURE_COOLDOWN: Duration = Duration::from_mins(5);

/// Per-`(upstream_name, subject)` "is this credential known-broken right now"
/// cache.
///
/// Without this, a dead refresh token (revoked, expired, `invalid_grant`, ...)
/// gets retried against the authorization server on every single request
/// forever — `TokenRefreshState::refresh_due()` is purely time-based and has
/// no memory of prior outcomes. That wastes latency on every real request
/// touching the upstream, and can itself contribute to the authorization
/// server rate-limiting or flagging the client_id, which is especially bad
/// when multiple upstreams share one client_id (see `labby-auth::upstream`
/// module docs).
#[derive(Clone)]
enum CachedRefreshFailure {
    NeedsReauth(String),
    Egress {
        kind: OAuthEgressKind,
        message: String,
    },
}

impl CachedRefreshFailure {
    fn from_error(error: &OauthError) -> Option<Self> {
        match error {
            OauthError::NeedsReauth(message) => Some(Self::NeedsReauth(message.clone())),
            OauthError::Egress { kind, message } => Some(Self::Egress {
                kind: *kind,
                message: message.clone(),
            }),
            _ => None,
        }
    }

    fn to_error(&self) -> OauthError {
        match self {
            Self::NeedsReauth(message) => OauthError::NeedsReauth(message.clone()),
            Self::Egress { kind, message } => OauthError::Egress {
                kind: *kind,
                message: message.clone(),
            },
        }
    }
}

#[derive(Default)]
pub struct RefreshFailureCache(DashMap<(String, String), (Instant, CachedRefreshFailure)>);

impl RefreshFailureCache {
    pub fn new() -> Self {
        Self(DashMap::new())
    }

    /// Record that a refresh just failed for `(upstream_name, subject)`.
    pub fn record_failure(&self, upstream_name: &str, subject: &str, error: &OauthError) {
        let Some(error) = CachedRefreshFailure::from_error(error) else {
            return;
        };
        self.0
            .retain(|_, (failed_at, _)| failed_at.elapsed() < REFRESH_FAILURE_COOLDOWN);
        if self.0.len() >= MAX_COORDINATION_ENTRIES {
            let oldest = self
                .0
                .iter()
                .max_by_key(|entry| entry.value().0.elapsed())
                .map(|entry| entry.key().clone());
            if let Some(oldest) = oldest {
                self.0.remove(&oldest);
            }
        }
        self.0.insert(
            (upstream_name.to_string(), subject.to_string()),
            (Instant::now(), error),
        );
    }

    /// Clear any recorded failure for `(upstream_name, subject)` — call this on
    /// any successful refresh, a fresh authorization completing, or explicit
    /// credential clearing, so a fix is picked up immediately instead of
    /// waiting out the cooldown.
    pub fn clear(&self, upstream_name: &str, subject: &str) {
        self.0
            .remove(&(upstream_name.to_string(), subject.to_string()));
    }

    /// Whether `(upstream_name, subject)` failed recently enough that a live
    /// retry should be skipped.
    pub fn recently_failed(&self, upstream_name: &str, subject: &str) -> bool {
        self.recent_error(upstream_name, subject).is_some()
    }

    pub fn recent_error(&self, upstream_name: &str, subject: &str) -> Option<OauthError> {
        let key = (upstream_name.to_string(), subject.to_string());
        let recent = self
            .0
            .get(&key)
            .filter(|entry| entry.value().0.elapsed() < REFRESH_FAILURE_COOLDOWN)
            .map(|entry| entry.value().1.to_error());
        if recent.is_none() {
            self.0.remove(&key);
        }
        recent
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{MAX_COORDINATION_ENTRIES, RefreshFailureCache, RefreshLocks};
    use crate::upstream::types::{OAuthEgressKind, OauthError};

    #[test]
    fn active_refresh_lock_references_cannot_grow_registry_past_cap() {
        let locks = RefreshLocks::new();
        let held = (0..MAX_COORDINATION_ENTRIES)
            .map(|index| locks.acquire("upstream", &format!("subject-{index}")))
            .collect::<Vec<_>>();
        let overflow_one = locks.acquire("upstream", "overflow-one");
        let overflow_two = locks.acquire("upstream", "overflow-two");
        assert_eq!(locks.len(), MAX_COORDINATION_ENTRIES);
        assert!(Arc::ptr_eq(&overflow_one, &overflow_two));
        drop(held);
    }

    #[test]
    fn fresh_cache_has_no_recent_failures() {
        let cache = RefreshFailureCache::new();
        assert!(!cache.recently_failed("google-drive", "gateway"));
    }

    #[test]
    fn recorded_failure_is_recently_failed_until_cleared() {
        let cache = RefreshFailureCache::new();
        cache.record_failure(
            "google-drive",
            "gateway",
            &OauthError::NeedsReauth("invalid_grant".to_string()),
        );
        assert!(cache.recently_failed("google-drive", "gateway"));

        cache.clear("google-drive", "gateway");
        assert!(!cache.recently_failed("google-drive", "gateway"));
    }

    #[test]
    fn failures_are_scoped_per_upstream_and_subject() {
        let cache = RefreshFailureCache::new();
        cache.record_failure(
            "google-drive",
            "gateway",
            &OauthError::NeedsReauth("invalid_grant".to_string()),
        );

        assert!(!cache.recently_failed("google-gmail", "gateway"));
        assert!(!cache.recently_failed("google-drive", "alice"));
    }

    #[test]
    fn transient_failure_cache_preserves_typed_egress_error() {
        let cache = RefreshFailureCache::new();
        cache.record_failure(
            "google-drive",
            "gateway",
            &OauthError::Egress {
                kind: OAuthEgressKind::Timeout,
                message: "provider deadline elapsed".to_string(),
            },
        );
        let error = cache
            .recent_error("google-drive", "gateway")
            .expect("cached typed failure");
        assert_eq!(error.kind(), "timeout");
        assert!(!matches!(error, OauthError::NeedsReauth(_)));
    }

    #[tokio::test]
    async fn concurrent_refresh_callers_join_one_typed_failure_result() {
        let locks = Arc::new(RefreshLocks::new());
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let run = |locks: Arc<RefreshLocks>, calls: Arc<std::sync::atomic::AtomicUsize>| {
            tokio::spawn(async move {
                locks
                    .run_shared("google-drive", "subject", || async move {
                        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        Err(OauthError::Egress {
                            kind: OAuthEgressKind::Timeout,
                            message: "provider timeout".to_string(),
                        })
                    })
                    .await
            })
        };
        let (first, second) = tokio::join!(
            run(Arc::clone(&locks), Arc::clone(&calls)),
            run(Arc::clone(&locks), Arc::clone(&calls))
        );
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_ne!(first.0, second.0);
        assert_eq!(first.1.unwrap_err().kind(), "timeout");
        assert_eq!(second.1.unwrap_err().kind(), "timeout");
    }
}
