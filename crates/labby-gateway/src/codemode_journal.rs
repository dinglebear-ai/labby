//! Append-only durable journal of `codemode.step` boundaries. Read/replay-only:
//! this store never gates or pauses a run (see docs/dev/CODE_MODE.md). Owner-
//! identity columns (`actor_key`/`route_scope`/`capability_filter_fingerprint`)
//! and `replayed_from` are persisted for the v2 replay-auth path (epic
//! lab-5dtw9) even though v1 never reads them.

pub mod notebook;
pub mod store;

pub use notebook::{CallSummary, Notebook, NotebookCell, project_notebook};
pub use store::{StepJournalStore, redact_journal_text};

/// One persisted `codemode.step` boundary. `value` is redacted, bounded JSON text.
#[derive(Debug, Clone, PartialEq)]
pub struct StepJournalRow {
    pub execution_id: String,
    pub step_ordinal: u64,
    /// The `step_begin` runner seq for this step, used for call-to-cell
    /// attribution in the notebook projection (not a replay cursor ordinate).
    pub seq_base: u64,
    pub name: String,
    pub value: String,
    pub ok: bool,
    pub elapsed_ms: u128,
    pub recorded_at: i64,
    pub actor_key: Option<String>,
    pub route_scope: String,
    pub capability_filter_fingerprint: Option<String>,
    pub replayed_from: Option<String>,
}
