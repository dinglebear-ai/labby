//! Shared durable snippet metadata; the journal owns persistence and quotas.
use super::GatewayManager;
use crate::codemode_journal::receipts::SnippetReceiptHistory;
use crate::codemode_journal::receipts::{SnippetExecutionReceipt, SnippetReceiptOwner};
use labby_runtime::error::ToolError;

impl GatewayManager {
    /// Bounded history with the same authority isolation as single-receipt lookup.
    pub async fn snippet_history(
        &self,
        owner: SnippetReceiptOwner,
        name: Option<String>,
        limit: usize,
        cursor: Option<String>,
    ) -> Result<SnippetReceiptHistory, ToolError> {
        if let Some(store) = &self.step_journal {
            store.snippet_history(owner, name, limit, cursor).await
        } else {
            Ok(SnippetReceiptHistory {
                receipts: Vec::new(),
                next_cursor: None,
                receipt_status: "disabled".into(),
            })
        }
    }
    /// None indicates journaling was disabled/unavailable; execution remains valid.
    pub async fn record_snippet_receipt(
        &self,
        receipt: SnippetExecutionReceipt,
        owner: SnippetReceiptOwner,
    ) -> Result<bool, ToolError> {
        let Some(store) = &self.step_journal else {
            return Ok(false);
        };
        store.record_snippet_receipt(receipt, owner).await?;
        Ok(true)
    }

    /// Read with owner isolation even for administrators.
    pub async fn snippet_receipt(
        &self,
        execution_id: &str,
        owner: SnippetReceiptOwner,
    ) -> Result<SnippetExecutionReceipt, ToolError> {
        let store = self.step_journal.as_ref().ok_or_else(|| ToolError::Sdk {
            sdk_kind: "journal_unavailable".into(),
            message: "snippet receipts require the configured Code Mode journal".into(),
        })?;
        store.snippet_receipt(execution_id, owner).await
    }
}
