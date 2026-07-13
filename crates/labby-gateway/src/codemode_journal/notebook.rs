//! Pure projection of the durable step journal into a read-only notebook:
//! one cell per `codemode.step` boundary (plus an optional prologue cell for
//! calls that ran before the first step), with tool calls attributed to the
//! step-cell whose runner `seq` span contains the call. No DB access — operates
//! on already-loaded slices. Read/replay-only: never gates a run.

use labby_codemode::CodeModeExecutedCall;
use serde::Serialize;

use super::StepJournalRow;

/// A compact per-call summary shown in a notebook cell.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CallSummary {
    pub id: String,
    pub ok: bool,
    pub elapsed_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<String>,
}

impl From<&CodeModeExecutedCall> for CallSummary {
    fn from(call: &CodeModeExecutedCall) -> Self {
        Self {
            id: call.id.clone(),
            ok: call.ok,
            elapsed_ms: call.elapsed_ms,
            error_kind: call.error_kind.clone(),
        }
    }
}

/// One notebook cell. A prologue cell (calls before the first step) has
/// `ordinal: None` and `name: None`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotebookCell {
    pub ordinal: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    pub calls: Vec<CallSummary>,
    pub elapsed_ms: u128,
    /// v1: every journaled cell reflects a step that executed (replay is v2).
    pub executed: bool,
}

/// A projected notebook: an ordered list of cells and whether the projection
/// was capped (by cell count or byte budget).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Notebook {
    pub cells: Vec<NotebookCell>,
    pub truncated: bool,
}

/// Project journal rows + executed calls into a notebook.
///
/// - Rows are sorted by `step_ordinal`; one cell per row (`executed: true`).
/// - A leading prologue cell (`ordinal: None`) collects calls whose `seq`
///   precedes the first step's `seq_base`.
/// - Each call attaches to the last cell whose `seq_base <= call.seq`.
/// - Cells are capped at `max_cells`; the running serialized-name+value byte
///   total is capped at `max_bytes`. Either cap sets `truncated = true`.
#[must_use]
pub fn project_notebook(
    rows: &[StepJournalRow],
    calls: &[(u64, CodeModeExecutedCall)],
    max_cells: usize,
    max_bytes: usize,
) -> Notebook {
    let mut sorted: Vec<&StepJournalRow> = rows.iter().collect();
    sorted.sort_by_key(|r| r.step_ordinal);

    let first_seq_base = sorted.first().map(|r| r.seq_base);
    let needs_prologue = match first_seq_base {
        Some(fsb) => calls.iter().any(|(seq, _)| *seq < fsb),
        None => !calls.is_empty(),
    };

    let mut cells: Vec<NotebookCell> = Vec::new();
    let mut truncated = false;
    let mut bytes: usize = 0;

    if needs_prologue {
        cells.push(NotebookCell {
            ordinal: None,
            name: None,
            value: None,
            calls: Vec::new(),
            elapsed_ms: 0,
            executed: true,
        });
    }
    let prologue_idx = needs_prologue.then_some(0usize);

    // (seq_base, cell_index) for each step cell, in build order, for attribution.
    let mut step_spans: Vec<(u64, usize)> = Vec::new();
    for row in &sorted {
        if cells.len() >= max_cells {
            truncated = true;
            break;
        }
        bytes = bytes
            .saturating_add(row.name.len())
            .saturating_add(row.value.len());
        if bytes > max_bytes && !cells.is_empty() {
            truncated = true;
            break;
        }
        let idx = cells.len();
        cells.push(NotebookCell {
            ordinal: Some(row.step_ordinal),
            name: Some(row.name.clone()),
            value: Some(row.value.clone()),
            calls: Vec::new(),
            elapsed_ms: row.elapsed_ms,
            executed: true,
        });
        step_spans.push((row.seq_base, idx));
    }

    // Attribute each call to the last step cell whose seq_base <= call.seq,
    // else to the prologue (calls before the first step or when there are none).
    for (seq, call) in calls {
        let target = step_spans
            .iter()
            .rev()
            .find(|(seq_base, _)| *seq_base <= *seq)
            .map(|(_, idx)| *idx)
            .or(prologue_idx);
        if let Some(idx) = target {
            cells[idx].calls.push(CallSummary::from(call));
        }
        // A call with no cell and no prologue (only possible if a later step
        // cell was truncated away) is dropped from the projection; `truncated`
        // already flags the incompleteness.
    }

    Notebook { cells, truncated }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ord: u64, name: &str, seq_base: u64) -> StepJournalRow {
        StepJournalRow {
            execution_id: "e1".into(),
            step_ordinal: ord,
            seq_base,
            name: name.into(),
            value: "\"v\"".into(),
            ok: true,
            elapsed_ms: 1,
            recorded_at: 0,
            actor_key: None,
            route_scope: "default".into(),
            capability_filter_fingerprint: None,
            replayed_from: None,
        }
    }

    fn call(seq: u64, id: &str) -> (u64, CodeModeExecutedCall) {
        (
            seq,
            CodeModeExecutedCall {
                id: id.into(),
                ok: true,
                elapsed_ms: 1,
                start_ms: None,
                params: None,
                error_kind: None,
                ui: None,
            },
        )
    }

    #[test]
    fn projects_calls_into_step_cells_by_seq_span() {
        let rows = vec![row(0, "a", 5), row(1, "b", 8)];
        let calls = vec![call(6, "up::x"), call(9, "up::y"), call(2, "up::pre")];
        let nb = project_notebook(&rows, &calls, 100, 1_000_000);
        assert_eq!(nb.cells[0].ordinal, None); // prologue holds seq-2 call
        assert_eq!(nb.cells[0].calls.len(), 1);
        assert_eq!(nb.cells[1].name.as_deref(), Some("a"));
        assert_eq!(nb.cells[1].calls.len(), 1);
        assert_eq!(nb.cells[2].name.as_deref(), Some("b"));
        assert_eq!(nb.cells[2].calls.len(), 1);
        assert!(nb.cells.iter().all(|c| c.executed));
        assert!(!nb.truncated);
    }

    #[test]
    fn no_prologue_when_all_calls_are_within_step_spans() {
        let rows = vec![row(0, "a", 5)];
        let calls = vec![call(6, "up::x")];
        let nb = project_notebook(&rows, &calls, 100, 1_000_000);
        assert_eq!(nb.cells.len(), 1);
        assert_eq!(nb.cells[0].name.as_deref(), Some("a"));
        assert_eq!(nb.cells[0].calls.len(), 1);
    }

    #[test]
    fn caps_cell_count() {
        let many = (0..500).map(|i| row(i, "s", i * 2)).collect::<Vec<_>>();
        let nb = project_notebook(&many, &[], 50, 1_000_000);
        assert!(nb.cells.len() <= 50 && nb.truncated);
    }

    #[test]
    fn caps_by_byte_budget() {
        let rows = (0..10)
            .map(|i| row(i, &"n".repeat(100), i))
            .collect::<Vec<_>>();
        let nb = project_notebook(&rows, &[], 100, 250);
        assert!(nb.truncated);
        assert!(nb.cells.len() < 10);
    }

    #[test]
    fn notebook_serializes() {
        let rows = vec![row(0, "a", 1)];
        let nb = project_notebook(&rows, &[], 10, 1_000_000);
        let json = serde_json::to_string(&nb).unwrap();
        assert!(json.contains("\"cells\""));
        assert!(json.contains("\"truncated\""));
    }
}
