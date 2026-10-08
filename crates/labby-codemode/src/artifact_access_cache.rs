//! Bounded caches of verified bytes and short-lived owner-indexed listing snapshots.
//! Neither cache is authority: callers revalidate metadata ownership and paths.
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

const MAX_CONTENT_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONTENT_ENTRIES: usize = 16;
const MAX_LIST_STORES: usize = 8;
const LIST_TTL: Duration = Duration::from_secs(1);
static LIST_GENERATION: AtomicU64 = AtomicU64::new(0);

pub(super) fn listing_generation() -> u64 {
    LIST_GENERATION.load(Ordering::Acquire)
}

#[cfg(test)]
fn verifications() -> &'static Mutex<BTreeMap<PathBuf, usize>> {
    static COUNTS: OnceLock<Mutex<BTreeMap<PathBuf, usize>>> = OnceLock::new();
    COUNTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
pub(super) fn record_verification(path: &Path) {
    *verifications()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(path.to_owned())
        .or_default() += 1;
}

#[cfg(all(test, unix))]
pub(super) fn verification_count(path: &Path) -> usize {
    verifications()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(path)
        .copied()
        .unwrap_or(0)
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct FileIdentity {
    #[cfg(unix)]
    fields: (u64, u64, u64, i64, i64, i64, i64),
}

impl FileIdentity {
    pub(super) fn from_metadata(metadata: &std::fs::Metadata) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Self {
                fields: (
                    metadata.dev(),
                    metadata.ino(),
                    metadata.len(),
                    metadata.mtime(),
                    metadata.mtime_nsec(),
                    metadata.ctime(),
                    metadata.ctime_nsec(),
                ),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            // Modification time alone cannot reliably detect same-size replacement.
            None
        }
    }
}

struct ContentEntry {
    path: PathBuf,
    digest: String,
    identity: FileIdentity,
    content: Arc<str>,
}

fn content_cache() -> &'static Mutex<VecDeque<ContentEntry>> {
    static CACHE: OnceLock<Mutex<VecDeque<ContentEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

pub(super) fn content(path: &Path, digest: &str, identity: &FileIdentity) -> Option<Arc<str>> {
    content_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|entry| entry.path == path && entry.digest == digest && entry.identity == *identity)
        .map(|entry| Arc::clone(&entry.content))
}

pub(super) fn remember_content(
    path: PathBuf,
    digest: String,
    identity: FileIdentity,
    content: Arc<str>,
) {
    if content.len() > MAX_CONTENT_BYTES {
        return;
    }
    let mut cache = content_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    cache.retain(|entry| entry.path != path);
    while cache.len() >= MAX_CONTENT_ENTRIES
        || cache.iter().map(|entry| entry.content.len()).sum::<usize>() + content.len()
            > MAX_CONTENT_BYTES
    {
        cache.pop_front();
    }
    cache.push_back(ContentEntry {
        path,
        digest,
        identity,
        content,
    });
}

#[derive(Clone)]
pub(super) struct Listing {
    pub(super) owners: BTreeMap<String, Vec<String>>,
    pub(super) incomplete: bool,
}

struct ListingEntry {
    store: PathBuf,
    at: Instant,
    listing: Arc<Listing>,
}

fn listing_cache() -> &'static Mutex<VecDeque<ListingEntry>> {
    static CACHE: OnceLock<Mutex<VecDeque<ListingEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

pub(super) fn listing(store: &Path) -> Option<Arc<Listing>> {
    let mut cache = listing_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    cache.retain(|entry| entry.at.elapsed() < LIST_TTL);
    cache
        .iter()
        .find(|entry| entry.store == store)
        .map(|entry| Arc::clone(&entry.listing))
}

pub(super) fn invalidate_listing(store: &Path) {
    let mut cache = listing_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    // Publish invalidation under the same lock as snapshot publication, fencing
    // a scan that began before an enrollment completed.
    LIST_GENERATION.fetch_add(1, Ordering::Release);
    cache.retain(|entry| entry.store != store);
}

pub(super) fn remember_listing(store: &Path, listing: Listing, generation: u64) -> Arc<Listing> {
    let listing = Arc::new(listing);
    let mut cache = listing_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if listing_generation() != generation {
        return listing;
    }
    cache.retain(|entry| entry.store != store);
    while cache.len() >= MAX_LIST_STORES {
        cache.pop_front();
    }
    cache.push_back(ListingEntry {
        store: store.to_owned(),
        at: Instant::now(),
        listing: Arc::clone(&listing),
    });
    listing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_invalidation_fences_an_older_scan() {
        let temp = tempfile::tempdir().expect("store");
        let generation = listing_generation();
        invalidate_listing(temp.path());
        remember_listing(
            temp.path(),
            Listing {
                owners: BTreeMap::new(),
                incomplete: false,
            },
            generation,
        );
        assert!(
            listing(temp.path()).is_none(),
            "a stale scan cannot republish after enrollment"
        );
    }
}
