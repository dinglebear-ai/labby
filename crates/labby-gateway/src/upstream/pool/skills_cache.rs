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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use super::skills_list::UpstreamSkills;
use labby_runtime::skills::{ValidatedSkill, parse_skill_resource_uri};

/// Shortest refresh interval honored from an upstream's `ttlMs`.
pub(super) const SKILLS_TTL_FLOOR: Duration = Duration::from_secs(5);

/// Longest refresh interval honored from an upstream's `ttlMs`.
pub(super) const SKILLS_TTL_CEILING: Duration = Duration::from_hours(1);

/// Refresh interval used when an upstream publishes no `ttlMs`.
pub(super) const SKILLS_TTL_DEFAULT: Duration = Duration::from_mins(5);

/// Evict a cached catalog untouched for this long.
pub(super) const SKILLS_CACHE_IDLE_TTL: Duration = Duration::from_mins(30);

/// Subject-scoped catalog cap per upstream. Non-OAuth upstreams collapse all callers
/// onto one cache shard, while OAuth upstreams retain strict per-subject isolation.
/// A per-upstream cap prevents one busy identity population from evicting unrelated
/// upstream catalogs.
pub(super) const SKILLS_CACHE_MAX_SUBJECTS_PER_UPSTREAM: usize = 64;

/// Failed background refreshes are negatively cached so an unhealthy upstream
/// is not hammered on every downstream read of the stale snapshot.
pub(super) const SKILLS_REFRESH_BACKOFF_BASE: Duration = Duration::from_secs(5);
pub(super) const SKILLS_REFRESH_BACKOFF_MAX: Duration = Duration::from_mins(1);

/// Direct `skills/get` manifests retained per catalog shard.
///
/// They are intentionally separate from `UpstreamSkills`: successfully getting
/// an unlisted skill must not make it appear in a later discovery response.
#[derive(Debug, Clone)]
pub(super) struct CachedDirectSkill {
    pub(super) skill: ValidatedSkill,
    pub(super) owned_uris: BTreeSet<String>,
    expires_at: Instant,
    last_used: Instant,
}

impl CachedDirectSkill {
    pub(super) fn new(skill: ValidatedSkill) -> Self {
        let now = Instant::now();
        let owned_uris = std::iter::once(&skill.entry.uri)
            .chain(
                skill
                    .entry
                    .resources
                    .iter()
                    .flatten()
                    .map(|resource| &resource.uri),
            )
            .filter_map(|uri| parse_skill_resource_uri(uri).ok().map(|uri| uri.to_uri()))
            .collect();
        Self {
            skill,
            owned_uris,
            expires_at: now + SKILLS_TTL_DEFAULT,
            last_used: now,
        }
    }

    pub(super) fn is_fresh(&self) -> bool {
        Instant::now() < self.expires_at
    }

    pub(super) fn touch(&mut self) {
        self.last_used = Instant::now();
    }

    #[cfg(test)]
    pub(super) fn expire_now(&mut self) {
        self.expires_at = Instant::now();
    }
}

/// Cache key: one entry per upstream per authorization context.
pub(super) type SkillsCacheKey = (String, Option<String>);

/// A cached catalog snapshot plus the bookkeeping that governs its lifetime.
#[derive(Debug, Clone)]
pub(super) struct CachedSkills {
    pub(super) skills: Arc<UpstreamSkills>,
    pub(super) direct: BTreeMap<String, CachedDirectSkill>,
    pub(super) direct_resource_index: BTreeMap<String, String>,
    /// When this snapshot was fetched.
    fetched_at: Instant,
    /// When it stops being fresh, already clamped.
    expires_at: Instant,
    /// Last read, for idle eviction.
    last_used: Instant,
    /// Set while a background refresh is in flight, so a burst of readers
    /// spawns one refresh rather than one per reader.
    pub(super) refreshing: bool,
    /// Start of the current background refresh, for operator diagnostics.
    refresh_started_at: Option<Instant>,
    /// Earliest instant a failed background refresh may be retried.
    retry_after: Option<Instant>,
    /// Consecutive failed background refreshes used for bounded exponential backoff.
    refresh_failures: u32,
}

impl CachedSkills {
    pub(super) fn new(skills: UpstreamSkills) -> Self {
        let now = Instant::now();
        let ttl = clamp_ttl(skills.ttl_ms);
        Self {
            skills: Arc::new(skills),
            direct: BTreeMap::new(),
            direct_resource_index: BTreeMap::new(),
            fetched_at: now,
            expires_at: now + ttl,
            last_used: now,
            refreshing: false,
            refresh_started_at: None,
            retry_after: None,
            refresh_failures: 0,
        }
    }

    pub(super) fn is_fresh(&self) -> bool {
        Instant::now() < self.expires_at
    }

    /// Shallow snapshot for list/status readers. Direct-get manifests are a
    /// separate mutable side cache and must not be deep-cloned just to inspect
    /// the listed catalog.
    pub(super) fn read_snapshot(&self) -> Self {
        Self {
            skills: Arc::clone(&self.skills),
            direct: BTreeMap::new(),
            direct_resource_index: BTreeMap::new(),
            fetched_at: self.fetched_at,
            expires_at: self.expires_at,
            last_used: self.last_used,
            refreshing: self.refreshing,
            refresh_started_at: self.refresh_started_at,
            retry_after: self.retry_after,
            refresh_failures: self.refresh_failures,
        }
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

    pub(super) fn prune_direct(&mut self) {
        let stale = self
            .direct
            .iter()
            .filter(|(_, snapshot)| {
                !snapshot.is_fresh() || snapshot.last_used.elapsed() >= SKILLS_CACHE_IDLE_TTL
            })
            .map(|(owner, _)| owner.clone())
            .collect::<Vec<_>>();
        for owner in stale {
            self.remove_direct(&owner);
        }
    }

    pub(super) fn remove_direct(&mut self, owner: &str) -> Option<CachedDirectSkill> {
        let removed = self.direct.remove(owner)?;
        for uri in &removed.owned_uris {
            if self
                .direct_resource_index
                .get(uri)
                .is_some_and(|indexed_owner| indexed_owner == owner)
            {
                self.direct_resource_index.remove(uri);
            }
        }
        Some(removed)
    }

    pub(super) fn insert_direct(&mut self, owner: String, snapshot: CachedDirectSkill) {
        self.remove_direct(&owner);
        for uri in &snapshot.owned_uris {
            self.direct_resource_index
                .insert(uri.clone(), owner.clone());
        }
        self.direct.insert(owner, snapshot);
    }

    /// Mark a background refresh in flight if no refresh or failure cooldown is active.
    pub(super) fn begin_refresh(&mut self) -> bool {
        let now = Instant::now();
        if self.refreshing
            || self
                .retry_after
                .is_some_and(|retry_after| retry_after > now)
        {
            return false;
        }
        self.refreshing = true;
        self.refresh_started_at = Some(now);
        true
    }

    /// Clear refresh state after a failure and arm bounded exponential backoff.
    pub(super) fn fail_refresh(&mut self) {
        self.refreshing = false;
        self.refresh_started_at = None;
        self.refresh_failures = self.refresh_failures.saturating_add(1);
        let shift = self.refresh_failures.saturating_sub(1).min(6);
        let multiplier = 1_u32 << shift;
        let delay = SKILLS_REFRESH_BACKOFF_BASE
            .saturating_mul(multiplier)
            .min(SKILLS_REFRESH_BACKOFF_MAX);
        self.retry_after = Some(Instant::now() + delay);
    }

    pub(super) fn refresh_age(&self) -> Option<Duration> {
        self.refresh_started_at.map(|started| started.elapsed())
    }

    pub(super) fn retry_after(&self) -> Option<Duration> {
        self.retry_after
            .map(|retry_after| retry_after.saturating_duration_since(Instant::now()))
    }

    pub(super) fn retain_direct_from(&mut self, previous: &Self) {
        for (owner, snapshot) in &previous.direct {
            if snapshot.is_fresh()
                && !snapshot
                    .owned_uris
                    .iter()
                    .any(|uri| self.skills.resource_index.contains_key(uri))
            {
                self.insert_direct(owner.clone(), snapshot.clone());
            }
        }
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
pub(super) fn evict(cache: &mut HashMap<SkillsCacheKey, CachedSkills>) -> usize {
    let before = cache.len();
    cache.retain(|_, entry| entry.last_used.elapsed() < SKILLS_CACHE_IDLE_TTL);

    let mut by_upstream: HashMap<String, Vec<(SkillsCacheKey, Instant)>> = HashMap::new();
    for (key @ (upstream, _), entry) in cache.iter() {
        by_upstream
            .entry(upstream.clone())
            .or_default()
            .push((key.clone(), entry.last_used));
    }
    for mut entries in by_upstream.into_values() {
        if entries.len() <= SKILLS_CACHE_MAX_SUBJECTS_PER_UPSTREAM {
            continue;
        }
        entries.sort_unstable_by_key(|(_, last_used)| *last_used);
        let remove = entries.len() - SKILLS_CACHE_MAX_SUBJECTS_PER_UPSTREAM;
        for (key, _) in entries.into_iter().take(remove) {
            cache.remove(&key);
        }
    }
    before.saturating_sub(cache.len())
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
        assert_eq!(clamp_ttl(Some(60_000)), Duration::from_mins(1));
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
    fn eviction_caps_subjects_per_upstream_without_cross_upstream_thrashing() {
        let mut cache: HashMap<SkillsCacheKey, CachedSkills> = HashMap::new();
        for i in 0..SKILLS_CACHE_MAX_SUBJECTS_PER_UPSTREAM + 10 {
            let mut entry = CachedSkills::new(snapshot(None));
            entry.last_used = Instant::now()
                .checked_sub(Duration::from_secs(1000 - i as u64))
                .expect("staggered instant is within range");
            cache.insert(("busy".to_string(), Some(format!("subject-{i}"))), entry);
        }
        for i in 0..8 {
            cache.insert(
                (format!("quiet-{i}"), None),
                CachedSkills::new(snapshot(None)),
            );
        }

        evict(&mut cache);

        let busy = cache
            .keys()
            .filter(|(upstream, _)| upstream == "busy")
            .count();
        assert_eq!(busy, SKILLS_CACHE_MAX_SUBJECTS_PER_UPSTREAM);
        assert!(!cache.contains_key(&("busy".to_string(), Some("subject-0".to_string()))));
        for i in 0..8 {
            assert!(cache.contains_key(&(format!("quiet-{i}"), None)));
        }
    }

    #[test]
    fn failed_refresh_enters_bounded_negative_backoff() {
        let mut entry = CachedSkills::new(snapshot(Some(60_000)));
        assert!(entry.begin_refresh());
        entry.fail_refresh();
        let first = entry.retry_after().expect("retry delay");
        assert!(first > Duration::ZERO);
        assert!(first <= SKILLS_REFRESH_BACKOFF_BASE);
        assert!(
            !entry.begin_refresh(),
            "backoff suppresses an immediate retry"
        );

        entry.retry_after = Some(Instant::now());
        assert!(entry.begin_refresh());
        entry.fail_refresh();
        let second = entry.retry_after().expect("second retry delay");
        assert!(second >= first);
        assert!(second <= SKILLS_REFRESH_BACKOFF_MAX);
    }

    #[test]
    fn eviction_drops_idle_entries_regardless_of_cap() {
        let mut cache: HashMap<SkillsCacheKey, CachedSkills> = HashMap::new();
        let mut idle = CachedSkills::new(snapshot(None));
        idle.last_used = Instant::now()
            .checked_sub(SKILLS_CACHE_IDLE_TTL + Duration::from_secs(1))
            .expect("idle instant is within range");
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
