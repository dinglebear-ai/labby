use super::*;

/// Budget auxiliary content against the complete MCP response, including its
/// duplicated text/structured channels and failure envelope. Mandatory trace
/// metadata keeps the existing best-effort behavior when it alone is over budget.
pub(super) fn bound_upstream_content(
    result: &mut CallToolResult,
    max_bytes: usize,
    max_tokens: usize,
) {
    #[cfg(test)]
    ENVELOPE_ENCODES.with(|count| count.set(count.get() + 1));
    let Ok(encoded) = serde_json::to_string(result) else {
        return;
    };
    let fits = |bytes: usize| bytes <= max_bytes && bytes.div_ceil(4) <= max_tokens;
    let mut bytes = encoded.len();
    if fits(bytes) {
        return;
    }
    let mut text = result
        .content
        .first()
        .and_then(|block| block.as_text())
        .and_then(|block| serde_json::from_str::<Value>(&block.text).ok());
    // The old loop normalized parsed text on its first omission. Account for
    // that once, then adjust only each affected JSON fragment (including its
    // additional escaping inside the outer MCP text string).
    let text_normalization = text.as_ref().and_then(|value| {
        let original = &result.content.first()?.as_text()?.text;
        Some((escaped_len(original), escaped_len(&value.to_string())))
    });
    let Some(calls) = result
        .structured_content
        .as_mut()
        .and_then(|value| value.get_mut("calls"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let mut changed = false;
    for (index, call) in calls.iter_mut().enumerate() {
        let Some(content) = call.get_mut("upstream_content") else {
            continue;
        };
        if content.get("content").is_none() {
            continue;
        }
        if !changed && let Some((old, new)) = text_normalization {
            bytes = bytes - old + new;
        }
        changed = true;
        bytes = replace_with_omission(content, bytes, false);
        if let Some(content) = text
            .as_mut()
            .and_then(|value| value.get_mut("calls"))
            .and_then(|value| value.get_mut(index))
            .and_then(|value| value.get_mut("upstream_content"))
        {
            bytes = replace_with_omission(content, bytes, true);
        }
        if fits(bytes) {
            break;
        }
    }
    if changed
        && let (Some(value), Some(ContentBlock::Text(block))) = (&text, result.content.first_mut())
    {
        block.text = value.to_string();
    }
}

fn escaped_len(text: &str) -> usize {
    serde_json::to_string(text).expect("JSON string").len()
}

fn replace_with_omission(content: &mut Value, bytes: usize, escaped: bool) -> usize {
    let size = |value: &Value| {
        let encoded = value.to_string();
        if escaped {
            escaped_len(&encoded)
        } else {
            encoded.len()
        }
    };
    let old = size(content);
    labby_codemode::omit_code_mode_upstream_content(content);
    bytes - old + size(content)
}

#[cfg(test)]
thread_local! { static ENVELOPE_ENCODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn omissions_use_bounded_whole_envelope_encodes() {
        for retained in [false, true] {
            for rows in [1, 10, 100, 1000] {
                let calls: Vec<_> = (0..rows).map(|index| json!({"id":format!("fixture::call{index}"),"ok":index%2==0,"upstream_content":{"content":if retained {json!([{"type":"text","text":"🦀\\\""}])} else {json!([])},"omitted_blocks":1,"omitted_bytes":20000}})).collect();
                let trace = json!({"calls":calls,"result":"r".repeat(100_000)});
                let mut result =
                    CallToolResult::success(vec![ContentBlock::text(trace.to_string())]);
                result.structured_content = Some(trace);
                ENVELOPE_ENCODES.with(|count| count.set(0));
                bound_upstream_content(&mut result, 1, 1);
                assert!(ENVELOPE_ENCODES.with(|count| count.get()) <= 2);
                for (index, call) in result.structured_content.as_ref().unwrap()["calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                {
                    assert_eq!(call["ok"], index % 2 == 0);
                    assert_eq!(
                        call["upstream_content"]["omitted_blocks"],
                        if retained { 2 } else { 1 }
                    );
                }
            }
        }
    }
    #[test]
    fn executed_error_keeps_asymmetric_envelope_under_byte_and_token_pressure() {
        let mut error =
            labby_codemode::CodeModeCallError::new("execution_error", "script failed after write")
                .with_side_effects(labby_codemode::CodeModeSideEffectRisk::Possible);
        error.recovery.guidance = "Inspect the completed write before retrying".into();
        let envelope = code_mode_error_envelope("codemode", "call_tool", &error);
        let trace = json!({
            "kind":"code_mode_execute_trace", "call_count":1,
            "calls":[{"id":"fixture::write","ok":true,"upstream_content":{"content":[{"type":"text","text":"🦀\\\"".repeat(1000)}],"omitted_blocks":0,"omitted_bytes":0}}],
            "error_kind":error.kind(), "error":error, "execution_id":"test-execution",
            "elapsed_ms":1, "input_tokens":10, "output_tokens":0,
            "result_shape":{"type":"undefined"}, "logs_count":0
        });
        let mut original = CallToolResult::error(vec![ContentBlock::text(envelope.to_string())]);
        original.structured_content = Some(trace);
        let mut mandatory = original.clone();
        labby_codemode::omit_code_mode_upstream_content(
            &mut mandatory.structured_content.as_mut().unwrap()["calls"][0]["upstream_content"],
        );
        let minimum = serde_json::to_vec(&mandatory).unwrap().len();
        let full = serde_json::to_vec(&original).unwrap().len();
        for (bytes, tokens, omitted) in [
            (full, usize::MAX, false),
            (minimum, usize::MAX, true),
            (usize::MAX, minimum.div_ceil(4), true),
            (1, 1, true),
        ] {
            let mut result = original.clone();
            bound_upstream_content(&mut result, bytes, tokens);
            assert_eq!(result.is_error, Some(true));
            let text = &result.content[0].as_text().unwrap().text;
            assert_eq!(text, &envelope.to_string());
            let parsed: Value = serde_json::from_str(text).unwrap();
            assert_eq!(parsed, envelope);
            assert_eq!(parsed["error"]["side_effects"], "possible");
            assert!(parsed["error"]["recovery"].is_object());
            let structured = result.structured_content.as_ref().unwrap();
            assert_eq!(structured["calls"][0]["ok"], true);
            assert_eq!(structured["error"], serde_json::to_value(&error).unwrap());
            assert_eq!(
                structured["calls"][0]["upstream_content"]["omitted_blocks"],
                usize::from(omitted)
            );
            assert_eq!(
                structured["calls"][0]["upstream_content"]
                    .get("content")
                    .is_none(),
                omitted
            );
            let encoded = serde_json::to_string(&result).unwrap();
            if bytes == 1 {
                assert!(encoded.len() > bytes, "mandatory metadata is best effort");
            } else {
                assert!(encoded.len() <= bytes);
                assert!(estimate_tokens(&encoded) <= tokens);
            }
        }
    }
    #[test]
    fn byte_deltas_match_first_fitting_omission_in_both_channels() {
        for failed in [false, true] {
            let calls: Vec<_> = (0..24).map(|index| json!({"id":format!("fixture::call{index}"),"ok":index%2==0,"upstream_content":{"content":if index%3==0 {json!([])} else {json!([{"type":"text","text":"🦀\\\"\n".repeat(index+1)}])},"omitted_blocks":9,"omitted_bytes":99}})).collect();
            let trace = json!({"calls":calls,"result":{"selected":1}});
            let text = if failed {
                let error = labby_codemode::CodeModeCallError::new(
                    "execution_error",
                    "after successful call",
                );
                code_mode_error_envelope("codemode", "call_tool", &error)
            } else {
                trace.clone()
            };
            let mut original = CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&text).unwrap(),
            )]);
            original.structured_content = Some(trace);
            original.is_error = Some(failed);
            let mut candidate = original.clone();
            for cut in 0..=24 {
                if cut > 0 {
                    labby_codemode::omit_code_mode_upstream_content(
                        &mut candidate.structured_content.as_mut().unwrap()["calls"][cut - 1]["upstream_content"],
                    );
                    let mut parsed: Value =
                        serde_json::from_str(&candidate.content[0].as_text().unwrap().text)
                            .unwrap();
                    if let Some(content) = parsed
                        .get_mut("calls")
                        .and_then(|value| value.get_mut(cut - 1))
                        .and_then(|value| value.get_mut("upstream_content"))
                    {
                        labby_codemode::omit_code_mode_upstream_content(content);
                    }
                    candidate.content[0] = ContentBlock::text(parsed.to_string());
                }
                let size = serde_json::to_vec(&candidate).unwrap().len();
                for limit in [size - 1, size, size + 1] {
                    for (bytes, tokens) in [(limit, usize::MAX), (usize::MAX, limit.div_ceil(4))] {
                        let mut expected = original.clone();
                        legacy_bound(&mut expected, bytes, tokens);
                        let mut actual = original.clone();
                        bound_upstream_content(&mut actual, bytes, tokens);
                        assert_eq!(
                            actual, expected,
                            "failed={failed}, cut={cut}, bytes={bytes}, tokens={tokens}"
                        );
                    }
                }
            }
        }
    }

    fn legacy_bound(result: &mut CallToolResult, max_bytes: usize, max_tokens: usize) {
        let fits = |candidate: &CallToolResult| {
            serde_json::to_string(candidate).is_ok_and(|encoded| {
                encoded.len() <= max_bytes && estimate_tokens(&encoded) <= max_tokens
            })
        };
        if fits(result) {
            return;
        }
        let mut text = result
            .content
            .first()
            .and_then(|block| block.as_text())
            .and_then(|block| serde_json::from_str::<Value>(&block.text).ok());
        let count = result
            .structured_content
            .as_ref()
            .and_then(|v| v.get("calls"))
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        for index in 0..count {
            if !result
                .structured_content
                .as_ref()
                .and_then(|v| v.get("calls"))
                .and_then(|v| v.get(index))
                .and_then(|v| v.get("upstream_content"))
                .is_some_and(|value| value.get("content").is_some())
            {
                continue;
            }
            if let Some(content) = result
                .structured_content
                .as_mut()
                .and_then(|v| v.get_mut("calls"))
                .and_then(|v| v.get_mut(index))
                .and_then(|v| v.get_mut("upstream_content"))
            {
                labby_codemode::omit_code_mode_upstream_content(content);
            }
            if let Some(content) = text
                .as_mut()
                .and_then(|v| v.get_mut("calls"))
                .and_then(|v| v.get_mut(index))
                .and_then(|v| v.get_mut("upstream_content"))
            {
                labby_codemode::omit_code_mode_upstream_content(content);
            }
            if let (Some(value), Some(ContentBlock::Text(block))) =
                (&text, result.content.first_mut())
            {
                block.text = value.to_string();
            }
            if fits(result) {
                break;
            }
        }
    }
}
