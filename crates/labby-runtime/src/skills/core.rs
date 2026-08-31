//! Provider-neutral Agent Skill identity and compact discovery vocabulary.
//!
//! Agent Skills is the package/content model. SEP-2640 is one transport that
//! can supply such packages. These types deliberately carry no MCP capability,
//! pagination, cache, or JSON-RPC fields so bundled, local, Artifact-backed,
//! and future registry providers can describe the same logical object.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use super::requirements::SkillRequirementsSummary;
use super::{
    SkillAvailabilitySummary, SkillCompatibilityClassification, SkillCompatibilityItem,
    ValidatedSkill,
};

/// The provider family responsible for discovering and reading a Skill.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillProviderKind {
    Bundled,
    OperatorLocal,
    McpUpstream,
    ArtifactStore,
    Registry,
    Other(String),
}

/// Host-scoped identity of one provider instance.
///
/// `instance` is assigned by Labby configuration, not accepted as an assertion
/// of trust from the provider. For MCP it is the host-assigned upstream name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SkillProviderId {
    kind: SkillProviderKind,
    instance: String,
}

impl SkillProviderId {
    #[must_use]
    pub fn new(kind: SkillProviderKind, instance: impl Into<String>) -> Self {
        let instance = instance.into();
        assert!(!instance.is_empty(), "provider instance must not be empty");
        Self { kind, instance }
    }

    #[must_use]
    pub fn kind(&self) -> &SkillProviderKind {
        &self.kind
    }

    #[must_use]
    pub fn instance(&self) -> &str {
        &self.instance
    }
}

impl<'de> Deserialize<'de> for SkillProviderId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            kind: SkillProviderKind,
            instance: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        if wire.instance.is_empty() {
            return Err(serde::de::Error::custom(
                "provider instance must not be empty",
            ));
        }
        Ok(Self::new(wire.kind, wire.instance))
    }
}

/// Canonical identity of one Skill within Labby.
///
/// `source_id` is opaque provider-native identity. For SEP-2640 it is the URI
/// published by the originating server. It is never replaced by a display name
/// or inferred from a URI scheme.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct SkillId {
    provider: SkillProviderId,
    source_id: String,
}

impl SkillId {
    #[must_use]
    pub fn new(provider: SkillProviderId, source_id: impl Into<String>) -> Self {
        let source_id = source_id.into();
        assert!(!source_id.is_empty(), "skill source id must not be empty");
        Self {
            provider,
            source_id,
        }
    }

    #[must_use]
    pub fn provider(&self) -> &SkillProviderId {
        &self.provider
    }

    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
}

impl<'de> Deserialize<'de> for SkillId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            provider: SkillProviderId,
            source_id: String,
        }

        let wire = Wire::deserialize(deserializer)?;
        if wire.source_id.is_empty() {
            return Err(serde::de::Error::custom(
                "skill source id must not be empty",
            ));
        }
        Ok(Self::new(wire.provider, wire.source_id))
    }
}

/// Compact provider-neutral metadata used for discovery and policy evaluation.
///
/// This descriptor intentionally excludes `SKILL.md` and supporting-file
/// bodies. Providers fetch those lazily when the Skill activates or a verified
/// file is read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillDescriptor {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    /// Source-native URI when the provider has one. This is address metadata,
    /// not global identity and not a trust signal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_uri: Option<String>,
    pub resource_count: usize,
    /// Fail-closed compatibility result. Callers must not offer descriptors
    /// whose availability is blocked.
    pub availability: SkillAvailabilitySummary,
    /// Source-authored activation requirements. Tool hints are never execution
    /// authorization and remain scoped to the source provider's context.
    #[serde(default, skip_serializing_if = "SkillRequirementsSummary::is_empty")]
    pub requirements: SkillRequirementsSummary,
    /// Provider metadata preserved outside author frontmatter. Adapters must not
    /// reinterpret permission-like vendor fields as Labby authorization.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub provider_metadata: Map<String, Value>,
}

impl SkillDescriptor {
    /// Adapt an already validated Skill entry without fetching package bytes.
    #[must_use]
    pub fn from_validated_entry(provider: SkillProviderId, skill: &ValidatedSkill) -> Self {
        let description = skill
            .entry
            .frontmatter
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let availability = SkillAvailabilitySummary::from_items(
            skill
                .entry
                .frontmatter
                .contains_key("allowed-tools")
                .then(|| {
                    SkillCompatibilityItem::new(
                        "allowed-tools",
                        SkillCompatibilityClassification::PreservedHint,
                    )
                    .with_detail("preserved as source metadata; never grants tool access")
                }),
        );
        let requirements = SkillRequirementsSummary::from_frontmatter(&skill.entry.frontmatter);
        Self {
            id: SkillId::new(provider, skill.entry.uri.clone()),
            name: skill.name.clone(),
            description,
            source_uri: Some(skill.entry.uri.clone()),
            resource_count: skill.entry.resources.as_ref().map_or(0, Vec::len),
            availability,
            requirements,
            provider_metadata: skill.entry.meta.clone().unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::{ResourceDigest, SkillEntry, SkillResource, validate_skill_entry};
    use serde_json::json;

    fn validated(uri: &str, name: &str) -> ValidatedSkill {
        let body = format!("---\nname: {name}\ndescription: demo\n---\nbody\n");
        let entry = SkillEntry {
            uri: uri.to_string(),
            frontmatter: json!({"name": name, "description": "demo"})
                .as_object()
                .expect("object")
                .clone(),
            resources: Some(vec![SkillResource {
                uri: uri.to_string(),
                digest: ResourceDigest::of_bytes(body.as_bytes()).to_wire(),
                size: body.len() as u64,
            }]),
            meta: Some(
                json!({"vendor.example/hint": "preserved"})
                    .as_object()
                    .expect("object")
                    .clone(),
            ),
        };
        validate_skill_entry(&entry).expect("validated skill")
    }

    #[test]
    fn same_source_id_from_two_providers_never_collides() {
        let source_id = "skill://shared/review/SKILL.md";
        let left = SkillId::new(
            SkillProviderId::new(SkillProviderKind::McpUpstream, "depot"),
            source_id,
        );
        let right = SkillId::new(
            SkillProviderId::new(SkillProviderKind::McpUpstream, "other"),
            source_id,
        );

        assert_ne!(left, right);
    }

    #[test]
    fn identity_deserialization_rejects_empty_components() {
        assert!(
            serde_json::from_value::<SkillProviderId>(json!({
                "kind": "bundled",
                "instance": ""
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SkillId>(json!({
                "provider": {"kind": "bundled", "instance": "built-in"},
                "source_id": ""
            }))
            .is_err()
        );
    }

    #[test]
    fn validated_adapter_is_metadata_only_and_preserves_provider_metadata() {
        let skill = validated("skill://catalog/review/SKILL.md", "review");
        let provider = SkillProviderId::new(SkillProviderKind::McpUpstream, "depot");

        let descriptor = SkillDescriptor::from_validated_entry(provider.clone(), &skill);

        assert_eq!(descriptor.id.provider(), &provider);
        assert_eq!(descriptor.id.source_id(), skill.entry.uri);
        assert_eq!(descriptor.name, "review");
        assert_eq!(descriptor.description, "demo");
        assert_eq!(descriptor.resource_count, 1);
        assert!(descriptor.availability.is_available());
        assert_eq!(
            descriptor.provider_metadata["vendor.example/hint"],
            "preserved"
        );
    }

    #[test]
    fn descriptor_json_keeps_provider_and_source_identity_explicit() {
        let skill = validated("skill://catalog/review/SKILL.md", "review");
        let descriptor = SkillDescriptor::from_validated_entry(
            SkillProviderId::new(SkillProviderKind::McpUpstream, "depot"),
            &skill,
        );

        let json = serde_json::to_value(descriptor).expect("descriptor JSON");
        assert_eq!(json["id"]["provider"]["kind"], "mcp_upstream");
        assert_eq!(json["id"]["provider"]["instance"], "depot");
        assert_eq!(json["id"]["source_id"], "skill://catalog/review/SKILL.md");
        assert!(json.get("instructions").is_none());
    }

    #[test]
    fn descriptor_projects_requirements_without_authorizing_tool_hints() {
        let uri = "skill://catalog/review/SKILL.md";
        let body = "---\nname: review\ndescription: demo\ncompatibility: Requires git\nallowed-tools: Read Grep\n---\nbody\n";
        let entry = SkillEntry {
            uri: uri.to_string(),
            frontmatter: json!({
                "name": "review",
                "description": "demo",
                "compatibility": "Requires git",
                "allowed-tools": "Read Grep"
            })
            .as_object()
            .expect("object")
            .clone(),
            resources: Some(vec![SkillResource {
                uri: uri.to_string(),
                digest: ResourceDigest::of_bytes(body.as_bytes()).to_wire(),
                size: body.len() as u64,
            }]),
            meta: None,
        };
        let skill = validate_skill_entry(&entry).expect("validated skill");
        let descriptor = SkillDescriptor::from_validated_entry(
            SkillProviderId::new(SkillProviderKind::McpUpstream, "depot"),
            &skill,
        );

        assert_eq!(
            descriptor.requirements.compatibility.as_deref(),
            Some("Requires git")
        );
        assert_eq!(descriptor.requirements.tool_hints, ["Read", "Grep"]);
        assert_eq!(
            descriptor.availability.items()[0].classification,
            SkillCompatibilityClassification::PreservedHint
        );
        let json = serde_json::to_value(descriptor).expect("descriptor JSON");
        assert!(json.get("authorized").is_none());
        assert!(json["requirements"].get("allowed_tools").is_none());
    }

    #[test]
    #[should_panic(expected = "provider instance must not be empty")]
    fn provider_identity_rejects_empty_instance() {
        drop(SkillProviderId::new(SkillProviderKind::Bundled, ""));
    }

    #[test]
    #[should_panic(expected = "skill source id must not be empty")]
    fn skill_identity_rejects_empty_source_id() {
        let provider = SkillProviderId::new(SkillProviderKind::Bundled, "labby");
        drop(SkillId::new(provider, ""));
    }
}
