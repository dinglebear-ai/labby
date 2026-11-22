//! Bounded caches of verified bytes and short-lived owner-indexed listing snapshots.
//! Neither cache is authority: callers revalidate metadata ownership and paths.
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

const MAX_CONTENT_BYTES: usize = 32 * 1024 * 1024;
const MAX_CONTENT_ENTRIES: usize = 16;
const MAX_LIST_STORES: usize = 8;
const LIST_TTL: Duration = Duration::from_secs(30);
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
pub(crate) struct FileIdentity {
    #[cfg(unix)]
    fields: (u64, u64, u64, i64, i64, i64, i64),
}

impl FileIdentity {
    pub(crate) fn from_metadata(metadata: &std::fs::Metadata) -> Option<Self> {
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
    listing: Arc<tokio::sync::OnceCell<Listing>>,
}

fn listing_cache() -> &'static Mutex<VecDeque<ListingEntry>> {
    static CACHE: OnceLock<Mutex<VecDeque<ListingEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(VecDeque::new()))
}

pub(super) fn listing(store: &Path) -> Arc<tokio::sync::OnceCell<Listing>> {
    let mut cache = listing_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    cache.retain(|entry| entry.at.elapsed() < LIST_TTL);
    if let Some(entry) = cache.iter().find(|entry| entry.store == store) {
        return Arc::clone(&entry.listing);
    }
    while cache.len() >= MAX_LIST_STORES {
        cache.pop_front();
    }
    let listing = Arc::new(tokio::sync::OnceCell::new());
    cache.push_back(ListingEntry {
        store: store.to_owned(),
        at: Instant::now(),
        listing: Arc::clone(&listing),
    });
    listing
}

pub(super) fn invalidate_listing(store: &Path) {
    listing_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|entry| entry.store != store);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_invalidation_fences_an_older_scan() {
        let temp = tempfile::tempdir().expect("store");
        let old = listing(temp.path());
        assert!(Arc::ptr_eq(&old, &listing(temp.path())));
        invalidate_listing(temp.path());
        assert!(
            old.set(Listing {
                owners: BTreeMap::new(),
                incomplete: false
            })
            .is_ok()
        );
        assert!(listing(temp.path()).get().is_none());
    }
}
