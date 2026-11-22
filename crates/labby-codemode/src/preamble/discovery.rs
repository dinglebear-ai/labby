use super::{WITHHELD_SCOPE_ADVICE, namespace_segment, withheld_guidance};
use crate::host::WithheldTools;
#[cfg(test)]
use crate::types::CodeModeDiscoveryEntry;

#[cfg(test)]
pub(crate) fn generate_discovery_js(
    entries: &[CodeModeDiscoveryEntry],
    blend_weight: f32,
    withheld: &[WithheldTools],
) -> Result<String, String> {
    let json = serde_json::to_string(entries)
        .map_err(|err| format!("failed to serialize Code Mode discovery catalog: {err}"))?;
    generate_discovery_js_from_json(&json, blend_weight, withheld)
}

pub(crate) fn generate_discovery_js_from_json(
    json: &str,
    blend_weight: f32,
    withheld: &[WithheldTools],
) -> Result<String, String> {
    let withheld_json = serde_json::to_string(
        &withheld
            .iter()
            .map(|item| {
                serde_json::json!({
                    "namespace": item.namespace(),
                    "key": crate::namespace_alias::namespace_alias_key(item.namespace()),
                    "helper": namespace_segment(item.namespace()),
                    "tool_count": item.tool_count(),
                    "guidance": withheld_guidance(item),
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|err| format!("failed to serialize Code Mode withheld summary: {err}"))?;
    let advice_json = serde_json::to_string(WITHHELD_SCOPE_ADVICE)
        .map_err(|err| format!("failed to serialize Code Mode withheld advice: {err}"))?;
    Ok(format!(
        concat!(include_str!("search.js.fmt"), "{}"),
        concat!(include_str!("describe.js"), include_str!("helpers.js")),
        json = json,
        withheld_json = withheld_json,
        advice_json = advice_json,
        blend_weight = blend_weight,
    ))
}
