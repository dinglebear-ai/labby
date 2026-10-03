//! Cursor-based bounded history in the same authority as receipt lookup.
use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

/// One page of execution evidence. Disabled journaling has an explicit empty state.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SnippetReceiptHistory {
    pub receipts: Vec<SnippetExecutionReceipt>,
    pub next_cursor: Option<String>,
    pub receipt_status: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    created_at_ms: i64,
    execution_id: String,
    filter_digest: String,
}
fn invalid(param: &str, message: &str) -> ToolError {
    ToolError::InvalidParam {
        param: param.into(),
        message: message.into(),
    }
}
fn filter_digest(owner: &SnippetReceiptOwner, name: Option<&str>) -> String {
    let bytes = serde_json::to_vec(&(
        &owner.owner_key,
        &owner.route_scope,
        &owner.capability_fingerprint,
        name,
    ))
    .expect("string tuple serialization is infallible");
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

impl StepJournalStore {
    /// Stable descending keyset pagination, always filtered by owner, route and capability.
    pub async fn snippet_history(
        &self,
        owner: SnippetReceiptOwner,
        name: Option<String>,
        limit: usize,
        cursor: Option<String>,
    ) -> Result<SnippetReceiptHistory, ToolError> {
        if !(1..=50).contains(&limit) {
            return Err(invalid("limit", "limit must be between 1 and 50"));
        }
        if name
            .as_ref()
            .is_some_and(|name| name.is_empty() || name.len() > 128)
        {
            return Err(invalid("name", "name must contain 1 to 128 bytes"));
        }
        let fingerprint = filter_digest(&owner, name.as_deref());
        let cursor = cursor
            .map(|raw| {
                if raw.len() > 1024 {
                    return Err(invalid("cursor", "invalid history cursor"));
                }
                let bytes = URL_SAFE_NO_PAD
                    .decode(raw)
                    .map_err(|_| invalid("cursor", "invalid history cursor"))?;
                let cursor: Cursor = serde_json::from_slice(&bytes)
                    .map_err(|_| invalid("cursor", "invalid history cursor"))?;
                if cursor.filter_digest != fingerprint
                    || cursor.created_at_ms < 0
                    || cursor.execution_id.is_empty()
                    || cursor.execution_id.len() > 128
                {
                    return Err(invalid(
                        "cursor",
                        "history cursor does not match this scope and filter",
                    ));
                }
                Ok(cursor)
            })
            .transpose()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let cutoff = i64::try_from(now)
            .unwrap_or(i64::MAX)
            .saturating_sub(RETENTION_MS);
        self.with_conn(move |conn| {
            let mut stmt = conn.prepare("SELECT receipt_json FROM snippet_receipts
                WHERE owner_key=?1 AND route_scope=?2 AND capability_fingerprint=?3 AND created_at_ms>=?4
                AND (?5 IS NULL OR json_extract(receipt_json,'$.snippet_name')=?5)
                AND (?6 IS NULL OR created_at_ms<?6 OR (created_at_ms=?6 AND execution_id<?7))
                ORDER BY created_at_ms DESC, execution_id DESC LIMIT ?8").map_err(store_error)?;
            let rows = stmt.query_map(params![owner.owner_key, owner.route_scope, owner.capability_fingerprint, cutoff, name,
                cursor.as_ref().map(|c| c.created_at_ms), cursor.as_ref().map(|c| &c.execution_id), i64::try_from(limit+1).expect("bounded page limit")], |row| row.get::<_,String>(0))
                .map_err(store_error)?;
            let mut receipts = Vec::new();
            for row in rows {
                let row = row.map_err(store_error)?;
                if row.len()>MAX_RECEIPT_BYTES { return Err(ToolError::internal_message("invalid stored snippet receipt")); }
                receipts.push(serde_json::from_str::<SnippetExecutionReceipt>(&row)
                    .map_err(|_| ToolError::internal_message("invalid stored snippet receipt"))?);
            }
            let more = receipts.len()>limit;
            receipts.truncate(limit);
            let next_cursor = if more { receipts.last().map(|r| URL_SAFE_NO_PAD.encode(
                serde_json::to_vec(&Cursor { created_at_ms:r.created_at_ms, execution_id:r.execution_id.clone(), filter_digest:fingerprint })
                    .expect("cursor serialization is infallible"))) } else { None };
            Ok(SnippetReceiptHistory { receipts, next_cursor, receipt_status:"persisted".into() })
        }).await
    }
}

#[cfg(test)]
mod tests;
