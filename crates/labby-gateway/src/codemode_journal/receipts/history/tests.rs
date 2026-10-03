use super::*;
use crate::codemode_journal::receipts::tests::{now, owner, receipt};

#[tokio::test]
async fn pagination_filters_and_all_authority_axes_are_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("journal.db");
    let store = StepJournalStore::open(path.clone()).await.unwrap();
    let time = now();
    for id in ["a", "b", "c", "d"] {
        let mut row = receipt(id, time);
        if id == "d" {
            row.snippet_name = "other".into();
        }
        store.record_snippet_receipt(row, owner()).await.unwrap();
    }
    store
        .record_snippet_receipt(receipt("expired", time - RETENTION_MS - 1), owner())
        .await
        .unwrap();
    let page = store
        .snippet_history(owner(), Some("test".into()), 2, None)
        .await
        .unwrap();
    assert_eq!(
        page.receipts
            .iter()
            .map(|r| r.execution_id.as_str())
            .collect::<Vec<_>>(),
        ["c", "b"]
    );
    let cursor = page.next_cursor.unwrap();
    drop(store);
    let store = StepJournalStore::open(path).await.unwrap();
    let next = store
        .snippet_history(owner(), Some("test".into()), 2, Some(cursor.clone()))
        .await
        .unwrap();
    assert_eq!(
        next.receipts
            .iter()
            .map(|r| r.execution_id.as_str())
            .collect::<Vec<_>>(),
        ["a"]
    );
    assert!(next.next_cursor.is_none());
    assert!(
        store
            .snippet_history(owner(), None, 2, Some(cursor.clone()))
            .await
            .is_err()
    );
    for field in 0..3 {
        let mut denied = owner();
        match field {
            0 => denied.owner_key = "different".into(),
            1 => denied.route_scope = "different".into(),
            _ => denied.capability_fingerprint = "different".into(),
        }
        assert!(
            store
                .snippet_history(denied.clone(), None, 50, None)
                .await
                .unwrap()
                .receipts
                .is_empty()
        );
        assert_eq!(
            store
                .snippet_history(denied, Some("test".into()), 2, Some(cursor.clone()))
                .await
                .unwrap_err()
                .kind(),
            "invalid_param"
        );
    }
    assert_eq!(
        store
            .snippet_history(owner(), None, 50, None)
            .await
            .unwrap()
            .receipts
            .len(),
        4
    );
}

#[tokio::test]
async fn history_bounds_and_cursor_validation_fail_before_query() {
    let directory = tempfile::tempdir().unwrap();
    let store = StepJournalStore::open(directory.path().join("journal.db"))
        .await
        .unwrap();
    for limit in [0, 51, usize::MAX] {
        assert!(
            store
                .snippet_history(owner(), None, limit, None)
                .await
                .is_err()
        );
    }
    for cursor in [
        "bad!".into(),
        "x".repeat(1025),
        URL_SAFE_NO_PAD.encode(b"{}"),
    ] {
        assert!(
            store
                .snippet_history(owner(), None, 20, Some(cursor))
                .await
                .is_err()
        );
    }
    assert!(
        store
            .snippet_history(owner(), Some("x".repeat(129)), 20, None)
            .await
            .is_err()
    );
}
