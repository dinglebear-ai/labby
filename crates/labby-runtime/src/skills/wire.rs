//! Wire DTOs for the skills extension (SEP-2640).
//!
//! These are plain `serde` types. `labby-runtime` carries no `rmcp` dependency
//! by design, so the conversion to and from `CustomRequest`/`CustomResult`
//! happens in the crates that own the transport (`labby-gateway` for the client
//! side, `labby` for the server side). The wire shapes here look superficially
//! like rmcp's resource types; they are deliberately independent of them.
//!
//! Field naming follows the SEP exactly, including the camelCase
//! `nextCursor`/`ttlMs`/`cacheScope` keys inherited from the base protocol's
//! list-caching attributes.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Extension capability key advertised in `capabilities.extensions`.
///
/// Note the slash: the reserved *frontmatter metadata* prefix is
/// `io.modelcontextprotocol/`, spelled differently in the resource `_meta`
/// context. They are not interchangeable.
pub const SKILLS_EXTENSION_KEY: &str = "io.modelcontextprotocol/skills";

/// JSON-RPC method that enumerates a server's skills.
pub const SKILLS_LIST_METHOD: &str = "skills/list";

/// JSON-RPC method that returns one skill entry by URI.
pub const SKILLS_GET_METHOD: &str = "skills/get";

/// Optional directory-listing method, gated on `directoryRead`.
pub const RESOURCES_DIRECTORY_READ_METHOD: &str = "resources/directory/read";

/// MIME type a `SKILL.md` resource should carry.
pub const SKILL_MD_MIME_TYPE: &str = "text/markdown";

/// Capability payload for `io.modelcontextprotocol/skills`.
///
/// An empty object means the extension is supported with no optional features;
/// that is what Labby advertises, since it does not implement
/// `resources/directory/read`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsCapability {
    /// Whether the server implements `resources/directory/read`. Clients MUST
    /// NOT call that method against a server that has not set this.
    #[serde(rename = "directoryRead", default, skip_serializing_if = "is_false")]
    pub directory_read: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// One `{uri, digest}` pair from a skill entry's `resources` manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillResource {
    pub uri: String,
    pub digest: String,
}

/// A skill entry, as carried by both `skills/list` and `skills/get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Full resource URI of the skill's `SKILL.md`.
    pub uri: String,
    /// The `SKILL.md` YAML frontmatter rendered verbatim as a JSON object.
    pub frontmatter: Map<String, Value>,
    /// Complete file manifest. Absent only for dynamically generated skills,
    /// which cannot publish stable digests and therefore cannot be
    /// content-bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Vec<SkillResource>>,
    /// Implementation metadata about where this entry came from.
    ///
    /// Deliberately **not** part of `frontmatter`: the SEP requires frontmatter
    /// to be the author's YAML verbatim, and a host is required to compare it
    /// field by field against the fetched `SKILL.md` and refuse the skill on any
    /// discrepancy. Anything Labby adds must therefore live outside it, which is
    /// what `_meta` is for. A client that does not recognize the key ignores it.
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
}

impl SkillEntry {
    /// True when the entry publishes no manifest, and so cannot be verified.
    #[must_use]
    pub fn is_unverifiable(&self) -> bool {
        self.resources.is_none()
    }
}

/// `skills/list` request parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// `skills/list` result.
///
/// A server whose catalog is large, generated, or otherwise unenumerable may
/// return an empty or partial listing, and hosts must not read an empty listing
/// as proof that a server has no skills.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsListResult {
    #[serde(default)]
    pub skills: Vec<SkillEntry>,
    #[serde(
        rename = "nextCursor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub next_cursor: Option<String>,
    /// Freshness hint for the listing, per the base protocol's list-caching
    /// attributes. A hint only — never an integrity property.
    #[serde(rename = "ttlMs", default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    /// Cache-scope marker, whose vocabulary the SEP delegates to the base
    /// protocol rather than enumerating. Kept as a free-form string so an
    /// unrecognized value round-trips instead of failing to deserialize.
    #[serde(
        rename = "cacheScope",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub cache_scope: Option<String>,
}

/// `skills/get` request parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsGetParams {
    pub uri: String,
}

/// `skills/get` result.
///
/// The entry is nested under a `skill` key rather than returned flat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsGetResult {
    pub skill: SkillEntry,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserializes_skills_list_result_from_spec_example() {
        let raw = json!({
            "skills": [
                {
                    "uri": "skill://git-workflow/SKILL.md",
                    "frontmatter": {
                        "name": "git-workflow",
                        "description": "Follow this team's Git conventions for branching and commits"
                    },
                    "resources": [
                        { "uri": "skill://git-workflow/SKILL.md", "digest": "sha256:a1b2c3d4" }
                    ]
                },
                {
                    "uri": "skill://acme/billing/refunds/SKILL.md",
                    "frontmatter": {
                        "name": "refunds",
                        "description": "Process customer refund requests per company policy",
                        "license": "Apache-2.0"
                    },
                    "resources": [
                        { "uri": "skill://acme/billing/refunds/SKILL.md", "digest": "sha256:b2c3d4e5" },
                        { "uri": "skill://acme/billing/refunds/examples/email.md", "digest": "sha256:c3d4e5f6" }
                    ]
                }
            ],
            "nextCursor": "page-2",
            "ttlMs": 300_000,
            "cacheScope": "per-server"
        });

        let result: SkillsListResult = serde_json::from_value(raw.clone()).expect("deserializes");
        assert_eq!(result.skills.len(), 2);
        assert_eq!(result.next_cursor.as_deref(), Some("page-2"));
        assert_eq!(result.ttl_ms, Some(300_000));
        assert_eq!(result.cache_scope.as_deref(), Some("per-server"));
        // The nested-path skill is named for the final segment, not the whole path.
        assert_eq!(
            result.skills[1].frontmatter.get("name"),
            Some(&json!("refunds"))
        );
        assert_eq!(serde_json::to_value(&result).expect("serializes"), raw);
    }

    #[test]
    fn deserializes_skills_get_result_with_nested_skill_key() {
        let raw = json!({
            "skill": {
                "uri": "skill://pdf-processing/SKILL.md",
                "frontmatter": {
                    "name": "pdf-processing",
                    "description": "Extract, fill, and assemble PDF documents",
                    "metadata": { "version": "2.1.0" }
                },
                "resources": [
                    { "uri": "skill://pdf-processing/SKILL.md", "digest": "sha256:d5e6f7a8" },
                    { "uri": "skill://pdf-processing/references/FORMS.md", "digest": "sha256:e6f7a8b9" }
                ]
            }
        });

        let result: SkillsGetResult = serde_json::from_value(raw.clone()).expect("deserializes");
        assert_eq!(result.skill.uri, "skill://pdf-processing/SKILL.md");
        assert_eq!(result.skill.resources.as_ref().expect("manifest").len(), 2);
        assert_eq!(serde_json::to_value(&result).expect("serializes"), raw);
    }

    #[test]
    fn empty_listing_round_trips() {
        // A server that cannot enumerate returns an empty listing; that must
        // deserialize cleanly rather than erroring.
        let result: SkillsListResult = serde_json::from_value(json!({ "skills": [] })).expect("ok");
        assert!(result.skills.is_empty());
        assert!(result.next_cursor.is_none());
    }

    #[test]
    fn entry_without_resources_is_unverifiable() {
        let entry: SkillEntry = serde_json::from_value(json!({
            "uri": "skill://generated/SKILL.md",
            "frontmatter": { "name": "generated", "description": "d" }
        }))
        .expect("deserializes");
        assert!(entry.is_unverifiable());
        // Absent, not null: an omitted manifest must not serialize back as an
        // explicit `"resources": null`.
        let encoded = serde_json::to_value(&entry).expect("serializes");
        assert!(encoded.get("resources").is_none());
    }

    #[test]
    fn unknown_cache_scope_round_trips() {
        let result: SkillsListResult =
            serde_json::from_value(json!({ "skills": [], "cacheScope": "some-future-scope" }))
                .expect("unknown scope tolerated");
        assert_eq!(result.cache_scope.as_deref(), Some("some-future-scope"));
    }

    #[test]
    fn capability_defaults_to_no_optional_features() {
        let capability = SkillsCapability::default();
        assert_eq!(
            serde_json::to_value(capability).expect("serializes"),
            json!({})
        );
    }

    #[test]
    fn capability_uses_the_camel_case_wire_key() {
        // A snake_case key would serialize to something no spec-compliant peer
        // recognizes, and would silently deserialize a real server's
        // `directoryRead: true` as false via the field default — so the wrong
        // key fails open, toward calling a method the server never offered.
        let declared = SkillsCapability {
            directory_read: true,
        };
        assert_eq!(
            serde_json::to_value(&declared).expect("serializes"),
            json!({ "directoryRead": true })
        );

        let parsed: SkillsCapability =
            serde_json::from_value(json!({ "directoryRead": true })).expect("deserializes");
        assert!(parsed.directory_read);

        let absent: SkillsCapability = serde_json::from_value(json!({})).expect("deserializes");
        assert!(!absent.directory_read);
    }
}
