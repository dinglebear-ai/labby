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

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::Mutex;

const MAX_COORDINATION_ENTRIES: usize = 4_096;

/// Per-`(upstream_name, subject)` mutex pool.
///
/// Idle entries are pruned at a bounded cardinality. Entries with another live
/// `Arc` are never removed, preserving single-flight for active callers.
#[derive(Default)]
pub struct RefreshLocks(DashMap<(String, String), Arc<Mutex<()>>>);

impl RefreshLocks {
    pub fn new() -> Self {
        Self(DashMap::new())
    }

    /// Return the mutex for `(upstream_name, subject)`, creating it if absent.
    pub fn acquire(&self, upstream_name: &str, subject: &str) -> Arc<Mutex<()>> {
        let key = (upstream_name.to_string(), subject.to_string());
        if self.0.len() >= MAX_COORDINATION_ENTRIES {
            self.0.retain(|_, lock| Arc::strong_count(lock) > 1);
        }
        self.0
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
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
#[derive(Default)]
pub struct RefreshFailureCache(DashMap<(String, String), Instant>);

impl RefreshFailureCache {
    pub fn new() -> Self {
        Self(DashMap::new())
    }

    /// Record that a refresh just failed for `(upstream_name, subject)`.
    pub fn record_failure(&self, upstream_name: &str, subject: &str) {
        self.prune_expired();
        let eviction = if self.0.len() >= MAX_COORDINATION_ENTRIES {
            self.0.iter().next().map(|entry| entry.key().clone())
        } else {
            None
        };
        if let Some(oldest) = eviction {
            self.0.remove(&oldest);
        }
        self.0.insert(
            (upstream_name.to_string(), subject.to_string()),
            Instant::now(),
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
        let key = (upstream_name.to_string(), subject.to_string());
        let recent = self
            .0
            .get(&key)
            .is_some_and(|entry| entry.elapsed() < REFRESH_FAILURE_COOLDOWN);
        if !recent {
            self.0.remove(&key);
        }
        recent
    }

    fn prune_expired(&self) {
        if self.0.len() >= MAX_COORDINATION_ENTRIES {
            self.0
                .retain(|_, failed_at| failed_at.elapsed() < REFRESH_FAILURE_COOLDOWN);
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_COORDINATION_ENTRIES, REFRESH_FAILURE_COOLDOWN, RefreshFailureCache, RefreshLocks,
    };

    #[test]
    fn fresh_cache_has_no_recent_failures() {
        let cache = RefreshFailureCache::new();
        assert!(!cache.recently_failed("google-drive", "gateway"));
    }

    #[test]
    fn recorded_failure_is_recently_failed_until_cleared() {
        let cache = RefreshFailureCache::new();
        cache.record_failure("google-drive", "gateway");
        assert!(cache.recently_failed("google-drive", "gateway"));

        cache.clear("google-drive", "gateway");
        assert!(!cache.recently_failed("google-drive", "gateway"));
    }

    #[test]
    fn failures_are_scoped_per_upstream_and_subject() {
        let cache = RefreshFailureCache::new();
        cache.record_failure("google-drive", "gateway");

        assert!(!cache.recently_failed("google-gmail", "gateway"));
        assert!(!cache.recently_failed("google-drive", "alice"));
    }

    #[test]
    fn coordination_maps_remain_bounded_under_high_cardinality() {
        let locks = RefreshLocks::new();
        let failures = RefreshFailureCache::new();
        for index in 0..100_000 {
            drop(locks.acquire("upstream", &format!("subject-{index}")));
            failures.record_failure("upstream", &format!("subject-{index}"));
        }
        assert!(locks.len() <= MAX_COORDINATION_ENTRIES);
        assert!(failures.len() <= MAX_COORDINATION_ENTRIES);
    }

    #[test]
    fn pruning_preserves_singleflight_for_a_live_hot_key() {
        let locks = RefreshLocks::new();
        let hot = locks.acquire("upstream", "hot-subject");
        for index in 0..100_000 {
            drop(locks.acquire("upstream", &format!("cold-{index}")));
        }
        let reacquired = locks.acquire("upstream", "hot-subject");
        assert!(std::sync::Arc::ptr_eq(&hot, &reacquired));
    }

    #[test]
    fn stale_failure_is_drained_on_access() {
        let cache = RefreshFailureCache::new();
        let key = ("upstream".to_string(), "subject".to_string());
        cache.0.insert(
            key,
            std::time::Instant::now()
                .checked_sub(REFRESH_FAILURE_COOLDOWN + std::time::Duration::from_secs(1))
                .expect("test duration fits Instant range"),
        );
        assert!(!cache.recently_failed("upstream", "subject"));
        assert_eq!(cache.len(), 0);
    }
}
