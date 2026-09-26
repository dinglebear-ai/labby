//! Regression cases for normalized triage output, using synthetic data only.
use labby_codemode::snippet::harness::{SnippetFixture, SnippetFixtureReport, run_fixture};
use labby_codemode::snippet::store::ResolvedSnippet;
use serde_json::{Value, json};
use std::path::Path;

fn pr(number: u64, title: &str, body: &str) -> Value {
    json!({"number":number,"title":title,"body":body,"state":"open","draft":false,
      "html_url":format!("https://github.com/unraid/core/pull/{number}"),
      "repository_url":"https://api.github.com/repos/unraid/core"})
}
fn scenario(items: Value) -> Value {
    json!({"calls":[
      {"tool":"linear-notification-worker::list_issues","result":{"issues":[
        {"id":"U8-12","title":"Issue twelve","status":"started"},
        {"id":"U8-120","title":"Issue one twenty","status":"started"}]}},
      {"tool":"github::get_me","result":{"login":"fixture-user"}},
      {"tool":"github::search_pull_requests","match":{"query":"\"U8-12\" org:unraid is:open OR \"U8-120\" org:unraid is:open"},"result":{"items":items}},
      {"tool":"github::search_pull_requests","match":{"query":"author:fixture-user is:open org:unraid"},"result":{"items":[]}}
    ]})
}
async fn run(snippet: &ResolvedSnippet, input: Value, data: Value) -> SnippetFixtureReport {
    let fixture: SnippetFixture = serde_json::from_value(data).unwrap();
    run_fixture(snippet, input, &fixture).await.unwrap()
}
fn assert_references(result: &Value) {
    let catalog = result["pullRequests"].as_object().unwrap();
    let mut references: Vec<_> = result["myOpenPRs"].as_array().unwrap().iter().collect();
    for issue in result["issues"].as_array().unwrap() {
        references.extend(issue["openPRs"].as_array().unwrap());
        if let Some(history) = issue.get("historicalPRs") {
            references.extend(history.as_array().unwrap());
        }
    }
    for reference in references {
        assert!(catalog.contains_key(reference.as_str().unwrap()));
    }
}

pub(super) async fn run_regressions(snippet: &ResolvedSnippet, builtin: &Path) {
    let sidecar: Value = serde_json::from_slice(
        &std::fs::read(builtin.join("unraid-linear-pr-triage.test.json")).unwrap(),
    )
    .unwrap();
    let report = run(snippet, json!({}), sidecar).await;
    assert!(report.passed, "{:?}", report.failures);
    assert_references(report.result.as_ref().unwrap());
    println!("PASS checked-in sidecar and normalized references");

    let mut data = scenario(json!([
        pr(1, "Fix U8-120", ""),
        pr(2, "chore: release 2.0", "U8-12 U8-120"),
        pr(3, "chore: release U8-12", ""),
        pr(4, "Implementation", "Fixes U8-12"),
        pr(4, "Implementation", "Fixes U8-12"),
    ]));
    data["expect"] = json!({"/schemaVersion":2,"/issues/0/openPRs":["unraid/core#3","unraid/core#4"],
        "/issues/1/openPRs":["unraid/core#1"],"/summary/uniquePRCount":3});
    let report = run(snippet, json!({}), data).await;
    assert!(report.passed, "{:?}", report.failures);
    assert_references(report.result.as_ref().unwrap());
    println!("PASS whole-ID matching, release suppression and PR deduplication");

    let mut data = scenario(json!([]));
    data["calls"][2]["result"]["total_count"] = json!(2);
    data["expect"] = json!({"/complete":false,"/issues/0/matchStatus":"incomplete","/issues/1/matchStatus":"incomplete","/summary/issuesWithoutObservedPRs":0});
    let report = run(snippet, json!({}), data).await;
    assert!(report.passed, "{:?}", report.failures);
    println!("PASS partial search coverage is not no-PR evidence");

    let mut data = scenario(json!([]));
    data["calls"][2].as_object_mut().unwrap().remove("result");
    data["calls"][2]["error"] = json!({"kind":"timeout","message":"Synthetic upstream timeout"});
    data["expect"] = json!({"/ok":false,"/complete":false,"/summary/relatedPRSearchFailures":1,"/summary/issuesWithoutObservedPRs":0});
    let report = run(snippet, json!({}), data).await;
    assert!(report.passed, "{:?}", report.failures);
    assert_eq!(report.result.as_ref().unwrap()["ok"], false);
    println!("PASS expected upstream failure survives batching");

    let mut data = scenario(json!([]));
    data["calls"].as_array_mut().unwrap().remove(2);
    data["calls"][0].as_object_mut().unwrap().remove("result");
    data["calls"][0]["error"] =
        json!({"kind":"timeout","message":"Synthetic issue lookup failure"});
    data["expect"] =
        json!({"/ok":false,"/summary/issueCount":null,"/summary/issuesWithoutObservedPRs":null});
    let report = run(snippet, json!({}), data).await;
    assert!(report.passed, "{:?}", report.failures);
    assert_eq!(report.result.as_ref().unwrap()["ok"], false);
    println!("PASS failed issue lookup is unknown, not zero issues");

    for initial in [0, 1] {
        let mut data = scenario(json!([]));
        data["calls"][initial]["result"] = Value::Null;
        data["calls"]
            .as_array_mut()
            .unwrap()
            .remove(if initial == 0 { 2 } else { 3 });
        data["expect"] = json!({"/ok":false,"/complete":false});
        let report = run(snippet, json!({}), data).await;
        assert!(report.passed, "{:?}", report.failures);
        assert_eq!(report.result.as_ref().unwrap()["ok"], false);
    }
    println!("PASS null issue and identity responses cannot report success");

    let mut malformed = pr(42, "U8-12 implementation", "");
    malformed.as_object_mut().unwrap().remove("html_url");
    let mut data = scenario(json!([malformed]));
    data["expect"] =
        json!({"/complete":false,"/issues/0/matchStatus":"incomplete","/summary/uniquePRCount":0});
    let report = run(snippet, json!({}), data).await;
    assert!(report.passed, "{:?}", report.failures);
    println!("PASS missing PR link marks coverage incomplete");

    let report = run(snippet, json!({"chunkSize":0}), json!({})).await;
    assert!(!report.passed);
    assert_eq!(report.metrics.tool_calls, 0);
    println!("PASS invalid input fails before any tool call");

    let items: Vec<_> = (1..=100)
        .map(|number| pr(number, &format!("U8-12 x{}", "🦀".repeat(100)), ""))
        .collect();
    let mut data = scenario(json!(items));
    data["expect"] = json!({"/complete":false});
    let report = run(snippet, json!({}), data).await;
    assert!(report.passed, "{:?}", report.failures);
    assert!(report.metrics.output_bytes <= 16_000);
    let result = report.result.unwrap();
    assert!(result["summary"]["outputOmitted"].as_u64().unwrap() > 0);
    assert_references(&result);
    println!("PASS Unicode output budget and reference-preserving pruning");
}
