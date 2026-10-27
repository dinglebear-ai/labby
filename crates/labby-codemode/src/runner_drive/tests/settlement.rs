use super::*;

/// Regression for the 60-second outer stall: after the host has settled
/// every tool call, a runner that never emits Done/Error must be evicted
/// after the short settlement grace rather than consuming the full run
/// timeout.

#[cfg(not(windows))]
#[tokio::test]
async fn drive_runner_bounds_post_tool_settlement() {
    let script = r#"
exec 3<&0
cat <&3 >/dev/null &
printf '{"type":"tool_call","seq":1,"id":"stub::tool","params":{}}\n'
sleep 3600
"#;
    let host = NoopHost::default();
    let broker = CodeModeBroker::new(Some(&host));
    let counters_before = code_mode_runtime_counters();
    let mut runner = PooledRunner::spawn_stub_script(script).expect("spawn script stub");
    let started = std::time::Instant::now();
    let outcome = broker
        .drive_runner(
            &mut runner,
            &test_config(Duration::from_secs(30)),
            tokio::time::Instant::now() + Duration::from_secs(30),
        )
        .await;

    // The claim is "settlement did not eat the 30s outer timeout", so the
    // ceiling only has to stay under 30s. Sitting just below it leaves the
    // most room for scheduler jitter while keeping the proof exact.
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "post-tool settlement must not consume the 30-second outer timeout: {:?}",
        started.elapsed()
    );
    match outcome {
        DriveOutcome::RunnerUnhealthy(err) => {
            assert_eq!(err.kind(), "timeout");
            assert!(err.to_string().contains("did not settle"));
            assert!(
                code_mode_runtime_counters().settlement_watchdog_expiries
                    >= counters_before
                        .settlement_watchdog_expiries
                        .saturating_add(1),
                "genuine post-tool watchdog expiry should increment its runtime counter"
            );
        }
        DriveOutcome::Completed(_)
        | DriveOutcome::ExecutionError(_)
        | DriveOutcome::RunnerUnavailableBeforeActivity(_) => {
            panic!("a runner that never emits Done/Error must be evicted")
        }
    }
}

/// When a tool settles too late for the full post-tool grace to fit inside
/// the execution budget, the outer Code Mode deadline is the real limiter.
/// Do not mislabel that as a runner settlement failure.
#[cfg(not(windows))]
#[tokio::test]
async fn drive_runner_reports_outer_timeout_when_settlement_budget_is_truncated() {
    let script = r#"
exec 3<&0
cat <&3 >/dev/null &
sleep 0.55
printf '{"type":"tool_call","seq":1,"id":"stub::tool","params":{}}\n'
sleep 3600
"#;
    let host = NoopHost::default();
    let broker = CodeModeBroker::new(Some(&host));
    let mut runner = PooledRunner::spawn_stub_script(script).expect("spawn script stub");
    let outcome = broker
        .drive_runner(
            &mut runner,
            &test_config(Duration::from_millis(800)),
            tokio::time::Instant::now() + Duration::from_millis(800),
        )
        .await;

    match outcome {
        DriveOutcome::RunnerUnhealthy(err) => {
            assert_eq!(err.kind(), "timeout");
            let message = err.to_string();
            assert!(message.contains("Code Mode execution timed out"));
            assert!(!message.contains("did not settle"));
        }
        DriveOutcome::Completed(_)
        | DriveOutcome::ExecutionError(_)
        | DriveOutcome::RunnerUnavailableBeforeActivity(_) => {
            panic!("the outer deadline must terminate the late-settling runner")
        }
    }
}

/// The wall-clock deadline path: a runner that never replies is killed when
/// the deadline fires, the run surfaces the stable `timeout` kind, and the
/// runner is classified `RunnerUnhealthy` so the pool evicts (never reuses) a
/// runtime interrupted mid-execution.
#[tokio::test]
async fn drive_runner_times_out_and_marks_runner_unhealthy() {
    let broker: CodeModeBroker<'_, NoopHost> = CodeModeBroker::new(None);
    let mut runner = PooledRunner::spawn_stub_silent().expect("spawn silent stub");
    let outcome = broker
        .drive_runner(
            &mut runner,
            &test_config(Duration::from_millis(80)),
            tokio::time::Instant::now() + Duration::from_millis(80),
        )
        .await;
    match outcome {
        DriveOutcome::RunnerUnhealthy(err) => {
            assert_eq!(
                err.kind(),
                "timeout",
                "wall-clock expiry must surface the `timeout` kind"
            );
        }
        DriveOutcome::Completed(_)
        | DriveOutcome::ExecutionError(_)
        | DriveOutcome::RunnerUnavailableBeforeActivity(_) => {
            panic!("a never-replying runner must time out as RunnerUnhealthy")
        }
    }
}
