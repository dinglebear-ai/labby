//! Q3 real-process Code Mode fanout and dependency qualification.

#![cfg(all(feature = "gateway", feature = "proxy-testkit"))]
#![allow(clippy::panic)]
#![allow(dead_code, reason = "shared real-process harness has a broader API")]

#[path = "support/codemode_qualification/harness.rs"]
mod codemode_harness;
#[path = "support/evidence.rs"]
mod evidence;
#[path = "support/live_labby.rs"]
mod live_labby;

use codemode_harness::{CodeModeQualification, Limits, write_report};
use serde_json::json;

#[tokio::test]
async fn q3_discovers_and_describes_the_live_fixture_before_execution() {
    let limits = Limits::default();
    let runner = CodeModeQualification::start(limits)
        .await
        .expect("Q3 runner");
    let before = runner
        .fixture_invocation_count()
        .await
        .expect("initial effects");
    let execution = runner
        .execute(
            r#"async () => {
      const hits = await codemode.search({query:"safe", limit:5});
      const hit = hits.results.find((row) => row.path === "forge.forge_safe");
      if (!hit) throw new Error("literal forge.forge_safe discovery result missing");
      const docs = await codemode.describe(hit.path);
      const value = await callTool("forge::forge.safe", {query:"discover-first",limit:1,enabled:true});
      return {hit:hit.path, described:docs.path, value:value};
    }"#,
        )
        .await
        .expect("discover/describe/execute response");
    assert!(
        !execution.is_error,
        "qualification result: {}",
        execution.structured
    );
    assert_eq!(
        execution.structured["result"]["hit"],
        json!("forge.forge_safe"),
        "search result: {}",
        execution.structured
    );
    assert_eq!(
        execution.structured["result"]["described"],
        json!("forge.forge_safe")
    );
    assert_eq!(
        execution.structured["result"]["value"]["arguments"]["query"],
        json!("discover-first")
    );
    let after = runner
        .fixture_invocation_count()
        .await
        .expect("final effects");
    assert_eq!(
        after - before,
        1,
        "discovery is inert and the call runs once"
    );
    let identity = runner.identity();
    let cleanup = runner.finish().await;
    write_report(
        "discover-describe-execute",
        &identity,
        limits,
        &execution,
        before,
        after,
        &cleanup,
    )
    .expect("discovery evidence");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn q3_fanout_preserves_partial_error_and_exact_effect_count() {
    let limits = Limits::default();
    let runner = CodeModeQualification::start(limits)
        .await
        .expect("Q3 runner");
    let before = runner
        .fixture_invocation_count()
        .await
        .expect("initial effects");
    let execution = runner
        .execute(
            r#"async () => {
      const settled = await Promise.allSettled([
        callTool("forge::forge.safe", {query:"literal-one",limit:1,enabled:true}),
        callTool("forge::forge.error", {})
      ]);
      return settled.map((entry) => entry.status === "fulfilled"
        ? {status:"fulfilled", value:entry.value}
        : {status:"rejected", error:String(entry.reason && entry.reason.message || entry.reason)});
    }"#,
        )
        .await
        .expect("fanout response");
    assert!(!execution.is_error, "fanout top-level result must succeed");
    assert_eq!(
        execution.structured["result"][0]["status"],
        json!("fulfilled")
    );
    assert_eq!(
        execution.structured["result"][1]["status"],
        json!("rejected")
    );
    assert_eq!(
        execution.structured["calls"].as_array().map(Vec::len),
        Some(2)
    );
    let after = runner
        .fixture_invocation_count()
        .await
        .expect("final effects");
    assert_eq!(
        after - before,
        2,
        "each fanout branch executes exactly once"
    );
    assert!(execution.elapsed.as_secs() < 10);
    assert!(execution.wire_bytes <= 1024 * 1024);
    let identity = runner.identity();
    let cleanup = runner.finish().await;
    write_report(
        "fanout-partial-error",
        &identity,
        limits,
        &execution,
        before,
        after,
        &cleanup,
    )
    .expect("fanout evidence");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn q3_dependent_call_consumes_actual_first_result() {
    let limits = Limits::default();
    let runner = CodeModeQualification::start(limits)
        .await
        .expect("Q3 runner");
    let before = runner
        .fixture_invocation_count()
        .await
        .expect("initial effects");
    let execution = runner.execute(r#"async () => {
      const first = await callTool("forge::forge.safe", {query:"dependency-source",limit:1,enabled:true});
      const actual = JSON.stringify(first);
      const second = await callTool("forge::forge.safe", {query:actual,limit:2,enabled:true});
      return {actual:actual, first:first, second:second};
    }"#).await.expect("dependent response");
    assert!(
        !execution.is_error,
        "dependent top-level result must succeed: {}",
        execution.structured
    );
    let actual = execution.structured["result"]["actual"]
        .as_str()
        .expect("actual first text");
    assert!(actual.contains("dependency-source"));
    assert_eq!(
        execution.structured["result"]["first"]["arguments"]["query"],
        json!("dependency-source")
    );
    assert_eq!(
        execution.structured["calls"][1]["params"]["query"],
        json!(actual)
    );
    assert_eq!(
        execution.structured["result"]["second"]["arguments"]["query"],
        json!(actual),
        "the leaf fixture must receive the actual first result"
    );
    let after = runner
        .fixture_invocation_count()
        .await
        .expect("final effects");
    assert_eq!(after - before, 2, "dependency chain executes exactly twice");
    let identity = runner.identity();
    let cleanup = runner.finish().await;
    write_report(
        "dependent-actual-result",
        &identity,
        limits,
        &execution,
        before,
        after,
        &cleanup,
    )
    .expect("dependency evidence");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn q3_seeded_bounded_stress_has_literal_counts_and_no_duplicate_effects() {
    const WORKLOAD: u64 = 12;
    const EXPECTED_ERRORS: u64 = 3;
    // This fixture verifies fanout accounting, not the deadline boundary.
    // Leave room for twelve concurrent calls under the full CI test matrix;
    // the timeout-specific case below exercises the strict budget.
    let limits = Limits {
        timeout_ms: 5_000,
        ..Limits::default()
    };
    let runner = CodeModeQualification::start(limits)
        .await
        .expect("Q3 runner");
    let before = runner
        .fixture_invocation_count()
        .await
        .expect("initial effects");
    let stress_code = [
        r"async () => {
      const jobs = Array.from(",
        "{",
        "length:12",
        "}",
        r#", (_, index) => index % 4 === 3
        ? callTool("forge::forge.error", {})
        : callTool("forge::forge.safe", {query:"seed-424242-" + index,limit:1,enabled:true}));
      const settled = await Promise.allSettled(jobs);
      return {
        fulfilled:settled.filter((row) => row.status === "fulfilled").length,
        rejected:settled.filter((row) => row.status === "rejected").length
      };
    }"#,
    ]
    .concat();
    let execution = runner
        .execute(&stress_code)
        .await
        .expect("bounded stress response");
    assert!(!execution.is_error);
    assert_eq!(
        execution.structured["result"]["fulfilled"],
        json!(9),
        "stress result: {}",
        execution.structured
    );
    assert_eq!(
        execution.structured["result"]["rejected"],
        json!(EXPECTED_ERRORS)
    );
    assert_eq!(
        execution.structured["calls"].as_array().map(Vec::len),
        Some(WORKLOAD as usize)
    );
    let after = runner
        .fixture_invocation_count()
        .await
        .expect("stress effects");
    assert_eq!(after - before, WORKLOAD, "stress calls execute once each");
    assert!(execution.elapsed < std::time::Duration::from_secs(10));
    let identity = runner.identity();
    let cleanup = runner.finish().await;
    write_report(
        "seeded-bounded-stress",
        &identity,
        limits,
        &execution,
        before,
        after,
        &cleanup,
    )
    .expect("stress evidence");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn q3_execution_timeout_is_typed_and_does_not_duplicate_the_effect() {
    let limits = Limits {
        // Use the normal two-second request budget. Code Mode reserves 500ms
        // for response delivery, leaving 1.5s for cold proxy generation and
        // execution; the deliberately pending upstream still takes 10s.
        timeout_ms: 2_000,
        ..Limits::default()
    };
    let runner = CodeModeQualification::start(limits)
        .await
        .expect("Q3 runner");
    assert_eq!(runner.limits.timeout_ms, 2_000);
    let prewarm = runner
        .execute(
            r#"async () => await callTool("forge::forge.safe", {query:"prewarm",limit:1,enabled:true})"#,
        )
        .await
        .expect("prewarm Code Mode and the upstream tool path");
    assert!(
        !prewarm.is_error,
        "prewarm must complete before the oracle: {}",
        prewarm.structured
    );
    let before = runner
        .fixture_invocation_count()
        .await
        .expect("initial effects");
    let execution = runner
        .execute(
            r#"async () => {
          await callTool("forge::forge.pending", {});
          return await callTool("forge::forge.safe", {query:"must-not-run",limit:1,enabled:true});
        }"#,
        )
        .await
        .expect("bounded timeout response");
    assert!(execution.is_error, "deadline must be a typed MCP error");
    assert_eq!(execution.structured["error"]["kind"], json!("timeout"));
    runner
        .fixture_settlement_barrier()
        .await
        .expect("fixture calls settle after cancellation");
    let after = runner
        .fixture_invocation_count()
        .await
        .expect("post-timeout effects");
    assert_eq!(
        after - before,
        1,
        "timed out call starts once and is never retried"
    );
    let identity = runner.identity();
    let cleanup = runner.finish().await;
    write_report(
        "timeout-effect-fence",
        &identity,
        limits,
        &execution,
        before,
        after,
        &cleanup,
    )
    .expect("timeout evidence");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn q3_output_limit_returns_literal_truncation_marker() {
    let limits = Limits {
        max_response_bytes: 4_096,
        max_response_tokens: 1_024,
        ..Limits::default()
    };
    let runner = CodeModeQualification::start(limits)
        .await
        .expect("Q3 runner");
    let before = runner
        .fixture_invocation_count()
        .await
        .expect("initial effects");
    let execution = runner
        .execute(r#"async () => await callTool("forge::forge.large", {})"#)
        .await
        .expect("bounded output response");
    assert!(!execution.is_error);
    assert_eq!(execution.structured["result"]["truncated"], json!(true));
    assert!(
        execution.structured["result"]["original_size"]
            .as_u64()
            .is_some_and(|size| size > 1_000_000)
    );
    assert!(execution.wire_bytes <= limits.max_response_bytes);
    let after = runner
        .fixture_invocation_count()
        .await
        .expect("final effects");
    assert_eq!(after - before, 1);
    let identity = runner.identity();
    let cleanup = runner.finish().await;
    write_report(
        "output-limit",
        &identity,
        limits,
        &execution,
        before,
        after,
        &cleanup,
    )
    .expect("output evidence");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn q3_call_budget_rejects_before_the_third_effect() {
    let limits = Limits {
        max_calls_per_run: Some(2),
        ..Limits::default()
    };
    let runner = CodeModeQualification::start(limits)
        .await
        .expect("Q3 runner");
    let before = runner
        .fixture_invocation_count()
        .await
        .expect("initial effects");
    let execution = runner
        .execute(
            r#"async () => {
          const outcomes = [];
          for (let index = 0; index < 3; index++) {
            try {
              outcomes.push({ok:true,value:await callTool("forge::forge.safe", {query:"budget-" + index,limit:1,enabled:true})});
            } catch (reason) {
              outcomes.push({ok:false,error:JSON.parse(String(reason.message))});
            }
          }
          return outcomes;
        }"#,
        )
        .await
        .expect("call budget response");
    assert!(!execution.is_error);
    assert_eq!(execution.structured["result"][0]["ok"], json!(true));
    assert_eq!(execution.structured["result"][1]["ok"], json!(true));
    assert_eq!(execution.structured["result"][2]["ok"], json!(false));
    assert_eq!(
        execution.structured["result"][2]["error"]["kind"],
        json!("call_budget_exceeded")
    );
    let after = runner
        .fixture_invocation_count()
        .await
        .expect("final effects");
    assert_eq!(after - before, 2, "third call is rejected before dispatch");
    let identity = runner.identity();
    let cleanup = runner.finish().await;
    write_report(
        "call-budget",
        &identity,
        limits,
        &execution,
        before,
        after,
        &cleanup,
    )
    .expect("call-budget evidence");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn q3_queue_limit_rejects_before_dispatch_and_settles_started_effect() {
    let limits = Limits {
        // Match the normal Code Mode budget so the 500ms response reserve
        // cannot exhaust a cold proxy before the queue oracle begins.
        timeout_ms: 2_000,
        upstream_request_timeout_ms: Some(50),
        upstream_max_in_flight: Some(1),
        ..Limits::default()
    };
    let runner = CodeModeQualification::start(limits)
        .await
        .expect("Q3 runner");
    let prewarm = runner
        .execute(
            r#"async () => await callTool("forge::forge.safe", {query:"prewarm",limit:1,enabled:true})"#,
        )
        .await
        .expect("prewarm Code Mode and the upstream tool path");
    assert!(
        !prewarm.is_error,
        "prewarm must complete before the queue oracle: {}",
        prewarm.structured
    );
    let before = runner
        .fixture_invocation_count()
        .await
        .expect("initial effects");
    let execution = runner
        .execute(
            r#"async () => {
          const settled = await Promise.allSettled([
            callTool("forge::forge.delay", {}),
            callTool("forge::forge.delay", {})
          ]);
          return settled.map((row) => ({
            status:row.status,
            kind:row.status === "rejected" ? JSON.parse(String(row.reason.message)).kind : "unexpected-success"
          }));
        }"#,
        )
        .await
        .expect("queue-bound response");
    assert!(
        !execution.is_error,
        "queue response: {}",
        execution.structured
    );
    let mut kinds = execution.structured["result"]
        .as_array()
        .expect("two queue outcomes")
        .iter()
        .map(|row| row["kind"].as_str().expect("typed queue error"))
        .collect::<Vec<_>>();
    kinds.sort_unstable();
    assert_eq!(
        kinds,
        vec!["queue_saturated", "timeout"],
        "queue trace: {}",
        execution.structured
    );
    runner
        .fixture_settlement_barrier()
        .await
        .expect("settlement");
    let after = runner
        .fixture_invocation_count()
        .await
        .expect("final effects");
    assert_eq!(after - before, 1, "queued call never reaches the fixture");
    let identity = runner.identity();
    let cleanup = runner.finish().await;
    write_report(
        "queue-limit",
        &identity,
        limits,
        &execution,
        before,
        after,
        &cleanup,
    )
    .expect("queue evidence");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn q3_fixed_memory_limit_fails_without_effect_and_runner_recovers() {
    let limits = Limits::default();
    let runner = CodeModeQualification::start(limits)
        .await
        .expect("Q3 runner");
    let before = runner
        .fixture_invocation_count()
        .await
        .expect("initial effects");
    let execution = runner
        .execute(r"async () => new ArrayBuffer(80 * 1024 * 1024)")
        .await
        .expect("memory-bound response");
    assert!(execution.is_error);
    assert_eq!(execution.structured["error"]["kind"], json!("server_error"));
    let after_failure = runner
        .fixture_invocation_count()
        .await
        .expect("failed effects");
    assert_eq!(
        after_failure, before,
        "memory failure runs no upstream tool"
    );
    let recovery = runner
        .execute(
            r#"async () => await callTool("forge::forge.safe", {query:"after-memory-limit",limit:1,enabled:true})"#,
        )
        .await
        .expect("runner recovery response");
    assert!(!recovery.is_error);
    assert_eq!(
        recovery.structured["result"]["arguments"]["query"],
        json!("after-memory-limit")
    );
    let after = runner
        .fixture_invocation_count()
        .await
        .expect("recovery effects");
    assert_eq!(after - before, 1);
    let identity = runner.identity();
    let cleanup = runner.finish().await;
    write_report(
        "memory-limit",
        &identity,
        limits,
        &execution,
        before,
        after,
        &cleanup,
    )
    .expect("memory evidence");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}

#[tokio::test]
async fn q3_cpu_loop_is_bounded_by_execution_deadline_without_effect() {
    let limits = Limits {
        timeout_ms: 50,
        ..Limits::default()
    };
    let runner = CodeModeQualification::start(limits)
        .await
        .expect("Q3 runner");
    let before = runner
        .fixture_invocation_count()
        .await
        .expect("initial effects");
    let execution = runner
        .execute(r"async () => { while (true) {} }")
        .await
        .expect("CPU-bound response");
    assert!(execution.is_error);
    assert_eq!(execution.structured["error"]["kind"], json!("timeout"));
    assert!(execution.elapsed < std::time::Duration::from_secs(2));
    let after = runner
        .fixture_invocation_count()
        .await
        .expect("final effects");
    assert_eq!(after, before, "CPU loop cannot reach the upstream");
    let identity = runner.identity();
    let cleanup = runner.finish().await;
    write_report(
        "cpu-deadline",
        &identity,
        limits,
        &execution,
        before,
        after,
        &cleanup,
    )
    .expect("CPU deadline evidence");
    assert!(cleanup.is_clean(), "cleanup: {:?}", cleanup.failures);
}
