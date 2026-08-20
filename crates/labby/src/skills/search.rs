//! Deterministic metadata-only search over already-discovered Agent Skills.
//!
//! Search is deliberately pure: it never reads SKILL.md or supporting files,
//! never connects an upstream, and never mutates the registry. Callers pass a
//! canonical visible snapshot and receive ranked clones of matching entries.

use labby_runtime::skills::parse_skill_uri;
use labby_runtime::skills::wire::SkillEntry;
use serde::Serialize;
use serde_json::Value;

const SCORE_NAME_EXACT: u16 = 500;
const SCORE_NAME_PREFIX: u16 = 400;
const SCORE_NAME_CONTAINS: u16 = 300;
const SCORE_DESCRIPTION: u16 = 200;
const SCORE_METADATA: u16 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SkillSearchHit {
    pub(crate) score: u16,
    pub(crate) match_fields: Vec<String>,
    pub(crate) skill: SkillEntry,
}

struct RankedSkill {
    hit: SkillSearchHit,
    origin: String,
    name: String,
}

/// Search a canonical visible skill snapshot without loading skill bodies.
///
/// Ranking follows the compatibility contract: exact name, name prefix, name
/// substring, description, then metadata. A hit may report multiple matching
/// fields, but its score is the strongest matching class rather than a sum so
/// a metadata-rich entry cannot outrank an exact name match.
pub(crate) fn search_skill_entries(
    entries: &[SkillEntry],
    query: &str,
    limit: usize,
) -> Vec<SkillSearchHit> {
    let query = query.trim().to_lowercase();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut ranked = entries
        .iter()
        .filter_map(|entry| rank_entry(entry, &query))
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| {
        right
            .hit
            .score
            .cmp(&left.hit.score)
            .then_with(|| left.origin.cmp(&right.origin))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.hit.skill.uri.cmp(&right.hit.skill.uri))
    });
    ranked.truncate(limit);
    ranked.into_iter().map(|ranked| ranked.hit).collect()
}

fn rank_entry(entry: &SkillEntry, query: &str) -> Option<RankedSkill> {
    let name = frontmatter_string(entry, "name");
    let description = frontmatter_string(entry, "description");
    let normalized_name = name.to_lowercase();
    let normalized_description = description.to_lowercase();

    let mut score = 0;
    let mut match_fields = Vec::new();
    if normalized_name == query {
        score = SCORE_NAME_EXACT;
        match_fields.push("name".to_string());
    } else if normalized_name.starts_with(query) {
        score = SCORE_NAME_PREFIX;
        match_fields.push("name".to_string());
    } else if normalized_name.contains(query) {
        score = SCORE_NAME_CONTAINS;
        match_fields.push("name".to_string());
    }

    if normalized_description.contains(query) {
        score = score.max(SCORE_DESCRIPTION);
        match_fields.push("description".to_string());
    }
    if metadata_matches(entry, query) {
        score = score.max(SCORE_METADATA);
        match_fields.push("metadata".to_string());
    }
    if score == 0 {
        return None;
    }

    let origin = parse_skill_uri(&entry.uri)
        .map(|uri| uri.origin().to_string())
        .unwrap_or_default();
    Some(RankedSkill {
        hit: SkillSearchHit {
            score,
            match_fields,
            skill: entry.clone(),
        },
        origin,
        name: normalized_name,
    })
}

fn frontmatter_string<'a>(entry: &'a SkillEntry, key: &str) -> &'a str {
    entry
        .frontmatter
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn metadata_matches(entry: &SkillEntry, query: &str) -> bool {
    entry
        .frontmatter
        .iter()
        .filter(|(key, _)| key.as_str() != "name" && key.as_str() != "description")
        .any(|(_, value)| value_contains_query(value, query))
        || entry.meta.as_ref().is_some_and(|meta| {
            meta.values()
                .any(|value| value_contains_query(value, query))
        })
}

fn value_contains_query(value: &Value, query: &str) -> bool {
    match value {
        Value::String(value) => value.to_lowercase().contains(query),
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_query(value, query)),
        Value::Object(values) => values
            .values()
            .any(|value| value_contains_query(value, query)),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    fn skill(uri: &str, name: &str, description: &str) -> SkillEntry {
        let mut frontmatter = Map::new();
        frontmatter.insert("name".to_string(), Value::String(name.to_string()));
        frontmatter.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
        SkillEntry {
            uri: uri.to_string(),
            frontmatter,
            resources: None,
            meta: None,
        }
    }

    #[test]
    fn ranking_follows_exact_prefix_substring_description_metadata_order() {
        let exact = skill("skill://e/alpha/SKILL.md", "alpha", "other");
        let prefix = skill("skill://p/alphabet/SKILL.md", "alphabet", "other");
        let contains = skill("skill://c/pre-alpha/SKILL.md", "pre-alpha", "other");
        let description = skill("skill://d/desc/SKILL.md", "desc", "alpha workflow");
        let mut metadata = skill("skill://m/meta/SKILL.md", "meta", "other");
        metadata.meta = Some(Map::from_iter([(
            "tags".to_string(),
            json!(["alpha", "routing"]),
        )]));

        let hits = search_skill_entries(
            &[metadata, description, contains, prefix, exact],
            "alpha",
            10,
        );
        let scores = hits.iter().map(|hit| hit.score).collect::<Vec<_>>();
        assert_eq!(scores, vec![500, 400, 300, 200, 100]);
        assert_eq!(
            hits.iter()
                .map(|hit| frontmatter_string(&hit.skill, "name"))
                .collect::<Vec<_>>(),
            vec!["alpha", "alphabet", "pre-alpha", "desc", "meta"]
        );
    }

    #[test]
    fn strongest_match_wins_but_all_matching_fields_are_reported() {
        let entry = skill(
            "skill://labby/alpha/SKILL.md",
            "alpha",
            "Alpha-focused workflow",
        );
        let hits = search_skill_entries(&[entry], "ALPHA", 10);
        assert_eq!(hits[0].score, SCORE_NAME_EXACT);
        assert_eq!(hits[0].match_fields, vec!["name", "description"]);
    }

    #[test]
    fn equal_scores_are_deterministic_by_origin_name_then_uri() {
        let b = skill("skill://b/same/SKILL.md", "same", "other");
        let a = skill("skill://a/same/SKILL.md", "same", "other");
        let hits = search_skill_entries(&[b, a], "same", 10);
        assert_eq!(hits[0].skill.uri, "skill://a/same/SKILL.md");
        assert_eq!(hits[1].skill.uri, "skill://b/same/SKILL.md");
    }

    #[test]
    fn empty_queries_and_zero_limits_return_no_hits() {
        let entry = skill("skill://labby/alpha/SKILL.md", "alpha", "other");
        assert!(search_skill_entries(&[entry.clone()], "   ", 10).is_empty());
        assert!(search_skill_entries(&[entry], "alpha", 0).is_empty());
    }

    #[test]
    fn limit_is_applied_after_global_ranking() {
        let prefix = skill("skill://p/alphabet/SKILL.md", "alphabet", "other");
        let exact = skill("skill://e/alpha/SKILL.md", "alpha", "other");
        let hits = search_skill_entries(&[prefix, exact], "alpha", 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].skill.uri, "skill://e/alpha/SKILL.md");
    }

    #[test]
    fn first_party_snapshot_is_searchable_without_loading_bodies() {
        let listing = crate::skills::list_first_party_skills();
        let hits = search_skill_entries(&listing.skills, "using-labby", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].skill.uri, "skill://labby/using-labby/SKILL.md");
    }
}
