//! Offline CLI contracts using the actual Labby binary and QuickJS runner.
use serde_json::{Value, json};
use std::fs;
use std::process::{Command, Output};

const BINARY: &str = env!("CARGO_BIN_EXE_labby");
const TRIAGE: &str = include_str!("../../../docs/snippets/unraid-linear-pr-triage.md");
const FAST: &str = include_str!("../../../docs/snippets/unraid-linear-pr-triage.test.json");
const DEEP: &str = include_str!("../../../docs/snippets/unraid-linear-pr-triage.deep.test.json");

fn execute(name: &str, source: &str, fixture: Option<&Value>, params: &[&str]) -> Output {
    let home = tempfile::tempdir().unwrap();
    let snippets = home.path().join("snippets");
    fs::create_dir_all(&snippets).unwrap();
    fs::write(snippets.join(format!("{name}.md")), source).unwrap();
    if let Some(fixture) = fixture {
        fs::write(
            snippets.join(format!("{name}.test.json")),
            serde_json::to_vec(fixture).unwrap(),
        )
        .unwrap();
    }
    let mut command = Command::new(BINARY);
    command
        .env_clear()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("LABBY_HOME", home.path())
        .env("LABBY_CODE_MODE_RUNNER_EXE", BINARY)
        .env("LABBY_CODE_MODE_POOL_SIZE", "0")
        .current_dir(home.path())
        .args(["--json", "--no-input", "snippet", "test", name]);
    for param in params {
        command.args(["--param", param]);
    }
    command.output().unwrap()
}
fn report(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid report: {e}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
#[test]
fn actual_runner_executes_fixture_without_gateway_or_credentials() {
    let output = execute(
        "demo",
        "async () => await callTool('demo::read', {key:1})",
        Some(
            &json!({"calls":[{"tool":"demo::read","match":{"key":1},"result":{"ok":true}}],"expect":{"/ok":true}}),
        ),
        &[],
    );
    let result = report(&output);
    assert!(output.status.success(), "{result}");
    assert_eq!(result["mode"], "mock");
    assert_eq!(result["metrics"]["tool_calls"], 1);
}
#[test]
fn failing_assertions_return_nonzero_exit_status() {
    let output = execute(
        "demo",
        "async () => ({count:2})",
        Some(&json!({"expect":{"/count":3}})),
        &[],
    );
    assert!(!output.status.success());
    assert_eq!(report(&output)["passed"], false);
}
#[test]
fn missing_fixture_never_falls_back_to_live_execution() {
    let output = execute(
        "demo",
        "async () => await callTool('github::get_me', {})",
        None,
        &[],
    );
    assert!(!output.status.success());
    assert!(
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .contains("fixture")
    );
}
#[test]
fn swallowed_unknown_call_still_fails() {
    let output = execute(
        "demo",
        "async () => { try { await callTool('demo::unknown', {}); } catch (_) {} return {ok:true}; }",
        Some(&json!({})),
        &[],
    );
    assert!(!output.status.success());
    assert_eq!(report(&output)["passed"], false);
}
#[test]
fn global_host_bridge_cannot_bypass_offline_scope() {
    let output = execute(
        "demo",
        "async () => { try { await globalThis.callTool('demo::read', {}); } catch (_) {} return {ok:true}; }",
        Some(&json!({})),
        &[],
    );
    assert!(!output.status.success());
    assert!(
        report(&output)["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap().contains("host bridge"))
    );
}
#[test]
fn artifact_writes_are_unavailable_in_fixture_mode() {
    let output = execute(
        "demo",
        "async () => { await writeArtifact('escape.txt', 'blocked'); return {ok:true}; }",
        Some(&json!({})),
        &[],
    );
    assert!(!output.status.success());
}
#[test]
fn native_batch_and_expected_rejections_are_exercised() {
    let output = execute(
        "demo",
        "async () => { const r=await codemode.batch([()=>callTool('demo::read',{id:1}),()=>callTool('demo::read',{id:2})]); return {ok:r.all_ok, success:r.ok.length, failures:r.failed.length}; }",
        Some(
            &json!({"calls":[{"tool":"demo::read","match":{"id":1},"result":1},{"tool":"demo::read","match":{"id":2},"error":{"kind":"test_error","message":"synthetic failure"}}],"expect":{"/ok":false,"/success":1,"/failures":1}}),
        ),
        &[],
    );
    let result = report(&output);
    assert!(output.status.success(), "{result}");
    assert_eq!(result["metrics"]["max_in_flight"], 2);
}
#[test]
fn undefined_result_produces_a_clear_failed_report() {
    let output = execute("demo", "async () => {}", Some(&json!({})), &[]);
    assert!(!output.status.success());
    assert!(
        report(&output)["failures"][0]
            .as_str()
            .unwrap()
            .contains("undefined")
    );
}
#[test]
fn raw_output_budget_is_enforced_before_display_shaping() {
    let output = execute(
        "demo",
        "async () => ({payload:'x'.repeat(1000)})",
        Some(&json!({"budgets":{"output_bytes":100}})),
        &[],
    );
    let result = report(&output);
    assert!(!output.status.success());
    assert_eq!(result["result"], Value::Null);
    assert!(result["metrics"]["output_bytes"].as_u64().unwrap() > 100);
}
#[test]
fn snapshot_normalization_runs_end_to_end() {
    let output = execute(
        "demo",
        "async () => ({count:2, updated:Date.now()})",
        Some(&json!({"snapshot":{"count":2,"updated":0},"ignore_paths":["/updated"]})),
        &[],
    );
    assert!(output.status.success(), "{}", report(&output));
}
#[test]
fn fast_triage_preserves_cloudflare_and_omits_deep_work() {
    let fixture: Value = serde_json::from_str(FAST).unwrap();
    let output = execute("unraid-linear-pr-triage", TRIAGE, Some(&fixture), &[]);
    let result = report(&output);
    assert!(output.status.success(), "{result}");
    assert_eq!(result["metrics"]["tool_calls"], 10);
    assert!(
        result["result"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i.get("historicalPRs").is_none() && i.get("handoff").is_none())
    );
}
#[test]
fn deep_triage_uses_compact_handoffs_and_history() {
    let fixture: Value = serde_json::from_str(DEEP).unwrap();
    let output = execute(
        "unraid-linear-pr-triage",
        TRIAGE,
        Some(&fixture),
        &["deep=true"],
    );
    let result = report(&output);
    assert!(output.status.success(), "{result}");
    assert_eq!(result["metrics"]["tool_calls"], 36);
}
#[test]
fn null_upstream_results_are_failures_not_empty_success() {
    let fixture = json!({"calls":[{"tool":"linear-notification-worker::list_issues","result":null},{"tool":"github::get_me","result":null}],"expect":{"/ok":false,"/complete":false,"/summary/failureCount":2}});
    let output = execute("unraid-linear-pr-triage", TRIAGE, Some(&fixture), &[]);
    assert!(output.status.success(), "{}", report(&output));
}

#[test]
fn fixture_response_preserves_proto_as_an_ordinary_data_key() {
    let source = "async () => { const r=await callTool('demo::read',{}); return {own:Object.prototype.hasOwnProperty.call(r,'__proto__'),value:r.__proto__}; }";
    let output = execute(
        "demo",
        source,
        Some(
            &json!({"calls":[{"tool":"demo::read","result":{"__proto__":1}}],"expect":{"/own":true,"/value":1}}),
        ),
        &[],
    );
    assert!(output.status.success(), "{}", report(&output));
}
#[test]
fn camel_case_inputs_are_typed_without_relaxing_snippet_names() {
    let source = r#"---
name: typed
description: Typed fixture test
tags: []
inputs:
  deepMode:
    type: boolean
    default: false
---

```js
async (input) => ({value:input.deepMode})
```
"#;
    let output = execute(
        "typed",
        source,
        Some(&json!({"expect":{"/value":true}})),
        &["deepMode=true"],
    );
    assert!(output.status.success(), "{}", report(&output));
    assert!(labby_codemode::snippet::store::validate_snippet_name("CamelCase").is_err());
}
#[test]
fn malformed_javascript_is_rejected_before_execution() {
    let output = execute("demo", "async () => {", Some(&json!({})), &[]);
    assert!(!output.status.success());
}
#[test]
fn runner_deadline_bounds_an_infinite_loop() {
    let started = std::time::Instant::now();
    let output = execute(
        "demo",
        "async () => { while (true) {} }",
        Some(&json!({"budgets":{"wall_clock_ms":500}})),
        &[],
    );
    assert!(!output.status.success());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

fn assert_reference_integrity(report: &Value) {
    let result = &report["result"];
    assert_eq!(result["schemaVersion"], 2);
    let catalog = result["pullRequests"].as_object().unwrap();
    let mut references = result["myOpenPRs"].as_array().unwrap().clone();
    for issue in result["issues"].as_array().unwrap() {
        references.extend(issue["openPRs"].as_array().unwrap().iter().cloned());
        if let Some(history) = issue["historicalPRs"].as_array() {
            references.extend(history.iter().cloned());
        }
    }
    for key in references {
        assert!(
            catalog.contains_key(key.as_str().unwrap()),
            "dangling PR reference {key}"
        );
    }
}
#[test]
fn large_deep_result_reports_omissions_without_dangling_references() {
    let mut fixture: Value = serde_json::from_str(DEEP).unwrap();
    fixture["expect"] = json!({"/ok":true,"/complete":false,"/schemaVersion":2});
    for call in fixture["calls"].as_array_mut().unwrap() {
        if let Some(items) = call["result"]["items"].as_array_mut() {
            for pr in items {
                pr["title"] = json!(format!(
                    "{} {}",
                    pr["title"].as_str().unwrap(),
                    "detail ".repeat(40)
                ));
            }
        }
    }
    let output = execute(
        "unraid-linear-pr-triage",
        TRIAGE,
        Some(&fixture),
        &["deep=true"],
    );
    let result = report(&output);
    assert!(output.status.success(), "{result}");
    assert!(
        result["result"]["summary"]["outputOmitted"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(result["metrics"]["output_bytes"].as_u64().unwrap() <= 16_000);
    assert_reference_integrity(&result);
}
#[test]
fn unknown_triage_inputs_fail_before_any_upstream_call() {
    let output = execute(
        "unraid-linear-pr-triage",
        TRIAGE,
        Some(&json!({"budgets":{"tool_calls":0}})),
        &["includeHisotry=true"],
    );
    let result = report(&output);
    assert!(!output.status.success());
    assert_eq!(result["metrics"]["tool_calls"], 0);
    assert!(
        result["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap().contains("Unknown input"))
    );
}
#[test]
fn missing_merge_evidence_is_null_not_an_unmerged_claim() {
    let mut fixture: Value = serde_json::from_str(DEEP).unwrap();
    fixture["expect"]["/pullRequests/unraid~1core#1100/merged"] = Value::Null;
    for call in fixture["calls"].as_array_mut().unwrap() {
        if let Some(items) = call["result"]["items"].as_array_mut() {
            for pr in items {
                if pr["number"] == 1100 {
                    pr["pull_request"]
                        .as_object_mut()
                        .unwrap()
                        .remove("merged_at");
                }
            }
        }
    }
    let output = execute(
        "unraid-linear-pr-triage",
        TRIAGE,
        Some(&fixture),
        &["deep=true"],
    );
    let result = report(&output);
    assert!(output.status.success(), "{result}");
    assert_reference_integrity(&result);
}
