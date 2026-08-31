//! Agent Skills compatibility projection into the Artifact family.
//!
//! This adapter consumes the existing validated SEP-2640 manifest plus verified
//! resource bytes. It does not alter Skill URIs, frontmatter, resource bytes, or
//! Skills-over-MCP behavior.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::skills::manifest::{ValidatedSkill, verify_manifest_file};
use crate::skills::uri::parse_skill_resource_uri;

use super::model::{
    ArtifactComponent, ArtifactDescriptor, ArtifactInterchange, ArtifactLicenseState,
    ArtifactLineage, ArtifactProvenance, ArtifactPublication, ArtifactRevision, Distribution,
    JsonMap, PublicationState, Visibility,
};
use super::{ArtifactError, invalid};

/// Project one verified Agent Skill into ArtifactInterchange v1.
///
/// `resources` is keyed by resource URI and contains the exact bytes fetched
/// under the existing Skills contract. Every manifest entry is reverified before
/// it becomes an Artifact component.
pub fn interchange_from_validated_skill(
    skill: &ValidatedSkill,
    resources: &BTreeMap<String, Vec<u8>>,
    mut provenance: ArtifactProvenance,
) -> Result<ArtifactInterchange, ArtifactError> {
    let manifest = skill
        .entry
        .resources
        .as_ref()
        .ok_or(ArtifactError::SkillVerification)?;
    let (skill_path, _) = skill
        .uri
        .skill_md_parts()
        .ok_or(ArtifactError::SkillVerification)?;
    let root = format!("{skill_path}/");

    let mut components = Vec::with_capacity(manifest.len());
    for resource in manifest {
        let parsed = parse_skill_resource_uri(&resource.uri)
            .map_err(|_| ArtifactError::SkillVerification)?;
        let canonical_uri = parsed.to_uri();
        let bytes = resources
            .get(&canonical_uri)
            .or_else(|| resources.get(&resource.uri))
            .ok_or(ArtifactError::SkillVerification)?;
        verify_manifest_file(skill, &resource.uri, bytes)
            .map_err(|_| ArtifactError::SkillVerification)?;
        let relative = parsed
            .full_path()
            .strip_prefix(&root)
            .ok_or(ArtifactError::SkillVerification)?;
        components.push(ArtifactComponent::from_bytes(relative, bytes, None)?);
    }

    let revision = ArtifactRevision::from_components(components, None, None, None, JsonMap::new())?;
    let mut descriptor = ArtifactDescriptor::for_source_identity(
        "skill",
        skill.uri.origin(),
        &skill.name,
        &skill.uri.to_uri(),
    )?;
    descriptor.title = frontmatter_string(skill, "title", 256);
    descriptor.description = frontmatter_string(skill, "description", 4_096);
    descriptor.metadata.insert(
        "compatibility".to_string(),
        Value::String("agent-skills".to_string()),
    );

    if provenance.adapter.is_none() {
        provenance.adapter = Some("labby.skill-compat/v1".to_string());
    }
    if provenance.original_format.is_none() {
        provenance.original_format = Some("agent-skill".to_string());
    }

    let publication = ArtifactPublication {
        state: PublicationState::Listed,
        visibility: Visibility::Private,
        distribution: Distribution::Metadata,
        ..ArtifactPublication::default()
    };
    let mut materialization_hints = JsonMap::new();
    materialization_hints.insert(
        "preferredPath".to_string(),
        Value::String(format!("skills/{}", skill.name)),
    );

    let interchange = ArtifactInterchange {
        schema_version: super::model::ARTIFACT_INTERCHANGE_SCHEMA.to_string(),
        descriptor,
        revision,
        provenance,
        license: ArtifactLicenseState::default(),
        lineage: ArtifactLineage::default(),
        publication,
        downloads: Vec::new(),
        materialization_hints,
    };
    interchange.validate()?;
    Ok(interchange)
}

fn frontmatter_string(skill: &ValidatedSkill, key: &str, max: usize) -> Option<String> {
    let value = skill.entry.frontmatter.get(key)?.as_str()?;
    if value.len() <= max && !value.contains('\0') {
        Some(value.to_string())
    } else {
        None
    }
}

/// Derive the relative path represented by a validated Skill resource URI.
pub fn skill_resource_relative_path(
    skill: &ValidatedSkill,
    resource_uri: &str,
) -> Result<String, ArtifactError> {
    let (skill_path, _) = skill
        .uri
        .skill_md_parts()
        .ok_or(ArtifactError::SkillVerification)?;
    let parsed =
        parse_skill_resource_uri(resource_uri).map_err(|_| ArtifactError::SkillVerification)?;
    if parsed.scheme() != skill.uri.scheme() {
        return Err(ArtifactError::SkillVerification);
    }
    let relative = parsed
        .full_path()
        .strip_prefix(&format!("{skill_path}/"))
        .ok_or(ArtifactError::SkillVerification)?;
    if relative.is_empty() {
        return Err(invalid("skill_resource", "empty_path"));
    }
    super::validation::validate_relative_path(relative)?;
    Ok(relative.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::digest::ResourceDigest;
    use crate::skills::manifest::validate_skill_entry;
    use crate::skills::wire::{SkillEntry, SkillResource};
    use serde_json::json;

    fn resource(uri: &str, bytes: &[u8]) -> SkillResource {
        SkillResource {
            uri: uri.to_string(),
            digest: ResourceDigest::of_bytes(bytes).to_wire(),
            size: bytes.len() as u64,
        }
    }

    #[test]
    fn skill_projection_preserves_manifest_paths_and_digests() {
        let skill_md = b"---\nname: demo\ndescription: d\n---\nbody\n";
        let reference = b"reference";
        let entry = SkillEntry {
            uri: "skill://labby/demo/SKILL.md".to_string(),
            frontmatter: json!({"name":"demo","description":"d"})
                .as_object()
                .unwrap()
                .clone(),
            resources: Some(vec![
                resource("skill://labby/demo/SKILL.md", skill_md),
                resource("skill://labby/demo/references/REF.md", reference),
            ]),
            meta: None,
        };
        let skill = validate_skill_entry(&entry).unwrap();
        let resources = BTreeMap::from([
            (entry.uri.clone(), skill_md.to_vec()),
            (
                "skill://labby/demo/references/REF.md".to_string(),
                reference.to_vec(),
            ),
        ]);
        let artifact =
            interchange_from_validated_skill(&skill, &resources, ArtifactProvenance::default())
                .unwrap();
        assert_eq!(artifact.descriptor.kind, "skill");
        assert_eq!(artifact.descriptor.name, "demo");
        assert_eq!(artifact.revision.components.len(), 2);
        assert!(
            artifact
                .revision
                .components
                .iter()
                .any(|file| file.path == "SKILL.md")
        );
        assert!(
            artifact
                .revision
                .components
                .iter()
                .any(|file| file.path == "references/REF.md")
        );
    }

    #[test]
    fn nested_skills_with_the_same_name_have_distinct_artifact_ids() {
        let skill_md = b"---\nname: demo\ndescription: d\n---\nbody\n";
        let mut ids = Vec::new();
        for uri in [
            "skill://labby/team-a/demo/SKILL.md",
            "skill://labby/team-b/demo/SKILL.md",
        ] {
            let entry = SkillEntry {
                uri: uri.to_string(),
                frontmatter: json!({"name":"demo","description":"d"})
                    .as_object()
                    .unwrap()
                    .clone(),
                resources: Some(vec![resource(uri, skill_md)]),
                meta: None,
            };
            let skill = validate_skill_entry(&entry).unwrap();
            let resources = BTreeMap::from([(uri.to_string(), skill_md.to_vec())]);
            let artifact =
                interchange_from_validated_skill(&skill, &resources, ArtifactProvenance::default())
                    .unwrap();
            ids.push(artifact.descriptor.id);
        }
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn invalid_provenance_version_is_not_silently_rewritten() {
        let skill_md = b"---\nname: demo\ndescription: d\n---\nbody\n";
        let uri = "skill://labby/demo/SKILL.md";
        let entry = SkillEntry {
            uri: uri.to_string(),
            frontmatter: json!({"name":"demo","description":"d"})
                .as_object()
                .unwrap()
                .clone(),
            resources: Some(vec![resource(uri, skill_md)]),
            meta: None,
        };
        let skill = validate_skill_entry(&entry).unwrap();
        let resources = BTreeMap::from([(uri.to_string(), skill_md.to_vec())]);
        let provenance = ArtifactProvenance {
            schema_version: 0,
            ..ArtifactProvenance::default()
        };
        assert!(
            interchange_from_validated_skill(&skill, &resources, provenance).is_err(),
            "invalid provenance must fail closed"
        );
    }
}
