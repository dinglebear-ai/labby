use super::*;

pub(super) fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}
pub(super) fn owner() -> SnippetReceiptOwner {
    SnippetReceiptOwner {
        owner_key: "actor-a".into(),
        route_scope: "team-a".into(),
        capability_fingerprint: "scope-a".into(),
    }
}
pub(super) fn receipt(id: &str, time: i64) -> SnippetExecutionReceipt {
    SnippetExecutionReceipt {
        execution_id: id.into(),
        snippet_name: "test".into(),
        snippet_digest: "sha256:source".into(),
        input_digest: "sha256:input".into(),
        effective_scope_fingerprint: "scope-a".into(),
        runtime_version: "test/1".into(),
        tool_schema_digests: None,
        surface: "api".into(),
        created_at_ms: time,
        elapsed_ms: 12,
        status: "succeeded".into(),
        error_kind: None,
        result_digest: Some("sha256:output".into()),
        result_bytes: Some(10),
        calls: vec![],
        tool_calls: 0,
        omitted_calls: 0,
        artifacts: vec![],
    }
}

#[test]
fn older_receipt_without_schema_evidence_remains_explicitly_unverifiable() {
    let mut value = serde_json::to_value(receipt("legacy", 0)).unwrap();
    value.as_object_mut().unwrap().remove("tool_schema_digests");
    let decoded: SnippetExecutionReceipt = serde_json::from_value(value).unwrap();
    assert!(decoded.tool_schema_digests.is_none());
}
#[tokio::test]
async fn durable_receipts_survive_reopen_and_enforce_all_owner_axes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.db");
    let store = StepJournalStore::open(path.clone()).await.unwrap();
    store
        .record_snippet_receipt(receipt("execution", now()), owner())
        .await
        .unwrap();
    drop(store);
    let store = StepJournalStore::open(path).await.unwrap();
    assert_eq!(
        store
            .snippet_receipt("execution", owner())
            .await
            .unwrap()
            .status,
        "succeeded"
    );
    for field in 0..3 {
        let mut denied = owner();
        match field {
            0 => denied.owner_key = "other".into(),
            1 => denied.route_scope = "other".into(),
            _ => denied.capability_fingerprint = "other".into(),
        }
        assert_eq!(
            store
                .snippet_receipt("execution", denied)
                .await
                .unwrap_err()
                .kind(),
            "unknown_execution"
        );
    }
    let mut colliding = receipt("execution", now());
    colliding.status = "failed".into();
    let mut other = owner();
    other.owner_key = "other".into();
    store
        .record_snippet_receipt(colliding, other)
        .await
        .unwrap();
    assert_eq!(
        store
            .snippet_receipt("execution", owner())
            .await
            .unwrap()
            .status,
        "succeeded"
    );
}
#[tokio::test]
async fn receipt_retention_and_owner_quota_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let store = StepJournalStore::open(dir.path().join("journal.db"))
        .await
        .unwrap();
    let time = now();
    store
        .record_snippet_receipt(receipt("old", time - RETENTION_MS - 1), owner())
        .await
        .unwrap();
    assert!(store.snippet_receipt("old", owner()).await.is_err());
    for i in 0..=MAX_OWNER_RECEIPTS {
        store
            .record_snippet_receipt(receipt(&format!("run-{i}"), time + i), owner())
            .await
            .unwrap();
    }
    assert!(store.snippet_receipt("run-0", owner()).await.is_err());
    assert!(store.snippet_receipt("run-100", owner()).await.is_ok());
    let count = store
        .with_conn(|conn| {
            conn.query_row("SELECT COUNT(*) FROM snippet_receipts", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(store_error)
        })
        .await
        .unwrap();
    assert_eq!(count, MAX_OWNER_RECEIPTS);
}
#[tokio::test]
async fn receipt_size_limit_does_not_evict_existing_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let store = StepJournalStore::open(dir.path().join("journal.db"))
        .await
        .unwrap();
    store
        .record_snippet_receipt(receipt("good", now()), owner())
        .await
        .unwrap();
    let mut huge = receipt("huge", now());
    huge.runtime_version = "x".repeat(MAX_RECEIPT_BYTES);
    assert!(store.record_snippet_receipt(huge, owner()).await.is_err());
    assert!(store.snippet_receipt("good", owner()).await.is_ok());
}
#[tokio::test]
async fn step_journal_v1_is_upgraded_without_changing_existing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.db");
    let store = StepJournalStore::open(path.clone()).await.unwrap();
    store
        .with_conn(|conn| {
            conn.execute_batch("DROP TABLE snippet_receipts;")
                .map_err(store_error)
        })
        .await
        .unwrap();
    drop(store);
    let reopened = StepJournalStore::open(path).await.unwrap();
    reopened
        .record_snippet_receipt(receipt("new", now()), owner())
        .await
        .unwrap();
    assert!(reopened.snippet_receipt("new", owner()).await.is_ok());
}
