//! Bounded snippet execution metadata retained in the existing journal database.
//! Raw source, parameters, logs and results are deliberately never stored here.
use labby_runtime::error::ToolError;
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::StepJournalStore;
mod history;
pub use history::SnippetReceiptHistory;

const MAX_RECEIPT_BYTES: usize = 64 * 1024;
const MAX_RECEIPTS: i64 = 1000;
const MAX_OWNER_RECEIPTS: i64 = 100;
const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS snippet_receipts (
 execution_id TEXT PRIMARY KEY NOT NULL,
 owner_key TEXT NOT NULL,
 route_scope TEXT NOT NULL,
 capability_fingerprint TEXT NOT NULL,
 created_at_ms INTEGER NOT NULL,
 receipt_json TEXT NOT NULL
); CREATE INDEX IF NOT EXISTS snippet_receipts_owner ON snippet_receipts(owner_key, route_scope, created_at_ms);";

/// Identity used for receipt access. No administrative cross-owner bypass.
#[derive(Debug, Clone)]
pub struct SnippetReceiptOwner {
    pub owner_key: String,
    pub route_scope: String,
    pub capability_fingerprint: String,
}

/// One call's identity, argument digest and timing, without sensitive payloads.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnippetReceiptCall {
    pub tool: String,
    pub params_digest: Option<String>,
    pub ok: bool,
    pub elapsed_ms: u128,
    pub error_kind: Option<String>,
}

/// Artifact reference; lookup of its bytes remains owned by the artifact store.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnippetReceiptArtifact {
    pub path: String,
    pub sha256: String,
    pub bytes: usize,
    pub content_type: String,
    /// Opaque broker-minted storage identity; absent in older receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_run_id: Option<String>,
}

/// Reproducibility metadata. `started` means completion was not recorded.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SnippetExecutionReceipt {
    pub execution_id: String,
    pub snippet_name: String,
    pub snippet_digest: String,
    pub input_digest: String,
    pub effective_scope_fingerprint: String,
    pub runtime_version: String,
    /// Bounded, caller-visible descriptor digests captured at execution start.
    /// Missing evidence (including older receipts) remains explicitly unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_schema_digests: Option<std::collections::BTreeMap<String, Option<String>>>,
    pub surface: String,
    pub created_at_ms: i64,
    pub elapsed_ms: u128,
    pub status: String,
    pub error_kind: Option<String>,
    pub result_digest: Option<String>,
    pub result_bytes: Option<usize>,
    pub calls: Vec<SnippetReceiptCall>,
    pub tool_calls: usize,
    pub omitted_calls: usize,
    pub artifacts: Vec<SnippetReceiptArtifact>,
}

pub(super) fn initialize(conn: &Connection) -> Result<(), ToolError> {
    conn.execute_batch(SCHEMA).map_err(store_error)?;
    let mut statement = conn
        .prepare("PRAGMA table_info(snippet_receipts)")
        .map_err(store_error)?;
    let columns = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(store_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(store_error)?;
    let expected = [
        ("execution_id", "TEXT", 1, 1),
        ("owner_key", "TEXT", 1, 0),
        ("route_scope", "TEXT", 1, 0),
        ("capability_fingerprint", "TEXT", 1, 0),
        ("created_at_ms", "INTEGER", 1, 0),
        ("receipt_json", "TEXT", 1, 0),
    ];
    if columns.len() != expected.len()
        || !columns
            .iter()
            .zip(expected)
            .all(|(a, b)| a.0 == b.0 && a.1.eq_ignore_ascii_case(b.1) && a.2 == b.2 && a.3 == b.3)
    {
        return Err(ToolError::internal_message(
            "snippet receipt journal schema mismatch",
        ));
    }
    Ok(())
}

fn store_error(error: rusqlite::Error) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "journal_store_error".into(),
        message: format!("snippet receipt storage failed: {error}"),
    }
}
fn unknown_receipt() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "unknown_execution".into(),
        message: "snippet receipt is unavailable in this owner/route scope, expired, or evicted"
            .into(),
    }
}

impl StepJournalStore {
    /// Persist one bounded receipt, with transactional global and owner quotas.
    pub async fn record_snippet_receipt(
        &self,
        receipt: SnippetExecutionReceipt,
        owner: SnippetReceiptOwner,
    ) -> Result<(), ToolError> {
        let json = serde_json::to_string(&receipt)
            .map_err(|_| ToolError::internal_message("receipt serialization failed"))?;
        if json.len() > MAX_RECEIPT_BYTES {
            return Err(ToolError::internal_message(
                "snippet receipt exceeds 64 KiB",
            ));
        }
        self.with_conn(move |conn| {
            let tx = conn.transaction().map_err(store_error)?;
            // Never replace a colliding execution identity belonging to another owner.
            tx.execute("INSERT INTO snippet_receipts VALUES (?1,?2,?3,?4,?5,?6)
                ON CONFLICT(execution_id) DO UPDATE SET receipt_json=excluded.receipt_json
                WHERE owner_key=excluded.owner_key AND route_scope=excluded.route_scope
                AND capability_fingerprint=excluded.capability_fingerprint
                AND (json_extract(snippet_receipts.receipt_json,'$.status')='started' OR json_extract(excluded.receipt_json,'$.status')!='started')", params![receipt.execution_id, owner.owner_key, owner.route_scope, owner.capability_fingerprint, receipt.created_at_ms, json]).map_err(store_error)?;
            tx.execute("DELETE FROM snippet_receipts WHERE created_at_ms < ?1", params![receipt.created_at_ms.saturating_sub(RETENTION_MS)]).map_err(store_error)?;
            tx.execute("DELETE FROM snippet_receipts WHERE execution_id IN (SELECT execution_id FROM snippet_receipts WHERE owner_key=?1 AND route_scope=?2 ORDER BY created_at_ms DESC, rowid DESC LIMIT -1 OFFSET ?3)", params![owner.owner_key, owner.route_scope, MAX_OWNER_RECEIPTS]).map_err(store_error)?;
            tx.execute("DELETE FROM snippet_receipts WHERE execution_id IN (SELECT execution_id FROM snippet_receipts ORDER BY created_at_ms DESC, rowid DESC LIMIT -1 OFFSET ?1)", params![MAX_RECEIPTS]).map_err(store_error)?;
            tx.commit().map_err(store_error)
        }).await
    }

    /// Resolve only an exact owner, route and caller-capability match.
    pub async fn snippet_receipt(
        &self,
        execution_id: &str,
        owner: SnippetReceiptOwner,
    ) -> Result<SnippetExecutionReceipt, ToolError> {
        let execution_id = execution_id.to_owned();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let cutoff = i64::try_from(now)
            .unwrap_or(i64::MAX)
            .saturating_sub(RETENTION_MS);
        self.with_conn(move |conn| {
            let json: Option<String> = conn.query_row("SELECT receipt_json FROM snippet_receipts WHERE execution_id=?1 AND owner_key=?2 AND route_scope=?3 AND capability_fingerprint=?4 AND created_at_ms>=?5", params![execution_id, owner.owner_key, owner.route_scope, owner.capability_fingerprint, cutoff], |row| row.get(0)).optional().map_err(store_error)?;
            let json = json.ok_or_else(unknown_receipt)?;
            serde_json::from_str(&json).map_err(|_| ToolError::internal_message("invalid stored snippet receipt"))
        }).await
    }
}

#[cfg(test)]
mod tests;
