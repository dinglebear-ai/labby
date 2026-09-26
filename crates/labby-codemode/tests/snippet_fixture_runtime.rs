//! Hermetic integration tests: this binary re-execs itself as the real runner.
//! No installed Labby executable, gateway, credentials or upstreams are required.
use labby_codemode::snippet::harness::{SnippetFixture, run_fixture};
use labby_codemode::snippet::store::{ResolvedSnippet, SnippetSource, resolve_snippet};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::ExitCode;

#[path = "fixtures/triage_regressions.rs"]
mod triage_regressions;

fn snippet(body: &str) -> ResolvedSnippet {
    ResolvedSnippet {
        name: "fixture-probe".into(),
        body: body.into(),
        description: None,
        tags: Vec::new(),
        inputs: Default::default(),
        tools: None,
        source: SnippetSource::User,
        path: PathBuf::from("fixture-probe.md"),
    }
}
fn fixture(value: Value) -> SnippetFixture {
    serde_json::from_value(value).unwrap()
}

// These strings are JavaScript object literals, not Rust format strings.
#[allow(clippy::literal_string_with_formatting_args)]
async fn suite() {
    let cases = [
        (
            "success",
            "async () => ({value:42})",
            json!({"expect":{"/value":42}}),
            true,
        ),
        (
            "snapshot",
            "async () => ({value:42,timestamp:2})",
            json!({"snapshot":{"value":42,"timestamp":1},"ignore_paths":["/timestamp"]}),
            true,
        ),
        (
            "assertion",
            "async () => ({value:42})",
            json!({"expect":{"/value":99}}),
            false,
        ),
        (
            "mock response",
            "async () => await callTool('test::read',{id:1})",
            json!({"calls":[{"tool":"test::read","match":{"id":1},"result":{"value":42}}],"expect":{"/value":42}}),
            true,
        ),
        (
            "caught unexpected call",
            "async () => {try {await callTool('test::missing',{});} catch (_) {} return {ok:true};}",
            json!({}),
            false,
        ),
        (
            "unused response",
            "async () => ({ok:true})",
            json!({"calls":[{"tool":"test::read","result":42}]}),
            false,
        ),
        (
            "expected rejection",
            "async () => {try {await callTool('test::read',{});} catch (e) {return {kind:e.kind};}}",
            json!({"calls":[{"tool":"test::read","error":{"kind":"timeout","message":"synthetic"}}],"expect":{"/kind":"timeout"}}),
            true,
        ),
        (
            "output budget",
            "async () => '🦀'",
            json!({"budgets":{"output_bytes":5}}),
            false,
        ),
        (
            "call budget",
            "async () => {try {await callTool('test::read',{});} catch (_) {} return 1;}",
            json!({"budgets":{"tool_calls":0}}),
            false,
        ),
        (
            "exception",
            "async () => {throw new Error('synthetic');}",
            json!({}),
            false,
        ),
        (
            "native host escape",
            "async () => {try {await (new Function('return callTool'))()('ghost::read',{});} catch (_) {} return {ok:true};}",
            json!({}),
            false,
        ),
    ];
    for (name, source, data, expected) in cases {
        let report = run_fixture(&snippet(source), json!({}), &fixture(data))
            .await
            .unwrap();
        assert_eq!(report.passed, expected, "{name}: {:?}", report.failures);
        println!("PASS {name}: {} ms", report.metrics.wall_clock_ms);
    }
    let batching = snippet(
        "async () => {const b=await codemode.batch([()=>callTool('test::read',{}),()=>callTool('test::read',{})]);return {count:b.ok.length};}",
    );
    let report = run_fixture(
        &batching,
        json!({}),
        &fixture(
            json!({"calls":[{"tool":"test::read","result":1,"times":2}],"expect":{"/count":2}}),
        ),
    )
    .await
    .unwrap();
    assert!(report.passed, "{:?}", report.failures);
    assert_eq!(report.metrics.tool_calls, 2);
    assert_eq!(report.metrics.max_in_flight, 2);
    println!("PASS production batch overlap");

    let timed = run_fixture(
        &snippet("async () => {while(true) {}}"),
        json!({}),
        &fixture(json!({"budgets":{"wall_clock_ms":200}})),
    )
    .await;
    assert!(
        timed.is_err(),
        "infinite loops must be killed by the production deadline"
    );
    println!("PASS production timeout containment");

    let home = tempfile::tempdir().unwrap();
    let builtin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/snippets");
    let triage = resolve_snippet(home.path(), &builtin, "unraid-linear-pr-triage").unwrap();
    for deep in [false, true] {
        let ids: Vec<_> = (1..=26).map(|i| format!("U8-{i}")).collect();
        let issues: Vec<_> = ids
            .iter()
            .map(|id| json!({"id":id,"title":format!("Synthetic {id}"),"status":"In Progress"}))
            .collect();
        let mut calls = vec![
            json!({"tool":"linear-notification-worker::list_issues","match":{"team":"U8","assignee":"me","state":"started"},"result":{"issues":issues}}),
            json!({"tool":"github::get_me","result":{"login":"fixture-user"}}),
        ];
        for chunk in ids.chunks(4) {
            let query = chunk
                .iter()
                .map(|id| format!("\"{id}\" org:unraid{}", if deep { "" } else { " is:open" }))
                .collect::<Vec<_>>()
                .join(" OR ");
            calls.push(json!({"tool":"github::search_pull_requests","match":{"query":query,"perPage":100},"result":{"total_count":0,"items":[]}}));
        }
        calls.push(json!({"tool":"github::search_pull_requests","match":{"query":"author:fixture-user is:open org:unraid"},"result":{"total_count":1,"items":[{"number":1512,"title":"Synthetic Cloudflare work","state":"open","html_url":"https://github.com/unraid/cloudflare/pull/1512","repository_url":"https://api.github.com/repos/unraid/cloudflare","user":{"login":"fixture-user"}}]}}));
        if deep {
            for id in &ids {
                calls.push(json!({"tool":"linear-notification-worker::get_handoff","match":{"issue":id},"result":{"success":true,"status":{"stateName":"In Progress","previousHandoffs":[{"large":"not copied"}],"previousQaRuns":[]}}}));
            }
        }
        let f = fixture(
            json!({"calls":calls,"expect":{"/summary/issueCount":26,"/summary/relatedPRSearchCalls":7,"/summary/relatedPRSearchFailures":0,"/summary/myOpenPRCount":1,"/complete":true},"budgets":{"tool_calls":40}}),
        );
        let report = run_fixture(&triage, json!({"deep":deep}), &f)
            .await
            .unwrap();
        assert!(report.passed, "deep={deep}: {:?}", report.failures);
        assert_eq!(report.metrics.tool_calls, if deep { 36 } else { 10 });
        let result = report.result.unwrap();
        for issue in result["issues"].as_array().unwrap() {
            assert_eq!(issue.get("historicalPRs").is_some(), deep);
            assert_eq!(issue.get("handoff").is_some(), deep);
        }
        assert!(report.metrics.max_in_flight >= 8);
        println!(
            "PASS 26-issue triage deep={deep}: {} calls, {} bytes",
            report.metrics.tool_calls, report.metrics.output_bytes
        );
    }
    triage_regressions::run_regressions(&triage, &builtin).await;
    println!("25 production-runner integration scenarios passed");
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["internal", "code-mode-runner"] {
        return labby_codemode::run_code_mode_runner_stdio();
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(suite());
    ExitCode::SUCCESS
}
