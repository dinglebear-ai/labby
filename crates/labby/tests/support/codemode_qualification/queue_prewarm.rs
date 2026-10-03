//! Recovery is exclusive to this fixed, non-mutating fixture prewarm. The
//! queue oracle itself is never retried and keeps its 150ms transport budget.
use std::{future::Future, time::Duration};

use serde_json::{Value, json};

use crate::codemode_harness::{CodeModeQualification, Execution};

const PREWARM_CODE: &str =
    r#"async () => await callTool("forge::forge.safe", {query:"prewarm",limit:1,enabled:true})"#;
const MAX_ATTEMPTS: usize = 3;
const DEADLINE: Duration = Duration::from_secs(10);

pub(crate) async fn prewarm_queue_fixture(runner: &CodeModeQualification) -> Result<(), String> {
    prewarm_with(
        || runner.execute(PREWARM_CODE),
        || runner.fixture_settlement_barrier(),
        DEADLINE,
    )
    .await
}

fn fixed_call(trace: &Value) -> bool {
    let calls = trace["calls"].as_array();
    trace["kind"] == "code_mode_execute_trace"
        && trace["call_count"] == 1
        && calls.is_some_and(|calls| calls.len() == 1)
        && trace["calls"][0]["id"] == "forge::forge.safe"
        && trace["calls"][0]["namespace"] == "forge"
        && trace["calls"][0]["tool"] == "forge.safe"
        && trace["calls"][0]["params"] == arguments()
}

fn arguments() -> Value {
    json!({"query":"prewarm", "limit":1, "enabled":true})
}

fn retryable_timeout(execution: &Execution) -> bool {
    let trace = &execution.structured;
    execution.is_error
        && fixed_call(trace)
        && trace["error_kind"] == "timeout"
        && trace["error"]["contract_version"] == 1
        && trace["error"]["kind"] == "timeout"
        && trace["error"]["origin"] == "upstream_transport"
        && trace["error"]["tool"] == "forge::forge.safe"
        && trace["calls"][0]["ok"] == false
        && trace["calls"][0]["error_kind"] == "timeout"
}

fn successful_prewarm(execution: &Execution) -> bool {
    let trace = &execution.structured;
    !execution.is_error
        && fixed_call(trace)
        && trace["calls"][0]["ok"] == true
        && trace.get("error").is_none()
        && trace.get("error_kind").is_none()
        && trace["calls"][0].get("error_kind").is_none()
        && trace["result"]["tool"] == "forge.safe"
        && trace["result"]["arguments"] == arguments()
}

async fn prewarm_with<F, E, S, B>(
    mut execute: F,
    mut settle: S,
    budget: Duration,
) -> Result<(), String>
where
    F: FnMut() -> E,
    E: Future<Output = Result<Execution, String>>,
    S: FnMut() -> B,
    B: Future<Output = Result<(), String>>,
{
    let mut failures = Vec::new();
    let recovery = async {
        // Health/ready does not prove a cold upstream catalog exists. An inert
        // resource read establishes the actual fixture connection before the
        // shorter Code Mode catalog budget begins; it shares this deadline.
        settle()
            .await
            .map_err(|error| format!("initial fixture readiness: {error}"))?;
        for attempt in 1..=MAX_ATTEMPTS {
            let execution = execute()
                .await
                .map_err(|error| format!("prewarm attempt {attempt}: {error}"))?;
            if successful_prewarm(&execution) {
                // Includes final success: the effect baseline must follow all
                // recovered calls settling, even after transport cancellation.
                settle()
                    .await
                    .map_err(|error| format!("prewarm settlement: {error}"))?;
                return Ok(());
            }
            let retryable = retryable_timeout(&execution);
            eprintln!(
                "queue fixture prewarm attempt {attempt}: {}",
                execution.structured
            );
            failures.push(execution.structured);
            if !retryable {
                return Err(format!("prewarm attempt {attempt} failed closed"));
            }
            settle()
                .await
                .map_err(|error| format!("prewarm settlement: {error}"))?;
        }
        Err(format!("prewarm exhausted {MAX_ATTEMPTS} attempts"))
    };
    match tokio::time::timeout(budget, recovery).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("{error}; prewarm failures: {}", json!(failures))),
        Err(_) => Err(format!(
            "prewarm deadline exhausted after {budget:?}; prewarm failures: {}",
            json!(failures)
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque};

    fn response(timeout: bool) -> Execution {
        let mut trace = json!({
            "kind":"code_mode_execute_trace", "call_count":1,
            "calls":[{"id":"forge::forge.safe", "namespace":"forge", "tool":"forge.safe",
                "params":arguments(), "ok":!timeout}]
        });
        if timeout {
            trace["error_kind"] = json!("timeout");
            trace["calls"][0]["error_kind"] = json!("timeout");
            trace["error"] = json!({"contract_version":1,"kind":"timeout",
                "origin":"upstream_transport","tool":"forge::forge.safe"});
        } else {
            trace["result"] =
                json!({"tool":"forge.safe", "arguments":arguments(), "schema_revision":1});
        }
        Execution {
            structured: trace,
            elapsed: Duration::ZERO,
            wire_bytes: 0,
            is_error: timeout,
        }
    }

    #[tokio::test]
    async fn timeout_then_success_settles_each_attempt_before_baseline() {
        let mut responses = VecDeque::from([response(true), response(false)]);
        let events = RefCell::new(Vec::new());
        let result = prewarm_with(
            || {
                events.borrow_mut().push("execute");
                std::future::ready(Ok(responses.pop_front().expect("bounded attempts")))
            },
            || {
                events.borrow_mut().push("settle");
                std::future::ready(Ok(()))
            },
            DEADLINE,
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            *events.borrow(),
            ["settle", "execute", "settle", "execute", "settle"]
        );
        assert!(responses.is_empty());
    }
    #[tokio::test]
    async fn rejects_non_timeout_foreign_tool_runtime_timeout_and_malformed_trace() {
        for (field, value) in [
            ("/error/kind", json!("queue_saturated")),
            ("/error/tool", json!("forge::forge.delay")),
            ("/error/origin", json!("runtime")),
            ("/call_count", json!(2)),
            ("/calls/0/id", json!("forge::forge.delay")),
            ("/calls/0/params/query", json!("other")),
        ] {
            let mut execution = response(true);
            *execution.structured.pointer_mut(field).expect("test field") = value;
            let mut calls = 0;
            let mut settlements = 0;
            let mut execution = Some(execution);
            let result = prewarm_with(
                || {
                    calls += 1;
                    std::future::ready(Ok(execution.take().expect("no retries")))
                },
                || {
                    settlements += 1;
                    std::future::ready(Ok(()))
                },
                DEADLINE,
            )
            .await;
            assert!(
                result.expect_err("fail closed").contains("failed closed"),
                "{field}"
            );
            assert_eq!(calls, 1, "{field}");
            assert_eq!(settlements, 1, "{field}");
        }
    }

    #[tokio::test]
    async fn all_timeouts_are_bounded_and_preserve_failure_receipts() {
        let mut calls = 0;
        let mut settlements = 0;
        let error = prewarm_with(
            || {
                calls += 1;
                std::future::ready(Ok(response(true)))
            },
            || {
                settlements += 1;
                std::future::ready(Ok(()))
            },
            DEADLINE,
        )
        .await
        .expect_err("must not proceed to queue oracle");
        assert_eq!(calls, MAX_ATTEMPTS);
        assert_eq!(settlements, MAX_ATTEMPTS + 1);
        assert!(error.contains("exhausted 3 attempts"));
        assert_eq!(error.matches("upstream_transport").count(), MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn settlement_error_is_not_suppressed_or_retried() {
        for timeout in [true, false] {
            let mut calls = 0;
            let mut settlements = 0;
            let error = prewarm_with(
                || {
                    calls += 1;
                    std::future::ready(Ok(response(timeout)))
                },
                || {
                    settlements += 1;
                    std::future::ready(if settlements == 1 {
                        Ok(())
                    } else {
                        Err("ledger unavailable".to_owned())
                    })
                },
                DEADLINE,
            )
            .await
            .expect_err("settlement mandatory");
            assert!(error.contains("ledger unavailable"));
            assert_eq!(calls, 1);
        }
    }

    #[tokio::test]
    async fn successful_flag_requires_the_exact_safe_result() {
        let mut execution = response(false);
        execution.structured["result"]["arguments"]["query"] = json!("wrong");
        let mut execution = Some(execution);
        let error = prewarm_with(
            || std::future::ready(Ok(execution.take().expect("no retries"))),
            || std::future::ready(Ok(())),
            DEADLINE,
        )
        .await
        .expect_err("expected arguments mandatory");
        assert!(error.contains("failed closed"));
    }

    #[tokio::test]
    async fn overall_deadline_bounds_execution_and_settlement() {
        let budget = Duration::from_millis(10);
        let error = prewarm_with(
            || std::future::pending::<Result<Execution, String>>(),
            || std::future::ready(Ok(())),
            budget,
        )
        .await
        .expect_err("execution deadline");
        assert!(error.contains("deadline exhausted"));
        let mut settlements = 0;
        let error = prewarm_with(
            || std::future::ready(Ok(response(true))),
            || {
                settlements += 1;
                let initial = settlements == 1;
                async move {
                    if initial {
                        Ok(())
                    } else {
                        std::future::pending::<Result<(), String>>().await
                    }
                }
            },
            budget,
        )
        .await
        .expect_err("settlement shares execution deadline");
        assert!(error.contains("deadline exhausted"));
        assert!(error.contains("upstream_transport"));
    }
    #[tokio::test]
    async fn initial_readiness_failure_or_deadline_never_executes_prewarm() {
        let error = prewarm_with(
            || async { panic!("no execution before readiness") },
            || std::future::ready(Err("fixture not ready".to_owned())),
            DEADLINE,
        )
        .await
        .expect_err("initial readiness mandatory");
        assert!(error.contains("initial fixture readiness: fixture not ready"));
        let error = prewarm_with(
            || async { panic!("no execution before readiness") },
            || std::future::pending::<Result<(), String>>(),
            Duration::from_millis(10),
        )
        .await
        .expect_err("initial readiness shares deadline");
        assert!(error.contains("deadline exhausted"));
    }
}
