//! Focused regressions for the per-upstream `list_changed` refresh workers:
//! one upstream's slow re-list never delays another upstream's forwarded
//! notification, a burst from one upstream coalesces into a bounded number of
//! re-lists with exactly one forwarded notification per completed refresh, and
//! dropping the refresher aborts its workers.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::mpsc;

use super::UpstreamPool;
use super::list_changed_refresh::{
    LIST_CHANGED_COALESCE_WINDOW, ListChangedKinds, ListChangedRefresher,
};
use super::notifications_tests::{SubscriptionServer, add_subscription_server};

/// Generous bound for one coalescing window plus an in-process re-list.
const FORWARD_DEADLINE: Duration = Duration::from_secs(3);
/// How long a test waits to prove that nothing was forwarded.
const QUIET_PERIOD: Duration = Duration::from_millis(500);

type Forwarded = (String, ListChangedKinds);

fn recording_refresher(
    pool: Arc<UpstreamPool>,
) -> (ListChangedRefresher, mpsc::UnboundedReceiver<Forwarded>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let refresher = ListChangedRefresher::new(pool, move |upstream: &str, kinds| {
        tx.send((upstream.to_string(), kinds))
            .expect("test receiver stays open");
    });
    (refresher, rx)
}

async fn expect_forwarded(rx: &mut mpsc::UnboundedReceiver<Forwarded>) -> Forwarded {
    tokio::time::timeout(FORWARD_DEADLINE, rx.recv())
        .await
        .expect("a forwarded notification arrives before the deadline")
        .expect("forwarder channel stays open")
}

/// Assert nothing is forwarded for `QUIET_PERIOD`. A closed channel is quiet
/// too: dropping the refresher drops the forwarder that owns the sender.
async fn expect_quiet(rx: &mut mpsc::UnboundedReceiver<Forwarded>) {
    if let Ok(Some(forwarded)) = tokio::time::timeout(QUIET_PERIOD, rx.recv()).await {
        panic!("unexpected forwarded notification: {forwarded:?}");
    }
}

/// A worker only observes its empty pending set one coalescing window after
/// its last run, so draining the task set is a deadline, not an instant.
async fn expect_workers_drain(refresher: &ListChangedRefresher) {
    tokio::time::timeout(FORWARD_DEADLINE, async {
        while refresher.active_worker_count_for_test() > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("an idle worker removes itself from the task set");
}

#[tokio::test]
async fn slow_upstream_refresh_does_not_delay_other_upstreams() {
    let pool = Arc::new(UpstreamPool::new());
    let slow = SubscriptionServer::accepting();
    let fast = SubscriptionServer::accepting();
    add_subscription_server(&pool, "slow", slow.clone()).await;
    add_subscription_server(&pool, "fast", fast.clone()).await;
    slow.resource_list_gate.send_replace(false);
    fast.replace_tools_and_notify(&["added_after_list_changed"])
        .await;

    let (refresher, mut forwarded) = recording_refresher(Arc::clone(&pool));
    refresher.schedule("slow", ListChangedKinds::RESOURCES);
    refresher.schedule("fast", ListChangedKinds::TOOLS);

    let started = std::time::Instant::now();
    let (upstream, kinds) = expect_forwarded(&mut forwarded).await;
    assert_eq!(upstream, "fast");
    assert_eq!(kinds, ListChangedKinds::TOOLS);
    assert!(
        started.elapsed() < FORWARD_DEADLINE,
        "fast upstream must not queue behind the slow re-list"
    );
    let tool_names = pool
        .healthy_tools_for_upstream("fast")
        .await
        .into_iter()
        .map(|tool| tool.tool.name.to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names,
        ["added_after_list_changed"],
        "the refresh completes before the notification is forwarded"
    );
    assert!(
        slow.resource_list_calls.load(Ordering::SeqCst) >= 1,
        "the slow upstream's re-list is in flight"
    );
    expect_quiet(&mut forwarded).await;

    slow.resource_list_gate.send_replace(true);
    let (upstream, kinds) = expect_forwarded(&mut forwarded).await;
    assert_eq!(upstream, "slow");
    assert_eq!(kinds, ListChangedKinds::RESOURCES);
    expect_workers_drain(&refresher).await;
}

#[tokio::test]
async fn list_changed_burst_for_one_upstream_is_coalesced() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "leaf", server.clone()).await;
    let baseline = server.resource_list_calls.load(Ordering::SeqCst);

    let (refresher, mut forwarded) = recording_refresher(Arc::clone(&pool));
    for _ in 0..3 {
        refresher.schedule("leaf", ListChangedKinds::RESOURCES);
    }

    let (upstream, kinds) = expect_forwarded(&mut forwarded).await;
    assert_eq!(upstream, "leaf");
    assert_eq!(kinds, ListChangedKinds::RESOURCES);
    // Let a possible second iteration (signals that landed mid-refresh) finish.
    tokio::time::sleep(LIST_CHANGED_COALESCE_WINDOW * 4).await;
    let mut forwarded_count = 1;
    while let Ok(next) = forwarded.try_recv() {
        assert_eq!(next.0, "leaf");
        forwarded_count += 1;
    }
    let relists = server.resource_list_calls.load(Ordering::SeqCst) - baseline;
    assert!(
        (1..=2).contains(&relists),
        "three signals must collapse into one or two re-lists, got {relists}"
    );
    assert_eq!(
        forwarded_count, relists,
        "exactly one forwarded notification per completed refresh"
    );
    expect_workers_drain(&refresher).await;
}

#[tokio::test]
async fn prompt_list_changed_is_forwarded_without_a_refresh() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "leaf", server.clone()).await;
    let baseline = server.resource_list_calls.load(Ordering::SeqCst);

    let (refresher, mut forwarded) = recording_refresher(Arc::clone(&pool));
    refresher.schedule("leaf", ListChangedKinds::PROMPTS);

    let (upstream, kinds) = expect_forwarded(&mut forwarded).await;
    assert_eq!(upstream, "leaf");
    assert_eq!(kinds, ListChangedKinds::PROMPTS);
    assert_eq!(
        server.resource_list_calls.load(Ordering::SeqCst),
        baseline,
        "prompts/list is served live, so no catalog re-list runs"
    );
}

#[tokio::test]
async fn dropping_the_refresher_aborts_in_flight_workers() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "slow", server.clone()).await;
    server.resource_list_gate.send_replace(false);

    let (refresher, mut forwarded) = recording_refresher(Arc::clone(&pool));
    refresher.schedule("slow", ListChangedKinds::RESOURCES);
    tokio::time::timeout(FORWARD_DEADLINE, async {
        while server.resource_list_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the worker starts its re-list");

    drop(refresher);
    server.resource_list_gate.send_replace(true);
    expect_quiet(&mut forwarded).await;
}

#[test]
fn kinds_union_and_emptiness() {
    assert!(ListChangedKinds::default().is_empty());
    let merged = ListChangedKinds::TOOLS
        .union(ListChangedKinds::PROMPTS)
        .union(ListChangedKinds::default());
    assert_eq!(
        merged,
        ListChangedKinds {
            tools: true,
            resources: false,
            prompts: true,
        }
    );
    assert!(!merged.is_empty());
}
