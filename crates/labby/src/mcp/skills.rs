//! First-party Agent Skills served over MCP (SEP-2640, Direction A).
//!
//! Labby serves the skills bundled with its own plugin under the reserved
//! `skill://labby/…` origin. The files are embedded at build time, so the
//! digest manifest is computed once from bytes that cannot change underneath
//! it — no TOCTOU window between publishing a digest and serving the file it
//! describes.
//!
//! # Why the file list is explicit
//!
//! Each file is named in [`EMBEDDED_FILES`] rather than discovered by walking
//! a directory. `include_str!` needs a literal path anyway, and an explicit
//! list means adding a file to a skill is a visible diff here — a skill's
//! manifest is what a user's approval binds to, so its contents should not be
//! able to change as a side effect of dropping a file into a directory.
//!
//! # Skills are data
//!
//! Nothing here interpolates skill content into a tool description, help text,
//! the action catalog, or a prompt. The bytes travel from `include_str!` to the
//! wire and nowhere else.

pub(crate) mod aggregate;

use std::collections::BTreeMap;
use std::sync::OnceLock;

use labby_runtime::skills::wire::{SkillEntry, SkillResource, SkillsListResult};
use labby_runtime::skills::{
    FIRST_PARTY_ORIGIN, ResourceDigest, SkillUri, parse_skill_md_frontmatter, parse_skill_uri,
};

/// Every embedded first-party file, as `(skill name, path within the skill,
/// contents)`.
///
/// The paths mirror `plugins/labby/skills/<name>/<path>` exactly, which is what
/// makes the URIs Labby publishes match the layout an operator sees on disk.
const EMBEDDED_FILES: &[(&str, &str, &str)] = &[
    (
        "using-labby",
        "SKILL.md",
        include_str!("../../../../plugins/labby/skills/using-labby/SKILL.md"),
    ),
    (
        "using-labby",
        "references/code-mode.md",
        include_str!("../../../../plugins/labby/skills/using-labby/references/code-mode.md"),
    ),
    (
        "using-labby",
        "references/service-catalog.md",
        include_str!("../../../../plugins/labby/skills/using-labby/references/service-catalog.md"),
    ),
    (
        "using-labby",
        "references/operator-cli.md",
        include_str!("../../../../plugins/labby/skills/using-labby/references/operator-cli.md"),
    ),
    (
        "using-labby",
        "references/gateway-operations.md",
        include_str!(
            "../../../../plugins/labby/skills/using-labby/references/gateway-operations.md"
        ),
    ),
    (
        "using-labby",
        "references/config-reference.md",
        include_str!("../../../../plugins/labby/skills/using-labby/references/config-reference.md"),
    ),
    (
        "using-labby",
        "agents/openai.yaml",
        include_str!("../../../../plugins/labby/skills/using-labby/agents/openai.yaml"),
    ),
    (
        "creating-snippets",
        "SKILL.md",
        include_str!("../../../../plugins/labby/skills/creating-snippets/SKILL.md"),
    ),
    (
        "creating-snippets",
        "README.md",
        include_str!("../../../../plugins/labby/skills/creating-snippets/README.md"),
    ),
    (
        "creating-snippets",
        "CHANGELOG.md",
        include_str!("../../../../plugins/labby/skills/creating-snippets/CHANGELOG.md"),
    ),
    (
        "creating-snippets",
        "agents/openai.yaml",
        include_str!("../../../../plugins/labby/skills/creating-snippets/agents/openai.yaml"),
    ),
];

/// A first-party skill: its published entry plus the bytes behind each URI.
#[derive(Debug)]
pub(crate) struct FirstPartySkill {
    pub(crate) entry: SkillEntry,
    files: BTreeMap<String, &'static str>,
}

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
        let mut contents = BTreeMap::new();
        for (path, body) in &files {
            let uri = first_party_uri(&skill, path);
            resources.push(SkillResource {
                uri: uri.clone(),
                digest: ResourceDigest::of_bytes(body.as_bytes()).to_wire(),
            });
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
                },
                files: contents,
            },
        );
    }
    built
}

fn first_party_skills() -> &'static BTreeMap<String, FirstPartySkill> {
    static SKILLS: OnceLock<BTreeMap<String, FirstPartySkill>> = OnceLock::new();
    SKILLS.get_or_init(build_first_party_skills)
}

/// The `skills/list` result for Labby's own skills.
///
/// Single page: the bundled set is small and fixed, so there is nothing to
/// paginate. The spec-shaped fields are still emitted so a client's pagination
/// handling is exercised the same way it would be against any other server.
pub(crate) fn list_first_party_skills() -> SkillsListResult {
    SkillsListResult {
        skills: first_party_skills()
            .values()
            .map(|skill| skill.entry.clone())
            .collect(),
        next_cursor: None,
        // Embedded and immutable for the life of the process, so a client may
        // cache the listing for as long as it likes.
        ttl_ms: Some(3_600_000),
        // Labby's own skills carry no per-caller data.
        cache_scope: Some("public".to_string()),
    }
}

/// One first-party skill entry by URI, for `skills/get`.
///
/// Accepts any URI belonging to the skill, not only its `SKILL.md`: a caller
/// holding a supporting file's URI should still be able to reach the entry that
/// binds it.
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
}

// ── MCP request handling ─────────────────────────────────────────────────────

use rmcp::RoleServer;
use rmcp::model::{CustomRequest, CustomResult, ErrorData};
use rmcp::service::RequestContext;

use labby_runtime::skills::wire::{SKILLS_GET_METHOD, SkillsGetParams, SkillsGetResult};

use crate::mcp::context::{auth_context_from_extensions, code_mode_read_scope_allowed};
use crate::mcp::server::LabMcpServer;

impl LabMcpServer {
    /// Answer `skills/list` and `skills/get`.
    ///
    /// Both are read-shaped, so they require the same scope as listing
    /// resources or prompts (`lab:read` and up) rather than admin. Agents are
    /// the intended consumers; requiring admin would defeat the feature, while
    /// leaving them ungated would make skills the one MCP surface with no scope
    /// decision behind it.
    pub(crate) async fn handle_skills_request(
        &self,
        request: &CustomRequest,
        context: &RequestContext<RoleServer>,
    ) -> Result<CustomResult, ErrorData> {
        let auth = auth_context_from_extensions(&context.extensions);
        if !code_mode_read_scope_allowed(auth) {
            return Err(ErrorData::new(
                rmcp::model::ErrorCode::INVALID_REQUEST,
                "reading skills requires the `lab:read` scope".to_string(),
                None,
            ));
        }

        if request.method == SKILLS_GET_METHOD {
            let params = request
                .params_as::<SkillsGetParams>()
                .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?
                .ok_or_else(|| ErrorData::invalid_params("skills/get requires `uri`", None))?;
            // -32602 is the spec's answer for a URI this server does not serve
            // as a skill. `invalid_params` is that code.
            let entry = first_party_skill_entry(&params.uri).ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("`{}` is not a skill this server serves", params.uri),
                    None,
                )
            })?;
            let result = SkillsGetResult { skill: entry };
            return serde_json::to_value(result)
                .map(CustomResult::new)
                .map_err(|error| ErrorData::internal_error(error.to_string(), None));
        }

        serde_json::to_value(list_first_party_skills())
            .map(CustomResult::new)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))
    }
}

#[cfg(test)]
mod serve_tests {
    use super::*;

    #[test]
    fn the_capability_is_advertised_with_no_optional_features() {
        // An empty object means "supported, no optional features". Declaring
        // `directoryRead` would invite a method Labby does not implement.
        let extensions = crate::mcp::server::mcp_extensions_for_test();
        let declared = extensions
            .get(labby_runtime::skills::wire::SKILLS_EXTENSION_KEY)
            .expect("skills extension is advertised when the feature is on");
        assert!(
            declared.is_empty(),
            "directoryRead must not be advertised: {declared:?}"
        );
    }

    #[test]
    fn skills_get_accepts_a_supporting_file_uri_and_returns_the_binding_entry() {
        let uri = "skill://labby/creating-snippets/README.md";
        let entry = first_party_skill_entry(uri).expect("resolves");
        assert_eq!(entry.uri, "skill://labby/creating-snippets/SKILL.md");
        // The entry that comes back is the one whose manifest binds that file.
        assert!(
            entry
                .resources
                .as_ref()
                .expect("manifest")
                .iter()
                .any(|r| r.uri == uri)
        );
    }

    #[test]
    fn a_client_can_verify_every_file_it_is_told_about() {
        // End to end, the way a conforming client works: read the listing, then
        // fetch and verify each manifest entry.
        let listing = list_first_party_skills();
        assert!(!listing.skills.is_empty());
        for entry in &listing.skills {
            for resource in entry.resources.as_ref().expect("manifest") {
                let body = read_first_party_skill_file(&resource.uri)
                    .expect("every listed file is served");
                let digest =
                    labby_runtime::skills::parse_digest(&resource.digest).expect("valid digest");
                assert!(digest.matches(body.as_bytes()), "{} failed", resource.uri);
            }
        }
    }

    #[test]
    fn unknown_skill_uris_are_not_served() {
        assert!(read_first_party_skill_file("skill://labby/using-labby/../escape.md").is_none());
        assert!(read_first_party_skill_file("skill://labby/nonexistent/SKILL.md").is_none());
        assert!(first_party_skill_entry("skill://labby/nonexistent/SKILL.md").is_none());
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use labby_runtime::gateway_config::UpstreamConfig;

    /// The smallest `UpstreamConfig` the aggregation tests need. Kept here so
    /// the submodule does not reach into the gateway crate's test fixtures.
    pub(crate) fn minimal_config() -> UpstreamConfig {
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
}
