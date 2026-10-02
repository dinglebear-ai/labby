//! Pure assembly shared by preflight preparation and composed guard regressions.
use super::super::execution::{receipt_owner, value_digest};
use super::{PreviewDrift, PreviewFingerprints, SnippetDispatchContext, SnippetExecutionReceipt};
use serde_json::json;

pub(super) fn assemble(
    name: &str,
    fingerprints: &PreviewFingerprints,
    config: &labby_runtime::CodeModeConfig,
    context: &SnippetDispatchContext,
    previous: Option<&SnippetExecutionReceipt>,
    drift: &[PreviewDrift],
) -> String {
    let owner = receipt_owner(context);
    value_digest(
        &json!({"version":1,"name":name,"fingerprints":fingerprints,"runtime_config":config,"owner":{"key":owner.owner_key,"route":owner.route_scope,"capabilities":owner.capability_fingerprint},"previous_execution":previous.map(|r|&r.execution_id),"drift":drift}),
    )
}
