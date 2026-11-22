use crate::trace::{CodeModeExecutedCall, CodeModeExecutedCallSchema};
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::VecDeque;

/// Kind of operation recorded in the bounded Code Mode history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodeModeHistoryKind {
    /// JavaScript execution entry.
    Execute,
}

/// Bounded observability record for one Code Mode operation.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct CodeModeHistoryEntry {
    /// Stable execution identifier, when one was assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<String>,
    /// Monotonic sequence number assigned by the in-memory history.
    pub seq: u64,
    /// Route-scope label under which the operation executed.
    pub route_scope: String,
    /// History operation class.
    pub kind: CodeModeHistoryKind,
    /// Whether the operation completed successfully.
    pub ok: bool,
    /// Operation duration in milliseconds.
    pub elapsed_ms: u128,
    /// Estimated input token count, when measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<usize>,
    /// Estimated output token count, when measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<usize>,
    /// Stable failure kind, when the operation failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
    /// Bounded tool-call metadata. Retained history excludes upstream response content.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[schemars(with = "Vec<CodeModeExecutedCallSchema>")]
    pub calls: Vec<CodeModeExecutedCall>,
    /// Search result count for discovery operations that populate this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_count: Option<usize>,
}

/// In-memory, entry-count- and byte-bounded Code Mode operation history.
#[derive(Debug, Clone)]
pub struct CodeModeHistory {
    entries: VecDeque<([u8; 32], CodeModeHistoryEntry)>,
    /// Accumulated serialized byte estimate for all entries in `entries`.
    ///
    /// Maintained as a running total to avoid re-serializing the entire deque
    /// on every push or eviction. Updated when entries are added or removed.
    /// The estimate uses the serialized JSON size of each individual entry; the
    /// VecDeque framing bytes (brackets, commas) are a constant ~2 bytes and are
    /// ignored — acceptable given the ~1 KB min entry size and 256 KB default cap.
    running_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
    next_seq: u64,
}

impl Default for CodeModeHistory {
    fn default() -> Self {
        Self::new(50, 256 * 1024)
    }
}

impl CodeModeHistory {
    /// Create a bounded history with minimum-safe entry and byte limits.
    #[must_use]
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            running_bytes: 0,
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1024),
            next_seq: 1,
        }
    }

    /// Append metadata to shared history, assigning a sequence number and evicting old entries.
    pub fn push(&mut self, owner: [u8; 32], mut entry: CodeModeHistoryEntry) {
        // Keep upstream response blocks on the execution trace, outside retained history.
        for call in &mut entry.calls {
            call.upstream_content = None;
        }
        entry.seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        let entry_bytes = entry_serialized_size(&entry);
        self.entries.push_back((owner, entry));
        self.running_bytes = self.running_bytes.saturating_add(entry_bytes);
        self.trim();
    }

    /// Return the retained history entries in oldest-to-newest order.
    #[must_use]
    pub fn snapshot(&self) -> Vec<CodeModeHistoryEntry> {
        self.entries
            .iter()
            .map(|(_, entry)| entry.clone())
            .collect()
    }

    /// Return history for the requesting caller and route authority.
    #[must_use]
    pub fn snapshot_for_owner(&self, owner: &[u8; 32]) -> Vec<CodeModeHistoryEntry> {
        self.entries
            .iter()
            .filter(|(stored, _)| stored == owner)
            .map(|(_, entry)| entry.clone())
            .collect()
    }

    fn trim(&mut self) {
        while self.entries.len() > self.max_entries {
            if let Some((_, evicted)) = self.entries.pop_front() {
                self.running_bytes = self
                    .running_bytes
                    .saturating_sub(entry_serialized_size(&evicted));
            }
        }
        while self.running_bytes > self.max_bytes && self.entries.len() > 1 {
            if let Some((_, evicted)) = self.entries.pop_front() {
                self.running_bytes = self
                    .running_bytes
                    .saturating_sub(entry_serialized_size(&evicted));
            }
        }
        if self.running_bytes > self.max_bytes {
            if let Some((owner, entry)) = self.entries.pop_back() {
                let old_bytes = entry_serialized_size(&entry);
                let sentinel = Self::oversized_entry_sentinel(entry.seq, entry.kind);
                let sentinel_bytes = entry_serialized_size(&sentinel);
                self.running_bytes = self
                    .running_bytes
                    .saturating_sub(old_bytes)
                    .saturating_add(sentinel_bytes);
                self.entries.push_back((owner, sentinel));
            }
        }
    }

    fn oversized_entry_sentinel(seq: u64, kind: CodeModeHistoryKind) -> CodeModeHistoryEntry {
        CodeModeHistoryEntry {
            execution_id: None,
            seq,
            route_scope: "root".to_string(),
            kind,
            ok: false,
            elapsed_ms: 0,
            input_tokens: None,
            output_tokens: None,
            error_kind: Some("history_entry_too_large".to_string()),
            calls: Vec::new(),
            match_count: None,
        }
    }
}

/// Serialized byte size of a single history entry.
///
/// Used to maintain the `running_bytes` counter without re-serializing the
/// entire deque on every mutation. Falls back to `usize::MAX` on a (very
/// unlikely) serialization error so the history is conservatively treated as
/// over-budget rather than silently growing without bound.
fn entry_serialized_size(entry: &CodeModeHistoryEntry) -> usize {
    serde_json::to_vec(entry)
        .map(|bytes| bytes.len() + 32)
        .unwrap_or(usize::MAX / 2)
}
