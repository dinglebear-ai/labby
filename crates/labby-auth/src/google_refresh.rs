//! Process-wide single-flight coordination for central Google credentials.

use std::sync::{Arc, OnceLock};

use dashmap::DashMap;
use tokio::sync::Mutex;

static GOOGLE_PROVIDER_REFRESH_LOCKS: OnceLock<DashMap<String, Arc<Mutex<()>>>> = OnceLock::new();

/// Return the process-wide mutex for one stable Google provider subject.
///
/// Inbound Labby token rotation, outbound Google MCP refresh, status probes, and
/// explicit revocation all use this same lock so one central refresh credential
/// is never refreshed or deleted concurrently by separate product surfaces.
pub(crate) fn lock(subject: &str) -> Arc<Mutex<()>> {
    GOOGLE_PROVIDER_REFRESH_LOCKS
        .get_or_init(DashMap::new)
        .entry(subject.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::lock;

    #[test]
    fn lock_is_shared_per_google_subject() {
        let left = lock("google-subject-lock-test");
        let right = lock("google-subject-lock-test");
        let other = lock("different-google-subject-lock-test");

        assert!(Arc::ptr_eq(&left, &right));
        assert!(!Arc::ptr_eq(&left, &other));
    }
}
