//! Canonical first-party Agent Skills registry.
//!
//! Bundled and operator-provided skills live here independently of any product
//! surface. Native MCP, compatibility tools, CLI, and API adapters must all
//! resolve these same entries and bytes rather than maintaining parallel skill
//! registries.
//!
//! Skills remain data: this module never interpolates skill content into tool
//! descriptions, prompts, or action catalogs.

pub(crate) mod aggregate;
pub(crate) mod facade;
pub(crate) mod local;
pub(crate) mod providers;
pub(crate) mod registry;
pub(crate) mod search;

use std::collections::BTreeMap;
#[cfg(test)]
use std::sync::OnceLock;

#[cfg(test)]
use labby_runtime::skills::SkillUri;
#[cfg(test)]
use labby_runtime::skills::parse_skill_uri;
#[cfg(test)]
use labby_runtime::skills::wire::{CACHE_SCOPE_PUBLIC, SkillsListResult};
use labby_runtime::skills::wire::{SkillEntry, SkillResource};
use labby_runtime::skills::{FIRST_PARTY_ORIGIN, ResourceDigest, parse_skill_md_frontmatter};

/// Every embedded first-party file, as `(skill name, path within the skill,
/// contents)`.
///
/// The paths mirror `plugins/labby/skills/<name>/<path>` exactly, which is what
/// makes the URIs Labby publishes match the layout an operator sees on disk.
const EMBEDDED_FILES: &[(&str, &str, &str)] = &[
    (
        "using-labby",
        "SKILL.md",
        include_str!("../../../plugins/labby/skills/using-labby/SKILL.md"),
    ),
    (
        "using-labby",
        "references/code-mode.md",
        include_str!("../../../plugins/labby/skills/using-labby/references/code-mode.md"),
    ),
    (
        "using-labby",
        "references/service-catalog.md",
        include_str!("../../../plugins/labby/skills/using-labby/references/service-catalog.md"),
    ),
    (
        "using-labby",
        "references/operator-cli.md",
        include_str!("../../../plugins/labby/skills/using-labby/references/operator-cli.md"),
    ),
    (
        "using-labby",
        "references/gateway-operations.md",
        include_str!("../../../plugins/labby/skills/using-labby/references/gateway-operations.md"),
    ),
    (
        "using-labby",
        "references/config-reference.md",
        include_str!("../../../plugins/labby/skills/using-labby/references/config-reference.md"),
    ),
    (
        "using-labby",
        "agents/openai.yaml",
        include_str!("../../../plugins/labby/skills/using-labby/agents/openai.yaml"),
    ),
    (
        "creating-snippets",
        "SKILL.md",
        include_str!("../../../plugins/labby/skills/creating-snippets/SKILL.md"),
    ),
    (
        "creating-snippets",
        "README.md",
        include_str!("../../../plugins/labby/skills/creating-snippets/README.md"),
    ),
    (
        "creating-snippets",
        "CHANGELOG.md",
        include_str!("../../../plugins/labby/skills/creating-snippets/CHANGELOG.md"),
    ),
    (
        "creating-snippets",
        "agents/openai.yaml",
        include_str!("../../../plugins/labby/skills/creating-snippets/agents/openai.yaml"),
    ),
];

/// A first-party skill: its published entry plus the bytes behind each URI.
#[derive(Debug)]
pub(crate) struct FirstPartySkill {
    pub(crate) entry: SkillEntry,
    #[cfg(test)]
    files: BTreeMap<String, &'static str>,
}

#[cfg(test)]
impl FirstPartySkill {
    /// Contents of one file of this skill, by full `skill://` URI.
    pub(crate) fn file(&self, uri: &str) -> Option<&'static str> {
        self.files.get(uri).copied()
    }
}

/// URI of a first-party skill's file.
fn first_party_uri(skill: &str, path: &str) -> String {
    format!("skill://{FIRST_PARTY_ORIGIN}/{skill}/{path}")
}

/// Build every first-party skill entry, computing digests from embedded bytes.
///
/// Runs once. A skill whose `SKILL.md` frontmatter cannot be parsed is skipped
/// rather than panicking: a malformed bundled asset should degrade Labby's own
/// skill listing, not prevent the server from starting.
fn build_first_party_skills() -> BTreeMap<String, FirstPartySkill> {
    let mut by_skill: BTreeMap<String, Vec<(&str, &str)>> = BTreeMap::new();
    for (skill, path, contents) in EMBEDDED_FILES {
        by_skill
            .entry((*skill).to_string())
            .or_default()
            .push((path, contents));
    }

    let mut built = BTreeMap::new();
    for (skill, files) in by_skill {
        let skill_md_uri = first_party_uri(&skill, "SKILL.md");
        let Some((_, skill_md)) = files.iter().find(|(path, _)| *path == "SKILL.md") else {
            tracing::warn!(skill = %skill, "bundled skill has no SKILL.md — skipping");
            continue;
        };
        let frontmatter = match parse_skill_md_frontmatter(skill_md) {
            Ok(frontmatter) => frontmatter,
            Err(error) => {
                tracing::warn!(
                    skill = %skill,
                    error = %error,
                    "bundled skill has unparseable frontmatter — skipping"
                );
                continue;
            }
        };

        let mut resources = Vec::with_capacity(files.len());
        #[cfg(test)]
        let mut contents = BTreeMap::new();
        for (path, body) in &files {
            let uri = first_party_uri(&skill, path);
            resources.push(SkillResource {
                uri: uri.clone(),
                digest: ResourceDigest::of_bytes(body.as_bytes()).to_wire(),
                size: body.len() as u64,
            });
            #[cfg(test)]
            contents.insert(uri, *body);
        }
        // Deterministic order so two processes publish byte-identical listings.
        resources.sort_by(|a, b| a.uri.cmp(&b.uri));

        built.insert(
            skill.clone(),
            FirstPartySkill {
                entry: SkillEntry {
                    uri: skill_md_uri,
                    frontmatter,
                    resources: Some(resources),
                    meta: None,
                },
                #[cfg(test)]
                files: contents,
            },
        );
    }
    built
}

#[cfg(test)]
fn first_party_skills() -> &'static BTreeMap<String, FirstPartySkill> {
    static SKILLS: OnceLock<BTreeMap<String, FirstPartySkill>> = OnceLock::new();
    SKILLS.get_or_init(|| {
        let mut skills = build_first_party_skills();
        // Operator skills join the same reserved origin, so from a client's
        // view there is one first-party namespace. An embedded skill wins a
        // name collision: a dropped-in directory must not be able to redefine
        // what `skill://labby/using-labby` means.
        for (name, local) in local::load_local_skills() {
            if skills.contains_key(&name) {
                tracing::warn!(
                    skill = %name,
                    "an operator skill shadows a bundled skill of the same name — keeping the bundled one"
                );
                continue;
            }
            skills.insert(
                name,
                FirstPartySkill {
                    entry: local.entry,
                    // Operator files are owned Strings; embedded ones are
                    // 'static. Leaking here is deliberate and bounded: the set
                    // is built once at startup and lives for the process.
                    files: local
                        .files
                        .into_iter()
                        .map(|(uri, body)| (uri, &*Box::leak(body.into_boxed_str())))
                        .collect(),
                },
            );
        }
        skills
    })
}

/// The `skills/list` result for Labby's own skills.
///
/// Single page: the bundled set is small and fixed, so there is nothing to
/// paginate. The spec-shaped fields are still emitted so a client's pagination
/// handling is exercised the same way it would be against any other server.
#[cfg(test)]
pub(crate) fn list_first_party_skills() -> SkillsListResult {
    SkillsListResult {
        result_type: Default::default(),
        skills: first_party_skills()
            .values()
            .map(|skill| skill.entry.clone())
            .collect(),
        next_cursor: None,
        // Embedded and immutable for the life of the process, so a client may
        // cache the listing for as long as it likes.
        ttl_ms: Some(3_600_000),
        // Labby's own skills carry no per-caller data. This holds only for
        // the first-party set — `absorb` downgrades it the moment per-caller
        // proxied entries are folded in.
        cache_scope: Some(CACHE_SCOPE_PUBLIC.to_string()),
        meta: None,
    }
}

/// One first-party skill entry by URI, for `skills/get`.
///
/// Accepts any URI belonging to the skill, not only its `SKILL.md`: a caller
/// holding a supporting file's URI should still be able to reach the entry that
/// binds it.
#[cfg(test)]
pub(crate) fn first_party_skill_entry(uri: &str) -> Option<SkillEntry> {
    let parsed = parse_skill_uri(uri).ok()?;
    if parsed.origin() != FIRST_PARTY_ORIGIN {
        return None;
    }
    first_party_skills()
        .values()
        .find(|skill| skill.entry.uri == uri || skill.files.contains_key(uri))
        .map(|skill| skill.entry.clone())
}

/// Contents of a first-party skill file, by URI.
#[cfg(test)]
pub(crate) fn read_first_party_skill_file(uri: &str) -> Option<&'static str> {
    let parsed: SkillUri = parse_skill_uri(uri).ok()?;
    if parsed.origin() != FIRST_PARTY_ORIGIN {
        return None;
    }
    first_party_skills()
        .values()
        .find_map(|skill| skill.file(uri))
}

/// True when `uri` is in the `skill://` namespace at all.
pub(crate) fn is_skill_uri(uri: &str) -> bool {
    uri.starts_with(labby_runtime::skills::SKILL_URI_SCHEME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::skills::compare_frontmatter;

    #[test]
    fn both_bundled_skills_publish_a_complete_manifest() {
        let skills = first_party_skills();
        assert!(skills.contains_key("using-labby"));
        assert!(skills.contains_key("creating-snippets"));

        for (name, skill) in skills {
            let resources = skill.entry.resources.as_ref().expect("manifest present");
            // The SEP requires the manifest to list the skill's own SKILL.md.
            assert!(
                resources.iter().any(|r| r.uri == skill.entry.uri),
                "{name} manifest omits its own SKILL.md"
            );
            // ...and to be complete: every served file appears exactly once.
            assert_eq!(
                resources.len(),
                skill.files.len(),
                "{name} manifest does not cover every served file"
            );
        }
    }

    #[test]
    fn every_published_digest_matches_the_bytes_actually_served() {
        // The property a client verifies on every read. If it can fail here it
        // can fail for a real client, and the client is required to refuse the
        // content when it does.
        for skill in first_party_skills().values() {
            for resource in skill.entry.resources.as_ref().expect("manifest") {
                let body = skill
                    .file(&resource.uri)
                    .unwrap_or_else(|| panic!("no bytes for {}", resource.uri));
                let digest = labby_runtime::skills::parse_digest(&resource.digest)
                    .expect("a well-formed digest");
                assert!(
                    digest.matches(body.as_bytes()),
                    "digest does not match served bytes for {}",
                    resource.uri
                );
            }
        }
    }

    #[test]
    fn published_frontmatter_matches_the_skill_md_it_describes() {
        // SEP-2640 requires a host to compare an entry's frontmatter field by
        // field against the fetched SKILL.md and refuse the skill on any
        // discrepancy. Both sides come from the same embedded bytes here, so
        // this is provable rather than hopeful — assert it instead of assuming.
        for (name, skill) in first_party_skills() {
            let body = skill.file(&skill.entry.uri).expect("SKILL.md bytes");
            let parsed = parse_skill_md_frontmatter(body).expect("parses");
            compare_frontmatter(&skill.entry.frontmatter, &parsed)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }

    #[test]
    fn entries_validate_against_the_shared_ingest_rules() {
        // Labby holds its own skills to exactly the bar it holds an upstream's.
        for (name, skill) in first_party_skills() {
            labby_runtime::skills::validate_skill_entry(&skill.entry)
                .unwrap_or_else(|reason| panic!("{name} fails ingest: {}", reason.as_str()));
        }
    }

    #[test]
    fn names_match_the_final_skill_path_segment() {
        for (name, skill) in first_party_skills() {
            let parsed = parse_skill_uri(&skill.entry.uri).expect("valid URI");
            let (_, recovered) = parsed.skill_md_parts().expect("a SKILL.md URI");
            assert_eq!(recovered, name);
            assert_eq!(
                skill.entry.frontmatter.get("name").and_then(|v| v.as_str()),
                Some(name.as_str())
            );
        }
    }

    #[test]
    fn lookup_resolves_entries_from_any_file_uri_and_rejects_other_origins() {
        let uri = "skill://labby/using-labby/references/code-mode.md";
        let entry = first_party_skill_entry(uri).expect("a supporting file resolves its entry");
        assert_eq!(entry.uri, "skill://labby/using-labby/SKILL.md");
        assert!(read_first_party_skill_file(uri).is_some());

        // A proxied origin is not ours to answer for.
        assert!(first_party_skill_entry("skill://upstream/x/SKILL.md").is_none());
        assert!(read_first_party_skill_file("skill://upstream/x/SKILL.md").is_none());
        // Nor is an unknown file within a real skill.
        assert!(read_first_party_skill_file("skill://labby/using-labby/nope.md").is_none());
    }

    #[test]
    fn the_listing_is_deterministic() {
        // Two processes must publish byte-identical listings, or a client
        // diffing them sees phantom changes.
        let first = serde_json::to_string(&list_first_party_skills()).expect("serializes");
        let second = serde_json::to_string(&list_first_party_skills()).expect("serializes");
        assert_eq!(first, second);
    }

    #[test]
    fn production_skill_routes_do_not_reference_the_legacy_static_registry() {
        for (route, source) in [
            ("native MCP", include_str!("mcp/skills.rs")),
            ("resources/read", include_str!("mcp/handlers_resources.rs")),
            (
                "compatibility dispatch",
                include_str!("dispatch/skills/dispatch.rs"),
            ),
            (
                "compatibility client",
                include_str!("dispatch/skills/client.rs"),
            ),
            ("API", include_str!("api/services/skills.rs")),
            ("CLI", include_str!("cli/skills.rs")),
            (
                "in-process Code Mode",
                include_str!("mcp/in_process_peer.rs"),
            ),
        ] {
            for legacy in [
                "first_party_skills()",
                "list_first_party_skills()",
                "first_party_skill_entry(",
                "read_first_party_skill_file(",
            ] {
                assert!(
                    !source.contains(legacy),
                    "{route} bypasses SkillRegistryContext through {legacy}"
                );
            }
        }
        assert!(include_str!("cli/skills.rs").contains("dispatch_at_cli_boundary"));
        assert!(include_str!("mcp/skills.rs").contains("dispatch_at_in_process_boundary"));
        assert!(include_str!("api/services/skills.rs").contains("dispatch_at_api_boundary"));
    }
}
