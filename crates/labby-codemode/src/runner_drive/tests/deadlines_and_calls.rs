use super::*;

#[test]
fn settlement_watch_uses_full_grace_when_execution_budget_allows() {
    let now = tokio::time::Instant::now();
    let execution_deadline = now + Duration::from_secs(10);
    let watch = SettlementWatch::new(now, execution_deadline);

    assert_eq!(watch.deadline, now + RUNNER_SETTLEMENT_GRACE);
    assert!(watch.grace_limited);
}

#[test]
fn settlement_watch_preserves_outer_deadline_as_the_actual_limiter() {
    let now = tokio::time::Instant::now();
    let execution_deadline = now + Duration::from_millis(250);
    let watch = SettlementWatch::new(now, execution_deadline);

    assert_eq!(watch.deadline, execution_deadline);
    assert!(!watch.grace_limited);
}

#[test]
fn settlement_watch_treats_equal_deadlines_as_outer_limited() {
    let now = tokio::time::Instant::now();
    let execution_deadline = now + RUNNER_SETTLEMENT_GRACE;
    let watch = SettlementWatch::new(now, execution_deadline);

    assert_eq!(watch.deadline, execution_deadline);
    assert!(!watch.grace_limited);
}

#[test]
fn external_tool_deadline_reserves_runner_settlement_budget() {
    let now = tokio::time::Instant::now();
    let execution_deadline = now + Duration::from_secs(30);

    assert_eq!(
        external_tool_deadline(now, execution_deadline, 0),
        execution_deadline - RUNNER_RESULT_ACK_RESERVE
    );
}

#[test]
fn result_ack_reserve_scales_with_fanout_and_is_capped() {
    assert_eq!(result_ack_reserve(0), RUNNER_RESULT_ACK_RESERVE);
    // The 512-call default gets ~1.3s to drain, not 250ms.
    assert_eq!(
        result_ack_reserve(512),
        RUNNER_RESULT_ACK_RESERVE + RUNNER_RESULT_ACK_PER_CALL * 512
    );
    assert_eq!(result_ack_reserve(u64::MAX), RUNNER_RESULT_ACK_RESERVE_MAX);
    assert!(RUNNER_RESULT_ACK_RESERVE_MAX < RUNNER_SETTLEMENT_GRACE);
}

#[test]
fn external_tool_deadline_keeps_short_execution_budget_intact() {
    let now = tokio::time::Instant::now();
    let execution_deadline = now + Duration::from_millis(400);

    assert_eq!(
        external_tool_deadline(now, execution_deadline, 0),
        execution_deadline
    );
}

/// A slow external tool must time out early enough that the sandbox can
/// consume the ToolError and acknowledge completion inside the same run,
/// rather than surfacing an ambiguous post-tool settlement failure.
#[cfg(not(windows))]
#[tokio::test]
async fn external_tool_timeout_leaves_budget_for_runner_acknowledgement() {
    let host = DelayedToolHost {
        inner: NoopHost::default(),
        delay: Duration::from_secs(2),
    };
    let broker = CodeModeBroker::new(Some(&host));
    let counters_before = code_mode_runtime_counters();
    let script = r#"
IFS= read -r _
printf '%s\n' '{"type":"tool_call","seq":1,"id":"stub::slow","params":{}}'
IFS= read -r reply
case "$reply" in
  *'"type":"tool_error"'*) ;;
  *) exit 23 ;;
esac
printf '%s\n' '{"type":"done","result":{"state":"json","value":{"acknowledged":true}},"logs":[]}'
sleep 3600
"#;
    let mut runner = PooledRunner::spawn_stub_script(script).expect("spawn acknowledgement stub");
    let started = std::time::Instant::now();
    let outcome = broker
        .drive_runner(
            &mut runner,
            &test_config(Duration::from_secs(2)),
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await;

    // The stub script ends in `sleep 3600`, so any ceiling far below that
    // proves the 2s deadline — not the script — ended the run. The real
    // assertions are on `outcome` below. A 3s ceiling sat close enough to
    // the 2s deadline that scheduler jitter under parallel test load failed
    // correct runs.
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "external timeout must leave time for the runner acknowledgement: {:?}",
        started.elapsed()
    );
    match outcome {
        DriveOutcome::Completed(response) => {
            assert_eq!(response.result, Some(json!({"acknowledged": true})));
            assert_eq!(response.calls.len(), 1);
            assert!(!response.calls[0].ok);
            assert_eq!(response.calls[0].error_kind.as_deref(), Some("timeout"));
            assert!(
                code_mode_runtime_counters().result_ack_reserve_uses
                    >= counters_before.result_ack_reserve_uses.saturating_add(1),
                "execution should record arming the result acknowledgement reserve"
            );
        }
        DriveOutcome::ExecutionError(error)
        | DriveOutcome::RunnerUnhealthy(error)
        | DriveOutcome::RunnerUnavailableBeforeActivity(error) => {
            panic!("tool timeout should settle through Done, got {error:?}");
        }
    }
}

/// The default 512-call fan-out ceiling must still fit its ToolError -> Done
/// control-plane traffic inside the reserved acknowledgement slice when all
/// calls hit the same external deadline together. This exercises the worst
/// common-case burst rather than only a single call.
#[cfg(not(windows))]
#[tokio::test]
async fn result_ack_reserve_handles_default_max_call_fanout() {
    // `awk` emits the calls and validates the replies in one process each:
    // a `sh` read-loop over 512 replies measures shell throughput rather
    // than the reserve, and starves on a loaded host.
    let script = r#"
IFS= read -r _
awk 'BEGIN{for(i=1;i<=512;i++) printf "{\"type\":\"tool_call\",\"seq\":%d,\"id\":\"stub::slow\",\"params\":{}}\n", i}'
awk '{ if ($0 !~ /"type":"tool_error"/) exit 24; n++; if (n==512) exit 0 } END{ if (n<512) exit 25 }'
status=$?
[ "$status" -eq 0 ] || exit "$status"
printf '%s\n' '{"type":"done","result":{"state":"json","value":{"acked":512}},"logs":[]}'
sleep 3600
"#;
    // Large enough that the scaled reserve reaches its cap, so the drain
    // budget under test is the product's maximum rather than a fraction
    // of a tight test deadline.
    let budget = Duration::from_secs(5);
    let host = DelayedToolHost {
        inner: NoopHost::default(),
        delay: budget,
    };
    let broker = CodeModeBroker::new(Some(&host));
    let mut runner = PooledRunner::spawn_stub_script(script).expect("spawn fanout stub");
    let started = std::time::Instant::now();
    let outcome = broker
        .drive_runner(
            &mut runner,
            &test_config(budget),
            tokio::time::Instant::now() + budget,
        )
        .await;

    // Same reasoning as the single-call case above: the stub ends in
    // `sleep 3600`, so this ceiling only has to be far below that.
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the execution deadline, not the stub script, must end the run: {:?}",
        started.elapsed()
    );
    match outcome {
        DriveOutcome::Completed(response) => {
            assert_eq!(response.result, Some(json!({"acked": 512})));
            assert_eq!(response.calls.len(), 512);
            assert!(response.calls.iter().all(|call| !call.ok));
            assert!(
                response
                    .calls
                    .iter()
                    .all(|call| { call.error_kind.as_deref() == Some("timeout") })
            );
        }
        DriveOutcome::ExecutionError(error)
        | DriveOutcome::RunnerUnhealthy(error)
        | DriveOutcome::RunnerUnavailableBeforeActivity(error) => {
            panic!("fanout acknowledgements must settle inside the reserve: {error:?}");
        }
    }
}

/// The `openapi` provider must dispatch WITHOUT taking `LOCAL_PROVIDER_LOCK`,
/// so a slow state/git op holding that lock cannot stall it. Holding the lock
/// and dispatching an (empty-registry) openapi call must return quickly with
/// an `unknown_instance` error rather than blocking on the mutex.
#[tokio::test]
async fn openapi_dispatch_does_not_block_on_local_provider_lock() {
    let lock = LOCAL_PROVIDER_LOCK.get_or_init(|| Mutex::new(()));
    let _held = lock.lock().await; // a slow state/git op holds the lock
    let reg = labby_openapi::OpenApiRegistry::default();
    let client = labby_openapi::http::build_dispatch_client().expect("test dispatch client");
    let call = LocalProviderCall {
        provider: LocalProviderName::Openapi,
        method: "vendor.getUser".to_string(),
        params: Value::Null,
    };
    let res = tokio::time::timeout(
        Duration::from_secs(2),
        dispatch_openapi_provider::<NoopHost>(
            &reg,
            &client,
            None,
            &CodeModeCaller::TrustedLocal,
            &ToolScope::default(),
            call,
            serde_json::json!({}),
        ),
    )
    .await;
    assert!(res.is_ok(), "openapi must not block on LOCAL_PROVIDER_LOCK");
    // Empty registry ⇒ unknown label.
    assert_eq!(res.unwrap().unwrap_err().kind(), "unknown_instance");
}

/// Budget/trace exclusion for reserved `__lab_internal::` pseudo-tool
/// calls: an internal call interleaved with exactly `max_calls_per_run`
/// ordinary calls must (a) never appear in the call trace and (b) never
/// consume a budget slot — if it did, the last ordinary call would be
/// rejected with `call_budget_exceeded`.
#[cfg(not(windows))]
#[tokio::test]
async fn drive_runner_excludes_lab_internal_calls_from_budget_and_trace() {
    let budget = max_calltool_per_run();
    // The stub emits 1 internal ToolCall + `budget` ordinary ToolCalls,
    // waits for the host to settle them (a background `cat` drains stdin
    // so ToolResult/ToolError writes back to the stub never block), then
    // emits Done.
    let script = format!(
        r#"
exec 3<&0
cat <&3 >/dev/null &
printf '{{"type":"tool_call","seq":1,"id":"__lab_internal::semantic_rank","params":{{"query":"q","limit":5}}}}\n'
i=2
while [ "$i" -le {last_seq} ]; do
  printf '{{"type":"tool_call","seq":%d,"id":"stub::tool","params":{{}}}}\n' "$i"
  i=$((i+1))
done
sleep 2
printf '{{"type":"done"}}\n'
sleep 3600
"#,
        last_seq = budget + 1
    );
    let host = NoopHost::default();
    let broker = CodeModeBroker::new(Some(&host));
    let mut runner = PooledRunner::spawn_stub_script(&script).expect("spawn script stub");
    let outcome = broker
        .drive_runner(
            &mut runner,
            &test_config(Duration::from_secs(30)),
            tokio::time::Instant::now() + Duration::from_secs(30),
        )
        .await;
    let response = match outcome {
        DriveOutcome::Completed(response) => response,
        DriveOutcome::ExecutionError(err)
        | DriveOutcome::RunnerUnavailableBeforeActivity(err)
        | DriveOutcome::RunnerUnhealthy(err) => {
            panic!("run must complete, got error kind `{}`", err.kind())
        }
    };
    assert!(
        response
            .calls
            .iter()
            .all(|call| !call.id.starts_with("__lab_internal::")),
        "internal calls must not appear in the call trace"
    );
    assert_eq!(
        response.calls.len(),
        usize::try_from(budget).expect("budget fits usize"),
        "every ordinary call must be traced"
    );
    assert!(
        response
            .calls
            .iter()
            .all(|call| call.error_kind.as_deref() != Some("call_budget_exceeded")),
        "the internal call must not consume a budget slot"
    );
}
