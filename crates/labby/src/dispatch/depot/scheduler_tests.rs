use std::sync::Arc;
use std::time::Duration;

use super::scheduler::{Pending, ProviderAdmission, Scheduler};
use tokio::time::Instant;

#[tokio::test]
async fn actor_has_one_active_and_two_queued_and_cancel_releases_slots() {
    let scheduler = Arc::new(Scheduler::default());
    let first = scheduler.admit("actor", Instant::now()).await.unwrap();
    let mut second = Box::pin(scheduler.admit("actor", Instant::now()));
    let mut third = Box::pin(scheduler.admit("actor", Instant::now()));
    assert!(futures::poll!(&mut second).is_pending());
    assert!(futures::poll!(&mut third).is_pending());
    assert!(matches!(
        scheduler.admit("actor", Instant::now()).await,
        Err(Pending)
    ));
    drop(second);
    drop(first);
    assert!(third.await.is_ok());
    assert!(scheduler.admit("actor", Instant::now()).await.is_ok());
}

#[tokio::test]
async fn four_federations_admit_and_sixteen_queue_without_unbounded_actor_state() {
    let scheduler = Arc::new(Scheduler::default());
    let mut active = Vec::new();
    for actor in ["a", "b", "c", "d"] {
        active.push(scheduler.admit(actor, Instant::now()).await.unwrap());
    }
    let actors: Vec<_> = (0..16).map(|n| format!("queued-{n}")).collect();
    let mut queued: Vec<_> = actors
        .iter()
        .map(|actor| Box::pin(scheduler.admit(actor, Instant::now())))
        .collect();
    for future in &mut queued {
        assert!(futures::poll!(future).is_pending());
    }
    assert!(matches!(
        scheduler.admit("overflow", Instant::now()).await,
        Err(Pending)
    ));
    drop(queued);
    drop(active);
    for n in 0..100 {
        drop(
            scheduler
                .admit(&format!("next-{n}"), Instant::now())
                .await
                .unwrap(),
        );
    }
    assert!(scheduler.retained_actors() <= 20);
}

#[tokio::test]
async fn provider_contention_holds_no_global_permit_and_all_call_caps_apply() {
    let scheduler = Arc::new(Scheduler::default());
    let mut federations = Vec::new();
    for actor in ["a", "b", "c", "d"] {
        federations.push(scheduler.admit(actor, Instant::now()).await.unwrap());
    }
    let busy = ProviderAdmission::default();
    let mut calls = vec![
        federations[0].try_call(&busy).unwrap(),
        federations[0].try_call(&busy).unwrap(),
    ];
    let probe = scheduler.probe(Instant::now()).unwrap();
    assert!(matches!(probe.try_call(&busy), Err(Pending)));
    for federation in &federations[1..] {
        for _ in 0..4 {
            calls.push(federation.try_call(&ProviderAdmission::default()).unwrap());
        }
        assert!(matches!(
            federation.try_call(&ProviderAdmission::default()),
            Err(Pending)
        ));
    }
    for _ in 0..2 {
        calls.push(
            federations[0]
                .try_call(&ProviderAdmission::default())
                .unwrap(),
        );
    }
    assert_eq!(calls.len(), 16);
    assert!(matches!(
        probe.try_call(&ProviderAdmission::default()),
        Err(Pending)
    ));
    drop(calls.pop());
    assert!(probe.try_call(&ProviderAdmission::default()).is_ok());
    drop(calls);
    assert!(federations[0].try_call(&busy).is_ok());
}

#[tokio::test]
async fn probe_is_single_admission_without_queue() {
    let scheduler = Arc::new(Scheduler::default());
    let probe = scheduler.probe(Instant::now()).unwrap();
    assert!(matches!(scheduler.probe(Instant::now()), Err(Pending)));
    drop(probe);
    assert!(scheduler.probe(Instant::now()).is_ok());
}

#[tokio::test]
async fn receipt_deadline_includes_queue_and_blocks_late_calls() {
    let scheduler = Arc::new(Scheduler::default());
    let first = scheduler.admit("actor", Instant::now()).await.unwrap();
    let receipt = Instant::now() - Duration::from_millis(4_970);
    assert!(matches!(
        scheduler.admit("actor", receipt).await,
        Err(Pending)
    ));
    drop(first);
    let receipt = Instant::now() - Duration::from_millis(4_980);
    let late = scheduler.admit("actor", receipt).await.unwrap();
    assert_eq!(late.deadline(), receipt + Duration::from_secs(5));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(matches!(
        late.try_call(&ProviderAdmission::default()),
        Err(Pending)
    ));
    assert!(matches!(
        scheduler.admit("actor", receipt).await,
        Err(Pending)
    ));
}

#[tokio::test]
async fn invalid_or_unbounded_actor_identity_never_allocates() {
    let scheduler = Arc::new(Scheduler::default());
    for actor in [String::new(), "a".repeat(257)] {
        assert!(matches!(
            scheduler.admit(&actor, Instant::now()).await,
            Err(Pending)
        ));
    }
    assert_eq!(scheduler.retained_actors(), 0);
}
