use super::*;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn expired_dispatch_deadline_prevents_host_effects() {
    let mut host = FixtureHost::new(Vec::new());
    host.tool_call_result = Some(json!({"ok": true}));
    let broker = CodeModeBroker::new(Some(&host));
    let result = broker
        .call_tool_id_before_deadline(
            "fixture::mutate",
            json!({}),
            tokio::time::Instant::now() - Duration::from_secs(1),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
            &CancellationToken::new(),
        )
        .await;
    assert_eq!(
        result.expect_err("expired dispatch must fail").kind(),
        "timeout"
    );
    assert_eq!(host.tool_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn cancelled_dispatch_prevents_ready_host_effects() {
    let mut host = FixtureHost::new(Vec::new());
    host.tool_call_result = Some(json!({"ok": true}));
    let broker = CodeModeBroker::new(Some(&host));
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    for _ in 0..100 {
        let result = broker
            .call_tool_id_before_deadline(
                "fixture::mutate",
                json!({}),
                tokio::time::Instant::now() + Duration::from_secs(1),
                CodeModeCaller::TrustedLocal,
                CodeModeSurface::Cli,
                &ToolScope::default(),
                ExecCtx::none(),
                &cancellation,
            )
            .await;
        assert_eq!(
            result.expect_err("cancelled dispatch must fail").kind(),
            "cancelled"
        );
    }
    assert_eq!(host.tool_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn healthy_dispatch_reaches_host_once() {
    let mut host = FixtureHost::new(Vec::new());
    host.tool_call_result = Some(json!({"ok": true}));
    let broker = CodeModeBroker::new(Some(&host));
    let result = broker
        .call_tool_id_before_deadline(
            "fixture::mutate",
            json!({}),
            tokio::time::Instant::now() + Duration::from_secs(1),
            CodeModeCaller::TrustedLocal,
            CodeModeSurface::Cli,
            &ToolScope::default(),
            ExecCtx::none(),
            &CancellationToken::new(),
        )
        .await
        .expect("healthy dispatch succeeds");
    assert_eq!(result.value, json!({"ok": true}));
    assert_eq!(host.tool_calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn cancellation_still_interrupts_an_in_flight_dispatch() {
    let mut host = FixtureHost::new(Vec::new());
    host.tool_call_pending = true;
    let broker = CodeModeBroker::new(Some(&host));
    let cancellation = CancellationToken::new();
    let scope = ToolScope::default();
    let dispatch = broker.call_tool_id_before_deadline(
        "fixture::mutate",
        json!({}),
        tokio::time::Instant::now() + Duration::from_secs(1),
        CodeModeCaller::TrustedLocal,
        CodeModeSurface::Cli,
        &scope,
        ExecCtx::none(),
        &cancellation,
    );
    let cancel_after_dispatch = async {
        while host.tool_calls.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(dispatch, cancel_after_dispatch);
    assert_eq!(
        result.expect_err("in-flight call is cancelled").kind(),
        "cancelled"
    );
    assert_eq!(host.tool_calls.load(Ordering::Relaxed), 1);
}
