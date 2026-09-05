use super::cursor::{Binding, CursorError, CursorStore, PageInput};
use std::time::{Duration, Instant};

fn binding(actor: &str) -> Binding {
    Binding {
        actor: actor.into(),
        authority_epoch: "auth-1".into(),
        scope: "all".into(),
        query: "storage".into(),
        page_contract: "v1:50".into(),
        registry_epoch: "registry-1".into(),
        providers: vec![("public".into(), "inc-1".into(), "list-1".into())],
    }
}

#[tokio::test]
async fn cursor_is_random_opaque_and_replay_is_byte_identical() {
    let store = CursorStore::default();
    let now = Instant::now();
    let cursor = store
        .create(binding("actor-a"), b"initial".to_vec(), now)
        .await
        .unwrap();
    assert_eq!(cursor.len(), 43);
    assert!(
        cursor
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    );
    let PageInput::Compute(lease) = store
        .begin(&cursor, &binding("actor-a"), now)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(lease.state(), b"initial");
    let page = lease
        .complete(b"serialized".to_vec(), Some(b"next".to_vec()), now)
        .await
        .unwrap();
    let replay = store
        .begin(&cursor, &binding("actor-a"), now)
        .await
        .unwrap();
    assert_eq!(replay.replay().unwrap(), page);
}

#[tokio::test]
async fn cursor_rejects_foreign_authority_and_expires_old_transitions() {
    let store = CursorStore::default();
    let now = Instant::now();
    let first = store.create(binding("actor-a"), vec![], now).await.unwrap();
    assert_eq!(
        store
            .begin(&first, &binding("actor-b"), now)
            .await
            .unwrap_err(),
        CursorError::Expired
    );
    let PageInput::Compute(one) = store.begin(&first, &binding("actor-a"), now).await.unwrap()
    else {
        panic!()
    };
    let second = one
        .complete(vec![1], Some(vec![2]), now)
        .await
        .unwrap()
        .next_cursor
        .unwrap();
    let PageInput::Compute(two) = store
        .begin(&second, &binding("actor-a"), now)
        .await
        .unwrap()
    else {
        panic!()
    };
    let third = two
        .complete(vec![2], Some(vec![3]), now)
        .await
        .unwrap()
        .next_cursor
        .unwrap();
    let PageInput::Compute(three) = store.begin(&third, &binding("actor-a"), now).await.unwrap()
    else {
        panic!()
    };
    three.complete(vec![3], None, now).await.unwrap();
    assert_eq!(
        store
            .begin(&first, &binding("actor-a"), now)
            .await
            .unwrap_err(),
        CursorError::Expired
    );
    assert_eq!(
        store
            .begin("bad", &binding("actor-a"), now)
            .await
            .unwrap_err(),
        CursorError::Expired
    );
}

#[tokio::test]
async fn cursor_accepts_current_provider_binding_without_a_live_listing_epoch() {
    let store = CursorStore::default();
    let now = Instant::now();
    let saved = binding("actor-a");
    let cursor = store.create(saved.clone(), vec![], now).await.unwrap();
    let mut current = saved;
    current.providers[0].2.clear();

    assert!(matches!(
        store.begin(&cursor, &current, now).await.unwrap(),
        PageInput::Compute(_)
    ));
}

#[tokio::test]
async fn cursor_still_rejects_a_changed_known_listing_epoch() {
    let store = CursorStore::default();
    let now = Instant::now();
    let saved = binding("actor-a");
    let cursor = store.create(saved.clone(), vec![], now).await.unwrap();
    let mut current = saved;
    current.providers[0].2 = "list-2".into();

    assert_eq!(
        store.begin(&cursor, &current, now).await.unwrap_err(),
        CursorError::Expired
    );
}

#[tokio::test]
async fn cursor_enforces_idle_and_absolute_expiry() {
    let store = CursorStore::default();
    let now = Instant::now();
    let cursor = store.create(binding("actor-a"), vec![], now).await.unwrap();
    assert_eq!(
        store
            .begin(
                &cursor,
                &binding("actor-a"),
                now + Duration::from_secs(3601)
            )
            .await
            .unwrap_err(),
        CursorError::Expired
    );
}

#[tokio::test]
async fn concurrent_same_input_waits_for_the_identical_published_page() {
    let store = CursorStore::default();
    let now = Instant::now();
    let cursor = store
        .create(binding("actor-a"), b"state".to_vec(), now)
        .await
        .unwrap();
    let PageInput::Compute(lease) = store
        .begin(&cursor, &binding("actor-a"), now)
        .await
        .unwrap()
    else {
        panic!()
    };
    let waiting_store = store.clone();
    let waiting_cursor = cursor.clone();
    let waiting = tokio::spawn(async move {
        waiting_store
            .begin(&waiting_cursor, &binding("actor-a"), now)
            .await
            .unwrap()
    });
    tokio::task::yield_now().await;
    let published = lease
        .complete(b"same bytes".to_vec(), None, now)
        .await
        .unwrap();
    assert_eq!(waiting.await.unwrap().replay().unwrap(), published);
}

#[tokio::test]
async fn failed_capacity_reservation_leaves_the_input_retriable() {
    let store = CursorStore::default();
    let now = Instant::now();
    let cursor = store.create(binding("actor-a"), vec![], now).await.unwrap();
    let PageInput::Compute(lease) = store
        .begin(&cursor, &binding("actor-a"), now)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        lease
            .complete(vec![0; 4 * 1024 * 1024 + 1], None, now)
            .await
            .unwrap_err(),
        CursorError::Capacity
    );
    assert!(matches!(
        store
            .begin(&cursor, &binding("actor-a"), now)
            .await
            .unwrap(),
        PageInput::Compute(_)
    ));
}
