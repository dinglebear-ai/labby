//! Per-`(upstream, subject)` cache for upstream skill catalogs.
//!
//! # Why shard per subject unconditionally
//!
//! SEP-2549 defines two cache scopes. `"private"` results may only be reused
//! within one authorization context. `"public"` results may be shared by "any
//! client, shared gateway, or caching proxy" — Labby *is* that shared gateway,
//! and the spec warns a public result "may be shared between callers even if
//! the Result is coming from an authenticated endpoint". Labby declines to do
//! that: every entry is keyed by subject regardless of declared scope. The spec
//! forbids over-sharing, never under-sharing, so being stricter is always legal
//! and costs only upstream requests.
//!
//! # Freshness without stalls
//!
//! An expired entry is served immediately while a refresh runs behind it
//! (stale-while-revalidate). Blocking the caller would tie a downstream
//! `skills/list` to the slowest upstream's full wall-clock budget, which is
//! exactly the p99 stall the budget exists to bound in the first place.
//!
//! Upstream-supplied `ttlMs` is clamped: it is untrusted input, and a `0` would
//! turn every read into a fetch while a very large one would pin a stale
//! catalog indefinitely.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use super::skills_list::UpstreamSkills;

/// Shortest refresh interval honored from an upstream's `ttlMs`.
pub(super) const SKILLS_TTL_FLOOR: Duration = Duration::from_secs(5);

/// Longest refresh interval honored from an upstream's `ttlMs`.
pub(super) const SKILLS_TTL_CEILING: Duration = Duration::from_hours(1);

/// Refresh interval used when an upstream publishes no `ttlMs`.
pub(super) const SKILLS_TTL_DEFAULT: Duration = Duration::from_mins(5);

/// Evict a cached catalog untouched for this long.
pub(super) const SKILLS_CACHE_IDLE_TTL: Duration = Duration::from_mins(30);

/// Hard cap on cached catalogs. Cardinality is upstreams × subjects, so an
/// OAuth deployment with many principals would otherwise grow without bound.
pub(super) const SKILLS_CACHE_MAX_ENTRIES: usize = 512;

/// Cache key: one entry per upstream per authorization context.
pub(super) type SkillsCacheKey = (String, Option<String>);

/// A cached catalog snapshot plus the bookkeeping that governs its lifetime.
#[derive(Debug, Clone)]
pub(super) struct CachedSkills {
    pub(super) skills: Arc<UpstreamSkills>,
    /// When this snapshot was fetched.
    fetched_at: Instant,
    /// When it stops being fresh, already clamped.
    expires_at: Instant,
    /// Last read, for idle eviction.
    last_used: Instant,
    /// Set while a background refresh is in flight, so a burst of readers
    /// spawns one refresh rather than one per reader.
    pub(super) refreshing: bool,
}

impl CachedSkills {
    pub(super) fn new(skills: UpstreamSkills) -> Self {
        let now = Instant::now();
        let ttl = clamp_ttl(skills.ttl_ms);
        Self {
            skills: Arc::new(skills),
            fetched_at: now,
            expires_at: now + ttl,
            last_used: now,
            refreshing: false,
        }
    }

    pub(super) fn is_fresh(&self) -> bool {
        Instant::now() < self.expires_at
    }

    /// How long this snapshot stays fresh. Zero once expired.
    ///
    /// A downstream listing that folds these entries in must not advertise a
    /// longer TTL than the data behind it actually has.
    pub(super) fn remaining_ttl(&self) -> Duration {
        self.expires_at.saturating_duration_since(Instant::now())
    }

    pub(super) fn age(&self) -> Duration {
        self.fetched_at.elapsed()
    }

    pub(super) fn touch(&mut self) {
        self.last_used = Instant::now();
    }
}

/// Clamp an upstream-supplied `ttlMs` into the range Labby will honor.
///
/// The value is a freshness *hint* from an untrusted peer, not a contract.
pub(super) fn clamp_ttl(ttl_ms: Option<u64>) -> Duration {
    match ttl_ms {
        None => SKILLS_TTL_DEFAULT,
        Some(ms) => Duration::from_millis(ms).clamp(SKILLS_TTL_FLOOR, SKILLS_TTL_CEILING),
    }
}

/// Drop entries that are idle or over the cap.
///
/// Eviction runs on insert rather than on a timer: the cache is only reachable
/// through the fetch path, so an entry that is never read again is also never
/// in anybody's way until the next insert needs the room.
pub(super) fn evict(cache: &mut HashMap<SkillsCacheKey, CachedSkills>) {
    cache.retain(|_, entry| entry.last_used.elapsed() < SKILLS_CACHE_IDLE_TTL);
    while cache.len() > SKILLS_CACHE_MAX_ENTRIES {
        let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        cache.remove(&oldest);
    }
}

/// Per-key single-flight guards.
///
/// Held across the fetch so concurrent readers for one key coalesce onto a
/// single upstream request. Keyed rather than global so a slow upstream cannot
/// serialize refreshes for every other upstream — the failure the code-mode
/// refresh guard's single global mutex would have had here.
#[derive(Debug, Default)]
pub(super) struct SkillsFetchLocks {
    locks: Mutex<HashMap<SkillsCacheKey, Arc<Mutex<()>>>>,
}

impl SkillsFetchLocks {
    /// The guard for `key`, creating it if absent.
    pub(super) async fn guard_for(&self, key: &SkillsCacheKey) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().await;
        Arc::clone(locks.entry(key.clone()).or_default())
    }

    /// Drop guards nothing else references, so the map does not accumulate one
    /// entry per subject ever seen.
    pub(super) async fn prune(&self) {
        let mut locks = self.locks.lock().await;
        locks.retain(|_, guard| Arc::strong_count(guard) > 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(ttl_ms: Option<u64>) -> UpstreamSkills {
        UpstreamSkills {
            ttl_ms,
            ..Default::default()
        }
    }

    #[test]
    fn ttl_is_clamped_against_hostile_and_absent_values() {
        assert_eq!(clamp_ttl(None), SKILLS_TTL_DEFAULT);
        // A zero TTL would turn every read into an upstream fetch.
        assert_eq!(clamp_ttl(Some(0)), SKILLS_TTL_FLOOR);
        // A decade would pin a stale catalog past any operator's patience.
        assert_eq!(clamp_ttl(Some(u64::MAX)), SKILLS_TTL_CEILING);
        assert_eq!(clamp_ttl(Some(60_000)), Duration::from_secs(60));
    }

    #[test]
    fn a_fresh_entry_reports_fresh_and_ages() {
        let entry = CachedSkills::new(snapshot(Some(60_000)));
        assert!(entry.is_fresh());
        assert!(entry.age() < Duration::from_secs(1));
        assert!(!entry.refreshing);
    }

    #[test]
    fn a_zero_ttl_entry_still_gets_the_floor_not_instant_expiry() {
        // Even the most aggressive upstream hint cannot force a fetch per read.
        let entry = CachedSkills::new(snapshot(Some(0)));
        assert!(entry.is_fresh());
    }

    #[test]
    fn eviction_drops_the_least_recently_used_over_the_cap() {
        let mut cache: HashMap<SkillsCacheKey, CachedSkills> = HashMap::new();
        for i in 0..SKILLS_CACHE_MAX_ENTRIES + 10 {
            let mut entry = CachedSkills::new(snapshot(None));
            // Stagger last_used so the eviction order is deterministic.
            entry.last_used = Instant::now() - Duration::from_secs(1000 - i as u64);
            cache.insert((format!("upstream-{i}"), None), entry);
        }
        evict(&mut cache);
        assert_eq!(cache.len(), SKILLS_CACHE_MAX_ENTRIES);
        // The oldest keys went first.
        assert!(!cache.contains_key(&("upstream-0".to_string(), None)));
        assert!(cache.contains_key(&(format!("upstream-{}", SKILLS_CACHE_MAX_ENTRIES + 9), None)));
    }

    #[test]
    fn eviction_drops_idle_entries_regardless_of_cap() {
        let mut cache: HashMap<SkillsCacheKey, CachedSkills> = HashMap::new();
        let mut idle = CachedSkills::new(snapshot(None));
        idle.last_used = Instant::now() - SKILLS_CACHE_IDLE_TTL - Duration::from_secs(1);
        cache.insert(("idle".to_string(), None), idle);
        cache.insert(
            ("live".to_string(), None),
            CachedSkills::new(snapshot(None)),
        );

        evict(&mut cache);
        assert!(!cache.contains_key(&("idle".to_string(), None)));
        assert!(cache.contains_key(&("live".to_string(), None)));
    }

    #[tokio::test]
    async fn one_guard_per_key_and_unused_guards_are_pruned() {
        let locks = SkillsFetchLocks::default();
        let key = ("up".to_string(), None);
        let first = locks.guard_for(&key).await;
        let second = locks.guard_for(&key).await;
        assert!(Arc::ptr_eq(&first, &second), "one guard serves one key");

        let other = locks.guard_for(&("other".to_string(), None)).await;
        assert!(!Arc::ptr_eq(&first, &other), "distinct keys never share");

        drop(first);
        drop(second);
        drop(other);
        locks.prune().await;
        assert!(locks.locks.lock().await.is_empty());
    }

    #[tokio::test]
    async fn subject_is_part_of_the_key() {
        // Two authorization contexts must never share a cached catalog, whatever
        // cacheScope the upstream declared.
        let locks = SkillsFetchLocks::default();
        let anonymous = locks.guard_for(&("up".to_string(), None)).await;
        let alice = locks
            .guard_for(&("up".to_string(), Some("alice".to_string())))
            .await;
        assert!(!Arc::ptr_eq(&anonymous, &alice));
    }
}
