use super::CodeModeSurface;
use crate::error::ToolError;
use crate::execute::source_capability_within_lookup;
use std::collections::VecDeque;

/// Ephemeral execution source retained so an administrator can promote it into a snippet.
#[derive(Debug, Clone)]
pub struct CodeModeExecutionSource {
    /// Execution identifier used as the promotion lookup key.
    pub execution_id: String,
    /// Source creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Redacted/stable actor key that owns this source.
    pub actor_key: Option<String>,
    /// Whether the original caller had administrative scope.
    pub is_admin: bool,
    /// Route scope in which the source executed.
    pub route_scope: String,
    /// Product surface that initiated the execution.
    pub surface: CodeModeSurface,
    /// Capability-scope fingerprint captured at execution time.
    pub capability_filter_fingerprint: String,
    /// Original JavaScript source eligible for promotion.
    pub code: String,
}

/// Authorization/scope context used when resolving an execution source for promotion.
#[derive(Debug, Clone)]
pub struct CodeModeSourceLookup {
    /// Actor key that must own the source.
    pub actor_key: Option<String>,
    /// Whether the resolving caller has administrative scope.
    pub is_admin: bool,
    /// Current route scope.
    pub route_scope: String,
    /// Current capability-scope fingerprint.
    pub capability_filter_fingerprint: String,
}

/// Bounded in-memory store of ephemeral Code Mode source suitable for snippet promotion.
#[derive(Debug, Clone)]
pub struct CodeModeSourceStore {
    entries: VecDeque<CodeModeExecutionSource>,
    running_bytes: usize,
    max_entries: usize,
    max_bytes: usize,
}

impl Default for CodeModeSourceStore {
    fn default() -> Self {
        Self::new(50, 512 * 1024)
    }
}

impl CodeModeSourceStore {
    /// Create a source store with minimum-safe entry and byte limits.
    #[must_use]
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            running_bytes: 0,
            max_entries: max_entries.max(1),
            max_bytes: max_bytes.max(1024),
        }
    }

    /// Retain a source when it fits the byte budget, evicting oldest entries as needed.
    pub fn push(&mut self, source: CodeModeExecutionSource) {
        let bytes = source.code.len();
        if bytes > self.max_bytes {
            return;
        }
        self.running_bytes = self.running_bytes.saturating_add(bytes);
        self.entries.push_back(source);
        while self.entries.len() > self.max_entries || self.running_bytes > self.max_bytes {
            if let Some(evicted) = self.entries.pop_front() {
                self.running_bytes = self.running_bytes.saturating_sub(evicted.code.len());
            } else {
                break;
            }
        }
    }

    /// Resolve a promotion source after enforcing admin, route, capability, and actor ownership.
    #[must_use]
    pub fn resolve(
        &self,
        execution_id: &str,
        lookup: &CodeModeSourceLookup,
    ) -> Result<CodeModeExecutionSource, ToolError> {
        let Some(source) = self
            .entries
            .iter()
            .find(|entry| entry.execution_id == execution_id)
            .cloned()
        else {
            return Err(ToolError::Sdk {
                sdk_kind: "unknown_execution".to_string(),
                message: "Code Mode promotion source is ephemeral and may have expired, been evicted, lived in another host process, or disappeared after restart".to_string(),
            });
        };
        if !lookup.is_admin {
            return Err(ToolError::Forbidden {
                message: "promoting Code Mode executions requires lab:admin".to_string(),
                required_scopes: vec!["lab:admin".to_string()],
            });
        }
        if source.route_scope != lookup.route_scope
            || !source_capability_within_lookup(
                &source.capability_filter_fingerprint,
                &lookup.capability_filter_fingerprint,
            )
        {
            return Err(ToolError::Forbidden {
                message: "Code Mode promotion source is outside this route or capability scope"
                    .to_string(),
                required_scopes: vec!["lab:admin".to_string()],
            });
        }
        if source.actor_key != lookup.actor_key {
            return Err(ToolError::Forbidden {
                message: "Code Mode promotion source belongs to a different actor".to_string(),
                required_scopes: vec!["lab:admin".to_string()],
            });
        }
        Ok(source)
    }
}
