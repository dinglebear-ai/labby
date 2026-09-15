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
    record_usage_call_with_response(pool, event, subject, outcome, elapsed_ms, None);
}

pub(super) fn record_usage_call_with_response(
    pool: &UpstreamPool,
    event: UpstreamRequestLog<'_>,
    subject: Option<&str>,
    outcome: &'static str,
    elapsed_ms: u128,
    response_bytes: Option<usize>,
) {
    let Some(store) = pool.usage_store.clone() else {
        return;
    };
    let mut attribution = labby_runtime::usage_actor::attribution().unwrap_or_default();
    attribution.inbound_actor = labby_runtime::usage_actor::current();
    attribution.upstream_subject_tag = subject.map(|subject| {
        use sha2::{Digest, Sha256};
        format!("oauth:{}", hex::encode(Sha256::digest(subject.as_bytes())))
    });
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
        actor: labby_runtime::usage_actor::current().unwrap_or_else(|| "unattributed".to_string()),
        attribution: Some(attribution),
        outcome: outcome.to_string(),
        elapsed_ms: i64::try_from(elapsed_ms).unwrap_or(i64::MAX),
        response_bytes: response_bytes.and_then(|value| i64::try_from(value).ok()),
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

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn inbound_actor_attributes_shared_calls_without_changing_credential_scope() {
        let directory = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(
            crate::usage::UsageStore::open(directory.path().join("usage.sqlite3"))
                .await
                .unwrap(),
        );
        let pool = UpstreamPool::new().with_usage_store(Some(std::sync::Arc::clone(&store)));
        labby_runtime::usage_actor::scope(Some("sub:verified123".into()), async {
            record_usage_call(
                &pool,
                UpstreamRequestLog::tool("shared", "search", false),
                None,
                "ok",
                12,
            );
            record_usage_call(
                &pool,
                UpstreamRequestLog::tool("oauth", "search", true),
                Some("credential-subject"),
                "ok",
                13,
            );
        })
        .await;
        record_usage_call(
            &pool,
            UpstreamRequestLog::tool("historical-shape", "search", false),
            None,
            "ok",
            14,
        );
        store.drain_pending_writes().await;
        let rows = store.with_conn(|connection| {
            let mut statement = connection.prepare("SELECT upstream_name,actor,subject_scoped FROM upstream_calls ORDER BY upstream_name").map_err(crate::usage::store::sqlite_error)?;
            statement.query_map([], |row| Ok((row.get::<_, String>(0)?,row.get::<_, String>(1)?,row.get::<_, bool>(2)?))).map_err(crate::usage::store::sqlite_error)?.collect::<Result<Vec<_>, _>>().map_err(crate::usage::store::sqlite_error)
        }).await.unwrap();
        assert_eq!(
            rows,
            vec![
                ("historical-shape".into(), "unattributed".into(), false),
                ("oauth".into(), "sub:verified123".into(), true),
                ("shared".into(), "sub:verified123".into(), false),
            ]
        );
    }
}
