use super::*;
use crate::shape::CodeModeResultShapeMetadata;
use crate::types::CodeModeExecutedCall;
use labby_runtime::CodeModeResultShapePolicy;

#[test]
fn automatic_result_marker_points_to_complete_saved_json() {
    let value = json!("x".repeat(100_000));
    let serialized = serde_json::to_string(&value).unwrap();
    let receipt = CodeModeArtifactReceipt {
        artifact_id: Some("saved-id".into()),
        path: "automatic/final-result.json".into(),
        absolute_path: "hidden".into(),
        content_type: "application/json".into(),
        bytes: serialized.len(),
        sha256: hex::encode(Sha256::digest(serialized.as_bytes())),
    };
    for compact in [false, true] {
        let marker = truncation_marker(&value, 4, std::slice::from_ref(&receipt), 512, compact);
        assert_eq!(marker["preserved_result_artifact_id"], "saved-id");
        assert!(
            marker["next_action"]
                .as_str()
                .unwrap()
                .contains("codemode.readArtifact")
        );
        assert!(
            !marker["next_action"]
                .as_str()
                .unwrap()
                .contains("not cached")
        );
    }
}

#[test]
fn a_matching_final_result_path_requires_matching_bytes_digest_and_handle() {
    let value = json!("x".repeat(100_000));
    let serialized = serde_json::to_string(&value).unwrap();
    let unrelated = serde_json::to_string(&json!("y".repeat(100_000))).unwrap();
    let valid = CodeModeArtifactReceipt {
        artifact_id: Some("saved-id".into()),
        path: "automatic/final-result.json".into(),
        absolute_path: "hidden".into(),
        content_type: "application/json".into(),
        bytes: serialized.len(),
        sha256: hex::encode(Sha256::digest(serialized.as_bytes())),
    };
    let mut bad_digest = valid.clone();
    bad_digest.sha256 = hex::encode(Sha256::digest(unrelated.as_bytes()));
    let mut bad_bytes = valid.clone();
    bad_bytes.bytes -= 1;
    let mut legacy = valid;
    legacy.artifact_id = None;
    for receipt in [bad_digest, bad_bytes, legacy] {
        for compact in [false, true] {
            let marker = truncation_marker(&value, 4, std::slice::from_ref(&receipt), 0, compact);
            assert!(marker.get("preserved_result_artifact_id").is_none());
            assert!(
                !marker["next_action"]
                    .as_str()
                    .unwrap()
                    .contains("Complete returned JSON saved")
            );
            assert_eq!(marker["artifacts"].as_array().unwrap().len(), 1);
        }
    }
}

fn response_with_logs(result: Value, logs: Vec<String>) -> CodeModeExecutionResponse {
    CodeModeExecutionResponse {
        execution_id: None,
        result: Some(result),
        result_shaping: None,
        ui: None,
        calls: Vec::new(),
        logs,
        artifacts: Vec::new(),
    }
}

/// Contract-derived expected cut: the smallest `k` such that the response
/// serialized with `original[k..]` as `logs` passes
/// [`response_within_budget`]. Probes real serde output per candidate, so
/// it is independent of the arithmetic implementation under test.
fn expected_drop_count(
    original: &[String],
    base: &CodeModeExecutionResponse,
    max_bytes: usize,
    max_tokens: usize,
    divisor: u32,
) -> usize {
    (0..=original.len())
        .find(|&k| {
            let mut probe = base.clone();
            probe.logs = original[k..].to_vec();
            response_within_budget(&probe, max_bytes, max_tokens, divisor)
        })
        .unwrap_or(original.len())
}

/// FR-5 (issue #210, lab-41e7m.2): the DEFAULT truncation path replaces an
/// over-budget result with an OBJECT marker — `truncated: true`,
/// `next_action`, a bounded `preview` — while required `calls[]` metadata
/// survives after optional trace params are shed. Structure must never
/// collapse to a bare string here; the
/// string-marker path is `shape.rs`, which only runs under a non-`Off`
/// result-shape policy.
#[test]
fn over_budget_result_becomes_object_marker_and_calls_survive() {
    let calls = vec![CodeModeExecutedCall {
        id: "demo::big_query".to_string(),
        ok: true,
        elapsed_ms: 42,
        start_ms: Some(1),
        params: Some(json!({"q": "everything"})),
        error_kind: None,
        ui: None,
    }];
    let mut response = response_with_logs(json!({"rows": vec!["r".repeat(64); 200]}), Vec::new());
    response.calls = calls.clone();
    response.result_shaping = Some(CodeModeResultShapeMetadata {
        policy: CodeModeResultShapePolicy::Off,
        changed: false,
        truncated: false,
        original_size_bytes: 0,
        shaped_size_bytes: 0,
        warning: None,
    });

    let truncated = truncate_execution_response(response, 4096, usize::MAX, 4);

    let marker = truncated.result.as_ref().expect("marker result");
    let marker = marker
        .as_object()
        .expect("marker must stay a JSON object, not a string");
    assert_eq!(marker["truncated"], json!(true));
    assert!(
        marker["next_action"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "marker carries agent guidance"
    );
    assert!(
        marker["next_action"]
            .as_str()
            .unwrap()
            .contains("Do not replay mutations")
    );
    assert_eq!(marker["resource_read_example"], RESOURCE_READ_EXAMPLE);
    assert!(response_within_budget(&truncated, 4096, usize::MAX, 4));
    assert!(
        marker["preview"].as_str().is_some_and(|s| s.len() <= 1024),
        "preview is bounded"
    );
    let mut expected_calls = calls;
    for call in &mut expected_calls {
        call.params = None;
    }
    assert_eq!(
        truncated.calls, expected_calls,
        "required calls[] metadata must survive after optional trace params are dropped"
    );
    assert!(
        truncated.result_shaping.is_none(),
        "stale shaping metadata must not describe the marker"
    );
}

#[test]
fn high_fanout_trace_params_are_dropped_before_compact_result() {
    let calls = (0..76)
        .map(|i| CodeModeExecutedCall {
            id: format!("ssh::{i}"),
            ok: true,
            elapsed_ms: 25,
            start_ms: Some(i * 3),
            params: Some(json!({
                "command": format!("ssh host-{i} {}", "x".repeat(2048)),
                "timeout": 20_000
            })),
            error_kind: None,
            ui: None,
        })
        .collect::<Vec<_>>();
    let expected_result = json!({
        "ok": true,
        "artifact": "homelab/docker-inventory.json",
        "containers": 138
    });
    let mut response = response_with_logs(expected_result.clone(), Vec::new());
    response.calls = calls;
    assert!(
        !response_within_budget(&response, 24 * 1024, 6_000, 4),
        "oracle must begin over budget"
    );

    let truncated = truncate_execution_response(response, 24 * 1024, 6_000, 4);

    assert_eq!(truncated.result, Some(expected_result));
    assert_eq!(truncated.calls.len(), 76, "call records must survive");
    assert!(
        truncated.calls.iter().all(|call| call.params.is_none()),
        "optional trace params should be the first pressure valve"
    );
    assert!(response_within_budget(&truncated, 24 * 1024, 6_000, 4));
}

#[test]
fn high_fanout_trace_params_stay_dropped_when_result_also_needs_truncation() {
    let calls = (0..24)
        .map(|i| CodeModeExecutedCall {
            id: format!("ssh::{i}"),
            ok: true,
            elapsed_ms: 25,
            start_ms: Some(i * 3),
            params: Some(json!({
                "command": format!("ssh host-{i} {}", "x".repeat(2048)),
                "timeout": 20_000
            })),
            error_kind: None,
            ui: None,
        })
        .collect::<Vec<_>>();
    let mut response = response_with_logs(json!({"rows": vec!["r".repeat(96); 180]}), Vec::new());
    response.calls = calls;
    let max_bytes = 12 * 1024;
    let max_tokens = 100_000;
    let divisor = 4;
    assert!(
        !response_within_budget(&response, max_bytes, max_tokens, divisor),
        "oracle must begin over budget"
    );
    let mut without_params = response.clone();
    for call in &mut without_params.calls {
        call.params = None;
    }
    assert!(
        !response_within_budget(&without_params, max_bytes, max_tokens, divisor),
        "the result must still need truncation after optional trace params are removed"
    );

    let truncated = truncate_execution_response(response, max_bytes, max_tokens, divisor);

    assert_eq!(truncated.calls.len(), 24, "call records must survive");
    assert!(
        truncated.calls.iter().all(|call| call.params.is_none()),
        "optional trace params must remain dropped while later pressure valves run"
    );
    assert_eq!(
        truncated
            .result
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|marker| marker.get("truncated")),
        Some(&json!(true)),
        "the independently oversized result should still become a truncation marker"
    );
    assert!(response_within_budget(
        &truncated, max_bytes, max_tokens, divisor
    ));
}

#[test]
fn large_log_set_cut_matches_probe_serialized_contract() {
    // Varied line lengths plus JSON-escaped and multibyte characters so the
    // arithmetic length model is exercised against real serde output.
    let logs: Vec<String> = (0..800)
        .map(|i| {
            format!(
                "line {i}: \"quoted\" \\ back {} — ünïcode",
                "x".repeat(i % 97)
            )
        })
        .collect();
    let response = response_with_logs(json!({"ok": true}), logs.clone());
    let (max_bytes, max_tokens, divisor) = (16 * 1024, 100_000, 4);

    let mut base = response.clone();
    base.logs = Vec::new();
    let expected = expected_drop_count(&logs, &base, max_bytes, max_tokens, divisor);
    assert!(
        expected > 0 && expected < logs.len(),
        "cut must land mid-range for this fixture, got {expected}"
    );

    let truncated = truncate_execution_response(response, max_bytes, max_tokens, divisor);

    // Logs-dominant response: the small result must survive untouched.
    assert_eq!(truncated.result, Some(json!({"ok": true})));
    let mut want = vec![format!(
        "[logs truncated to fit response budget — {expected} line(s) dropped]"
    )];
    want.extend_from_slice(&logs[expected..]);
    assert_eq!(truncated.logs, want);
}

#[test]
fn token_budget_alone_can_force_the_cut() {
    let logs: Vec<String> = (0..300).map(|i| format!("log line number {i}")).collect();
    let response = response_with_logs(json!({"ok": true}), logs.clone());
    // Byte budget is generous; the estimated-token term must drive the cut.
    let (max_bytes, max_tokens, divisor) = (1024 * 1024, 512, 4);

    let mut base = response.clone();
    base.logs = Vec::new();
    let expected = expected_drop_count(&logs, &base, max_bytes, max_tokens, divisor);
    assert!(
        expected > 0 && expected < logs.len(),
        "cut must land mid-range for this fixture, got {expected}"
    );

    let truncated = truncate_execution_response(response, max_bytes, max_tokens, divisor);
    let mut want = vec![format!(
        "[logs truncated to fit response budget — {expected} line(s) dropped]"
    )];
    want.extend_from_slice(&logs[expected..]);
    assert_eq!(truncated.logs, want);
}

#[test]
fn oversized_single_line_drops_everything() {
    let logs = vec!["z".repeat(64 * 1024)];
    let response = response_with_logs(json!({"ok": true}), logs);
    let truncated = truncate_execution_response(response, 4 * 1024, 100_000, 4);
    assert_eq!(
        truncated.logs,
        vec!["[logs truncated to fit response budget — 1 line(s) dropped]".to_string()]
    );
}

#[test]
fn preservation_probe_matches_truncation_and_restores_every_field() {
    for result in [
        json!({"ok": true}),
        json!({"rows": vec!["🦀\\\"".repeat(64); 100]}),
    ] {
        for with_params in [false, true] {
            let mut response = response_with_logs(result.clone(), vec!["log".repeat(100); 10]);
            response.calls.push(CodeModeExecutedCall {
                id: "demo::query".into(),
                ok: true,
                elapsed_ms: 4,
                start_ms: Some(1),
                params: with_params.then(|| json!({"query": "x".repeat(8000)})),
                error_kind: None,
                ui: None,
            });
            response.result_shaping = Some(CodeModeResultShapeMetadata {
                policy: CodeModeResultShapePolicy::Truncate,
                changed: false,
                truncated: false,
                original_size_bytes: 11,
                shaped_size_bytes: 11,
                warning: Some("soft warning".into()),
            });
            for (bytes, tokens, divisor) in [
                (512, usize::MAX, 4),
                (2048, 512, 4),
                (4096, usize::MAX, 4),
                (usize::MAX, 1, 1),
                (100_000, usize::MAX, 4),
            ] {
                let original = response.clone();
                let expected =
                    truncate_execution_response(original.clone(), bytes, tokens, divisor);
                let preserve = result_would_be_truncated(&mut response, bytes, tokens, divisor);
                assert_eq!(preserve, expected.result != original.result);
                assert_eq!(
                    response, original,
                    "borrowed probe must restore raw response"
                );
            }
        }
    }
}

#[test]
fn log_pressure_preserves_result_shaping_when_no_marker_shrinks_result() {
    let mut response = response_with_logs(
        json!({"ok": true}),
        vec!["log \"quoted\" — unicode".repeat(200); 12],
    );
    let metadata = CodeModeResultShapeMetadata {
        policy: CodeModeResultShapePolicy::Truncate,
        changed: false,
        truncated: false,
        original_size_bytes: 11,
        shaped_size_bytes: 11,
        warning: None,
    };
    response.result_shaping = Some(metadata.clone());
    let truncated = truncate_execution_response(response, 2048, usize::MAX, 4);
    assert_eq!(truncated.result, Some(json!({"ok": true})));
    assert_eq!(truncated.result_shaping, Some(metadata));
    assert!(response_within_budget(&truncated, 2048, usize::MAX, 4));
}

#[test]
fn within_budget_response_is_returned_unchanged() {
    let logs: Vec<String> = (0..4).map(|i| format!("short {i}")).collect();
    let response = response_with_logs(json!({"ok": true}), logs.clone());
    let untouched = truncate_execution_response(response.clone(), 1024 * 1024, 100_000, 4);
    assert_eq!(untouched, response);
}
