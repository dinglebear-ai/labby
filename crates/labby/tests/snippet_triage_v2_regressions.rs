//! Regression fixtures execute the documented v2 snippet through the product CLI.
use serde_json::{Value, json};
use std::{fs, process::Command};

fn run(fixture: Value, params: &[&str]) -> (bool, Value) {
    let home = tempfile::tempdir().expect("isolated snippet home");
    let snippets = home.path().join("snippets");
    fs::create_dir(&snippets).unwrap();
    fs::write(
        snippets.join("unraid-linear-pr-triage-v2.md"),
        include_str!("../../../docs/snippets/unraid-linear-pr-triage-v2.md"),
    )
    .unwrap();
    fs::write(
        snippets.join("unraid-linear-pr-triage-v2.test.json"),
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
        .args(["--json", "snippet", "test", "unraid-linear-pr-triage-v2"]);
    for param in params {
        command.args(["--param", param]);
    }
    let output = command.output().expect("run product snippet test");
    let report = serde_json::from_slice(&output.stdout)
        .map_err(|error| {
            format!(
                "invalid report: {error}; stdout={}; stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
        .expect("valid JSON report");
    (output.status.success(), report)
}

#[test]
fn malformed_successful_bootstrap_responses_are_not_an_empty_success() {
    for invalid in [Value::Null, json!(false), json!(0), json!([])] {
        let fixture = json!({
            "calls": [
                {"tool":"linear-notification-worker::list_issues","result":invalid},
                {"tool":"github::get_me","result":invalid}
            ],
            "expect": {"/ok":false,"/complete":false,"/summary/issueCount":null}
        });
        let (success, report) = run(fixture, &[]);
        assert!(success, "expected error state was not reported: {report}");
        assert_eq!(report["passed"], true, "{report}");
        let failures = report["result"]["failures"].as_array().unwrap();
        assert_eq!(failures.len(), 2, "{report}");
        for phase in ["issues", "identity"] {
            assert!(failures.iter().any(|failure| {
                failure["phase"] == phase && failure["kind"] == "invalid_result"
            }));
        }
    }
}

#[test]
fn null_entries_and_invalid_pr_authors_preserve_usable_partial_results() {
    let fixture = json!({"calls":[
        {"tool":"linear-notification-worker::list_issues","result":{"issues":[null,7,{"id":"U8-1","title":"Valid issue"}]}},
        {"tool":"github::get_me","result":{"login":"tester"}},
        {"tool":"github::search_pull_requests","times":2,"result":{"items":[
            null,
            {"repository_url":"https://api.github.com/repos/unraid/core","number":2,"state":"open","user":{"login":7}},
            {"repository_url":"https://api.github.com/repos/unraid/core","number":1,"state":"open","title":"U8-1 fix","user":{"login":"tester"}}
        ]}}
    ],"expect":{"/ok":true,"/complete":false,"/summary/issueCount":1}});
    let (success, report) = run(fixture, &[]);
    assert!(success, "{report}");
    let result = &report["result"];
    assert_eq!(result["pullRequests"].as_object().unwrap().len(), 1);
    assert_eq!(result["issues"][0]["matchStatus"], "incomplete");
    assert!(
        result["coverage"]["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["reason"] == "invalid_identifier")
    );
    assert!(
        result["coverage"]["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|gap| gap["reason"] == "invalid_pr")
    );
}

#[test]
fn invalid_string_inputs_fail_before_any_upstream_call() {
    for param in [
        "org=true",
        "team=false",
        "assignee=7",
        "state=false",
        "team=",
        "issueCursor=7",
    ] {
        let (success, report) = run(json!({}), &[param]);
        assert!(!success, "{param}: {report}");
        assert_eq!(report["passed"], false, "{param}: {report}");
        assert_eq!(report["metrics"]["tool_calls"], 0, "{param}: {report}");
    }
}

#[test]
fn final_output_budget_includes_omissions_and_timing() {
    let prs: Vec<_> = (1..=25)
        .map(|number| {
            json!({
                "repository_url":"https://api.github.com/repos/unraid/core",
                "number":number,"title":"x".repeat(120),"state":"open","user":{"login":"tester"}
            })
        })
        .collect();
    let fixture = json!({"calls":[
        {"tool":"linear-notification-worker::list_issues","result":{"issues":[]}},
        {"tool":"github::get_me","result":{"login":"tester"}},
        {"tool":"github::search_pull_requests","result":{"items":prs}}
    ]});
    for (param, max_bytes) in [("maxOutputBytes=4000", 4000), ("maxOutputBytes=4300", 4300)] {
        let (success, report) = run(fixture.clone(), &[param]);
        assert!(success, "{report}");
        assert!(
            report["metrics"]["output_bytes"].as_u64().unwrap() <= max_bytes,
            "{report}"
        );
        let result = &report["result"];
        assert_eq!(result["complete"], false);
        assert!(result["coverage"]["omittedPRs"].as_u64().unwrap() > 0);
        let gap = result["coverage"]["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|gap| gap["reason"] == "output_budget")
            .unwrap();
        assert_eq!(gap["omittedPRs"], result["coverage"]["omittedPRs"]);
        for key in result["myOpenPRs"].as_array().unwrap() {
            assert!(result["pullRequests"].get(key.as_str().unwrap()).is_some());
        }
    }
}
