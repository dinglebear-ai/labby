//! Synthetic fixture coverage for the checked-in workflow, never personal snapshots.
use super::super::store::{builtin_snippet_dir, resolve_snippet};
use super::*;
use serde_json::json;

fn triage() -> ResolvedSnippet {
    let home = tempfile::tempdir().unwrap();
    resolve_snippet(
        home.path(),
        &builtin_snippet_dir(),
        "unraid-linear-pr-triage",
    )
    .unwrap()
}

fn case(name: &str) -> SnippetFixture {
    let text = match name {
        "fast" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/snippets/unraid-triage-fast.json"
        )),
        "deep" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/snippets/unraid-triage-deep.json"
        )),
        "repo" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/snippets/unraid-triage-repo.json"
        )),
        _ => panic!("unknown fixture"),
    };
    SnippetFixture::from_value(serde_json::from_str(text).unwrap()).unwrap()
}

#[tokio::test]
async fn fast_path_handles_26_issues_in_ten_calls_without_history_or_handoffs() {
    let report = test_fixture(&triage(), json!({}), case("fast"))
        .await
        .unwrap();
    assert!(report.passed, "{:?}", report);
    let result = report.result.unwrap();
    for issue in result["issues"].as_array().unwrap() {
        assert!(issue.get("historicalPRs").is_none());
        assert!(issue.get("handoff").is_none());
    }
    assert_eq!(result["pullRequests"].as_array().unwrap().len(), 14);
}

#[tokio::test]
async fn deep_path_handles_26_issues_in_36_calls_and_compacts_handoffs() {
    let report = test_fixture(&triage(), json!({"deep": true}), case("deep"))
        .await
        .unwrap();
    assert!(report.passed, "{:?}", report);
    let result = report.result.unwrap();
    assert_eq!(result["issues"][0]["handoff"]["previousHandoffs"], 2);
    assert!(result["issues"][0]["handoff"].get("movement").is_none());
    assert!(report.metrics.output_bytes < 16000);
}

#[tokio::test]
async fn repo_override_does_not_narrow_my_organization_prs() {
    let report = test_fixture(
        &triage(),
        json!({"repos": ["unraid/unraid-e2e"]}),
        case("repo"),
    )
    .await
    .unwrap();
    assert!(report.passed, "{:?}", report);
    assert_eq!(
        report.result.unwrap()["myOpenPRs"],
        json!(["unraid/cloudflare-scripts#9999"])
    );
}

#[tokio::test]
async fn no_issues_still_fetches_my_open_prs() {
    let mut fixture = case("fast");
    fixture.calls.retain(|call| {
        call.tool != "github::search_pull_requests"
            || call.params["query"]
                .as_str()
                .unwrap()
                .starts_with("author:")
    });
    fixture.calls[0].response =
        FixtureResponse::Returns(json!({"issues": [], "hasNextPage": false}));
    fixture.expect = serde_json::from_value(json!({"/ok": true, "/summary/toolCalls": 3,
        "/summary/issueCount": 0, "/issues": [], "/myOpenPRs": ["unraid/cloudflare-scripts#9999"]}))
    .unwrap();
    let report = test_fixture(&triage(), json!({}), fixture).await.unwrap();
    assert!(report.passed, "{:?}", report);
}

#[tokio::test]
async fn failed_chunk_retains_other_results_and_marks_partial_coverage() {
    let mut fixture = case("fast");
    fixture.calls[2].response = FixtureResponse::Error(FixtureError {
        kind: "upstream_timeout".to_owned(),
        message: "synthetic timeout".to_owned(),
    });
    fixture.expected_failures = 1;
    fixture.expect.insert("/ok".to_owned(), json!(false));
    fixture
        .expect
        .insert("/coverage/complete".to_owned(), json!(false));
    fixture
        .expect
        .insert("/summary/issuesWithOpenPRs".to_owned(), json!(11));
    fixture
        .expect
        .insert("/summary/relatedPRSearchFailures".to_owned(), json!(1));
    let report = test_fixture(&triage(), json!({}), fixture).await.unwrap();
    assert!(report.passed, "{:?}", report);
}

#[tokio::test]
async fn missing_pages_are_reported_not_silently_treated_as_complete() {
    let mut fixture = case("fast");
    if let FixtureResponse::Returns(value) = &mut fixture.calls[2].response {
        value["total_count"] = json!(200);
    }
    fixture.expect.insert("/ok".to_owned(), json!(false));
    fixture
        .expect
        .insert("/coverage/complete".to_owned(), json!(false));
    fixture.expect.insert(
        "/coverage/incomplete/0/reason".to_owned(),
        json!("more_pages"),
    );
    let report = test_fixture(&triage(), json!({}), fixture).await.unwrap();
    assert!(report.passed, "{:?}", report);
}

#[tokio::test]
async fn call_budget_overflow_stops_before_searches() {
    let mut fixture = case("fast");
    fixture.calls.truncate(2);
    fixture.expect = serde_json::from_value(json!({"/ok": false,
        "/error/kind": "call_budget_exceeded", "/summary/requiredCalls": 10}))
    .unwrap();
    let report = test_fixture(&triage(), json!({"maxCalls": 3}), fixture)
        .await
        .unwrap();
    assert!(report.passed, "{:?}", report);
    assert_eq!(report.metrics.tool_calls, 2);
}

#[tokio::test]
async fn oversized_data_returns_an_explicit_small_error() {
    let mut fixture = case("fast");
    if let FixtureResponse::Returns(value) = &mut fixture.calls[0].response {
        value["issues"][0]["title"] = json!("x".repeat(20000));
    }
    fixture.expect =
        serde_json::from_value(json!({"/ok": false, "/error/kind": "output_budget_exceeded"}))
            .unwrap();
    let report = test_fixture(&triage(), json!({}), fixture).await.unwrap();
    assert!(report.passed, "{:?}", report);
    assert!(report.metrics.output_bytes < 2000);
}

#[tokio::test]
async fn exact_issue_boundaries_and_release_noise_are_respected() {
    let pr = |number, title: &str, body: &str| {
        json!({
            "number": number, "title": title, "body": body, "state": "open",
            "repository_url": "https://api.github.com/repos/unraid/core",
            "html_url": format!("https://github.com/unraid/core/pull/{number}"),
        })
    };
    let fixture = SnippetFixture::from_value(json!({"calls": [
        {"tool": "linear-notification-worker::list_issues", "response": {"returns": {"issues": [
            {"id": "U8-12", "title": "short id"}, {"id": "U8-120", "title": "long id"}]}}},
        {"tool": "github::get_me", "response": {"returns": {"login": "fixture-user"}}},
        {"tool": "github::search_pull_requests", "params": {"query": "\"U8-12\" org:unraid is:open OR \"U8-120\" org:unraid is:open"},
            "response": {"returns": {"items": [
                pr(7, "Fix U8-120", ""), pr(8, "chore(main): release 9", "U8-12"),
                pr(9, "release fix U8-12", "")], "total_count": 3}}},
        {"tool": "github::search_pull_requests", "params": {"query": "author:fixture-user is:open org:unraid"},
            "response": {"returns": {"items": [], "total_count": 0}}}
    ], "expect": {"/ok": true, "/issues/0/openPRs": ["unraid/core#9"],
        "/issues/1/openPRs": ["unraid/core#7"]}})).unwrap();
    let report = test_fixture(&triage(), json!({}), fixture).await.unwrap();
    assert!(report.passed, "{:?}", report);
}
