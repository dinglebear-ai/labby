use labby_runtime::skills::parse_skill_uri;
use labby_runtime::skills::wire::{SkillEntry, SkillResource};
use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SkillSummary {
    pub(crate) uri: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) origin: String,
    pub(crate) frontmatter: Map<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) resources: Option<Vec<SkillResource>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) provenance: Option<Map<String, Value>>,
}

impl From<SkillEntry> for SkillSummary {
    fn from(entry: SkillEntry) -> Self {
        let name = entry
            .frontmatter
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let description = entry
            .frontmatter
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let origin = parse_skill_uri(&entry.uri)
            .map(|uri| uri.origin().to_string())
            .unwrap_or_default();
        Self {
            uri: entry.uri,
            name,
            description,
            origin,
            frontmatter: entry.frontmatter,
            resources: entry.resources,
            provenance: entry.meta,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct SkillListResponse {
    pub(crate) skills: Vec<SkillSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) incomplete: Option<Map<String, Value>>,
    pub(crate) total_returned: usize,
    pub(crate) truncated: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct SkillSearchResponse {
    pub(crate) query: String,
    pub(crate) matches: Vec<SkillSearchResponseHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) incomplete: Option<Map<String, Value>>,
    pub(crate) total_returned: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct SkillSearchResponseHit {
    pub(crate) score: u16,
    pub(crate) match_fields: Vec<String>,
    pub(crate) skill: SkillSummary,
}

#[derive(Debug, Serialize)]
pub(crate) struct SkillGetResponse {
    pub(crate) skill: SkillSummary,
}

pub(crate) fn sort_entries(entries: &mut [SkillEntry]) {
    entries.sort_by(|left, right| entry_sort_key(left).cmp(&entry_sort_key(right)));
}

fn entry_sort_key(entry: &SkillEntry) -> (String, String, String) {
    let origin = parse_skill_uri(&entry.uri)
        .map(|uri| uri.origin().to_string())
        .unwrap_or_default();
    let name = entry
        .frontmatter
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    (origin, name, entry.uri.clone())
}
