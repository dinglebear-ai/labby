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
pub(crate) mod local;

#[cfg(feature = "gateway")]
use futures::{StreamExt, stream};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::OnceLock;

use labby_runtime::skills::wire::{
    CACHE_SCOPE_PRIVATE, CACHE_SCOPE_PUBLIC, SkillEntry, SkillResource, SkillsListResult,
};
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
                    meta: None,
                },
                files: contents,
            },
        );
    }
    built
}

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
    #[cfg(feature = "gateway")]
    async fn skill_origin_meta(
        &self,
        origin: &str,
        pool: &labby_gateway::upstream::pool::UpstreamPool,
    ) -> serde_json::Map<String, Value> {
        let code_mode = match self.gateway_manager.as_ref() {
            Some(manager) => manager.code_mode_enabled().await,
            None => false,
        };
        let access = if code_mode {
            aggregate::ToolAccess::CodeModeOnly
        } else {
            aggregate::ToolAccess::Direct
        };
        let reachable = if access == aggregate::ToolAccess::Direct {
            pool.healthy_tools_for_upstream(origin)
                .await
                .into_iter()
                .map(|tool| tool.tool.name.to_string())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        aggregate::origin_meta(origin, None, access, &reachable)
    }

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
        // OBSERVABILITY.md requires one structured dispatch event per
        // user-visible action. Wrapping the whole handler — rather than logging
        // inside it — is what makes the scope denial observable too; a refused
        // caller is exactly the event an operator needs and the easiest one to
        // leave untraced by logging only the success paths.
        let start = std::time::Instant::now();
        let action = if request.method == SKILLS_GET_METHOD {
            "skills.get"
        } else {
            "skills.list"
        };
        let subject_log = self.request_subject_log_tag(context);
        let outcome = self.dispatch_skills_request(request, context).await;

        match &outcome {
            Ok(_) => tracing::info!(
                surface = "mcp",
                service = "labby",
                action,
                subject = %subject_log,
                elapsed_ms = start.elapsed().as_millis(),
                "dispatch finish"
            ),
            Err(error) => tracing::warn!(
                surface = "mcp",
                service = "labby",
                action,
                subject = %subject_log,
                elapsed_ms = start.elapsed().as_millis(),
                kind = %error.code.0,
                "dispatch error"
            ),
        }
        outcome
    }

    async fn dispatch_skills_request(
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
            // Proxied entries must resolve here too. Answering -32602 for a URI
            // this same server just published in `skills/list` tells a
            // conforming client the skill was withdrawn — the code means "not a
            // skill I serve", which a client acts on by dropping it. Resolving
            // against the same aggregate the listing is built from is what
            // keeps the two answers from diverging.
            let entry = match first_party_skill_entry(&params.uri) {
                Some(entry) => entry,
                None => self
                    .proxied_skill_entry(context, &params.uri)
                    .await
                    .ok_or_else(|| {
                        // -32602 is the spec's answer for a URI this server does
                        // not serve as a skill. `invalid_params` is that code.
                        ErrorData::invalid_params(
                            format!("`{}` is not a skill this server serves", params.uri),
                            None,
                        )
                    })?,
            };
            let result = SkillsGetResult { skill: entry };
            return serde_json::to_value(result)
                .map(CustomResult::new)
                .map_err(|error| ErrorData::internal_error(error.to_string(), None));
        }

        let mut listing = list_first_party_skills();
        let proxied = self.proxied_skill_entries(context).await;

        // Proxied entries are fetched per OAuth subject and filtered by the
        // caller's route scope, so folding them into a listing that advertises
        // `cacheScope: public` would tell every downstream cache it may serve
        // one caller's entries to another. `absorb` collapses the scope and
        // takes the shorter TTL rather than inheriting the first-party terms.
        listing.absorb(
            proxied.entries,
            proxied.cache_scope.as_deref(),
            proxied.ttl_ms,
        );

        // An agent cannot act on "this listing may be partial" unless it is
        // told so. Without these, a listing missing four unreachable upstreams
        // is byte-identical to one where they genuinely had no skills.
        if proxied.unreachable_upstreams > 0 {
            listing.note_incomplete(
                "unreachableUpstreams",
                Value::from(proxied.unreachable_upstreams),
            );
        }
        if proxied.excluded_count > 0 {
            listing.note_incomplete("excludedSkills", Value::from(proxied.excluded_count));
        }
        if proxied.truncated {
            listing.note_incomplete("truncated", Value::Bool(true));
        }

        serde_json::to_value(listing)
            .map(CustomResult::new)
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))
    }

    /// One proxied skill entry by URI, resolved against the same aggregate
    /// `skills/list` is built from.
    ///
    /// Deliberately not a separate upstream fetch: two independent resolution
    /// paths are exactly how a server ends up publishing a URI in one and
    /// denying it in the other. Reusing the aggregate makes agreement
    /// structural rather than something two code paths have to maintain.
    #[cfg(feature = "gateway")]
    async fn proxied_skill_entry(
        &self,
        context: &RequestContext<RoleServer>,
        uri: &str,
    ) -> Option<SkillEntry> {
        if let Some(entry) = self
            .proxied_skill_entries(context)
            .await
            .entries
            .into_iter()
            .find(|entry| entry.uri == uri)
        {
            return Some(entry);
        }

        // Not in the listing. The SEP requires a host to load a skill given
        // only its URI, and says an empty or partial listing is never proof a
        // server has no skills — a budget-truncated walk or a narrowed cache
        // would otherwise make a servable skill permanently unreachable.
        let parsed = parse_skill_uri(uri).ok()?;
        let origin = parsed.origin().to_string();
        if !self.route_scope.allows_upstream(&origin) {
            return None;
        }
        let manager = self.gateway_manager.as_ref()?;
        let config = manager.upstream_config(&origin).await?;
        if !config.enabled || !config.proxy_skills {
            return None;
        }
        let pool = self.current_upstream_pool().await?;

        // Ask the upstream under the URI it knows, then relabel the answer back
        // into this origin exactly as the listing does.
        let upstream_uri = parsed.upstream_uri_for_origin(&config.name)?;
        let skill = pool
            .fetch_unlisted_skill(&config, self.request_subject(context), &upstream_uri)
            .await?;
        let meta = self.skill_origin_meta(&config.name, &pool).await;
        aggregate::mint_proxied_entry(&config.name, &skill, Some(&meta))
    }

    /// Without the gateway feature there are no proxied skills to resolve.
    #[cfg(not(feature = "gateway"))]
    async fn proxied_skill_entry(
        &self,
        _context: &RequestContext<RoleServer>,
        _uri: &str,
    ) -> Option<SkillEntry> {
        None
    }

    /// Skills aggregated from upstreams, relabelled under each origin.
    ///
    /// Route scope is applied here rather than in the pool: the pool has no
    /// notion of a named route, and this is where the caller's scope is known.
    /// A restricted route must not see skills from an upstream it cannot reach,
    /// or the agent's skill world would not match its tool world.
    #[cfg(feature = "gateway")]
    async fn proxied_skill_entries(&self, context: &RequestContext<RoleServer>) -> ProxiedSkills {
        let Some(manager) = self.gateway_manager.as_ref() else {
            return ProxiedSkills::default();
        };
        let Some(pool) = self.current_upstream_pool().await else {
            return ProxiedSkills::default();
        };
        let subject = self.request_subject(context);
        let scope = self.route_scope.clone();

        // With Code Mode on, raw upstream tools are hidden from tools/list, so
        // no downstream name exists for a skill's `allowed-tools` to resolve
        // against. Computed once: it is a gateway-wide setting.
        let tool_access = if manager.code_mode_enabled().await {
            aggregate::ToolAccess::CodeModeOnly
        } else {
            aggregate::ToolAccess::Direct
        };

        let configs = manager
            .current_config()
            .await
            .upstream
            .into_iter()
            .filter(|config| config.enabled && config.proxy_skills)
            .filter(|config| scope.allows_upstream(&config.name))
            .collect::<Vec<_>>();
        let mut results = stream::iter(configs)
            .map(|config| {
                let pool = std::sync::Arc::clone(&pool);
                async move {
                    let result = pool.upstream_skills(&config, subject).await;
                    (config, result)
                }
            })
            .buffer_unordered(8)
            .collect::<Vec<_>>()
            .await;
        results.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name));

        let mut aggregated = ProxiedSkills::default();
        if !results.is_empty() {
            aggregated.cache_scope = Some(CACHE_SCOPE_PRIVATE.to_string());
        }
        let entries = &mut aggregated.entries;
        for (config, result) in results {
            match result {
                Ok(exposed) => {
                    // Completeness bookkeeping the pool computed. Dropping it
                    // here is what makes a partial listing indistinguishable
                    // from a complete one.
                    aggregated.excluded_count += exposed.excluded_count;
                    aggregated.truncated |= exposed.truncated;
                    // Every proxied entry is subject- and scope-dependent, so
                    // one is enough to make the whole listing non-shareable.
                    aggregated.ttl_ms = min_ttl(aggregated.ttl_ms, exposed.ttl_ms);
                    // Facts about Labby's own catalog, so a client can scope
                    // `allowed-tools` to this origin instead of resolving it
                    // against the flattened aggregate (threat model T3).
                    let reachable_tools: Vec<String> = match tool_access {
                        aggregate::ToolAccess::Direct => pool
                            .healthy_tools_for_upstream(&config.name)
                            .await
                            .into_iter()
                            .map(|tool| tool.tool.name.to_string())
                            .collect(),
                        aggregate::ToolAccess::CodeModeOnly => Vec::new(),
                    };
                    let meta =
                        aggregate::origin_meta(&config.name, None, tool_access, &reachable_tools);
                    let minted =
                        aggregate::mint_proxied_entries(&config, &exposed.skills, Some(&meta));
                    aggregated.excluded_count += minted.excluded_count;
                    entries.extend(minted.entries);
                }
                Err(error) => {
                    // Partial results: one unreachable upstream must not empty
                    // the whole listing. The failure is already recorded on the
                    // circuit breaker by the pool. Counted, not just logged —
                    // an operator sees the log, but the agent acting on the
                    // listing sees only what rides on the wire.
                    aggregated.unreachable_upstreams += 1;
                    tracing::warn!(
                        upstream = %config.name,
                        error = %error,
                        "skipping an upstream while aggregating skills"
                    );
                }
            }
        }
        aggregated
    }

    #[cfg(not(feature = "gateway"))]
    async fn proxied_skill_entries(
        &self,
        _context: &RequestContext<RoleServer>,
    ) -> Vec<SkillEntry> {
        Vec::new()
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

    #[test]
    fn unlisted_proxy_lookup_removes_the_gateway_label_instead_of_prepending_it_again() {
        assert_eq!(
            parse_skill_uri("skill://gh/skill/acme/refunds/SKILL.md")
                .expect("published URI")
                .upstream_uri_for_origin("gh")
                .expect("reconstructable skill URI"),
            "skill://acme/refunds/SKILL.md"
        );
        assert!(
            parse_skill_uri("skill://other/skill/acme/refunds/SKILL.md")
                .expect("published URI")
                .upstream_uri_for_origin("gh")
                .is_none(),
            "a URI outside the selected gateway origin must not be reconstructed"
        );
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

/// Skills aggregated from upstreams, with the bookkeeping that says how
/// complete the set is.
///
/// The counts are the point. A listing that silently omits four unreachable
/// upstreams is byte-identical to one where those upstreams genuinely had no
/// skills, and an agent reading it concludes the skill it needs does not
/// exist. The SEP is explicit that an empty or partial listing is never proof
/// of absence — which a client can only honor if it is told which case it has.
#[cfg(feature = "gateway")]
#[derive(Debug, Default)]
pub(crate) struct ProxiedSkills {
    pub(crate) entries: Vec<SkillEntry>,
    /// Upstreams that failed and were skipped rather than emptying the listing.
    pub(crate) unreachable_upstreams: usize,
    /// Skills dropped for integrity or budget reasons, summed across upstreams.
    pub(crate) excluded_count: usize,
    /// Whether any upstream's walk was cut short by a budget.
    pub(crate) truncated: bool,
    /// Strictest cache scope across the folded-in sources.
    pub(crate) cache_scope: Option<String>,
    /// Shortest remaining lifetime across the folded-in snapshots.
    pub(crate) ttl_ms: Option<u64>,
}

/// Shorter of two optional TTLs, preferring whichever is present.
#[cfg(feature = "gateway")]
fn min_ttl(current: Option<u64>, incoming: Option<u64>) -> Option<u64> {
    match (current, incoming) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

impl LabMcpServer {
    /// Serve one file of a proxied skill.
    ///
    /// Enforces the same three gates the listing does — the upstream must opt
    /// in, the skill must pass `expose_skills`, and the caller's route must be
    /// allowed to reach that upstream — before the pool performs the
    /// manifest-bound, digest-verified read. Filtering only the listing would
    /// leave the file fetchable by URI, which is a bypass rather than a
    /// restriction.
    ///
    /// `subject` is the caller's real OAuth identity, used to key the
    /// per-subject skills cache and select the upstream token. `subject_log` is
    /// its redacted form and is only ever logged — the two must not be swapped.
    #[cfg(feature = "gateway")]
    pub(crate) async fn read_proxied_skill_file_impl(
        &self,
        uri: &str,
        redacted_uri: &str,
        subject: Option<&str>,
        subject_log: &str,
        start: std::time::Instant,
    ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
        let parsed = parse_skill_uri(uri)
            .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
        let origin = parsed.origin().to_string();

        let unknown = || {
            ErrorData::invalid_params(
                format!("`{redacted_uri}` is not a skill file this server serves"),
                None,
            )
        };

        let Some(manager) = self.gateway_manager.as_ref() else {
            return Err(unknown());
        };
        // Route scope first: an upstream this route cannot reach must look
        // absent, not forbidden.
        if !self.route_scope.allows_upstream(&origin) {
            return Err(unknown());
        }
        let Some(config) = manager.upstream_config(&origin).await else {
            return Err(unknown());
        };
        if !config.enabled || !config.proxy_skills {
            return Err(unknown());
        }
        let Some(pool) = self.current_upstream_pool().await else {
            return Err(unknown());
        };

        let upstream_uri = parsed
            .upstream_uri_for_origin(&origin)
            .ok_or_else(unknown)?;
        let verified = pool
            .read_proxied_skill_file(&config, subject, &upstream_uri)
            .await
            .map_err(|error| {
                let payload = serde_json::to_string(&error).unwrap_or_else(|_| error.to_string());
                // -32602 is this server's answer for "not a skill I serve" —
                // an authoritative negative a conforming client acts on by
                // dropping the skill. An integrity failure is the opposite
                // claim: the skill exists and something is wrong with it.
                // Collapsing the two would make tampering indistinguishable
                // from a typo, and would teach a client to forget the skill
                // rather than refresh it.
                match error.kind() {
                    labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH
                    | labby_runtime::skills::KIND_SKILL_MANIFEST_STALE => {
                        ErrorData::internal_error(payload, None)
                    }
                    _ => ErrorData::invalid_params(payload, None),
                }
            })?;

        tracing::info!(
            surface = "mcp",
            service = "labby",
            action = "read_resource",
            subject = %subject_log,
            resource_uri = %redacted_uri,
            upstream = %origin,
            elapsed_ms = start.elapsed().as_millis(),
            "dispatch finish"
        );
        let mut contents = rmcp::model::ResourceContents::text(verified.text, uri.to_string());
        if let Some(mime) = verified.mime_type {
            contents = contents.with_mime_type(mime);
        }
        Ok(rmcp::model::ReadResourceResult::new(vec![contents]).into())
    }

    #[cfg(not(feature = "gateway"))]
    pub(crate) async fn read_proxied_skill_file_impl(
        &self,
        _uri: &str,
        redacted_uri: &str,
        _subject: &str,
        _start: std::time::Instant,
    ) -> Result<rmcp::model::ReadResourceResponse, ErrorData> {
        Err(ErrorData::invalid_params(
            format!("`{redacted_uri}` is not a skill file this server serves"),
            None,
        ))
    }
}
