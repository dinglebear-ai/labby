//! Bounded, surface-neutral lexical discovery over an already-rendered catalog.

use serde::Serialize;

use crate::error::ToolError;
use crate::{
    CodeModeCatalogKind, CodeModeToolSafety, ToolDescriptor, ToolScope, discovery_entry_visible,
};

pub const QUERY_MAX_BYTES: usize = 1_024;
pub const TARGET_MAX_BYTES: usize = 4_096;
pub const DESCRIPTION_MAX_BYTES: usize = 4 * 1_024;
pub const SIGNATURE_MAX_BYTES: usize = 8 * 1_024;
pub const TAGS_MAX: usize = 32;
pub const TAG_MAX_BYTES: usize = 256;
pub const DTS_MAX_BYTES: usize = 64 * 1_024;
pub const SEARCH_RESPONSE_MAX_BYTES: usize = 256 * 1_024;
pub const DESCRIBE_RESPONSE_MAX_BYTES: usize = 128 * 1_024;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeSearchHit {
    pub path: String,
    pub id: String,
    pub kind: CodeModeCatalogKind,
    pub namespace: String,
    pub name: String,
    pub description: String,
    pub signature: String,
    pub tags: Vec<String>,
    pub score: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety: Option<CodeModeToolSafety>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeSearchResponse {
    pub results: Vec<CodeModeSearchHit>,
    pub total: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeModeDescribeResponse {
    pub path: String,
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub description: String,
    pub helper: String,
    pub signature: String,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub safety: Option<CodeModeToolSafety>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typescript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typescript_omitted: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    index: usize,
    score: u32,
}

#[derive(Serialize)]
struct SearchResponseRef<'a> {
    results: &'a [CodeModeSearchHit],
    total: usize,
    truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<&'static str>,
}

pub fn search_visible_tools(
    entries: &[ToolDescriptor],
    scope: &ToolScope,
    query: &str,
    limit: usize,
) -> Result<CodeModeSearchResponse, ToolError> {
    validate_bytes("query", query, QUERY_MAX_BYTES)?;
    let tokens = tokens(query);
    let limit = limit.clamp(1, 50);
    // The response contract caps this collection at 50 entries. Reserve the
    // fixed contract maximum instead of propagating the caller-derived value
    // into an allocation, even after clamping.
    let mut candidates = Vec::with_capacity(50);
    let mut total = 0_usize;
    for (index, entry) in entries.iter().enumerate() {
        if entry.kind != CodeModeCatalogKind::Tool || !discovery_entry_visible(entry, scope) {
            continue;
        }
        let path = tool_path(entry);
        let fields = [
            (normalize(&path), 12_u32),
            (normalize(&entry.name), 10),
            (normalize(&entry.namespace), 8),
            (normalize(&entry.description), 5),
            (normalize(&entry.tags.join(" ")), 7),
        ];
        let mut covered = 0_usize;
        let mut score = 0_u32;
        for token in &tokens {
            let token_score = fields
                .iter()
                .filter(|(field, _)| field.contains(token))
                .map(|(_, weight)| *weight)
                .max()
                .unwrap_or(0);
            if token_score > 0 {
                covered += 1;
                score += token_score;
            }
        }
        let required = if tokens.len() <= 2 {
            tokens.len()
        } else {
            (tokens.len() * 3).div_ceil(5)
        };
        if covered >= required {
            total += 1;
            candidates.push(Candidate { index, score });
            candidates.sort_unstable_by(|a, b| compare_candidate(*a, *b, entries));
            candidates.truncate(limit);
        }
    }
    let mut results = candidates
        .into_iter()
        .map(|candidate| hit(&entries[candidate.index], candidate.score))
        .collect::<Vec<_>>();
    while serialized_len(&SearchResponseRef {
        results: &results,
        total,
        truncated: total > results.len(),
        hint: None,
    })? > SEARCH_RESPONSE_MAX_BYTES
    {
        if results.pop().is_none() {
            break;
        }
    }
    Ok(CodeModeSearchResponse {
        truncated: total > results.len(),
        results,
        total,
        hint: None,
    })
}

pub fn describe_visible_tool(
    entries: &[ToolDescriptor],
    scope: &ToolScope,
    target: &str,
) -> Result<CodeModeDescribeResponse, ToolError> {
    validate_bytes("target", target, TARGET_MAX_BYTES)?;
    let target = target.trim();
    if target.is_empty() {
        return Err(invalid("target", "target must not be blank"));
    }
    let visible = entries
        .iter()
        .filter(|entry| {
            entry.kind == CodeModeCatalogKind::Tool && discovery_entry_visible(entry, scope)
        })
        .collect::<Vec<_>>();
    let mut exact = visible
        .iter()
        .copied()
        .filter(|entry| target == entry.id || target == tool_path(entry) || target == helper(entry))
        .collect::<Vec<_>>();
    if exact.is_empty() {
        let bare = visible
            .iter()
            .copied()
            .filter(|entry| target == entry.name)
            .collect::<Vec<_>>();
        if bare.len() == 1 {
            exact = bare;
        } else if bare.len() > 1 {
            let mut valid = bare.into_iter().map(tool_path).collect::<Vec<_>>();
            valid.sort();
            return Err(ToolError::AmbiguousTool {
                message: "tool target is ambiguous".into(),
                valid,
            });
        }
    }
    let Some(entry) = exact.first().copied() else {
        return Err(unknown());
    };
    if exact.len() > 1 {
        return Err(unknown());
    }
    let (typescript, typescript_omitted) = if entry.dts.len() <= DTS_MAX_BYTES {
        (Some(entry.dts.clone()), None)
    } else {
        (None, Some("size_limit"))
    };
    let response = CodeModeDescribeResponse {
        path: tool_path(entry),
        id: entry.id.clone(),
        namespace: entry.namespace.clone(),
        name: entry.name.clone(),
        description: truncate(&entry.description, DESCRIPTION_MAX_BYTES),
        helper: helper(entry),
        signature: truncate(&entry.signature, SIGNATURE_MAX_BYTES),
        tags: bounded_tags(&entry.tags),
        safety: entry.safety,
        typescript,
        typescript_omitted,
    };
    if serialized_len(&response)? > DESCRIBE_RESPONSE_MAX_BYTES {
        return Err(ToolError::Sdk {
            sdk_kind: "response_too_large".into(),
            message: "tool description exceeds response budget".into(),
        });
    }
    Ok(response)
}

fn hit(entry: &ToolDescriptor, score: u32) -> CodeModeSearchHit {
    CodeModeSearchHit {
        path: tool_path(entry),
        id: entry.id.clone(),
        kind: entry.kind,
        namespace: entry.namespace.clone(),
        name: entry.name.clone(),
        description: truncate(&entry.description, DESCRIPTION_MAX_BYTES),
        signature: truncate(&entry.signature, SIGNATURE_MAX_BYTES),
        tags: bounded_tags(&entry.tags),
        score,
        safety: entry.safety,
    }
}
fn compare_candidate(a: Candidate, b: Candidate, entries: &[ToolDescriptor]) -> std::cmp::Ordering {
    b.score
        .cmp(&a.score)
        .then_with(|| tool_path(&entries[a.index]).cmp(&tool_path(&entries[b.index])))
}
fn tool_path(entry: &ToolDescriptor) -> String {
    format!(
        "{}.{}",
        crate::preamble::namespace_segment(&entry.namespace),
        crate::preamble::tool_name_to_snake(&entry.name)
    )
}
fn helper(entry: &ToolDescriptor) -> String {
    format!("codemode.{}", tool_path(entry))
}
fn normalize(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn tokens(value: &str) -> Vec<String> {
    normalize(value)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}
fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_owned();
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
fn bounded_tags(tags: &[String]) -> Vec<String> {
    tags.iter()
        .take(TAGS_MAX)
        .map(|tag| truncate(tag, TAG_MAX_BYTES))
        .collect()
}
fn validate_bytes(param: &str, value: &str, max: usize) -> Result<(), ToolError> {
    if value.len() > max {
        Err(invalid(param, format!("{param} exceeds {max} UTF-8 bytes")))
    } else {
        Ok(())
    }
}
fn invalid(param: &str, message: impl Into<String>) -> ToolError {
    ToolError::InvalidParam {
        message: message.into(),
        param: param.into(),
    }
}
fn unknown() -> ToolError {
    ToolError::Sdk {
        sdk_kind: "unknown_tool".into(),
        message: "No visible Code Mode tool matched the requested target".into(),
    }
}
fn serialized_len(value: &impl Serialize) -> Result<usize, ToolError> {
    serde_json::to_vec(value)
        .map(|v| v.len())
        .map_err(|e| ToolError::Sdk {
            sdk_kind: "internal_error".into(),
            message: e.to_string(),
        })
}
