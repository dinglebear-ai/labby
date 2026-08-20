//! Frozen Artifact v1 validation and safety budgets.

use std::collections::BTreeSet;
use std::str::FromStr as _;

use serde_json::Value;
use url::Url;

use super::model::{
    ArtifactComponent, ArtifactDescriptor, ArtifactInterchange, ArtifactLicenseState,
    ArtifactLineage, ArtifactProvenance, ArtifactPublication, ArtifactRevision, Distribution,
    Redistribution,
};
use super::{ArtifactError, invalid};

/// Maximum component count in one immutable Artifact revision.
pub const MAX_COMPONENTS: usize = 2_000;
/// Maximum size of one local imported file.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// Maximum aggregate bytes accepted from one local package import.
pub const MAX_PACKAGE_BYTES: u64 = 200 * 1024 * 1024;
/// Maximum logical component path length.
pub const MAX_PATH_BYTES: usize = 4_096;
/// Maximum local directory nesting depth during package import.
pub const MAX_DIRECTORY_DEPTH: usize = 64;
/// Maximum directory entries traversed during one local package import.
pub const MAX_DIRECTORY_ENTRIES: usize = 10_000;
/// Maximum revisions retained in one mutable local head record.
pub const MAX_REVISIONS_PER_ARTIFACT: usize = 10_000;
/// Maximum serialized local Artifact head-record bytes.
pub const MAX_RECORD_JSON_BYTES: u64 = 2 * 1024 * 1024;
/// Maximum serialized immutable revision-manifest bytes.
pub const MAX_REVISION_MANIFEST_BYTES: u64 = 256 * 1024 * 1024;
/// Maximum metadata nesting depth.
pub const MAX_JSON_DEPTH: usize = 8;
/// Maximum keys in one metadata object.
pub const MAX_MAP_ENTRIES: usize = 128;
/// Maximum entries in one metadata list.
pub const MAX_LIST_ENTRIES: usize = 256;
/// Maximum bytes in one metadata string.
pub const MAX_JSON_STRING_BYTES: usize = 16_384;

const INTERCHANGE_SCHEMA: &str = "dinglebear.artifact-interchange/v1";
const BLOCKED_URI_SCHEMES: &[&str] = &["file", "data", "javascript"];
const SECRET_KEYS: &[&str] = &[
    "authorization",
    "password",
    "passwd",
    "token",
    "secret",
    "api_key",
    "apikey",
    "credential",
    "cookie",
];

/// Validate an entire portable ArtifactInterchange v1 envelope.
pub fn validate_interchange(value: &ArtifactInterchange) -> Result<(), ArtifactError> {
    if value.schema_version != INTERCHANGE_SCHEMA {
        return Err(ArtifactError::UnsupportedSchema);
    }
    validate_descriptor(&value.descriptor)?;
    validate_revision(&value.revision)?;
    validate_provenance(&value.provenance)?;
    validate_license(&value.license)?;
    validate_lineage(&value.lineage)?;
    validate_publication(&value.publication, &value.license)?;
    validate_json(
        &Value::Array(value.downloads.clone()),
        "downloads",
        65_536,
        true,
    )?;
    validate_json(
        &serde_json::to_value(&value.materialization_hints)?,
        "materialization_hints",
        32_768,
        true,
    )?;
    value.revision.verify_content_digest()?;
    Ok(())
}

/// Validate a descriptor without assigning identity.
pub fn validate_descriptor(value: &ArtifactDescriptor) -> Result<(), ArtifactError> {
    version_one(value.schema_version, "descriptor")?;
    validate_id(&value.id, "artifact_id")?;
    validate_kind(&value.kind)?;
    validate_slug(&value.namespace, "namespace")?;
    validate_slug(&value.name, "name")?;
    optional_string(value.title.as_deref(), "title", 256)?;
    optional_string(value.description.as_deref(), "description", 4_096)?;
    if value.tags.len() > 64 {
        return Err(invalid("tags", "too_many"));
    }
    let mut tags = BTreeSet::new();
    for tag in &value.tags {
        bounded_string(tag, "tag", 64, 1)?;
        if !tags.insert(tag) {
            return Err(invalid("tags", "duplicate"));
        }
    }
    validate_json(
        &serde_json::to_value(&value.metadata)?,
        "metadata",
        32_768,
        true,
    )
}

/// Validate one package-local component.
pub fn validate_component(value: &ArtifactComponent) -> Result<(), ArtifactError> {
    version_one(value.schema_version, "component")?;
    validate_id(&value.id, "component_id")?;
    validate_kind(&value.kind)?;
    validate_relative_path(&value.path)?;
    validate_digest(&value.digest, "component_digest")?;
    if value.size > 1_073_741_824 {
        return Err(invalid("size", "too_large"));
    }
    optional_string(value.media_type.as_deref(), "media_type", 256)?;
    validate_json(
        &serde_json::to_value(&value.metadata)?,
        "metadata",
        32_768,
        true,
    )?;
    validate_json(
        &Value::Array(value.dependencies.clone()),
        "dependencies",
        32_768,
        true,
    )?;
    validate_json(
        &Value::Array(value.requirements.clone()),
        "requirements",
        32_768,
        true,
    )
}

/// Validate an immutable revision and its canonical component inventory.
pub fn validate_revision(value: &ArtifactRevision) -> Result<(), ArtifactError> {
    version_one(value.schema_version, "revision")?;
    validate_reference_id(&value.id, "revision_id")?;
    validate_digest(&value.content_digest, "content_digest")?;
    if value.components.is_empty() || value.components.len() > MAX_COMPONENTS {
        return Err(invalid("components", "invalid_count"));
    }
    let mut ids = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for component in &value.components {
        validate_component(component)?;
        if !ids.insert(component.id.as_str()) || !paths.insert(component.path.as_str()) {
            return Err(invalid("components", "duplicate"));
        }
    }
    if let Some(parent) = value.parent_revision_id.as_deref() {
        validate_reference_id(parent, "parent_revision_id")?;
        if parent == value.id {
            return Err(invalid("parent_revision_id", "cycle"));
        }
    }
    optional_timestamp(value.authored_at.as_deref(), "authored_at")?;
    optional_string(value.message.as_deref(), "message", 4_096)?;
    validate_json(
        &serde_json::to_value(&value.metadata)?,
        "metadata",
        32_768,
        true,
    )
}

/// Validate portable provenance evidence. Provenance is never treated as trust.
pub fn validate_provenance(value: &ArtifactProvenance) -> Result<(), ArtifactError> {
    version_one(value.schema_version, "provenance")?;
    optional_string(value.provider.as_deref(), "provider", 128)?;
    optional_source_uri(value.source_uri.as_deref(), "source_uri")?;
    optional_string(value.registry.as_deref(), "registry", 512)?;
    optional_string(value.repository.as_deref(), "repository", 512)?;
    optional_string(value.reference.as_deref(), "ref", 512)?;
    if let Some(path) = value.source_path.as_deref() {
        validate_relative_path(path)?;
    }
    if let Some(digest) = value.source_digest.as_deref() {
        validate_digest(digest, "source_digest")?;
    }
    optional_timestamp(value.observed_at.as_deref(), "observed_at")?;
    optional_string(value.adapter.as_deref(), "adapter", 256)?;
    optional_string(value.original_format.as_deref(), "original_format", 128)?;
    optional_string(value.original_version.as_deref(), "original_version", 128)?;
    validate_json(
        &serde_json::to_value(&value.integrity_evidence)?,
        "integrity_evidence",
        32_768,
        true,
    )?;
    validate_json(
        &serde_json::to_value(&value.metadata)?,
        "metadata",
        32_768,
        true,
    )
}

/// Validate explicit license/redistribution state.
pub fn validate_license(value: &ArtifactLicenseState) -> Result<(), ArtifactError> {
    version_one(value.schema_version, "license")?;
    optional_string(value.declared.as_deref(), "declared_license", 512)?;
    validate_json(
        &Value::Array(value.detected.clone()),
        "detected_license",
        32_768,
        true,
    )?;
    validate_json(
        &Value::Array(value.notices.clone()),
        "notices",
        32_768,
        true,
    )?;
    optional_timestamp(value.evidence_at.as_deref(), "evidence_at")?;
    validate_json(
        &serde_json::to_value(&value.metadata)?,
        "metadata",
        32_768,
        true,
    )
}

/// Validate fork/upstream lineage independently of revision identity.
pub fn validate_lineage(value: &ArtifactLineage) -> Result<(), ArtifactError> {
    version_one(value.schema_version, "lineage")?;
    for (field, id) in [
        (
            "upstream_artifact_id",
            value.upstream_artifact_id.as_deref(),
        ),
        (
            "forked_from_artifact_id",
            value.forked_from_artifact_id.as_deref(),
        ),
    ] {
        if let Some(id) = id {
            validate_id(id, field)?;
        }
    }
    for (field, id) in [
        (
            "upstream_revision_id",
            value.upstream_revision_id.as_deref(),
        ),
        (
            "forked_from_revision_id",
            value.forked_from_revision_id.as_deref(),
        ),
        (
            "last_observed_upstream_revision_id",
            value.last_observed_upstream_revision_id.as_deref(),
        ),
    ] {
        if let Some(id) = id {
            validate_reference_id(id, field)?;
        }
    }
    optional_timestamp(value.forked_at.as_deref(), "forked_at")?;
    validate_json(
        &serde_json::to_value(&value.metadata)?,
        "metadata",
        32_768,
        true,
    )
}

/// Validate publication metadata against explicit redistribution rights.
pub fn validate_publication(
    value: &ArtifactPublication,
    license: &ArtifactLicenseState,
) -> Result<(), ArtifactError> {
    version_one(value.schema_version, "publication")?;
    if value.distribution == Distribution::Bytes
        && !matches!(
            license.redistribution,
            Redistribution::Redistributable | Redistribution::Forkable
        )
    {
        return Err(invalid("publication", "bytes_not_redistributable"));
    }
    optional_timestamp(value.published_at.as_deref(), "published_at")?;
    optional_timestamp(value.withdrawn_at.as_deref(), "withdrawn_at")?;
    validate_json(
        &serde_json::to_value(&value.metadata)?,
        "metadata",
        32_768,
        true,
    )
}

/// Validate a logical Artifact path using the Depot-frozen v1 rules.
pub fn validate_relative_path(path: &str) -> Result<(), ArtifactError> {
    bounded_string(path, "path", MAX_PATH_BYTES, 1)?;
    if path.starts_with('/') || looks_like_windows_absolute(path) {
        return Err(ArtifactError::UnsafePath("absolute_path"));
    }
    if path.contains('\\') {
        return Err(ArtifactError::UnsafePath("backslash"));
    }
    if path
        .split('/')
        .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err(ArtifactError::UnsafePath("unsafe_segment"));
    }
    Ok(())
}

/// Validate a frozen lowercase `sha256:<hex>` digest.
pub fn validate_digest(value: &str, field: &'static str) -> Result<(), ArtifactError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(invalid(field, "invalid_digest"));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid(field, "invalid_digest"));
    }
    Ok(())
}

/// Validate an opaque local Artifact/component identifier.
pub fn validate_id(value: &str, field: &'static str) -> Result<(), ArtifactError> {
    bounded_string(value, field, 160, 1)?;
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return Err(invalid(field, "invalid_id"));
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(invalid(field, "invalid_id"));
    }
    if !bytes.all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
    }) {
        return Err(invalid(field, "invalid_id"));
    }
    Ok(())
}

/// Validate either an opaque ID or a content-addressed revision ID.
pub fn validate_reference_id(value: &str, field: &'static str) -> Result<(), ArtifactError> {
    if validate_digest(value, field).is_ok() {
        Ok(())
    } else {
        validate_id(value, field)
    }
}

/// Validate bounded metadata recursively and reject secret-shaped keys.
pub fn validate_json(
    value: &Value,
    field: &'static str,
    max_bytes: usize,
    reject_secrets: bool,
) -> Result<(), ArtifactError> {
    validate_json_inner(value, 0, reject_secrets)?;
    if serde_json::to_vec(value)?.len() > max_bytes {
        return Err(invalid(field, "json_too_large"));
    }
    Ok(())
}

fn validate_json_inner(
    value: &Value,
    depth: usize,
    reject_secrets: bool,
) -> Result<(), ArtifactError> {
    if depth > MAX_JSON_DEPTH {
        return Err(invalid("metadata", "json_too_deep"));
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(value) => {
            if value.len() > MAX_JSON_STRING_BYTES || value.contains('\0') {
                return Err(invalid("metadata", "invalid_string"));
            }
            Ok(())
        }
        Value::Array(values) => {
            if values.len() > MAX_LIST_ENTRIES {
                return Err(invalid("metadata", "list_too_large"));
            }
            for value in values {
                validate_json_inner(value, depth + 1, reject_secrets)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            if map.len() > MAX_MAP_ENTRIES {
                return Err(invalid("metadata", "map_too_large"));
            }
            for (key, value) in map {
                bounded_string(key, "metadata_key", 128, 1)?;
                if reject_secrets && is_artifact_secret_key(key) {
                    return Err(invalid("metadata", "secret_key"));
                }
                validate_json_inner(value, depth + 1, reject_secrets)?;
            }
            Ok(())
        }
    }
}

fn is_artifact_secret_key(key: &str) -> bool {
    let mut normalized = String::with_capacity(key.len());
    let mut separator = false;
    for ch in key.to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch);
            separator = false;
        } else if !separator {
            normalized.push('_');
            separator = true;
        }
    }
    let compact = normalized.replace('_', "");
    SECRET_KEYS.iter().any(|secret| {
        let compact_secret = secret.replace('_', "");
        normalized == *secret
            || compact == compact_secret
            || normalized.ends_with(&format!("_{secret}"))
            || compact.ends_with(&compact_secret)
    })
}

fn validate_kind(value: &str) -> Result<(), ArtifactError> {
    bounded_string(value, "kind", 64, 1)?;
    let mut previous_hyphen = true;
    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'0'..=b'9' => previous_hyphen = false,
            b'-' if !previous_hyphen => previous_hyphen = true,
            _ => return Err(invalid("kind", "invalid_kind")),
        }
    }
    if previous_hyphen {
        return Err(invalid("kind", "invalid_kind"));
    }
    Ok(())
}

fn validate_slug(value: &str, field: &'static str) -> Result<(), ArtifactError> {
    bounded_string(value, field, 128, 1)?;
    let bytes = value.as_bytes();
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return Err(invalid(field, "invalid_slug"));
    }
    let mut separator = false;
    for byte in bytes {
        if byte.is_ascii_alphanumeric() {
            separator = false;
        } else if matches!(byte, b'.' | b'_' | b'-') && !separator {
            separator = true;
        } else {
            return Err(invalid(field, "invalid_slug"));
        }
    }
    Ok(())
}

fn optional_source_uri(value: Option<&str>, field: &'static str) -> Result<(), ArtifactError> {
    let Some(value) = value else {
        return Ok(());
    };
    bounded_string(value, field, 4_096, 1)?;
    let url = Url::parse(value).map_err(|_| invalid(field, "invalid_uri"))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid(field, "credential_uri"));
    }
    let scheme = url.scheme().to_ascii_lowercase();
    if BLOCKED_URI_SCHEMES.contains(&scheme.as_str()) {
        return Err(invalid(field, "blocked_uri_scheme"));
    }
    Ok(())
}

fn optional_timestamp(value: Option<&str>, field: &'static str) -> Result<(), ArtifactError> {
    let Some(value) = value else {
        return Ok(());
    };
    bounded_string(value, field, 64, 1)?;
    jiff::Timestamp::from_str(value).map_err(|_| invalid(field, "invalid_timestamp"))?;
    Ok(())
}

fn optional_string(
    value: Option<&str>,
    field: &'static str,
    max: usize,
) -> Result<(), ArtifactError> {
    if let Some(value) = value {
        bounded_string(value, field, max, 0)?;
    }
    Ok(())
}

fn bounded_string(
    value: &str,
    field: &'static str,
    max: usize,
    min: usize,
) -> Result<(), ArtifactError> {
    let len = value.len();
    if len < min {
        return Err(invalid(field, "too_short"));
    }
    if len > max {
        return Err(invalid(field, "too_long"));
    }
    if value.contains('\0') {
        return Err(invalid(field, "nul_byte"));
    }
    Ok(())
}

fn version_one(version: u8, field: &'static str) -> Result<(), ArtifactError> {
    if version == 1 {
        Ok(())
    } else {
        Err(invalid(field, "unsupported_schema_version"))
    }
}

fn looks_like_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_match_frozen_cross_product_rules() {
        for valid in ["SKILL.md", "references/readme.md", "a%2Fb.md"] {
            validate_relative_path(valid).expect(valid);
        }
        for invalid in [
            "/etc/passwd",
            "C:/Windows/system.ini",
            "../escape",
            "a/../b",
            "./x",
            "a//b",
            "a\\b",
        ] {
            assert!(
                validate_relative_path(invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn metadata_rejects_secret_shaped_keys_recursively() {
        for key in ["apiKey", "credential", "nested_access_token", "myApiKey"] {
            let value = serde_json::json!({"safe": {key: "not-exported"}});
            assert!(
                validate_json(&value, "metadata", 32_768, true).is_err(),
                "accepted {key}"
            );
        }
        for key in ["code", "cwd", "terminal_id", "public_key"] {
            let value = serde_json::json!({key: "portable-metadata"});
            validate_json(&value, "metadata", 32_768, true).expect(key);
        }
    }

    #[test]
    fn source_uri_rejects_credentials_and_active_local_schemes() {
        assert!(
            optional_source_uri(Some("https://user:pass@example.com/x"), "source_uri").is_err()
        );
        assert!(optional_source_uri(Some("file:///etc/passwd"), "source_uri").is_err());
        assert!(optional_source_uri(Some("https://example.com/x"), "source_uri").is_ok());
    }
}
