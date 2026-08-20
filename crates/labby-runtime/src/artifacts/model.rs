//! ArtifactInterchange v1 and local Artifact domain types.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::canonical_json;
use super::validation::{self, MAX_COMPONENTS, MAX_FILE_BYTES, MAX_REVISIONS_PER_ARTIFACT};
use super::{ArtifactError, invalid};

/// Frozen portable interchange schema identifier.
pub const ARTIFACT_INTERCHANGE_SCHEMA: &str = "dinglebear.artifact-interchange/v1";

/// Deterministically ordered JSON extension map.
pub type JsonMap = BTreeMap<String, Value>;

const fn schema_one() -> u8 {
    1
}

fn interchange_schema() -> String {
    ARTIFACT_INTERCHANGE_SCHEMA.to_string()
}

/// Stable Artifact identity and human-facing metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDescriptor {
    /// Domain schema version.
    #[serde(default = "schema_one")]
    pub schema_version: u8,
    /// Stable opaque Artifact ID.
    pub id: String,
    /// Stable lower-case Artifact family. Skills use `skill`.
    pub kind: String,
    /// Publisher/local namespace.
    pub namespace: String,
    /// Stable lookup name.
    pub name: String,
    /// Optional display title.
    pub title: Option<String>,
    /// Optional bounded description.
    pub description: Option<String>,
    /// Unique bounded tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Bounded extension metadata.
    #[serde(default)]
    pub metadata: JsonMap,
}

impl ArtifactDescriptor {
    /// Build a deterministic local identity using the same v1 identity seed as Depot.
    pub fn for_identity(kind: &str, namespace: &str, name: &str) -> Result<Self, ArtifactError> {
        let seed = json!({"kind": kind, "namespace": namespace, "name": name});
        Self::from_identity_seed(kind, namespace, name, &seed)
    }

    /// Build a stable identity keyed by an immutable source identifier.
    ///
    /// Compatibility adapters use this when the human namespace/name pair is
    /// not globally unique. The descriptor fields stay human-readable while
    /// the opaque Artifact ID remains collision-resistant across source paths.
    pub fn for_source_identity(
        kind: &str,
        namespace: &str,
        name: &str,
        source_identity: &str,
    ) -> Result<Self, ArtifactError> {
        let seed = json!({"kind": kind, "sourceIdentity": source_identity});
        Self::from_identity_seed(kind, namespace, name, &seed)
    }

    fn from_identity_seed(
        kind: &str,
        namespace: &str,
        name: &str,
        seed: &Value,
    ) -> Result<Self, ArtifactError> {
        let digest = canonical_json::digest(seed)?;
        let id = format!(
            "art_{}",
            digest
                .strip_prefix("sha256:")
                .ok_or_else(|| invalid("artifact_id", "invalid_digest"))?
        );
        let descriptor = Self {
            schema_version: 1,
            id,
            kind: kind.to_string(),
            namespace: namespace.to_string(),
            name: name.to_string(),
            title: None,
            description: None,
            tags: Vec::new(),
            metadata: JsonMap::new(),
        };
        validation::validate_descriptor(&descriptor)?;
        Ok(descriptor)
    }
}

/// Component execution-risk classification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionRisk {
    /// Risk has not been classified.
    #[default]
    Unknown,
    /// Non-executable content.
    Passive,
    /// Content is directly executable.
    Executable,
    /// Content requires elevated authority.
    Privileged,
    /// Content is intentionally classified as dangerous.
    Dangerous,
}

/// One immutable file/logical component in a revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactComponent {
    /// Domain schema version.
    #[serde(default = "schema_one")]
    pub schema_version: u8,
    /// Stable package-local component ID.
    pub id: String,
    /// Component kind. Local package files use `file`.
    pub kind: String,
    /// Normalized forward-slash relative path.
    pub path: String,
    /// SHA-256 digest of file bytes.
    pub digest: String,
    /// File size in bytes.
    pub size: u64,
    /// Optional media type. Kept as explicit null on the wire when unknown.
    pub media_type: Option<String>,
    /// Bounded extension metadata.
    #[serde(default)]
    pub metadata: JsonMap,
    /// Bounded portable dependency evidence.
    #[serde(default)]
    pub dependencies: Vec<Value>,
    /// Bounded portable requirement evidence.
    #[serde(default)]
    pub requirements: Vec<Value>,
    /// Execution-risk classification.
    #[serde(default)]
    pub execution_risk: ExecutionRisk,
}

impl ArtifactComponent {
    /// Build a file component from verified local bytes.
    pub fn from_bytes(
        path: &str,
        bytes: &[u8],
        unix_mode: Option<u32>,
    ) -> Result<Self, ArtifactError> {
        validation::validate_relative_path(path)?;
        let size = u64::try_from(bytes.len()).map_err(|_| ArtifactError::LimitExceeded {
            what: "file_size",
            limit: MAX_FILE_BYTES,
        })?;
        if size > MAX_FILE_BYTES {
            return Err(ArtifactError::LimitExceeded {
                what: "file_size",
                limit: MAX_FILE_BYTES,
            });
        }
        let id_digest = canonical_json::digest(&json!({"path": path}))?;
        let mut metadata = JsonMap::new();
        let safe_mode = unix_mode.map(|mode| mode & 0o0755);
        if let Some(mode) = safe_mode {
            metadata.insert("unixMode".to_string(), Value::from(mode));
        }
        let execution_risk = if safe_mode.is_some_and(|mode| mode & 0o0111 != 0) {
            ExecutionRisk::Executable
        } else {
            ExecutionRisk::Passive
        };
        let component = Self {
            schema_version: 1,
            id: format!(
                "cmp_{}",
                id_digest
                    .strip_prefix("sha256:")
                    .ok_or_else(|| invalid("component_id", "invalid_digest"))?
            ),
            kind: "file".to_string(),
            path: path.to_string(),
            digest: canonical_json::sha256_bytes(bytes),
            size,
            media_type: media_type(path).map(str::to_string),
            metadata,
            dependencies: Vec::new(),
            requirements: Vec::new(),
            execution_risk,
        };
        validation::validate_component(&component)?;
        Ok(component)
    }

    /// Return the stored safe Unix mode, if one was captured.
    #[must_use]
    pub fn unix_mode(&self) -> Option<u32> {
        self.metadata
            .get("unixMode")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .map(|mode| mode & 0o0755)
    }
}

/// One immutable content-addressed Artifact revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRevision {
    /// Domain schema version.
    #[serde(default = "schema_one")]
    pub schema_version: u8,
    /// Immutable revision ID. V1 defaults to `content_digest`.
    pub id: String,
    /// Canonical SHA-256 digest of the normalized component inventory.
    pub content_digest: String,
    /// Ordered component inventory.
    pub components: Vec<ArtifactComponent>,
    /// Optional parent revision.
    pub parent_revision_id: Option<String>,
    /// Optional RFC3339 authored timestamp.
    pub authored_at: Option<String>,
    /// Optional semantic revision message.
    pub message: Option<String>,
    /// Bounded extension metadata.
    #[serde(default)]
    pub metadata: JsonMap,
}

impl ArtifactRevision {
    /// Create a revision and derive its portable content digest/ID.
    pub fn from_components(
        mut components: Vec<ArtifactComponent>,
        parent_revision_id: Option<String>,
        authored_at: Option<String>,
        message: Option<String>,
        metadata: JsonMap,
    ) -> Result<Self, ArtifactError> {
        if components.is_empty() || components.len() > MAX_COMPONENTS {
            return Err(invalid("components", "invalid_count"));
        }
        components.sort_by(|left, right| (&left.path, &left.id).cmp(&(&right.path, &right.id)));
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for component in &components {
            validation::validate_component(component)?;
            if !ids.insert(component.id.as_str()) || !paths.insert(component.path.as_str()) {
                return Err(invalid("components", "duplicate"));
            }
        }
        let content_digest = canonical_json::digest(&components)?;
        if parent_revision_id.as_deref() == Some(content_digest.as_str()) {
            return Err(invalid("parent_revision_id", "cycle"));
        }
        let revision = Self {
            schema_version: 1,
            id: content_digest.clone(),
            content_digest,
            components,
            parent_revision_id,
            authored_at,
            message,
            metadata,
        };
        validation::validate_revision(&revision)?;
        Ok(revision)
    }

    /// Recalculate and verify the canonical revision content digest.
    ///
    /// V1 defaults the revision ID to this digest but permits a caller-supplied
    /// opaque/reference ID, matching Depot's frozen contract.
    pub fn verify_content_digest(&self) -> Result<(), ArtifactError> {
        let mut components = self.components.clone();
        components.sort_by(|left, right| (&left.path, &left.id).cmp(&(&right.path, &right.id)));
        let calculated = canonical_json::digest(&components)?;
        if calculated != self.content_digest {
            return Err(ArtifactError::Conflict("revision_digest_mismatch"));
        }
        Ok(())
    }
}

/// Portable provenance evidence. It is evidence, not a trust decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactProvenance {
    /// Domain schema version.
    #[serde(default = "schema_one")]
    pub schema_version: u8,
    /// Source/provider family.
    pub provider: Option<String>,
    /// Canonical source URI.
    pub source_uri: Option<String>,
    /// Registry/catalog identifier.
    pub registry: Option<String>,
    /// Repository identifier.
    pub repository: Option<String>,
    /// Immutable or observed source ref.
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    /// Relative source path.
    pub source_path: Option<String>,
    /// Source snapshot digest.
    pub source_digest: Option<String>,
    /// Observation timestamp.
    pub observed_at: Option<String>,
    /// Normalizing adapter/version.
    pub adapter: Option<String>,
    /// Original source format.
    pub original_format: Option<String>,
    /// Original format version.
    pub original_version: Option<String>,
    /// Bounded integrity evidence.
    #[serde(default)]
    pub integrity_evidence: JsonMap,
    /// Bounded extension metadata.
    #[serde(default)]
    pub metadata: JsonMap,
}

impl Default for ArtifactProvenance {
    fn default() -> Self {
        Self {
            schema_version: 1,
            provider: None,
            source_uri: None,
            registry: None,
            repository: None,
            reference: None,
            source_path: None,
            source_digest: None,
            observed_at: None,
            adapter: None,
            original_format: None,
            original_version: None,
            integrity_evidence: JsonMap::new(),
            metadata: JsonMap::new(),
        }
    }
}

/// Redistribution classification, independent from trust and integrity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Redistribution {
    MetadataOnly,
    CacheForIndex,
    Redistributable,
    Forkable,
    Restricted,
    #[default]
    Unknown,
}

/// Human/legal review state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    #[default]
    Unreviewed,
    Reviewed,
    Disputed,
}

/// Takedown state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TakedownState {
    #[default]
    None,
    Requested,
    Restricted,
    Removed,
}

/// Explicit license and redistribution evidence/state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactLicenseState {
    #[serde(default = "schema_one")]
    pub schema_version: u8,
    pub declared: Option<String>,
    #[serde(default)]
    pub detected: Vec<Value>,
    #[serde(default)]
    pub notices: Vec<Value>,
    #[serde(default)]
    pub redistribution: Redistribution,
    #[serde(default)]
    pub review_state: ReviewState,
    #[serde(default)]
    pub takedown_state: TakedownState,
    pub evidence_at: Option<String>,
    #[serde(default)]
    pub metadata: JsonMap,
}

impl Default for ArtifactLicenseState {
    fn default() -> Self {
        Self {
            schema_version: 1,
            declared: None,
            detected: Vec::new(),
            notices: Vec::new(),
            redistribution: Redistribution::Unknown,
            review_state: ReviewState::Unreviewed,
            takedown_state: TakedownState::None,
            evidence_at: None,
            metadata: JsonMap::new(),
        }
    }
}

/// Fork and upstream-following lineage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactLineage {
    #[serde(default = "schema_one")]
    pub schema_version: u8,
    pub upstream_artifact_id: Option<String>,
    pub upstream_revision_id: Option<String>,
    pub forked_from_artifact_id: Option<String>,
    pub forked_from_revision_id: Option<String>,
    pub forked_at: Option<String>,
    #[serde(default)]
    pub following: bool,
    pub last_observed_upstream_revision_id: Option<String>,
    #[serde(default)]
    pub metadata: JsonMap,
}

impl Default for ArtifactLineage {
    fn default() -> Self {
        Self {
            schema_version: 1,
            upstream_artifact_id: None,
            upstream_revision_id: None,
            forked_from_artifact_id: None,
            forked_from_revision_id: None,
            forked_at: None,
            following: false,
            last_observed_upstream_revision_id: None,
            metadata: JsonMap::new(),
        }
    }
}

/// Portable publication lifecycle state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublicationState {
    #[default]
    Draft,
    Listed,
    Published,
    Withdrawn,
}

/// Portable visibility state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    #[default]
    Private,
    Unlisted,
    Public,
}

/// Whether publication exposes only metadata or byte payloads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Distribution {
    #[default]
    Metadata,
    Bytes,
}

/// Portable publication metadata. Local Labby defaults to private metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPublication {
    #[serde(default = "schema_one")]
    pub schema_version: u8,
    #[serde(default)]
    pub state: PublicationState,
    #[serde(default)]
    pub visibility: Visibility,
    #[serde(default)]
    pub distribution: Distribution,
    pub published_at: Option<String>,
    pub withdrawn_at: Option<String>,
    #[serde(default)]
    pub metadata: JsonMap,
}

impl Default for ArtifactPublication {
    fn default() -> Self {
        Self {
            schema_version: 1,
            state: PublicationState::Draft,
            visibility: Visibility::Private,
            distribution: Distribution::Metadata,
            published_at: None,
            withdrawn_at: None,
            metadata: JsonMap::new(),
        }
    }
}

/// Frozen cross-product ArtifactInterchange v1 envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactInterchange {
    /// Frozen cross-product schema identifier.
    #[serde(default = "interchange_schema")]
    pub schema_version: String,
    pub descriptor: ArtifactDescriptor,
    pub revision: ArtifactRevision,
    pub provenance: ArtifactProvenance,
    pub license: ArtifactLicenseState,
    pub lineage: ArtifactLineage,
    pub publication: ArtifactPublication,
    #[serde(default)]
    pub downloads: Vec<Value>,
    #[serde(default)]
    pub materialization_hints: JsonMap,
}

impl ArtifactInterchange {
    /// Validate the full v1 envelope and cross-field policy.
    pub fn validate(&self) -> Result<(), ArtifactError> {
        validation::validate_interchange(self)
    }
}

/// Local mutable Artifact head record. Revision payloads remain immutable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    #[serde(default = "schema_one")]
    pub schema_version: u8,
    pub descriptor: ArtifactDescriptor,
    pub current_revision_id: String,
    #[serde(default)]
    pub revision_ids: Vec<String>,
    pub provenance: ArtifactProvenance,
    pub license: ArtifactLicenseState,
    pub lineage: ArtifactLineage,
    pub publication: ArtifactPublication,
}

impl ArtifactRecord {
    /// Validate local mutable state and its immutable revision references.
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != 1 {
            return Err(ArtifactError::UnsupportedSchema);
        }
        validation::validate_descriptor(&self.descriptor)?;
        validation::validate_reference_id(&self.current_revision_id, "current_revision_id")?;
        if self.revision_ids.is_empty() || !self.revision_ids.contains(&self.current_revision_id) {
            return Err(invalid("revision_ids", "missing_current_revision"));
        }
        if self.revision_ids.len() > MAX_REVISIONS_PER_ARTIFACT {
            return Err(invalid("revision_ids", "too_many"));
        }
        let unique: BTreeSet<_> = self.revision_ids.iter().collect();
        if unique.len() != self.revision_ids.len() {
            return Err(invalid("revision_ids", "duplicate"));
        }
        for revision_id in &self.revision_ids {
            validation::validate_reference_id(revision_id, "revision_id")?;
        }
        validation::validate_provenance(&self.provenance)?;
        validation::validate_license(&self.license)?;
        validation::validate_lineage(&self.lineage)?;
        validation::validate_publication(&self.publication, &self.license)
    }
}

fn media_type(path: &str) -> Option<&'static str> {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    match extension.as_deref() {
        Some("md") => Some("text/markdown"),
        Some("json") => Some("application/json"),
        Some("yaml" | "yml") => Some("application/yaml"),
        Some("txt") => Some("text/plain"),
        Some("toml") => Some("application/toml"),
        Some("js") => Some("text/javascript"),
        Some("ts") => Some("text/typescript"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_ids_match_identity_not_input_order() {
        let first = ArtifactDescriptor::for_identity("skill", "dinglebear-ai", "demo").unwrap();
        let second = ArtifactDescriptor::for_identity("skill", "dinglebear-ai", "demo").unwrap();
        assert_eq!(first.id, second.id);
        assert!(first.id.starts_with("art_"));
    }

    #[test]
    fn revision_digest_is_independent_of_component_input_order() {
        let a = ArtifactComponent::from_bytes("a.md", b"a", None).unwrap();
        let b = ArtifactComponent::from_bytes("b.md", b"b", None).unwrap();
        let left = ArtifactRevision::from_components(
            vec![a.clone(), b.clone()],
            None,
            None,
            None,
            JsonMap::new(),
        )
        .unwrap();
        let right = ArtifactRevision::from_components(vec![b, a], None, None, None, JsonMap::new())
            .unwrap();
        assert_eq!(left.id, right.id);
        assert_eq!(left.components, right.components);
    }

    #[test]
    fn frozen_v1_digest_verification_allows_custom_revision_id_and_unsorted_input() {
        let a = ArtifactComponent::from_bytes("a.md", b"a", None).unwrap();
        let b = ArtifactComponent::from_bytes("b.md", b"b", None).unwrap();
        let canonical = ArtifactRevision::from_components(
            vec![a.clone(), b.clone()],
            None,
            None,
            None,
            JsonMap::new(),
        )
        .unwrap();
        let mut supplied = canonical.clone();
        supplied.id = "rev_custom".to_string();
        supplied.components = vec![b, a];
        validation::validate_revision(&supplied).unwrap();
        supplied.verify_content_digest().unwrap();
    }
}
