//! Aggregating first-party and proxied skills into one downstream surface.
//!
//! # Per-origin namespacing (threat model T8)
//!
//! SEP-2640: *"When skills from different origins collide on `name`, hosts MUST
//! resolve the name within a per-origin namespace, identifying servers by a
//! host-assigned label; an MCP-served skill MUST NOT silently shadow, or be
//! silently substituted for, a same-named skill from any other origin."*
//!
//! Labby's label is the upstream's configured name, and it occupies the first
//! URI segment. Two upstreams both serving `refunds` therefore produce two
//! distinct entries under two distinct URIs, and **nothing is ever deduplicated
//! by name** — the spec also warns that names are labels, not identifiers, and
//! that one server may legitimately serve two skills sharing a final segment.
//!
//! # Minting a proxied URI keeps digests valid
//!
//! Only the origin label changes; the remainder of the path is byte-identical,
//! and file contents are untouched. A digest the upstream computed therefore
//! still describes exactly the bytes Labby relays.

use std::collections::BTreeSet;

use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::wire::{SkillEntry, SkillResource};
use labby_runtime::skills::{ValidatedSkill, parse_skill_resource_uri};

/// `_meta` key carrying Labby's provenance and tool-reachability facts.
pub(crate) const SKILL_ORIGIN_META_KEY: &str = "ai.dinglebear.labby/skillOrigin";

/// How an origin's tools can actually be reached from downstream.
///
/// This is the T3 mitigation. A skill's `allowed-tools` frontmatter names tools
/// in *its origin's* namespace, but downstream of a gateway the catalog is
/// aggregated, so those names may resolve against a different server's tools or
/// against Labby's own privileged ones. Telling a client which downstream tool
/// names this origin actually accounts for lets it scope the field instead of
/// resolving it against the flattened catalog.
///
/// Every value here is a fact about *Labby's own catalog*, never an
/// interpretation of skill content — the skill is still data, not directives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolAccess {
    /// The origin's tools appear in `tools/list` under these names.
    Direct,
    /// Code Mode is enabled, so raw upstream tools are hidden from `tools/list`
    /// entirely and reachable only through the Code Mode entry points. There is
    /// no downstream tool name to scope `allowed-tools` against.
    CodeModeOnly,
}

impl ToolAccess {
    fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::CodeModeOnly => "code_mode_only",
        }
    }
}

/// Build the provenance `_meta` for one origin.
pub(crate) fn origin_meta(
    origin: &str,
    access: ToolAccess,
    reachable_tools: &[String],
) -> serde_json::Map<String, serde_json::Value> {
    let mut origin_block = serde_json::Map::new();
    origin_block.insert("label".into(), serde_json::json!(origin));
    origin_block.insert("toolAccess".into(), serde_json::json!(access.as_str()));
    match access {
        // Only meaningful when the names exist downstream at all.
        ToolAccess::Direct => {
            origin_block.insert("reachableTools".into(), serde_json::json!(reachable_tools));
        }
        ToolAccess::CodeModeOnly => {
            origin_block.insert(
                "note".into(),
                serde_json::json!(
                    "Code Mode is enabled: this origin's tools are not present in tools/list, so \
                     `allowed-tools` names cannot be resolved against the downstream catalog."
                ),
            );
        }
    }

    let mut meta = serde_json::Map::new();
    meta.insert(
        SKILL_ORIGIN_META_KEY.to_string(),
        serde_json::Value::Object(origin_block),
    );
    meta
}

/// Re-render one upstream skill entry under Labby's origin label for it.
///
/// Returns `None` when a URI in the entry cannot be re-parsed or relabelled,
/// which excludes the skill rather than emitting a half-rewritten manifest a
/// client could never verify against.
pub(crate) fn mint_proxied_entry(
    origin: &str,
    skill: &ValidatedSkill,
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<SkillEntry> {
    let uri = parse_skill_resource_uri(&skill.entry.uri).ok()?;
    let minted_uri = uri.with_origin(origin).ok()?.to_uri();

    let resources = match skill.entry.resources.as_ref() {
        None => None,
        Some(resources) => {
            let mut minted = Vec::with_capacity(resources.len());
            for resource in resources {
                let parsed = parse_skill_resource_uri(&resource.uri).ok()?;
                minted.push(SkillResource {
                    uri: parsed.with_origin(origin).ok()?.to_uri(),
                    // Untouched: the label moved, the bytes did not.
                    digest: resource.digest.clone(),
                    size: resource.size,
                });
            }
            Some(minted)
        }
    };

    Some(SkillEntry {
        uri: minted_uri,
        frontmatter: skill.entry.frontmatter.clone(),
        resources,
        meta: meta.cloned(),
    })
}

/// Every skill an upstream exposes, relabelled under its origin.
pub(crate) fn mint_proxied_entries(
    config: &UpstreamConfig,
    skills: &[ValidatedSkill],
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> MintedEntries {
    // A skill is identified by its manifest URI within its server origin.
    // Supporting-resource overlap is valid: a nested skill's files are also
    // supporting files of its parent and therefore appear in both manifests.
    let mut minted: Vec<SkillEntry> = Vec::with_capacity(skills.len());
    let mut manifest_owners = BTreeSet::new();
    let mut excluded_uris = BTreeSet::new();
    for skill in skills {
        let Some(entry) = mint_proxied_entry(&config.name, skill, meta) else {
            continue;
        };
        if !manifest_owners.insert(entry.uri.clone()) {
            excluded_uris.insert(entry.uri.clone());
            tracing::warn!(
                upstream = %config.name,
                skill = %entry.uri,
                "excluding duplicate skill identity from one upstream listing"
            );
            continue;
        }
        minted.push(entry);
    }
    let excluded_count = skills.len().saturating_sub(minted.len());
    MintedEntries {
        entries: minted,
        excluded_count,
        excluded_uris,
    }
}

/// Proxied entries plus the number that could not be published honestly.
pub(crate) struct MintedEntries {
    pub(crate) entries: Vec<SkillEntry>,
    pub(crate) excluded_count: usize,
    pub(crate) excluded_uris: BTreeSet<String>,
}

impl MintedEntries {
    pub(crate) fn excludes_uri(&self, uri: &str) -> bool {
        self.excluded_uris.contains(uri)
    }

    /// Whether an unlisted candidate would make URI ownership ambiguous with
    /// the current published set or with a skill already excluded for a
    /// collision.
    pub(crate) fn conflicts_with(&self, candidate: &SkillEntry) -> bool {
        self.excluded_uris.contains(&candidate.uri)
            || self.entries.iter().any(|entry| entry.uri == candidate.uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::skills::validate_skill_entry;
    use serde_json::json;

    pub(super) fn upstream_skill_for_meta() -> ValidatedSkill {
        upstream_skill("their-label", "refunds")
    }

    fn minimal_config() -> UpstreamConfig {
        UpstreamConfig {
            enabled: true,
            name: "upstream".to_string(),
            url: None,
            transport: None,
            socket_path: None,
            headers: Default::default(),
            bearer_token_env: None,
            command: None,
            args: vec![],
            env: Default::default(),
            proxy_resources: false,
            proxy_prompts: false,
            expose_tools: None,
            expose_resources: None,
            expose_prompts: None,
            proxy_skills: true,
            expose_skills: None,
            code_mode_hint: None,
            oauth: None,
            imported_from: None,
            priority: 1.0,
        }
    }

    fn upstream_skill_with_scheme(scheme: &str, origin: &str, name: &str) -> ValidatedSkill {
        let uri = format!("{scheme}://{origin}/{name}/SKILL.md");
        let entry: SkillEntry = serde_json::from_value(json!({
            "uri": uri,
            "frontmatter": { "name": name, "description": "d" },
            "resources": [
                { "uri": uri, "digest": labby_runtime::skills::ResourceDigest::of_bytes(b"a").to_wire(), "size": 1 },
            ]
        }))
        .expect("entry");
        validate_skill_entry(&entry).expect("valid")
    }

    fn upstream_skill(origin: &str, name: &str) -> ValidatedSkill {
        let uri = format!("skill://{origin}/{name}/SKILL.md");
        let extra = format!("skill://{origin}/{name}/notes.md");
        let entry: SkillEntry = serde_json::from_value(json!({
            "uri": uri,
            "frontmatter": { "name": name, "description": "d" },
            "resources": [
                { "uri": uri, "digest": labby_runtime::skills::ResourceDigest::of_bytes(b"a").to_wire(), "size": 1 },
                { "uri": extra, "digest": labby_runtime::skills::ResourceDigest::of_bytes(b"b").to_wire(), "size": 1 },
            ]
        }))
        .expect("entry");
        validate_skill_entry(&entry).expect("valid")
    }

    #[test]
    fn minting_relabels_every_uri_and_leaves_digests_alone() {
        let skill = upstream_skill("their-label", "refunds");
        let original_digests: Vec<String> = skill
            .entry
            .resources
            .as_ref()
            .expect("manifest")
            .iter()
            .map(|r| r.digest.clone())
            .collect();

        let minted = mint_proxied_entry("gh", &skill, None).expect("mints");
        assert_eq!(minted.uri, "skill://gh/skill/their-label/refunds/SKILL.md");
        let minted_resources = minted.resources.as_ref().expect("manifest");
        assert!(
            minted_resources
                .iter()
                .all(|r| r.uri.starts_with("skill://gh/"))
        );
        // The bytes never moved, so neither did the digests.
        let minted_digests: Vec<String> =
            minted_resources.iter().map(|r| r.digest.clone()).collect();
        assert_eq!(original_digests, minted_digests);
        // The manifest still lists the entry's own SKILL.md after relabelling.
        assert!(minted_resources.iter().any(|r| r.uri == minted.uri));
    }

    #[test]
    fn a_minted_entry_still_passes_ingest_validation() {
        // Relabelling must not produce something Labby would itself reject —
        // otherwise a downstream host applying the same rules would refuse it.
        let skill = upstream_skill("their-label", "refunds");
        let minted = mint_proxied_entry("gh", &skill, None).expect("mints");
        validate_skill_entry(&minted).expect("a relabelled entry is still well-formed");
    }

    #[test]
    fn two_origins_serving_one_name_stay_distinct() {
        // The T8 case: same skill name, two servers. Both survive, under
        // distinct URIs, and neither shadows the other.
        let a = mint_proxied_entry("alpha", &upstream_skill("x", "refunds"), None).expect("a");
        let b = mint_proxied_entry("beta", &upstream_skill("y", "refunds"), None).expect("b");

        assert_ne!(a.uri, b.uri);
        assert_eq!(a.uri, "skill://alpha/skill/x/refunds/SKILL.md");
        assert_eq!(b.uri, "skill://beta/skill/y/refunds/SKILL.md");
        // Names collide by design; the URIs are what identify them.
        assert_eq!(a.frontmatter.get("name"), b.frontmatter.get("name"));
    }

    #[test]
    fn one_upstreams_two_same_named_skills_both_survive() {
        // The spec's own example: acme/billing/refunds and acme/support/refunds
        // are different skills that share a final segment.
        let billing = upstream_skill("acme", "refunds");
        let minted = mint_proxied_entry("acme-corp", &billing, None).expect("mints");
        // The upstream's own `acme` prefix survives: the label is prepended,
        // not substituted, so nothing the upstream organized by is discarded.
        assert_eq!(minted.uri, "skill://acme-corp/skill/acme/refunds/SKILL.md");
        // Nothing here dedupes by name, so a sibling at another path is
        // unaffected — asserting the absence of a name-keyed collapse. The two
        // skills must differ by *path*, as the comment above describes; passing
        // the identical skill twice tested a degenerate case a real listing
        // cannot produce, and now correctly trips the same-URI collision guard.
        let result = mint_proxied_entries(
            &UpstreamConfig {
                name: "acme-corp".to_string(),
                ..minimal_config()
            },
            &[
                upstream_skill("acme/billing", "refunds"),
                upstream_skill("acme/support", "refunds"),
            ],
            None,
        );
        assert_eq!(result.entries.len(), 2, "no name-keyed deduplication");
        assert_ne!(
            result.entries[0].uri, result.entries[1].uri,
            "distinct paths, distinct URIs"
        );
        assert_eq!(result.excluded_count, 0);
    }

    #[test]
    fn two_schemes_mint_to_distinct_reversible_uris() {
        let result = mint_proxied_entries(
            &UpstreamConfig {
                name: "gh".to_string(),
                ..minimal_config()
            },
            &[
                upstream_skill_with_scheme("skill", "acme", "refunds"),
                upstream_skill_with_scheme("github", "acme", "refunds"),
            ],
            None,
        );
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.excluded_count, 0);
        assert_eq!(
            result.entries[0].uri,
            "skill://gh/skill/acme/refunds/SKILL.md"
        );
        assert_eq!(
            result.entries[1].uri,
            "skill://gh/github/acme/refunds/SKILL.md"
        );
    }

    #[test]
    fn nested_skills_may_publish_overlapping_supporting_resources() {
        let mut parent = upstream_skill("acme", "parent");
        let mut child = upstream_skill("acme/parent", "child");
        let shared = "skill://acme/parent/child/shared.md";
        parent
            .entry
            .resources
            .as_mut()
            .expect("manifest")
            .push(SkillResource {
                uri: shared.to_string(),
                digest: labby_runtime::skills::ResourceDigest::of_bytes(b"parent").to_wire(),
                size: b"parent".len() as u64,
            });
        child
            .entry
            .resources
            .as_mut()
            .expect("manifest")
            .push(SkillResource {
                uri: shared.to_string(),
                digest: labby_runtime::skills::ResourceDigest::of_bytes(b"child").to_wire(),
                size: b"child".len() as u64,
            });
        let result = mint_proxied_entries(
            &UpstreamConfig {
                name: "gh".into(),
                ..minimal_config()
            },
            &[parent, child],
            None,
        );
        assert_eq!(result.entries.len(), 2);
        assert_eq!(result.excluded_count, 0);
        assert!(result.excluded_uris.is_empty());
    }

    #[test]
    fn unlisted_candidate_may_reuse_a_supporting_resource_uri() {
        let published = upstream_skill("acme", "published");
        let result = mint_proxied_entries(
            &UpstreamConfig {
                name: "gh".into(),
                ..minimal_config()
            },
            &[published],
            None,
        );
        let published_resource = result.entries[0]
            .resources
            .as_ref()
            .and_then(|resources| resources.get(1))
            .expect("published notes resource")
            .uri
            .clone();
        let mut candidate = result.entries[0].clone();
        candidate.uri = "skill://gh/skill/acme/unlisted/SKILL.md".to_string();
        candidate.resources = Some(vec![SkillResource {
            uri: published_resource,
            digest: labby_runtime::skills::ResourceDigest::of_bytes(b"candidate").to_wire(),
            size: b"candidate".len() as u64,
        }]);

        assert!(!result.conflicts_with(&candidate));
    }

    #[test]
    fn an_unlabellable_origin_excludes_the_skill_rather_than_half_rewriting_it() {
        // A manifest rewritten only partway would fail every client's
        // verification; dropping the skill is the honest outcome.
        let skill = upstream_skill("their-label", "refunds");
        assert!(mint_proxied_entry("Not A Label", &skill, None).is_none());
    }

    #[test]
    fn a_digest_less_entry_relabels_without_inventing_a_manifest() {
        let mut skill = upstream_skill("their-label", "refunds");
        skill.entry.resources = None;
        let minted = mint_proxied_entry("gh", &skill, None).expect("mints");
        assert_eq!(minted.uri, "skill://gh/skill/their-label/refunds/SKILL.md");
        assert!(minted.resources.is_none(), "must not fabricate a manifest");
    }
}

#[cfg(test)]
mod origin_meta_tests {
    use super::*;

    #[test]
    fn direct_access_names_the_tools_a_client_can_actually_scope_against() {
        let tools = vec!["create_issue".to_string(), "list_repos".to_string()];
        let meta = origin_meta("gh", ToolAccess::Direct, &tools);
        let block = meta
            .get(SKILL_ORIGIN_META_KEY)
            .and_then(|value| value.as_object())
            .expect("origin block");

        assert_eq!(block.get("label").and_then(|v| v.as_str()), Some("gh"));
        assert_eq!(
            block.get("toolAccess").and_then(|v| v.as_str()),
            Some("direct")
        );
        assert_eq!(
            block
                .get("reachableTools")
                .and_then(|v| v.as_array())
                .map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn code_mode_reports_that_there_is_nothing_to_scope_against() {
        // The honest answer under Code Mode: raw upstream tools are absent from
        // tools/list, so a reachableTools list would be a lie rather than a
        // mitigation.
        let meta = origin_meta("gh", ToolAccess::CodeModeOnly, &[]);
        let block = meta
            .get(SKILL_ORIGIN_META_KEY)
            .and_then(|value| value.as_object())
            .expect("origin block");

        assert_eq!(
            block.get("toolAccess").and_then(|v| v.as_str()),
            Some("code_mode_only")
        );
        assert!(
            block.get("reachableTools").is_none(),
            "must not publish downstream names that do not exist"
        );
        assert!(block.get("note").is_some(), "the reason is stated");
    }

    #[test]
    fn origin_meta_never_touches_frontmatter() {
        // The SEP requires frontmatter to be the author's YAML verbatim, and a
        // host must refuse the skill on any field-by-field discrepancy against
        // the fetched SKILL.md. Provenance therefore has to live outside it.
        let skill = tests::upstream_skill_for_meta();
        let meta = origin_meta("gh", ToolAccess::Direct, &[]);
        let minted = mint_proxied_entry("gh", &skill, Some(&meta)).expect("mints");

        assert_eq!(
            minted.frontmatter, skill.entry.frontmatter,
            "frontmatter must survive minting byte-for-byte"
        );
        assert!(minted.meta.is_some(), "provenance rides in _meta instead");
    }

    #[test]
    fn first_party_entries_carry_no_origin_meta() {
        // Labby's own skills need no cross-origin scoping hint, and an absent
        // `_meta` must serialize away entirely rather than as an empty object.
        let listing = crate::skills::list_first_party_skills();
        for entry in &listing.skills {
            assert!(entry.meta.is_none());
        }
        let encoded = serde_json::to_value(&listing.skills[0]).expect("serializes");
        assert!(encoded.get("_meta").is_none());
    }
}
