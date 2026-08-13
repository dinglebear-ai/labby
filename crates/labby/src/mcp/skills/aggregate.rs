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

use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::wire::{SkillEntry, SkillResource};
use labby_runtime::skills::{ValidatedSkill, parse_skill_uri};

/// Re-render one upstream skill entry under Labby's origin label for it.
///
/// Returns `None` when a URI in the entry cannot be re-parsed or relabelled,
/// which excludes the skill rather than emitting a half-rewritten manifest a
/// client could never verify against.
pub(crate) fn mint_proxied_entry(origin: &str, skill: &ValidatedSkill) -> Option<SkillEntry> {
    let uri = parse_skill_uri(&skill.entry.uri).ok()?;
    let minted_uri = uri.with_origin(origin).ok()?.to_uri();

    let resources = match skill.entry.resources.as_ref() {
        None => None,
        Some(resources) => {
            let mut minted = Vec::with_capacity(resources.len());
            for resource in resources {
                let parsed = parse_skill_uri(&resource.uri).ok()?;
                minted.push(SkillResource {
                    uri: parsed.with_origin(origin).ok()?.to_uri(),
                    // Untouched: the label moved, the bytes did not.
                    digest: resource.digest.clone(),
                });
            }
            Some(minted)
        }
    };

    Some(SkillEntry {
        uri: minted_uri,
        frontmatter: skill.entry.frontmatter.clone(),
        resources,
    })
}

/// Every skill an upstream exposes, relabelled under its origin.
pub(crate) fn mint_proxied_entries(
    config: &UpstreamConfig,
    skills: &[ValidatedSkill],
) -> Vec<SkillEntry> {
    skills
        .iter()
        .filter_map(|skill| mint_proxied_entry(&config.name, skill))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::skills::validate_skill_entry;
    use serde_json::json;

    fn upstream_skill(origin: &str, name: &str) -> ValidatedSkill {
        let uri = format!("skill://{origin}/{name}/SKILL.md");
        let extra = format!("skill://{origin}/{name}/notes.md");
        let entry: SkillEntry = serde_json::from_value(json!({
            "uri": uri,
            "frontmatter": { "name": name, "description": "d" },
            "resources": [
                { "uri": uri, "digest": labby_runtime::skills::ResourceDigest::of_bytes(b"a").to_wire() },
                { "uri": extra, "digest": labby_runtime::skills::ResourceDigest::of_bytes(b"b").to_wire() },
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

        let minted = mint_proxied_entry("gh", &skill).expect("mints");
        assert_eq!(minted.uri, "skill://gh/refunds/SKILL.md");
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
        let minted = mint_proxied_entry("gh", &skill).expect("mints");
        validate_skill_entry(&minted).expect("a relabelled entry is still well-formed");
    }

    #[test]
    fn two_origins_serving_one_name_stay_distinct() {
        // The T8 case: same skill name, two servers. Both survive, under
        // distinct URIs, and neither shadows the other.
        let a = mint_proxied_entry("alpha", &upstream_skill("x", "refunds")).expect("a");
        let b = mint_proxied_entry("beta", &upstream_skill("y", "refunds")).expect("b");

        assert_ne!(a.uri, b.uri);
        assert_eq!(a.uri, "skill://alpha/refunds/SKILL.md");
        assert_eq!(b.uri, "skill://beta/refunds/SKILL.md");
        // Names collide by design; the URIs are what identify them.
        assert_eq!(a.frontmatter.get("name"), b.frontmatter.get("name"));
    }

    #[test]
    fn one_upstreams_two_same_named_skills_both_survive() {
        // The spec's own example: acme/billing/refunds and acme/support/refunds
        // are different skills that share a final segment.
        let billing = upstream_skill("acme", "refunds");
        let minted = mint_proxied_entry("acme-corp", &billing).expect("mints");
        assert_eq!(minted.uri, "skill://acme-corp/refunds/SKILL.md");
        // Nothing here dedupes by name, so a sibling at another path is
        // unaffected — asserting the absence of a name-keyed collapse.
        let entries = mint_proxied_entries(
            &labby_runtime::gateway_config::UpstreamConfig {
                name: "acme-corp".to_string(),
                ..super::super::tests_support::minimal_config()
            },
            &[
                upstream_skill("acme", "refunds"),
                upstream_skill("acme", "refunds"),
            ],
        );
        assert_eq!(entries.len(), 2, "no name-keyed deduplication");
    }

    #[test]
    fn an_unlabellable_origin_excludes_the_skill_rather_than_half_rewriting_it() {
        // A manifest rewritten only partway would fail every client's
        // verification; dropping the skill is the honest outcome.
        let skill = upstream_skill("their-label", "refunds");
        assert!(mint_proxied_entry("Not A Label", &skill).is_none());
    }

    #[test]
    fn a_digest_less_entry_relabels_without_inventing_a_manifest() {
        let mut skill = upstream_skill("their-label", "refunds");
        skill.entry.resources = None;
        let minted = mint_proxied_entry("gh", &skill).expect("mints");
        assert_eq!(minted.uri, "skill://gh/refunds/SKILL.md");
        assert!(minted.resources.is_none(), "must not fabricate a manifest");
    }
}
