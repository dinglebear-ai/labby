//! A real QuickJS rejection can abandon a reply already in the runner pipe.
#![cfg(all(feature = "gateway", unix))]

use serde_json::{Value, json};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn real_quickjs_promise_all_rejection_exposes_the_late_reply_boundary() {
    let home = tempfile::tempdir().expect("owned runner HOME");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .env_clear()
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path())
        .env("TMPDIR", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("real product runner");
    let mut stdin = child.stdin.take().expect("runner stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("runner stdout")).lines();
    let scenario = async {
        let start = json!({
            "type": "start", "proxy": "",
            "code": "async () => await Promise.all([callTool('fixture::fail', {}), callTool('fixture::effect', {})])",
        });
        stdin
            .write_all(format!("{start}\n").as_bytes())
            .await
            .expect("Start");
        stdin.flush().await.expect("Start flush");
        let mut requests = Vec::new();
        for _ in 0..2 {
            let line = stdout
                .next_line()
                .await
                .expect("runner line")
                .expect("tool request");
            let request: Value = serde_json::from_str(&line).expect("request JSON");
            assert_eq!(request["type"], "tool_call");
            requests.push(request);
        }
        let failed = requests
            .iter()
            .find(|request| request["id"] == "fixture::fail")
            .expect("first rejected promise");
        let late = requests
            .iter()
            .find(|request| request["id"] == "fixture::effect")
            .expect("second in-flight promise");
        // Both replies enter stdin together: the first rejects Promise.all and
        // drops the runtime before the second frame is read as a new Start.
        let error = json!({"type":"tool_error", "seq":failed["seq"],
            "kind":"tool_error", "message":"owned first rejection", "tool":"fixture::fail"});
        let completion = json!({"type":"tool_result", "seq":late["seq"],
            "result":{"applied":true}});
        stdin
            .write_all(format!("{error}\n{completion}\n").as_bytes())
            .await
            .expect("in-transit replies");
        stdin.flush().await.expect("reply flush");
        let first: Value = serde_json::from_str(
            &stdout
                .next_line()
                .await
                .expect("first Error read")
                .expect("first Error"),
        )
        .expect("first Error JSON");
        assert_eq!(first["type"], "error");
        assert_eq!(first["kind"], "tool_error");
        assert_eq!(first["message"], "owned first rejection");
        assert_eq!(first["tool"], "fixture::fail");
        let late: Value = serde_json::from_str(
            &stdout
                .next_line()
                .await
                .expect("late Error read")
                .expect("late Error"),
        )
        .expect("late Error JSON");
        assert_eq!(late["type"], "error");
        assert_eq!(late["message"], "runner expected start message");
    };
    tokio::time::timeout(Duration::from_secs(10), scenario)
        .await
        .expect("bounded real QuickJS scenario");
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .expect("runner exits on stdin EOF")
        .expect("runner exit status");
    assert!(status.success());
}

async fn caught_done_native_proof(clear_pending_map: bool) {
    let home = tempfile::tempdir().expect("owned caught runner HOME");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .env_clear()
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path())
        .env("TMPDIR", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("real product runner");
    let mut stdin = child.stdin.take().expect("runner stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("runner stdout")).lines();
    let scenario = async {
        let clear = if clear_pending_map {
            "globalThis.__labPendingToolCalls.clear();"
        } else {
            ""
        };
        let start = json!({
            "type":"start", "proxy":"",
            "code":format!("async () => {{ try {{ await Promise.all([callTool('fixture::fail', {{}}), callTool('fixture::effect', {{}})]); }} catch {{}} {clear} return {{owner:'A'}}; }}"),
        });
        stdin
            .write_all(format!("{start}\n").as_bytes())
            .await
            .expect("caught Start");
        stdin.flush().await.expect("Start flush");
        let mut requests = Vec::new();
        for _ in 0..2 {
            let line = stdout
                .next_line()
                .await
                .expect("request line")
                .expect("tool call");
            let request: Value = serde_json::from_str(&line).expect("request JSON");
            assert_eq!(request["type"], "tool_call");
            requests.push(request);
        }
        let failed = requests
            .iter()
            .find(|request| request["id"] == "fixture::fail")
            .expect("failed request");
        let late = requests
            .iter()
            .find(|request| request["id"] == "fixture::effect")
            .expect("applied request");
        let error = json!({"type":"tool_error", "seq":failed["seq"],
            "kind":"tool_error", "message":"owned caught rejection"});
        let completion = json!({"type":"tool_result", "seq":late["seq"],
            "result":{"applied":true}});
        stdin
            .write_all(format!("{error}\n{completion}\n").as_bytes())
            .await
            .expect("both already-sent replies");
        stdin.flush().await.expect("reply flush");
        let done: Value = serde_json::from_str(
            &stdout
                .next_line()
                .await
                .expect("Done line")
                .expect("caught Done"),
        )
        .expect("Done JSON");
        assert_eq!(done["type"], "done");
        assert_eq!(done["result"]["value"]["owner"], "A");
        // Raw-child mechanism supplement: the second input really remained
        // unread after successful completion. The parent must evict this child.
        let stale: Value = serde_json::from_str(
            &stdout
                .next_line()
                .await
                .expect("late frame line")
                .expect("late Error"),
        )
        .expect("late JSON");
        assert_eq!(stale["type"], "error");
        assert_eq!(stale["message"], "runner expected start message");
        assert_eq!(
            done["reuse_ready"], false,
            "native proof must reject unread replies even if JS clears its pending map"
        );
    };
    tokio::time::timeout(Duration::from_secs(10), scenario)
        .await
        .expect("bounded caught QuickJS scenario");
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .expect("runner EOF deadline")
        .expect("runner exit");
    assert!(status.success());
}

#[tokio::test]
async fn real_quickjs_caught_fan_out_requires_non_reusable_done() {
    caught_done_native_proof(false).await;
}

#[tokio::test]
async fn real_quickjs_caught_fan_out_cannot_forge_reuse_by_clearing_js_pending_map() {
    caught_done_native_proof(true).await;
}

#[tokio::test]
async fn real_quickjs_clean_typed_replies_qualify_done_and_the_same_child_serves_next_start() {
    let home = tempfile::tempdir().expect("owned clean runner HOME");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_labby"))
        .args(["internal", "code-mode-runner"])
        .env_clear()
        .env("HOME", home.path())
        .env("LABBY_HOME", home.path())
        .env("TMPDIR", home.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("real product runner");
    let mut stdin = child.stdin.take().expect("runner stdin");
    let mut stdout = BufReader::new(child.stdout.take().expect("runner stdout")).lines();
    let scenario = async {
        let start = json!({"type":"start", "proxy":"",
            "code": "async () => { let caught; try { await callTool('fixture::fail', {}); } catch (error) { caught = JSON.parse(error.message).kind; } const artifact = await writeArtifact('owned.txt', 'v'); const snippet = await __labRunSnippet('owned', {}); const step = await __labCodemodeStep('checked', async () => 4); return {caught, artifact, snippet, step}; }"});
        stdin
            .write_all(format!("{start}\n").as_bytes())
            .await
            .expect("clean Start");
        stdin.flush().await.expect("Start flush");
        let mut phases = Vec::new();
        let mut step_seq = None;
        loop {
            let line = stdout
                .next_line()
                .await
                .expect("native line")
                .expect("native frame");
            let out: Value = serde_json::from_str(&line).expect("frame JSON");
            let kind = out["type"].as_str().expect("frame type");
            if kind == "done" {
                assert_eq!(
                    out["reuse_ready"], true,
                    "fully consumed replies qualify reuse"
                );
                assert_eq!(
                    out["result"]["value"],
                    json!({"caught":"tool_error",
                    "artifact":{"path":"owned.txt"}, "snippet":9, "step":4})
                );
                break;
            }
            phases.push(kind.to_owned());
            let seq = out["seq"].clone();
            let response = match kind {
                "tool_call" => Some(json!({"type":"tool_error","seq":seq,
                    "kind":"tool_error","message":"caught single rejection"})),
                "artifact_write" => {
                    Some(json!({"type":"tool_result","seq":seq,"result":{"path":"owned.txt"}}))
                }
                "snippet_resolve" => Some(json!({"type":"snippet_resolved","seq":seq,
                    "code":"async () => 9", "input":{}})),
                "step_begin" => {
                    step_seq = Some(seq.clone());
                    Some(json!({"type":"step_decision","seq":seq,"replay":null}))
                }
                "step_result" => {
                    assert_eq!(
                        Some(&seq),
                        step_seq.as_ref(),
                        "step phases reuse the same seq"
                    );
                    assert_eq!(out["value"], 4);
                    Some(json!({"type":"step_recorded","seq":seq}))
                }
                _ => None,
            }
            .expect("expected typed native request");
            stdin
                .write_all(format!("{response}\n").as_bytes())
                .await
                .expect("typed reply");
            stdin.flush().await.expect("reply flush");
        }
        assert_eq!(
            phases,
            [
                "tool_call",
                "artifact_write",
                "snippet_resolve",
                "step_begin",
                "step_result"
            ]
        );
        let b = json!({"type":"start","proxy":"","code":"async () => ({owner:'B'})"});
        stdin
            .write_all(format!("{b}\n").as_bytes())
            .await
            .expect("same child next Start");
        stdin.flush().await.expect("next Start flush");
        let done: Value =
            serde_json::from_str(&stdout.next_line().await.expect("B line").expect("B Done"))
                .expect("B JSON");
        assert_eq!(done["type"], "done");
        assert_eq!(done["reuse_ready"], true);
        assert_eq!(done["result"]["value"]["owner"], "B");
    };
    tokio::time::timeout(Duration::from_secs(10), scenario)
        .await
        .expect("bounded clean scenario");
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .expect("runner EOF deadline")
        .expect("runner exit");
    assert!(status.success());
}
