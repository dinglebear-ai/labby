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

use serde::{Deserialize, Deserializer, Serialize, Serializer};
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

/// One `{uri, digest, size}` entry from a skill's `resources` manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillResource {
    /// Resource URI relative to the skill protocol namespace.
    pub uri: String,
    /// Content digest advertised for integrity verification.
    pub digest: String,
    /// Length in bytes of the raw content covered by `digest`.
    pub size: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DynamicResources {
    Dynamic,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ResourcesWire {
    Manifest(Vec<SkillResource>),
    Dynamic(DynamicResources),
}

fn deserialize_resources<'de, D>(deserializer: D) -> Result<Option<Vec<SkillResource>>, D::Error>
where
    D: Deserializer<'de>,
{
    ResourcesWire::deserialize(deserializer).map(|resources| match resources {
        ResourcesWire::Manifest(resources) => Some(resources),
        ResourcesWire::Dynamic(DynamicResources::Dynamic) => None,
    })
}

fn serialize_resources<S>(
    resources: &Option<Vec<SkillResource>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match resources {
        Some(resources) => resources.serialize(serializer),
        None => DynamicResources::Dynamic.serialize(serializer),
    }
}

/// A skill entry, as carried by both `skills/list` and `skills/get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillEntry {
    /// Full resource URI of the skill's `SKILL.md`.
    pub uri: String,
    /// The `SKILL.md` YAML frontmatter rendered verbatim as a JSON object.
    pub frontmatter: Map<String, Value>,
    /// Complete file manifest, or `"dynamic"` for generated skills that cannot
    /// publish stable content. The wire field itself is always required.
    #[serde(
        deserialize_with = "deserialize_resources",
        serialize_with = "serialize_resources"
    )]
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
    /// Opaque pagination cursor returned by the previous list response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The complete-result marker required on extension result objects.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompleteResultType {
    #[default]
    Complete,
}

/// `skills/list` result.
///
/// A server whose catalog is large, generated, or otherwise unenumerable may
/// return an empty or partial listing, and hosts must not read an empty listing
/// as proof that a server has no skills.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsListResult {
    /// Identifies this as a complete MCP result rather than an input request.
    #[serde(rename = "resultType", default)]
    pub result_type: CompleteResultType,
    /// Skill entries returned on this page.
    pub skills: Vec<SkillEntry>,
    #[serde(
        rename = "nextCursor",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    /// Opaque cursor for the next page, or `None` when pagination is complete.
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
    /// Listing-level metadata. Carries the completeness bookkeeping an agent
    /// needs to know the listing is partial — see [`SkillsListResult::absorb`].
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Map<String, Value>>,
}

/// The strictest cache scope: shareable by any client, gateway, or proxy.
pub const CACHE_SCOPE_PUBLIC: &str = "public";
/// Cache scope for a listing whose contents depend on who asked.
pub const CACHE_SCOPE_PRIVATE: &str = "private";

impl SkillsListResult {
    /// Fold entries from another source into this listing.
    ///
    /// Merging two listings is not concatenation. Each carries freshness and
    /// sharing terms that describe *its own* entries, and the combined listing
    /// can only honor the strictest of them:
    ///
    /// - `cacheScope` collapses to `private` unless every source was `public`.
    ///   Appending per-caller entries to a `public` listing tells every
    ///   downstream cache it may serve one caller's entries to another — the
    ///   exact over-sharing Labby's own per-subject cache sharding exists to
    ///   prevent. A gateway that refuses to over-share internally must not
    ///   instruct its clients to over-share on its behalf.
    /// - `ttlMs` takes the minimum, since the combined listing goes stale as
    ///   soon as its shortest-lived component does.
    ///
    /// Concatenating `skills` directly is what makes this go wrong silently,
    /// so prefer this over extending the field in place.
    pub fn absorb(&mut self, entries: Vec<SkillEntry>, scope: Option<&str>, ttl_ms: Option<u64>) {
        if entries.is_empty() && scope.is_none() && ttl_ms.is_none() {
            return;
        }
        self.skills.extend(entries);

        // Absent is not public: a source that declined to state its terms
        // cannot be assumed to permit the widest sharing.
        let both_public = matches!(self.cache_scope.as_deref(), Some(CACHE_SCOPE_PUBLIC))
            && matches!(scope, Some(CACHE_SCOPE_PUBLIC));
        if !both_public {
            self.cache_scope = Some(CACHE_SCOPE_PRIVATE.to_string());
        }

        self.ttl_ms = match (self.ttl_ms, ttl_ms) {
            (Some(current), Some(incoming)) => Some(current.min(incoming)),
            (current, incoming) => current.or(incoming),
        };
    }

    /// Record that this listing is known to be incomplete.
    ///
    /// The SEP is explicit that an empty or partial listing is never proof a
    /// server has no skills, but a client can only act on that if it is told
    /// which case it is looking at. Without this, a listing missing four
    /// unreachable upstreams is byte-identical to one where those upstreams
    /// genuinely had nothing — and an agent concludes the skill it needs does
    /// not exist.
    pub fn note_incomplete(&mut self, key: &str, value: Value) {
        self.meta
            .get_or_insert_with(Map::new)
            .insert(key.to_string(), value);
    }
}

/// `skills/get` request parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsGetParams {
    /// Full URI of the skill entry to retrieve.
    pub uri: String,
}

/// `skills/get` result.
///
/// The entry is nested under a `skill` key rather than returned flat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillsGetResult {
    /// Identifies this as a complete MCP result rather than an input request.
    #[serde(rename = "resultType", default)]
    pub result_type: CompleteResultType,
    /// Retrieved skill entry.
    pub skill: SkillEntry,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserializes_skills_list_result_from_spec_example() {
        let raw = json!({
            "resultType": "complete",
            "skills": [
                {
                    "uri": "skill://git-workflow/SKILL.md",
                    "frontmatter": {
                        "name": "git-workflow",
                        "description": "Follow this team's Git conventions for branching and commits"
                    },
                    "resources": [
                        { "uri": "skill://git-workflow/SKILL.md", "digest": "sha256:a1b2c3d4", "size": 2314 }
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
                        { "uri": "skill://acme/billing/refunds/SKILL.md", "digest": "sha256:b2c3d4e5", "size": 3871 },
                        { "uri": "skill://acme/billing/refunds/examples/email.md", "digest": "sha256:c3d4e5f6", "size": 962 }
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
            "resultType": "complete",
            "skill": {
                "uri": "skill://pdf-processing/SKILL.md",
                "frontmatter": {
                    "name": "pdf-processing",
                    "description": "Extract, fill, and assemble PDF documents",
                    "metadata": { "version": "2.1.0" }
                },
                "resources": [
                    { "uri": "skill://pdf-processing/SKILL.md", "digest": "sha256:d5e6f7a8", "size": 5120 },
                    { "uri": "skill://pdf-processing/references/FORMS.md", "digest": "sha256:e6f7a8b9", "size": 18433 }
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
        let result: SkillsListResult = serde_json::from_value(json!({
            "resultType": "complete",
            "skills": []
        }))
        .expect("ok");
        assert!(result.skills.is_empty());
        assert!(result.next_cursor.is_none());
    }

    #[test]
    fn dynamic_resources_are_unverifiable_and_round_trip() {
        let entry: SkillEntry = serde_json::from_value(json!({
            "uri": "skill://generated/SKILL.md",
            "frontmatter": { "name": "generated", "description": "d" },
            "resources": "dynamic"
        }))
        .expect("deserializes");
        assert!(entry.is_unverifiable());
        let encoded = serde_json::to_value(&entry).expect("serializes");
        assert_eq!(encoded["resources"], "dynamic");
    }

    #[test]
    fn missing_resources_is_rejected() {
        let result = serde_json::from_value::<SkillEntry>(json!({
            "uri": "skill://generated/SKILL.md",
            "frontmatter": { "name": "generated", "description": "d" }
        }));
        assert!(result.is_err());
    }

    #[test]
    fn missing_skill_payload_fields_are_rejected_but_legacy_result_type_defaults() {
        assert!(serde_json::from_value::<SkillsListResult>(json!({ "skills": [] })).is_ok());
        assert!(
            serde_json::from_value::<SkillsListResult>(json!({ "resultType": "complete" }))
                .is_err()
        );
        assert!(
            serde_json::from_value::<SkillsGetResult>(json!({
                "skill": {
                    "uri": "skill://generated/SKILL.md",
                    "frontmatter": { "name": "generated", "description": "d" },
                    "resources": "dynamic"
                }
            }))
            .is_ok()
        );
    }

    #[test]
    fn unknown_cache_scope_round_trips() {
        let result: SkillsListResult = serde_json::from_value(
            json!({ "resultType": "complete", "skills": [], "cacheScope": "some-future-scope" }),
        )
        .expect("unknown scope tolerated");
        assert_eq!(result.cache_scope.as_deref(), Some("some-future-scope"));
    }

    #[test]
    fn empty_private_absorb_still_downgrades_cache_scope() {
        let mut result = SkillsListResult {
            result_type: CompleteResultType::Complete,
            skills: Vec::new(),
            next_cursor: None,
            ttl_ms: Some(60_000),
            cache_scope: Some(CACHE_SCOPE_PUBLIC.to_string()),
            meta: None,
        };

        result.absorb(Vec::new(), Some(CACHE_SCOPE_PRIVATE), Some(5_000));

        assert_eq!(result.cache_scope.as_deref(), Some(CACHE_SCOPE_PRIVATE));
        assert_eq!(result.ttl_ms, Some(5_000));
    }

    #[test]
    fn absent_empty_source_does_not_downgrade_cache_scope() {
        let mut result = SkillsListResult {
            result_type: CompleteResultType::Complete,
            skills: Vec::new(),
            next_cursor: None,
            ttl_ms: Some(60_000),
            cache_scope: Some(CACHE_SCOPE_PUBLIC.to_string()),
            meta: None,
        };

        result.absorb(Vec::new(), None, None);

        assert_eq!(result.cache_scope.as_deref(), Some(CACHE_SCOPE_PUBLIC));
        assert_eq!(result.ttl_ms, Some(60_000));
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
