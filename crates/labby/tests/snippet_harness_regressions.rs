//! Hermetic process-boundary regressions for the triage snippet and mock harness.
//! All tool responses are synthetic; no personal data or live credentials are used.
use serde_json::{Value, json};
use std::{fs, process::Command};

const TRIAGE: &str = include_str!("../../../docs/snippets/unraid-linear-pr-triage.md");

fn run(source: &str, name: &str, fixture: Value, params: &[&str]) -> (bool, Value) {
    let home = tempfile::tempdir().unwrap();
    let snippets = home.path().join("snippets");
    fs::create_dir_all(&snippets).unwrap();
    fs::write(snippets.join(format!("{name}.md")), source).unwrap();
    let fixture_path = home.path().join("fixture.json");
    fs::write(&fixture_path, serde_json::to_vec(&fixture).unwrap()).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_labby"));
    command
        .env_clear()
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path())
        .current_dir(home.path())
        .args(["--json", "--no-input", "snippet", "test", name, "--fixture"])
        .arg(fixture_path);
    for param in params {
        command.args(["--param", param]);
    }
    let output = command.output().unwrap();
    let report = serde_json::from_slice::<Value>(&output.stdout);
    assert!(
        report.is_ok(),
        "invalid JSON report: {:?}; stdout={}; stderr={}",
        report.as_ref().err(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = report.unwrap();
    (output.status.success(), report)
}

fn issues(ids: &[String]) -> Value {
    json!({"issues": ids.iter().map(|id| json!({"id":id,"title":format!("Synthetic {id}"),"status":"In Progress"})).collect::<Vec<_>>(),"hasNextPage":false})
}
fn rule(tool: &str, params: Value, result: Value) -> Value {
    json!({"tool":tool,"match":params,"result":result})
}
fn search_query(ids: &[String], state: &str) -> String {
    ids.iter()
        .map(|id| format!("\"{id}\" org:unraid is:{state}"))
        .collect::<Vec<_>>()
        .join(" OR ")
}
fn pr(number: usize, title: &str, body: &str, repo: &str, state: &str) -> Value {
    json!({"number":number,"title":title,"body":body,"state":state,"draft":false,
        "html_url":format!("https://github.com/{repo}/pull/{number}"),
        "repository_url":format!("https://api.github.com/repos/{repo}"),"user":{"login":"jmagar"}})
}
fn initial(ids: &[String]) -> Vec<Value> {
    vec![
        rule(
            "linear-notification-worker::list_issues",
            json!({"team":"U8","state":"started"}),
            issues(ids),
        ),
        rule("github::get_me", json!({}), json!({"login":"jmagar"})),
    ]
}
fn mine(items: Vec<Value>) -> Value {
    rule(
        "github::search_pull_requests",
        json!({"query":"author:jmagar is:open org:unraid"}),
        json!({"total_count":items.len(),"items":items}),
    )
}
fn assert_passed(result: (bool, Value)) -> Value {
    assert!(result.0, "CLI failed: {}", result.1);
    assert_eq!(result.1["passed"], true, "{}", result.1);
    assert_eq!(result.1["mode"], "mock");
    result.1
}

#[test]
fn fast_path_has_seven_bounded_searches_for_26_issues_and_keeps_cloudflare() {
    let ids: Vec<_> = (1..=26).map(|i| format!("U8-{i}")).collect();
    let mut calls = initial(&ids);
    for (i, chunk) in ids.chunks(4).enumerate() {
        let query = search_query(chunk, "open");
        assert!(query.matches(" OR ").count() <= 3);
        assert!(query.len() < 256);
        let items: Vec<_> = chunk
            .iter()
            .enumerate()
            .map(|(j, id)| pr(i * 4 + j + 1, id, "", "unraid/core", "open"))
            .collect();
        calls.push(rule(
            "github::search_pull_requests",
            json!({"query":query,"perPage":100}),
            json!({"total_count":items.len(),"items":items}),
        ));
    }
    calls.push(mine(vec![pr(
        1512,
        "Synthetic Cloudflare work",
        "",
        "unraid/cloudflare",
        "open",
    )]));
    let report = assert_passed(run(
        TRIAGE,
        "unraid-linear-pr-triage",
        json!({"calls":calls,
        "expect":{"/ok":true,"/complete":true,"/summary/issueCount":26,"/summary/toolCalls":10,
            "/summary/relatedPRSearchCalls":7,"/summary/handoffCalls":0,"/summary/myOpenPRCount":1},
        "budgets":{"tool_calls":10}}),
        &[],
    ));
    let value = &report["result"];
    assert_eq!(value["myOpenPRs"][0]["repo"], "unraid/cloudflare");
    for row in value["issues"].as_array().unwrap() {
        assert!(row.get("historicalPRs").is_none());
        assert!(row.get("handoff").is_none());
        assert_eq!(row["openPRs"].as_array().unwrap().len(), 1);
    }
    assert!(report["metrics"]["max_in_flight"].as_u64().unwrap() >= 2);
    assert!(report["metrics"]["output_bytes"].as_u64().unwrap() <= 16_000);
}

#[test]
fn whole_issue_ids_and_release_noise_do_not_cross_link() {
    let ids = vec!["U8-12".into(), "U8-123".into()];
    let mut calls = initial(&ids);
    let items = vec![
        pr(
            1,
            "chore(main): release 2.0",
            "U8-12",
            "unraid/core",
            "open",
        ),
        pr(2, "Fix U8-123", "", "unraid/core", "open"),
        pr(3, "release U8-12 implementation", "", "unraid/core", "open"),
    ];
    calls.push(rule(
        "github::search_pull_requests",
        json!({"query":search_query(&ids,"open")}),
        json!({"total_count":3,"items":items}),
    ));
    calls.push(mine(vec![]));
    let report = assert_passed(run(
        TRIAGE,
        "unraid-linear-pr-triage",
        json!({"calls":calls,"expect":{"/ok":true}}),
        &[],
    ));
    let rows = report["result"]["issues"].as_array().unwrap();
    assert_eq!(rows[0]["openPRs"].as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["openPRs"][0]["number"], 3);
    assert_eq!(rows[1]["openPRs"].as_array().unwrap().len(), 1);
    assert_eq!(rows[1]["openPRs"][0]["number"], 2);
}

#[test]
fn failed_search_is_incomplete_not_evidence_of_no_pr() {
    let ids = vec!["U8-12".into()];
    let mut calls = initial(&ids);
    calls.push(
        json!({"tool":"github::search_pull_requests","match":{"query":search_query(&ids,"open")},
        "error":{"kind":"network_error","message":"synthetic failure"}}),
    );
    calls.push(mine(vec![]));
    assert_passed(run(
        TRIAGE,
        "unraid-linear-pr-triage",
        json!({"calls":calls,
        "expect":{"/ok":false,"/complete":false,"/summary/relatedPRSearchFailures":1,"/summary/issuesWithoutObservedPRs":0,"/issues/0/matchStatus":"incomplete"}}),
        &[],
    ));
}

#[test]
fn null_initial_responses_fail_closed() {
    let calls = vec![
        rule(
            "linear-notification-worker::list_issues",
            json!({}),
            Value::Null,
        ),
        rule("github::get_me", json!({}), Value::Null),
    ];
    assert_passed(run(
        TRIAGE,
        "unraid-linear-pr-triage",
        json!({"calls":calls,
        "expect":{"/ok":false,"/complete":false,"/summary/failureCount":2,"/summary/issueCount":null,"/summary/issuesWithOpenPRs":null,"/summary/issuesWithoutObservedPRs":null}}),
        &[],
    ));
}

#[test]
fn swallowed_unexpected_call_produces_failing_cli_exit() {
    let (success, report) = run(
        r#"async () => {try {await callTool("github::get_me",{});} catch (_) {} return {ok:true};}"#,
        "unexpected-call",
        json!({}),
        &[],
    );
    assert!(
        !success,
        "unexpected calls must fail CI even when swallowed"
    );
    assert_eq!(report["passed"], false);
}

#[test]
fn failed_budget_and_snapshot_produce_failing_cli_exit() {
    let (success, report) = run(
        r#"async () => ({ok:true,payload:"éé"})"#,
        "budget-case",
        json!({"budgets":{"output_bytes":4},"snapshot":{"ok":true,"payload":"different"}}),
        &[],
    );
    assert!(!success);
    assert_eq!(report["passed"], false);
    assert!(report["result"].is_null());
    assert!(report["failures"].as_array().unwrap().len() >= 2);
}
