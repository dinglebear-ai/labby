//! Execution-local projection of independent upstream MCP content.
use serde_json::{Value, json};

const MAX_AUXILIARY_BYTES: usize = 16 * 1024;

/// Preserve content independently of script projection, without duplicating a
/// JSON text rendering of structuredContent. The full-result API is lossless.
pub(crate) fn capture(full: &Value, used: &mut usize) -> Option<Value> {
    let content = full.get("content")?.as_array()?;
    let structured = full.get("structuredContent");
    let mut blocks = Vec::new();
    let mut omitted_blocks = 0;
    let mut omitted_bytes = 0;
    for block in content {
        if block.get("type").and_then(Value::as_str) == Some("text")
            && let Some(text) = block.get("text").and_then(Value::as_str)
            && let Ok(parsed) = serde_json::from_str::<Value>(text)
            && structured == Some(&parsed)
            && block.as_object().is_some_and(|object| {
                object
                    .keys()
                    .all(|key| matches!(key.as_str(), "type" | "text"))
            })
        {
            continue;
        }
        let bytes = serde_json::to_vec(block).map_or(usize::MAX, |v| v.len());
        // A large first block does not consume space needed by later small notices.
        if bytes <= MAX_AUXILIARY_BYTES.saturating_sub(*used) {
            *used += bytes;
            blocks.push(block.clone());
        } else {
            omitted_blocks += 1;
            omitted_bytes = usize::saturating_add(omitted_bytes, bytes);
        }
    }
    if blocks.is_empty() && omitted_blocks == 0 {
        return None;
    }
    Some(
        json!({"content": blocks, "omitted_blocks": omitted_blocks, "omitted_bytes": omitted_bytes}),
    )
}

/// Replace retained blocks with explicit omission metadata under final pressure.
pub fn omit_code_mode_upstream_content(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let Some(Value::Array(blocks)) = object.remove("content") else {
        return;
    };
    let count = object
        .get("omitted_blocks")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let bytes = object
        .get("omitted_bytes")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let extra = blocks
        .iter()
        .map(|block| serde_json::to_vec(block).map_or(0, |v| v.len() as u64))
        .sum::<u64>();
    object.insert("omitted_blocks".into(), json!(count + blocks.len() as u64));
    object.insert("omitted_bytes".into(), json!(bytes.saturating_add(extra)));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_proven_structured_duplicates_are_suppressed() {
        let mut used = 0;
        let full = json!({"content":[{"type":"text","text":"{\"rows\":[1]}"},{"type":"text","text":"{\"advisory\":true}"}]});
        let kept = capture(&full, &mut used).unwrap();
        assert_eq!(kept["content"].as_array().unwrap().len(), 2);
        let mut structured = full;
        structured["structuredContent"] = json!({"rows":[1]});
        assert_eq!(
            capture(&structured, &mut 0).unwrap()["content"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn oversized_first_block_does_not_erase_later_notice_and_omission_is_explicit() {
        let full = json!({"structuredContent":{},"content":[{"type":"image","data":"x".repeat(MAX_AUXILIARY_BYTES),"mimeType":"image/png"},{"type":"text","text":"small notice"}]});
        let mut used = 0;
        let mut kept = capture(&full, &mut used).unwrap();
        assert_eq!(kept["omitted_blocks"], 1);
        assert!(kept["omitted_bytes"].as_u64().unwrap() > MAX_AUXILIARY_BYTES as u64);
        assert_eq!(kept["content"][0]["text"], "small notice");
        omit_code_mode_upstream_content(&mut kept);
        assert!(kept.get("content").is_none());
        assert_eq!(kept["omitted_blocks"], 2);
        assert!(used <= MAX_AUXILIARY_BYTES);
    }
    #[test]
    fn aggregate_capture_is_bounded_and_execution_local() {
        let full = json!({"content":[{"type":"text","text":"x".repeat(9000)}]});
        let mut used = 0;
        assert_eq!(capture(&full, &mut used).unwrap()["omitted_blocks"], 0);
        assert_eq!(capture(&full, &mut used).unwrap()["omitted_blocks"], 1);
        assert_eq!(capture(&full, &mut 0).unwrap()["omitted_blocks"], 0);
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::*;
    #[test]
    fn duplicate_json_with_annotations_or_metadata_preserves_the_complete_block() {
        for extra in [
            json!({"annotations":{"audience":["assistant"],"priority":1}}),
            json!({"_meta":{"advisory":"Use cached result"}}),
        ] {
            let mut block = json!({"type":"text","text":"{\"rows\":[1]}"});
            block
                .as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let full = json!({"structuredContent":{"rows":[1]},"content":[block]});
            let projected = capture(&full, &mut 0).unwrap();
            assert_eq!(projected["content"][0], block);
            assert_eq!(projected["omitted_blocks"], 0);
        }
    }
}
