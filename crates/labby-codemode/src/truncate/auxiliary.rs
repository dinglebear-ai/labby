//! Linear byte accounting for auxiliary-first execution envelope reduction.
use super::*;

/// Return whether the first fitting omission was reached and the removed values.
/// Retaining originals lets the preservation probe restore every field by move.
pub(super) fn omit_until_fitting(
    response: &mut CodeModeExecutionResponse,
    max_bytes: usize,
    max_tokens: usize,
    divisor: u32,
) -> (bool, Vec<(usize, Value)>) {
    #[cfg(test)]
    ENVELOPE_ENCODES.with(|count| count.set(count.get() + 1));
    let mut bytes = serde_json::to_vec(response).map_or(usize::MAX, |v| v.len());
    let fits =
        |bytes: usize| bytes <= max_bytes && estimated_tokens(bytes, divisor) <= max_tokens.max(1);
    let mut originals = Vec::new();
    if fits(bytes) {
        return (true, originals);
    }
    for (index, call) in response.calls.iter_mut().enumerate() {
        let Some(content) = call.upstream_content.as_mut() else {
            continue;
        };
        let Some(omitted) = crate::upstream_content::omission_projection(content) else {
            continue;
        };
        let old = serde_json::to_vec(content).expect("JSON value").len();
        let new = serde_json::to_vec(&omitted).expect("JSON value").len();
        bytes = bytes - old + new;
        originals.push((index, std::mem::replace(content, omitted)));
        if fits(bytes) {
            return (true, originals);
        }
    }
    (false, originals)
}
