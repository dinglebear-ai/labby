//! Artifact lifecycle planning and revision comparison primitives.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::model::{
    ArtifactComponent, ArtifactProvenance, ArtifactRecord, ArtifactRevision, JsonMap,
};
use super::validation;
use super::{ArtifactError, invalid};

const fn schema_one() -> u8 {
    1
}

/// Semantic classification for one path-level revision change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactChangeKind {
    /// The path exists only in the target revision.
    Added,
    /// The path exists only in the base revision.
    Removed,
    /// The path exists in both revisions but its component contract changed.
    Modified,
}

/// One deterministic path-level change between two revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactComponentChange {
    /// Normalized package path.
    pub path: String,
    /// Change classification.
    pub kind: ArtifactChangeKind,
    /// Base component when the path existed in the base revision.
    pub before: Option<ArtifactComponent>,
    /// Target component when the path exists in the target revision.
    pub after: Option<ArtifactComponent>,
}

/// Deterministic semantic diff between two immutable Artifact revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRevisionDiff {
    /// Base revision identifier.
    pub from_revision_id: String,
    /// Target revision identifier.
    pub to_revision_id: String,
    /// Changes ordered lexicographically by normalized path.
    #[serde(default)]
    pub changes: Vec<ArtifactComponentChange>,
}

impl ArtifactRevisionDiff {
    /// Compare two validated revisions without mutating either revision or a store.
    pub fn between(from: &ArtifactRevision, to: &ArtifactRevision) -> Result<Self, ArtifactError> {
        validation::validate_revision(from)?;
        from.verify_content_digest()?;
        validation::validate_revision(to)?;
        to.verify_content_digest()?;

        let before = components_by_path(&from.components)?;
        let after = components_by_path(&to.components)?;
        let paths = before
            .keys()
            .chain(after.keys())
            .copied()
            .collect::<BTreeSet<_>>();
        let mut changes = Vec::new();

        for path in paths {
            match (before.get(path), after.get(path)) {
                (None, Some(component)) => changes.push(ArtifactComponentChange {
                    path: path.to_string(),
                    kind: ArtifactChangeKind::Added,
                    before: None,
                    after: Some((*component).clone()),
                }),
                (Some(component), None) => changes.push(ArtifactComponentChange {
                    path: path.to_string(),
                    kind: ArtifactChangeKind::Removed,
                    before: Some((*component).clone()),
                    after: None,
                }),
                (Some(left), Some(right)) if left != right => {
                    changes.push(ArtifactComponentChange {
                        path: path.to_string(),
                        kind: ArtifactChangeKind::Modified,
                        before: Some((*left).clone()),
                        after: Some((*right).clone()),
                    });
                }
                (Some(_), Some(_)) => {}
                (None, None) => unreachable!("path came from at least one revision"),
            }
        }

        Ok(Self {
            from_revision_id: from.id.clone(),
            to_revision_id: to.id.clone(),
            changes,
        })
    }

    /// Whether the two revisions have identical component contracts.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

fn components_by_path(
    components: &[ArtifactComponent],
) -> Result<BTreeMap<&str, &ArtifactComponent>, ArtifactError> {
    let mut by_path = BTreeMap::new();
    for component in components {
        if by_path.insert(component.path.as_str(), component).is_some() {
            return Err(invalid("components", "duplicate_path"));
        }
    }
    Ok(by_path)
}

/// Metadata attached when committing the editable workspace as a revision.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactWorkspaceSnapshotRequest {
    /// Optional RFC3339 authored timestamp.
    pub authored_at: Option<String>,
    /// Optional bounded revision message.
    pub message: Option<String>,
    /// Bounded local revision metadata.
    #[serde(default)]
    pub metadata: JsonMap,
}

/// Result of snapshotting one editable workspace into the immutable store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactWorkspaceSnapshot {
    /// Updated local head record.
    pub record: ArtifactRecord,
    /// Immutable revision selected by the snapshot.
    pub revision: ArtifactRevision,
    /// True only when a previously unseen immutable revision was persisted.
    pub created_revision: bool,
    /// True when the mutable head changed to a different revision.
    pub moved_head: bool,
}

/// Explicit, non-applying update plan derived from an acquired source revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactUpdatePlan {
    /// Domain schema version.
    #[serde(default = "schema_one")]
    pub schema_version: u8,
    /// Local Artifact whose head was used as the plan base.
    pub target_artifact_id: String,
    /// Exact local head revision observed while planning.
    pub base_revision_id: String,
    /// Canonical source Artifact identity supplied by the provider.
    pub source_artifact_id: String,
    /// Exact source revision proposed by the provider.
    pub source_revision_id: String,
    /// Canonical source provenance supplied by the provider.
    pub source_provenance: ArtifactProvenance,
    /// Deterministic component diff from local base to proposed source revision.
    pub diff: ArtifactRevisionDiff,
}

impl ArtifactUpdatePlan {
    /// Validate plan identity and cross-field invariants.
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != 1 {
            return Err(ArtifactError::UnsupportedSchema);
        }
        validation::validate_id(&self.target_artifact_id, "target_artifact_id")?;
        validation::validate_reference_id(&self.base_revision_id, "base_revision_id")?;
        validation::validate_id(&self.source_artifact_id, "source_artifact_id")?;
        validation::validate_reference_id(&self.source_revision_id, "source_revision_id")?;
        validation::validate_provenance(&self.source_provenance)?;
        if self.diff.from_revision_id != self.base_revision_id {
            return Err(invalid("diff", "base_revision_mismatch"));
        }
        if self.diff.to_revision_id != self.source_revision_id {
            return Err(invalid("diff", "source_revision_mismatch"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision(files: &[(&str, &[u8])]) -> ArtifactRevision {
        ArtifactRevision::from_components(
            files
                .iter()
                .map(|(path, bytes)| ArtifactComponent::from_bytes(path, bytes, None).unwrap())
                .collect(),
            None,
            None,
            None,
            JsonMap::new(),
        )
        .unwrap()
    }

    #[test]
    fn revision_diff_is_path_ordered_and_classifies_changes() {
        let from = revision(&[("b.txt", b"old"), ("removed.txt", b"gone")]);
        let to = revision(&[("a.txt", b"new"), ("b.txt", b"changed")]);
        let diff = ArtifactRevisionDiff::between(&from, &to).unwrap();
        assert_eq!(
            diff.changes
                .iter()
                .map(|change| (change.path.as_str(), change.kind))
                .collect::<Vec<_>>(),
            vec![
                ("a.txt", ArtifactChangeKind::Added),
                ("b.txt", ArtifactChangeKind::Modified),
                ("removed.txt", ArtifactChangeKind::Removed),
            ]
        );
    }

    #[test]
    fn revision_diff_is_empty_for_equivalent_component_sets() {
        let left = revision(&[("a.txt", b"a"), ("b.txt", b"b")]);
        let right = ArtifactRevision::from_components(
            left.components.iter().rev().cloned().collect(),
            None,
            None,
            None,
            JsonMap::new(),
        )
        .unwrap();
        assert!(
            ArtifactRevisionDiff::between(&left, &right)
                .unwrap()
                .is_empty()
        );
    }
}
