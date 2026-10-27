//! Count-, byte-, and age-bounded early relay advisories. Expiry is checked on
//! admission/drain; a silent peer retains only the capped buffer until drop.
use serde::Serialize;
use std::{collections::HashMap, hash::Hash, time::Duration};
use tokio::time::Instant;

const BYTE_LIMIT: usize = 64 * 1024;
const TTL: Duration = Duration::from_secs(30);

struct Entry<V> {
    values: Vec<V>,
    received: Instant,
    bytes: usize,
}

pub(super) struct PendingAdvisories<K, V> {
    entries: HashMap<K, Entry<V>>,
    bytes: usize,
    last_diagnostic: Option<Instant>,
}

impl<K, V> Default for PendingAdvisories<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            bytes: 0,
            last_diagnostic: None,
        }
    }
}

impl<K: Eq + Hash + Serialize, V: Serialize> PendingAdvisories<K, V> {
    fn expire(&mut self) {
        let now = Instant::now();
        let mut expired = false;
        self.entries.retain(|_, entry| {
            if now.duration_since(entry.received) >= TTL {
                self.bytes -= entry.bytes;
                expired = true;
                false
            } else {
                true
            }
        });
        if expired {
            self.discard();
        }
    }

    fn discard(&mut self) {
        let now = Instant::now();
        if self
            .last_diagnostic
            .is_none_or(|last| now.duration_since(last) >= TTL)
        {
            tracing::debug!("discarded early relay advisory at retention limit or expiry");
            self.last_diagnostic = Some(now);
        }
    }

    pub(super) fn admit(&mut self, key: K, value: V, ids: usize, per_id: usize) {
        self.expire();
        let existing = self.entries.get(&key);
        if existing.is_none() && self.entries.len() >= ids
            || existing.is_some_and(|entry| entry.values.len() >= per_id)
        {
            self.discard();
            return;
        }
        // Serialized key plus colon/brackets/comma, root braces, and each
        // serialized event plus comma. Count caps independently bound metadata.
        let remaining = BYTE_LIMIT.saturating_sub(self.bytes + 2);
        let key_bytes = if existing.is_none() {
            super::task_pending::json_size(&key, remaining).and_then(|size| size.checked_add(4))
        } else {
            Some(0)
        };
        let size = key_bytes.and_then(|key_bytes| {
            let cap = remaining.checked_sub(key_bytes + 1)?;
            let event_bytes = super::task_pending::json_size(&value, cap)?;
            Some(key_bytes + event_bytes + 1)
        });
        let Some(size) = size else {
            self.discard();
            return;
        };
        let entry = self.entries.entry(key).or_insert_with(|| Entry {
            values: Vec::new(),
            received: Instant::now(),
            bytes: 0,
        });
        entry.values.push(value);
        entry.bytes += size;
        self.bytes += size;
    }

    pub(super) fn remove(&mut self, key: &K) -> Option<Vec<V>> {
        self.expire();
        let entry = self.entries.remove(key)?;
        self.bytes -= entry.bytes;
        Some(entry.values)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
    #[cfg(test)]
    pub(super) fn get(&self, key: &K) -> Option<&Vec<V>> {
        self.entries.get(key).map(|entry| &entry.values)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exact_byte_boundary_and_drain_release() {
        let mut pending = PendingAdvisories::<String, String>::default();
        // {"k":["<payload>",],} plus conservative commas is payload + 12 bytes.
        pending.admit("k".into(), "x".repeat(65_524), 256, 16);
        assert_eq!(pending.bytes, 65_534);
        assert_eq!(pending.len(), 1);
        pending.remove(&"k".into()).unwrap();
        assert_eq!(pending.bytes, 0);
        pending.admit("k".into(), "x".repeat(65_525), 256, 16);
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn escaped_utf8_keys_count_toward_budget_and_expiry_releases_it() {
        let mut pending = PendingAdvisories::<String, String>::default();
        // JSON key "é\\n" occupies 6 bytes; its retained framing occupies 4.
        pending.admit("é\n".into(), "x".repeat(65_521), 256, 16);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending.bytes, 65_534);
        tokio::time::advance(TTL).await;
        pending.admit("é\n".into(), "x".repeat(65_522), 256, 16);
        assert_eq!(pending.len(), 0);
        assert_eq!(pending.bytes, 0);
        pending.admit("healthy".into(), "message".into(), 256, 16);
        assert_eq!(
            pending.remove(&"healthy".into()),
            Some(vec!["message".into()])
        );
        assert_eq!(pending.bytes, 0);
    }
}
