//! Fire-and-forget usage-record write, called from `capability_call.rs` after
//! every tool/resource/prompt call outcome. Never blocks or fails the call
//! path: if `pool.usage_store` is `None`, this is a no-op; if the write
//! itself fails, it is logged and dropped.
//!
//! Backpressure: in-flight write tasks are bounded by `UsageStore`'s internal
//! semaphore (`WRITE_SEMAPHORE_PERMITS`). A burst of calls that saturates the
//! semaphore drops the write and logs a warning rather than spawning an
//! unbounded number of concurrent writer tasks — telemetry is best-effort.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::usage::UpstreamCallRecord;

use super::UpstreamPool;
use super::logging::UpstreamRequestLog;

pub(super) fn record_usage_call(
    pool: &UpstreamPool,
    event: UpstreamRequestLog<'_>,
    subject: Option<&str>,
    outcome: &'static str,
    elapsed_ms: u128,
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
        actor: subject.map_or_else(|| "unattributed".to_string(), str::to_string),
        outcome: outcome.to_string(),
        elapsed_ms: i64::try_from(elapsed_ms).unwrap_or(i64::MAX),
    };
    // Acquire an owned permit *before* spawning so a saturated semaphore
    // actually bounds the number of spawned tasks, not just the number of
    // concurrent DB-write attempts inside already-spawned tasks.
    let Ok(permit) = store.write_semaphore().try_acquire_owned() else {
        tracing::warn!(
            upstream = %record.upstream_name,
            tool = %record.tool_name,
            "usage store write dropped: too many in-flight writes"
        );
        return;
    };
    tokio::spawn(async move {
        let _permit = permit;
        if let Err(error) = store.record_call(record).await {
            tracing::warn!(error = %error, "usage store record_call failed");
        }
    });
}
