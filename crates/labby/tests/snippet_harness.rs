//! End-to-end fixture tests use the real product CLI and QuickJS child process.
use serde_json::{Value, json};
use std::{fs, process::Command};

fn run(name: &str, source: &str, fixture: Value, input: &[&str]) -> (bool, Value) {
    let home = tempfile::tempdir().expect("temporary fixture home");
    let snippets = home.path().join("snippets");
    fs::create_dir(&snippets).unwrap();
    fs::write(snippets.join(format!("{name}.md")), source).unwrap();
    fs::write(
        snippets.join(format!("{name}.test.json")),
        serde_json::to_vec(&fixture).unwrap(),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .env_clear()
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path())
        .env("LABBY_CODE_MODE_RUNNER_EXE", env!("CARGO_BIN_EXE_labby"))
        .args(["--json", "snippet", "test", name]);
    for parameter in input {
        command.args(["--param", parameter]);
    }
    let output = command.output().expect("run fixture CLI");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report = serde_json::from_str(&stdout)
        .map_err(|error| {
            format!(
                "invalid CLI JSON: {error}; stdout={stdout}; stderr={}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .expect("valid JSON report");
    (output.status.success(), report)
}

#[test]
fn triage_fast_path_and_deep_path_use_real_runner() {
    let source = include_str!("../../../docs/snippets/unraid-linear-pr-triage.md");
    for (fixture, params, expected_calls) in [
        (
            include_str!("../../../docs/snippets/unraid-linear-pr-triage.test.json"),
            vec![],
            10,
        ),
        (
            include_str!("../../../docs/snippets/unraid-linear-pr-triage.deep.test.json"),
            vec!["deep=true"],
            36,
        ),
    ] {
        let (success, report) = run(
            "unraid-linear-pr-triage",
            source,
            serde_json::from_str(fixture).unwrap(),
            &params,
        );
        assert!(success, "fixture failed: {report}");
        assert_eq!(report["passed"], true, "{report}");
        assert_eq!(report["mode"], "mock");
        assert_eq!(report["metrics"]["tool_calls"], expected_calls);
        assert!(report["metrics"]["max_in_flight"].as_u64().unwrap() >= 8);
        assert!(report["metrics"]["output_bytes"].as_u64().unwrap() <= 16_000);
        if params.is_empty() {
            for issue in report["result"]["issues"].as_array().unwrap() {
                assert!(issue.get("historicalPRs").is_none());
                assert!(issue.get("handoff").is_none());
                assert!(issue["openPRs"].as_array().unwrap().len() <= 1);
            }
        }
    }
}

#[test]
fn unexpected_calls_fail_even_when_the_snippet_swallows_the_error() {
    let source = "async () => { try { await callTool('fixture::missing', {}); } catch (_) {} return {ok:true}; }";
    let (success, report) = run("unexpected", source, json!({}), &[]);
    assert!(!success);
    assert_eq!(report["passed"], false);
    assert!(
        report["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x.as_str().unwrap().contains("unexpected"))
    );
}

#[test]
fn caught_fixture_errors_and_normalized_snapshots_can_be_asserted() {
    let source = "async () => { const result = await codemode.batch([() => callTool('fixture::read', {})]); return {ok:false, failed:result.failed.length, time:Date.now()}; }";
    let fixture = json!({"calls":[{"tool":"fixture::read","error":{"kind":"synthetic","message":"fixture rejection"}}],
        "expect":{"/ok":false,"/failed":1},"snapshot":{"ok":false,"failed":1,"time":0},"ignore_paths":["/time"]});
    let (success, report) = run("expected-error", source, fixture, &[]);
    assert!(success, "{report}");
    assert_eq!(report["passed"], true);
}

#[test]
fn output_and_call_budgets_produce_nonzero_exit_codes() {
    let (success, report) = run(
        "large-output",
        "async () => 'é'.repeat(100)",
        json!({"budgets":{"output_bytes":20}}),
        &[],
    );
    assert!(!success);
    assert!(report["result"].is_null());
    assert!(report["metrics"]["output_bytes"].as_u64().unwrap() > 20);
    let (success, report) = run(
        "call-budget",
        "async () => { try { await callTool('fixture::read',{}); } catch (_) {} return true; }",
        json!({"budgets":{"tool_calls":0}}),
        &[],
    );
    assert!(!success);
    assert_eq!(report["metrics"]["tool_calls"], 1);
}

#[test]
fn global_tool_bridge_cannot_reach_a_live_upstream() {
    let source = "async () => { try { await globalThis.callTool('github::get_me',{}); } catch (_) {} return {ok:true}; }";
    let (success, report) = run("bridge-escape", source, json!({}), &[]);
    assert!(
        !success,
        "a real host bridge attempt must be recorded: {report}"
    );
}

#[test]
fn triage_v2_acceptance_fixtures_run_through_product_dispatch() {
    let source = include_str!("../../../docs/snippets/unraid-linear-pr-triage-v2.md");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../labby-codemode/tests/fixtures/snippet-harness");
    for (case, fixture, input, expected) in [
        ("fast", "fast", vec![], true),
        ("diagnostics", "fast", vec!["diagnostics=true"], true),
        ("deep", "deep", vec!["deep=true"], true),
        ("pagination", "paginated", vec![], true),
        ("page budget", "page-budget", vec!["maxPages=1"], true),
        ("rate limit", "rate-limit", vec![], true),
        ("worker failure", "worker-failure", vec![], true),
        ("wrong assertion", "wrong-assertion", vec![], false),
        ("invalid input", "empty", vec!["chunkSize=1.5"], false),
    ] {
        let fixture =
            serde_json::from_slice(&fs::read(root.join(format!("{fixture}.json"))).unwrap())
                .unwrap();
        let (success, report) = run("unraid-linear-pr-triage-v2", source, fixture, &input);
        assert_eq!(success, expected, "{case}: {report}");
        assert_eq!(report["passed"], expected, "{case}: {report}");
        if case == "diagnostics" {
            assert!(report["result"]["diagnostics"]["calls"].is_array());
        }
    }
}

#[test]
fn null_snapshot_mismatch_fails_the_cli_exit_status() {
    let (success, report) = run(
        "null-snapshot",
        r#"async () => ({"unexpected":true})"#,
        json!({"snapshot":null}),
        &[],
    );
    assert!(!success, "{report}");
    assert_eq!(report["passed"], false);
    let (success, report) = run(
        "null-snapshot",
        "async () => null",
        json!({"snapshot":null}),
        &[],
    );
    assert!(success, "{report}");
}
