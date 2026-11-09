use super::*;

#[test]
fn auxiliary_content_obeys_byte_token_and_tiny_execution_budgets() {
    let mut response = response_with_logs(json!({"selected":1}), vec![]);
    response.calls.push(CodeModeExecutedCall{ id:"demo::lookup".into(), ok:true, elapsed_ms:0,start_ms:None,params:None,params_digest:None,error_kind:None,ui:None,upstream_content:Some(json!({"content":[{"type":"text","text":"x".repeat(5000)}],"omitted_blocks":0,"omitted_bytes":0})) });
    for (bytes, tokens) in [(1000, 1000), (10000, 250), (1, 1)] {
        let output = truncate_execution_response(response.clone(), bytes, tokens, 4);
        assert!(output.calls[0].ok);
        assert_eq!(output.result, Some(json!({"selected":1})));
        let auxiliary = output.calls[0].upstream_content.as_ref().unwrap();
        assert_eq!(auxiliary["omitted_blocks"], 1);
        assert!(auxiliary["omitted_bytes"].as_u64().unwrap() >= 5000);
        assert!(auxiliary.get("content").is_none());
        if bytes > 1 {
            assert!(response_within_budget(&output, bytes, tokens, 4));
        }
    }
}

#[test]
fn auxiliary_only_pressure_does_not_request_preservation() {
    let mut response = response_with_logs(json!("r".repeat(2000)), vec![]);
    response.calls.push(CodeModeExecutedCall {
        id: "demo::lookup".into(), ok: true, elapsed_ms: 0, start_ms: None,
        params: None, params_digest: None, error_kind: None, ui: None,
        upstream_content: Some(json!({"content":[{"type":"text","text":"x".repeat(5000)}],"omitted_blocks":0,"omitted_bytes":0})),
    });
    for (bytes, tokens) in [(4096, usize::MAX), (usize::MAX, 1024)] {
        let original = response.clone();
        assert!(!result_would_be_truncated(&mut response, bytes, tokens, 4));
        assert_eq!(response, original);
        let output = truncate_execution_response(response.clone(), bytes, tokens, 4);
        assert_eq!(output.result, original.result);
    }
}

#[test]
fn auxiliary_omission_has_bounded_whole_envelope_encodes() {
    for retained in [false, true] {
        for rows in [1, 10, 100, 1000] {
            let mut response = response_with_logs(json!("r".repeat(100_000)), vec![]);
            for index in 0..rows {
                response.calls.push(CodeModeExecutedCall {
                id: format!("demo::lookup{index}"), ok: index % 2 == 0, elapsed_ms: 0, start_ms: None,
                params: None, params_digest: None, error_kind: None, ui: None,
                upstream_content: Some(json!({"content":if retained {json!([{"type":"text","text":"🦀\\\""}])} else {json!([])},"omitted_blocks":1,"omitted_bytes":20000})),
            });
            }
            ENVELOPE_ENCODES.with(|count| count.set(0));
            let output = truncate_execution_response(response, 1, 1, 4);
            let encodes = ENVELOPE_ENCODES.with(|count| count.get());
            assert!(
                encodes < 12,
                "retained={retained}: {encodes} whole-envelope encodes"
            );
            assert_eq!(output.calls.len(), rows);
            for (index, call) in output.calls.iter().enumerate() {
                assert_eq!(call.ok, index % 2 == 0);
                assert_eq!(
                    call.upstream_content.as_ref().unwrap()["omitted_blocks"],
                    if retained { 2 } else { 1 }
                );
            }
        }
    }
}

#[test]
fn auxiliary_byte_deltas_keep_the_first_fitting_prefix_exactly() {
    let mut original = response_with_logs(json!({"selected":1}), vec![]);
    for index in 0..24 {
        original.calls.push(CodeModeExecutedCall {
            id:format!("demo::lookup{index}"),ok:index%2==0,elapsed_ms:0,start_ms:None,params:None,params_digest:None,error_kind:None,ui:None,
            upstream_content:Some(json!({"content":if index%3==0 {json!([])} else {json!([{"type":"text","text":"🦀\\\"\n".repeat(index+1)}])},"omitted_blocks":9,"omitted_bytes":99})),
        });
    }
    let mut candidate = original.clone();
    for cut in 0..=24 {
        if cut > 0 {
            crate::upstream_content::omit_code_mode_upstream_content(
                candidate.calls[cut - 1].upstream_content.as_mut().unwrap(),
            );
        }
        let size = serde_json::to_vec(&candidate).unwrap().len();
        for limit in [size - 1, size, size + 1] {
            for (bytes, tokens) in [(limit, usize::MAX), (usize::MAX, limit.div_ceil(3))] {
                let mut expected = original.clone();
                if !response_within_budget(&expected, bytes, tokens, 3) {
                    for index in 0..expected.calls.len() {
                        crate::upstream_content::omit_code_mode_upstream_content(
                            expected.calls[index].upstream_content.as_mut().unwrap(),
                        );
                        if response_within_budget(&expected, bytes, tokens, 3) {
                            break;
                        }
                    }
                }
                let output = truncate_execution_response(original.clone(), bytes, tokens, 3);
                assert_eq!(output, expected, "cut={cut},bytes={bytes},tokens={tokens}");
            }
        }
    }
}

#[test]
fn within_budget_response_is_returned_unchanged() {
    let logs: Vec<String> = (0..4).map(|i| format!("short {i}")).collect();
    let response = response_with_logs(json!({"ok": true}), logs.clone());
    let untouched = truncate_execution_response(response.clone(), 1024 * 1024, 100_000, 4);
    assert_eq!(untouched, response);
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
