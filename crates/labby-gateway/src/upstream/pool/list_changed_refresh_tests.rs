//! Focused regressions for the per-upstream `list_changed` refresh workers.
//!
//! The cross-upstream isolation these workers exist for is proven end to end
//! against the real notification consumer in `crates/labby/src/mcp/peers.rs`
//! (`upstream_notification_consumer_tests`). What is pinned here is the worker
//! machinery itself: coalescing inside a real window, mid-refresh signals,
//! forwarding after a *failed* re-list, merged kinds, and worker liveness.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::mpsc;

use super::UpstreamPool;
use super::list_changed_refresh::{
    LIST_CHANGED_COALESCE_WINDOW, ListChangedKinds, ListChangedRefresher,
};
use super::notification_testkit::{SubscriptionServer, add_subscription_server};

/// Deliberately generous, for waits whose only failure mode is never
/// happening, so a long deadline costs nothing and removes timing sensitivity.
const FORWARD_DEADLINE: Duration = Duration::from_secs(30);
/// The bound that carries the cross-upstream isolation claim, which therefore
/// cannot be generous: a blocked re-list is itself bounded by
/// `catalog_listing_timeout` (10 s), so a serialized implementation recovers
/// after that and would satisfy any larger deadline.
const UNBLOCKED_BOUND: Duration = Duration::from_secs(5);
/// How long a test waits to prove that nothing was forwarded.
const QUIET_PERIOD: Duration = Duration::from_millis(500);
/// A window long enough that separate `schedule` calls land in the same one
/// only because the window is real, not because the worker had no chance to
/// run between them.
const LONG_WINDOW: Duration = Duration::from_secs(2);

type Forwarded = (String, ListChangedKinds);

fn recording_refresher(
    pool: Arc<UpstreamPool>,
    window: Duration,
) -> (ListChangedRefresher, mpsc::UnboundedReceiver<Forwarded>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let refresher =
        ListChangedRefresher::with_coalesce_window(pool, window, move |upstream: &str, kinds| {
            drop(tx.send((upstream.to_string(), kinds)));
        });
    (refresher, rx)
}

async fn expect_forwarded(rx: &mut mpsc::UnboundedReceiver<Forwarded>) -> Forwarded {
    expect_forwarded_within(rx, FORWARD_DEADLINE).await
}

async fn expect_forwarded_within(
    rx: &mut mpsc::UnboundedReceiver<Forwarded>,
    bound: Duration,
) -> Forwarded {
    tokio::time::timeout(bound, rx.recv())
        .await
        .expect("a forwarded notification arrives before the deadline")
        .expect("forwarder channel stays open")
}

/// Assert nothing is forwarded for `QUIET_PERIOD`. A closed channel counts as
/// quiet: an aborted worker drops the last sender clone.
async fn expect_quiet(rx: &mut mpsc::UnboundedReceiver<Forwarded>) {
    if let Ok(Some(forwarded)) = tokio::time::timeout(QUIET_PERIOD, rx.recv()).await {
        panic!("unexpected forwarded notification: {forwarded:?}");
    }
}

/// A worker only observes its empty pending set one coalescing window after its
/// last run, so draining the task set is a deadline, not an instant. Counting
/// re-lists or forwards at this point counts them at a quiescent state.
async fn expect_workers_drain(refresher: &ListChangedRefresher) {
    tokio::time::timeout(FORWARD_DEADLINE, async {
        while refresher.active_worker_count_for_test() > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("an idle worker removes itself from the task set");
}

async fn wait_for(deadline_label: &str, mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(FORWARD_DEADLINE, async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{deadline_label}"));
}

fn drain_forwards(rx: &mut mpsc::UnboundedReceiver<Forwarded>) -> Vec<Forwarded> {
    let mut seen = Vec::new();
    while let Ok(next) = rx.try_recv() {
        seen.push(next);
    }
    seen
}

#[tokio::test]
async fn one_upstreams_blocked_relist_does_not_block_another_upstreams_worker() {
    let pool = Arc::new(UpstreamPool::new());
    let slow = SubscriptionServer::accepting();
    let fast = SubscriptionServer::accepting();
    add_subscription_server(&pool, "slow", slow.clone()).await;
    add_subscription_server(&pool, "fast", fast.clone()).await;
    slow.close_resource_list_gate();
    fast.replace_tools_and_notify(&["added_after_list_changed"])
        .await;

    let (refresher, mut forwarded) =
        recording_refresher(Arc::clone(&pool), LIST_CHANGED_COALESCE_WINDOW);
    refresher.schedule("slow", ListChangedKinds::RESOURCES);
    refresher.schedule("fast", ListChangedKinds::TOOLS);

    // Bounded well under `catalog_listing_timeout`, so a worker pool that
    // serialized these two upstreams could not satisfy it by waiting out the
    // blocked listing.
    let (upstream, kinds) = expect_forwarded_within(&mut forwarded, UNBLOCKED_BOUND).await;
    assert_eq!(upstream, "fast");
    assert_eq!(kinds, ListChangedKinds::TOOLS);
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
    wait_for("the slow upstream's re-list starts", || {
        slow.resource_list_calls.load(Ordering::SeqCst) >= 1
    })
    .await;
    expect_quiet(&mut forwarded).await;

    slow.open_resource_list_gate();
    let (upstream, kinds) = expect_forwarded(&mut forwarded).await;
    assert_eq!(upstream, "slow");
    assert_eq!(kinds, ListChangedKinds::RESOURCES);
    expect_workers_drain(&refresher).await;
}

#[tokio::test]
async fn signals_inside_one_window_collapse_into_a_single_relist() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "leaf", server.clone()).await;
    let baseline = server.resource_list_calls.load(Ordering::SeqCst);

    // A long window, with real time passing between the signals: they merge
    // because the window is open, not because the worker never got polled.
    let (refresher, mut forwarded) = recording_refresher(Arc::clone(&pool), LONG_WINDOW);
    refresher.schedule("leaf", ListChangedKinds::RESOURCES);
    tokio::time::sleep(Duration::from_millis(200)).await;
    refresher.schedule("leaf", ListChangedKinds::RESOURCES);
    tokio::time::sleep(Duration::from_millis(200)).await;
    refresher.schedule("leaf", ListChangedKinds::RESOURCES);

    let (upstream, kinds) = expect_forwarded(&mut forwarded).await;
    assert_eq!(upstream, "leaf");
    assert_eq!(kinds, ListChangedKinds::RESOURCES);
    expect_workers_drain(&refresher).await;
    assert_eq!(
        server.resource_list_calls.load(Ordering::SeqCst) - baseline,
        1,
        "three signals inside one window are exactly one re-list"
    );
    assert!(
        drain_forwards(&mut forwarded).is_empty(),
        "and exactly one forwarded notification"
    );
}

#[tokio::test]
async fn a_signal_arriving_mid_refresh_runs_a_second_pass() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "leaf", server.clone()).await;
    let baseline = server.resource_list_calls.load(Ordering::SeqCst);
    server.close_resource_list_gate();

    let (refresher, mut forwarded) =
        recording_refresher(Arc::clone(&pool), LIST_CHANGED_COALESCE_WINDOW);
    refresher.schedule("leaf", ListChangedKinds::RESOURCES);
    wait_for("the worker enters its re-list", || {
        server.resource_list_calls.load(Ordering::SeqCst) > baseline
    })
    .await;

    // This lands while the worker is inside the refresh, so it must not be
    // swallowed by the batch already in flight.
    refresher.schedule("leaf", ListChangedKinds::RESOURCES);
    server.open_resource_list_gate();

    let first = expect_forwarded(&mut forwarded).await;
    let second = expect_forwarded(&mut forwarded).await;
    assert_eq!(first, ("leaf".to_string(), ListChangedKinds::RESOURCES));
    assert_eq!(second, first);
    expect_workers_drain(&refresher).await;
    assert_eq!(
        server.resource_list_calls.load(Ordering::SeqCst) - baseline,
        2,
        "the mid-refresh signal earns its own re-list"
    );
    assert!(drain_forwards(&mut forwarded).is_empty());
}

#[tokio::test]
async fn a_failed_resource_relist_still_forwards_exactly_once() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "leaf", server.clone()).await;
    pool.list_upstream_resources().await;
    assert_eq!(pool.cached_upstream_resources_allowed(None).await.len(), 1);
    server.fail_list_resources.store(true, Ordering::SeqCst);

    let (refresher, mut forwarded) =
        recording_refresher(Arc::clone(&pool), LIST_CHANGED_COALESCE_WINDOW);
    refresher.schedule("leaf", ListChangedKinds::RESOURCES);

    // A failed re-list withholds the upstream's rows, so what peers can see
    // did change: the notification must still be forwarded, exactly once.
    let (upstream, kinds) = expect_forwarded(&mut forwarded).await;
    assert_eq!(upstream, "leaf");
    assert_eq!(kinds, ListChangedKinds::RESOURCES);
    assert!(
        pool.cached_upstream_resources_allowed(None)
            .await
            .is_empty(),
        "stale rows are withheld after a failed re-list"
    );
    expect_workers_drain(&refresher).await;
    assert!(drain_forwards(&mut forwarded).is_empty());
}

#[tokio::test]
async fn a_failed_tool_relist_still_forwards_exactly_once() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "leaf", server.clone()).await;
    server.fail_list_tools.store(true, Ordering::SeqCst);
    let baseline = server.tool_list_calls.load(Ordering::SeqCst);

    let (refresher, mut forwarded) =
        recording_refresher(Arc::clone(&pool), LIST_CHANGED_COALESCE_WINDOW);
    refresher.schedule("leaf", ListChangedKinds::TOOLS);

    let (upstream, kinds) = expect_forwarded(&mut forwarded).await;
    assert_eq!(upstream, "leaf");
    assert_eq!(kinds, ListChangedKinds::TOOLS);
    assert!(
        server.tool_list_calls.load(Ordering::SeqCst) > baseline,
        "the failing re-list was actually attempted"
    );
    expect_workers_drain(&refresher).await;
    assert!(drain_forwards(&mut forwarded).is_empty());
}

#[tokio::test]
async fn merged_kinds_refresh_both_catalogs_and_forward_one_notification() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "leaf", server.clone()).await;
    server
        .replace_tools_and_notify(&["added_after_list_changed"])
        .await;
    let resource_baseline = server.resource_list_calls.load(Ordering::SeqCst);
    let tool_baseline = server.tool_list_calls.load(Ordering::SeqCst);

    let (refresher, mut forwarded) = recording_refresher(Arc::clone(&pool), LONG_WINDOW);
    refresher.schedule("leaf", ListChangedKinds::TOOLS);
    refresher.schedule("leaf", ListChangedKinds::RESOURCES);

    let (upstream, kinds) = expect_forwarded(&mut forwarded).await;
    assert_eq!(upstream, "leaf");
    assert_eq!(
        kinds,
        ListChangedKinds {
            tools: true,
            resources: true,
            prompts: false,
        },
        "one forward carries both kinds"
    );
    assert_eq!(
        server.tool_list_calls.load(Ordering::SeqCst) - tool_baseline,
        1
    );
    assert_eq!(
        server.resource_list_calls.load(Ordering::SeqCst) - resource_baseline,
        1
    );
    let tool_names = pool
        .healthy_tools_for_upstream("leaf")
        .await
        .into_iter()
        .map(|tool| tool.tool.name.to_string())
        .collect::<Vec<_>>();
    assert_eq!(tool_names, ["added_after_list_changed"]);
    expect_workers_drain(&refresher).await;
    assert!(drain_forwards(&mut forwarded).is_empty());
}

#[tokio::test]
async fn prompt_list_changed_is_forwarded_without_a_refresh() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "leaf", server.clone()).await;
    let resource_baseline = server.resource_list_calls.load(Ordering::SeqCst);
    let tool_baseline = server.tool_list_calls.load(Ordering::SeqCst);

    let (refresher, mut forwarded) =
        recording_refresher(Arc::clone(&pool), LIST_CHANGED_COALESCE_WINDOW);
    refresher.schedule("leaf", ListChangedKinds::PROMPTS);

    let (upstream, kinds) = expect_forwarded(&mut forwarded).await;
    assert_eq!(upstream, "leaf");
    assert_eq!(kinds, ListChangedKinds::PROMPTS);
    assert_eq!(
        server.resource_list_calls.load(Ordering::SeqCst),
        resource_baseline,
        "prompts/list is served live, so no catalog re-list runs"
    );
    assert_eq!(server.tool_list_calls.load(Ordering::SeqCst), tool_baseline);
}

#[tokio::test]
async fn a_worker_that_panics_does_not_strand_its_upstream() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "leaf", server.clone()).await;

    // The forwarder is caller-supplied, so an unwind out of it is a real
    // failure mode. The dying worker must release its claim on the upstream:
    // otherwise `schedule` keeps seeing a map entry, treats it as a live
    // worker, and merges every later signal into a pending set nobody reads —
    // the upstream would stop refreshing for the life of the pool.
    let (tx, mut forwarded) = mpsc::unbounded_channel();
    let forwards = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let panics = Arc::clone(&forwards);
    let refresher = ListChangedRefresher::with_coalesce_window(
        Arc::clone(&pool),
        LIST_CHANGED_COALESCE_WINDOW,
        move |upstream: &str, kinds| {
            drop(tx.send((upstream.to_string(), kinds)));
            assert!(
                panics.fetch_add(1, Ordering::SeqCst) != 0,
                "forwarder unwinds on its first call"
            );
        },
    );

    refresher.schedule("leaf", ListChangedKinds::RESOURCES);
    assert_eq!(
        expect_forwarded(&mut forwarded).await,
        ("leaf".to_string(), ListChangedKinds::RESOURCES)
    );
    wait_for("the panicking worker releases its entry", || {
        refresher.active_worker_count_for_test() == 0
    })
    .await;

    // A fresh worker must take the upstream over.
    refresher.schedule("leaf", ListChangedKinds::RESOURCES);
    assert_eq!(
        expect_forwarded(&mut forwarded).await,
        ("leaf".to_string(), ListChangedKinds::RESOURCES),
        "the upstream still refreshes and forwards after its worker died"
    );
    expect_workers_drain(&refresher).await;
}

#[tokio::test]
async fn dropping_the_refresher_aborts_in_flight_workers() {
    let pool = Arc::new(UpstreamPool::new());
    let server = SubscriptionServer::accepting();
    add_subscription_server(&pool, "slow", server.clone()).await;
    server.close_resource_list_gate();

    let (refresher, mut forwarded) =
        recording_refresher(Arc::clone(&pool), LIST_CHANGED_COALESCE_WINDOW);
    refresher.schedule("slow", ListChangedKinds::RESOURCES);
    wait_for("the worker starts its re-list", || {
        server.resource_list_calls.load(Ordering::SeqCst) > 0
    })
    .await;
    let handle = refresher
        .worker_handle_for_test("slow")
        .expect("the worker is registered while it refreshes");

    drop(refresher);
    server.open_resource_list_gate();
    expect_quiet(&mut forwarded).await;
    assert!(
        handle.is_finished(),
        "the worker task is terminated, not merely unreferenced"
    );
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
