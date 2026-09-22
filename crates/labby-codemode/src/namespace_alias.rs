//! Spelling-tolerant resolution of Code Mode namespaces.
//!
//! Discovery renders upstream namespaces as JS identifiers, so the configured
//! upstream `claude-macpoo` appears as `claude_macpoo` in `codemode.search()`
//! paths and helpers. Agents echo either spelling back through `upstreams`,
//! `tools`, `callTool`, and `describe`. Every surface resolves aliases and
//! explains unknown names through this module so the rules cannot drift.

use std::collections::BTreeSet;

/// Most names listed in an unknown-namespace message.
const MAX_LISTED_NAMES: usize = 25;

/// Spelling-insensitive identity: ASCII case and `-`/`_`/`.`/space
/// separators do not distinguish namespaces.
#[must_use]
pub fn namespace_alias_key(name: &str) -> String {
    name.trim()
        .chars()
        .map(|ch| match ch {
            '-' | '.' | ' ' => '_',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

/// Outcome of resolving a requested namespace against known names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamespaceResolution<'a> {
    /// The requested name, or its unique alias.
    Resolved(&'a str),
    /// No known name matches, even as an alias.
    Unknown,
    /// Several known names share the requested alias key.
    Ambiguous(Vec<&'a str>),
}

/// Resolve `requested` against `known`: exact names win, then a unique alias.
#[must_use]
pub fn resolve_namespace_alias<'a>(
    requested: &str,
    known: impl IntoIterator<Item = &'a str>,
) -> NamespaceResolution<'a> {
    let requested = requested.trim();
    let key = namespace_alias_key(requested);
    let mut aliases = Vec::new();
    for candidate in known {
        if candidate == requested {
            return NamespaceResolution::Resolved(candidate);
        }
        if namespace_alias_key(candidate) == key {
            aliases.push(candidate);
        }
    }
    match aliases.as_slice() {
        [] => NamespaceResolution::Unknown,
        [only] => NamespaceResolution::Resolved(only),
        _ => {
            aliases.sort_unstable();
            aliases.dedup();
            NamespaceResolution::Ambiguous(aliases)
        }
    }
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (i, lc) in left.chars().enumerate() {
        let mut current = vec![i + 1; right.len() + 1];
        for (j, rc) in right.iter().enumerate() {
            let substitution = previous[j] + usize::from(lc != *rc);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        previous = current;
    }
    previous[right.len()]
}

/// Up to three known names that look like likely typos of `requested`.
#[must_use]
pub fn similar_namespaces<'a>(
    requested: &str,
    known: impl IntoIterator<Item = &'a str>,
) -> Vec<&'a str> {
    let key = namespace_alias_key(requested);
    let threshold = (key.chars().count() / 4).max(2);
    let mut scored = known
        .into_iter()
        .filter_map(|candidate| {
            let candidate_key = namespace_alias_key(candidate);
            let distance = edit_distance(&key, &candidate_key);
            let contains =
                key.len() >= 3 && (candidate_key.contains(&key) || key.contains(&candidate_key));
            (distance <= threshold || contains).then_some((distance, candidate))
        })
        .collect::<Vec<_>>();
    scored.sort_unstable();
    scored.dedup();
    scored.into_iter().take(3).map(|(_, name)| name).collect()
}

/// Render names as a comma-separated list of backticked identifiers.
#[must_use]
pub fn backtick_list<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    names
        .into_iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Guidance for a namespace that matched nothing: a "did you mean" when a
/// near-match exists, otherwise the (bounded) list of valid names. Callers
/// must pass only names visible to the caller.
#[must_use]
pub fn unknown_namespace_guidance(requested: &str, visible: &BTreeSet<String>) -> String {
    let similar = similar_namespaces(requested, visible.iter().map(String::as_str));
    if !similar.is_empty() {
        return format!("Did you mean {}?", backtick_list(similar));
    }
    if visible.is_empty() {
        return "No upstreams are visible to this caller.".to_string();
    }
    let listed = backtick_list(visible.iter().take(MAX_LISTED_NAMES).map(String::as_str));
    if visible.len() > MAX_LISTED_NAMES {
        format!(
            "Known upstreams: {listed} (and {} more).",
            visible.len() - MAX_LISTED_NAMES
        )
    } else {
        format!("Known upstreams: {listed}.")
    }
}

/// Message for a namespace alias that matches several configured names.
#[must_use]
pub fn ambiguous_namespace_message(requested: &str, matches: &[&str]) -> String {
    format!(
        "Code Mode upstream `{requested}` is ambiguous: it matches {} ignoring case and `-`/`_` separators. Use the exact configured name.",
        backtick_list(matches.iter().copied())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    #[test]
    fn alias_key_ignores_case_and_separators() {
        assert_eq!(namespace_alias_key("Claude-MacPoo"), "claude_macpoo");
        assert_eq!(namespace_alias_key(" claude.macpoo "), "claude_macpoo");
    }

    #[test]
    fn exact_name_wins_over_alias_siblings() {
        let known = ["a-b", "a_b"];
        assert_eq!(
            resolve_namespace_alias("a_b", known),
            NamespaceResolution::Resolved("a_b")
        );
        assert_eq!(
            resolve_namespace_alias("A-B", known),
            NamespaceResolution::Ambiguous(vec!["a-b", "a_b"])
        );
    }

    #[test]
    fn unique_alias_resolves_and_unknown_reports_unknown() {
        let known = ["claude-macpoo", "github"];
        assert_eq!(
            resolve_namespace_alias("claude_macpoo", known),
            NamespaceResolution::Resolved("claude-macpoo")
        );
        assert_eq!(
            resolve_namespace_alias("nope", known),
            NamespaceResolution::Unknown
        );
    }

    #[test]
    fn guidance_prefers_near_matches_then_lists_names() {
        let visible = set(&["claude-macpoo", "claude-squirts", "github"]);
        assert_eq!(
            unknown_namespace_guidance("claude-macpo", &visible),
            "Did you mean `claude-macpoo`?"
        );
        assert_eq!(
            unknown_namespace_guidance("zzz", &set(&["alpha", "beta"])),
            "Known upstreams: `alpha`, `beta`."
        );
        assert_eq!(
            unknown_namespace_guidance("zzz", &BTreeSet::new()),
            "No upstreams are visible to this caller."
        );
    }
}
