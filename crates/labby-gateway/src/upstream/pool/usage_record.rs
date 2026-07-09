//! Fire-and-forget usage-record write, called from `capability_call.rs` after
//! every tool/resource/prompt call outcome. Never blocks or fails the call
//! path: if `pool.usage_store` is `None`, this is a no-op; if the write
//! itself fails, it is logged and dropped.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::usage::UpstreamCallRecord;

use super::UpstreamPool;
use super::logging::UpstreamRequestLog;

#[allow(clippy::too_many_arguments)]
pub(super) fn record_usage_call(
    pool: &UpstreamPool,
    event: UpstreamRequestLog<'_>,
    subject: Option<&str>,
    outcome: &'static str,
    error_kind: Option<&'static str>,
    elapsed_ms: u128,
    response_bytes: Option<usize>,
) {
    let Some(store) = pool.usage_store.clone() else {
        return;
    };
    let record = UpstreamCallRecord {
        ts_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        upstream_name: event.upstream.to_string(),
        tool_name: event.item.unwrap_or_default().to_string(),
        capability: event.capability.to_string(),
        operation: event.operation.to_string(),
        subject_scoped: event.subject_scoped,
        actor: subject.map(str::to_string),
        outcome: outcome.to_string(),
        error_kind: error_kind.map(str::to_string),
        elapsed_ms: i64::try_from(elapsed_ms).unwrap_or(i64::MAX),
        response_bytes: response_bytes.map(|b| i64::try_from(b).unwrap_or(i64::MAX)),
    };
    tokio::spawn(async move {
        if let Err(error) = store.record_call(record).await {
            tracing::warn!(error = %error, "usage store record_call failed");
        }
    });
}
