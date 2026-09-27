use super::*;
use crate::{CodeModeHost, ExecCtx};
use serde_json::json;
fn fixture(value: Value) -> SnippetTestCase {
    serde_json::from_value(value).unwrap()
}
async fn call(
    host: &FixtureHost,
    id: &str,
    params: Value,
) -> Result<crate::ToolCallOutcome, crate::CodeModeCallError> {
    host.call_tool(
        id,
        params,
        &CodeModeCaller::TrustedLocal,
        CodeModeSurface::Cli,
        &host.scope,
        ExecCtx::none(),
    )
    .await
}
#[tokio::test]
async fn fixture_consumes_exact_params_and_preserves_parallel_independence() {
    let host = FixtureHost::new(&fixture(json!({"calls":[
        {"tool":"demo::read","params":{"id":1},"response":{"n":1}},
        {"tool":"demo::read","params":{"id":2},"response":{"n":2}}
    ]})))
    .unwrap();
    let (a, b) = tokio::join!(
        call(&host, "demo::read", json!({"id":2})),
        call(&host, "demo::read", json!({"id":1}))
    );
    assert_eq!(a.unwrap().value, json!({"n":2}));
    assert_eq!(b.unwrap().value, json!({"n":1}));
    assert!(host.failures(0).is_empty());
}
#[tokio::test]
async fn wrong_params_and_swallowed_unexpected_calls_still_fail() {
    let host = FixtureHost::new(&fixture(
        json!({"calls":[{"tool":"demo::read","params":{"id":1}}]}),
    ))
    .unwrap();
    assert!(call(&host, "demo::read", json!({"id":2})).await.is_err());
    let failures = host.failures(1);
    assert!(failures.iter().any(|f| f.contains("consumed 0")));
    assert!(failures.iter().any(|f| f.contains("unexpected")));
}
#[tokio::test]
async fn simulated_errors_are_expected_but_exhausted_rules_are_not() {
    let host = FixtureHost::new(&fixture(json!({"calls":[{"tool":"demo::read","error":{"kind":"timeout","message":"fixture timeout"}}]}))).unwrap();
    assert!(call(&host, "demo::read", json!({})).await.is_err());
    assert!(host.failures(1).is_empty());
    assert!(call(&host, "demo::read", json!({})).await.is_err());
    assert!(!host.failures(2).is_empty());
}
#[tokio::test]
async fn zero_call_budget_and_empty_catalog_are_deny_all() {
    let host = FixtureHost::new(&fixture(json!({"budgets":{"tool_calls":0}}))).unwrap();
    assert!(host.scope.is_scoped());
    assert!(host.scope.is_read_only());
    assert!(!host.scope.allows("demo", "read"));
    assert!(call(&host, "demo::read", json!({})).await.is_err());
    assert!(!host.failures(1).is_empty());
}
