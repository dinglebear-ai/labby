//! Request/response types for the MCP Registry v0.1 API.
//!
//! These types closely follow the official MCP Registry OpenAPI specification
//! plus the Lab-specific extension metadata stored alongside registry records.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lab-owned MCP Registry metadata extension namespace.
pub const LAB_REGISTRY_METADATA_NAMESPACE: &str = "dev.labby/registry";

// ---------------------------------------------------------------------------
// Core registry types (mirrors the upstream API)
// ---------------------------------------------------------------------------

/// A server record as returned by the registry API.
///
/// `server` holds the serialisable MCP server definition (stored verbatim in
/// the local SQLite mirror). `meta` carries registry-managed extension data
/// such as `is_latest`, publication timestamps, and Lab-specific annotations
/// that are merged in at read time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerResponse {
    /// The MCP server definition.
    pub server: ServerJSON,
    /// Registry-managed metadata attached to this response.
    /// `None` when absent in both the upstream response and the local store.
    #[serde(rename = "_meta", alias = "meta")]
    pub meta: Option<ResponseMeta>,
}

/// Serialisable MCP server definition — stored verbatim in the local registry
/// mirror and re-parsed on each read.
///
/// Fields align with the MCP Registry v0.1 `server` object schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerJSON {
    /// JSON-LD / JSON Schema `$schema` URL.
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Qualified server name, e.g. `io.modelcontextprotocol/everything`.
    pub name: String,
    /// Human-readable display title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Human-readable description of the server's purpose.
    pub description: String,
    /// Semver version string for this entry.
    pub version: String,
    /// Package distributions available for this server.
    #[serde(default)]
    pub packages: Vec<Package>,
    /// Remote transport endpoints.
    #[serde(default)]
    pub remotes: Vec<Remote>,
    /// Source repository metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<Repository>,
    /// Icon references (URL or data URI).
    #[serde(default)]
    pub icons: Vec<Icon>,
    /// Canonical website URL, if any.
    #[serde(
        rename = "websiteUrl",
        alias = "website_url",
        skip_serializing_if = "Option::is_none"
    )]
    pub website_url: Option<String>,
}

impl ServerJSON {
    /// Convenience: look up the first remote URL, if any.
    #[must_use]
    pub fn first_remote_url(&self) -> Option<&str> {
        self.remotes.iter().find_map(|r| r.url.as_deref())
    }
}

/// A package distribution for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    /// Registry type: `"npm"`, `"pypi"`, `"docker"`, `"mcpb"`, etc.
    #[serde(rename = "registryType", alias = "registry_type")]
    pub registry_type: String,
    /// Package identifier within that registry (e.g. `@scope/name`).
    pub identifier: String,
    /// Optional pinned package version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Transport configuration for this package.
    pub transport: Transport,
    /// Runtime hint: `"npx"`, `"uvx"`, `"docker"`, etc.
    #[serde(
        rename = "runtimeHint",
        alias = "runtime_hint",
        skip_serializing_if = "Option::is_none"
    )]
    pub runtime_hint: Option<String>,
    /// Extra arguments prepended before the package identifier.
    #[serde(rename = "runtimeArguments", alias = "runtime_arguments", default)]
    pub runtime_arguments: Vec<Value>,
    /// Extra arguments appended after the package identifier.
    #[serde(rename = "packageArguments", alias = "package_arguments", default)]
    pub package_arguments: Vec<Value>,
    /// Environment variables accepted or required by this package.
    #[serde(
        rename = "environmentVariables",
        alias = "environment_variables",
        default
    )]
    pub environment_variables: Vec<EnvironmentVariable>,
    /// SHA-256 hash of the binary artifact (MCPB packages only).
    #[serde(
        rename = "fileSha256",
        alias = "file_sha256",
        skip_serializing_if = "Option::is_none"
    )]
    pub file_sha256: Option<String>,
    /// Override base URL for the package registry (used by self-hosted npm mirrors).
    #[serde(
        rename = "registryBaseUrl",
        alias = "registry_base_url",
        skip_serializing_if = "Option::is_none"
    )]
    pub registry_base_url: Option<String>,
}

/// Transport configuration attached to a package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transport {
    /// Transport type: `"stdio"`, `"sse"`, `"http"`, etc.
    #[serde(rename = "type", alias = "transport_type")]
    pub transport_type: String,
    /// URL for HTTP-based transports.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Static HTTP headers to send with every request.
    #[serde(default)]
    pub headers: Vec<Header>,
    /// Dynamic variable definitions (template substitution).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<Value>,
}

/// A static HTTP header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    /// Header name (e.g. `Authorization`).
    pub name: String,
    /// Header value or template (e.g. `Bearer ${API_KEY}`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Human-readable description for caller-supplied headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this header must be supplied by the caller.
    #[serde(
        rename = "isRequired",
        alias = "is_required",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_required: Option<bool>,
    /// Whether this header should be treated as secret.
    #[serde(
        rename = "isSecret",
        alias = "is_secret",
        skip_serializing_if = "Option::is_none"
    )]
    pub is_secret: Option<bool>,
    /// Placeholder text shown in UIs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Format hint (e.g. `"token"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Enumerated choices for the header value.
    #[serde(default)]
    pub choices: Vec<String>,
    /// Dynamic variable definitions (template substitution).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<Value>,
}

/// An environment variable declaration for a package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentVariable {
    /// Variable name (e.g. `GITHUB_TOKEN`).
    pub name: String,
    /// Human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this variable must be set.
    #[serde(rename = "isRequired", alias = "is_required", default)]
    pub is_required: bool,
    /// Whether this variable should be treated as a secret.
    #[serde(rename = "isSecret", alias = "is_secret", default)]
    pub is_secret: bool,
    /// Default value to use when the caller does not provide one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Enumerated choices for the variable value.
    #[serde(default)]
    pub choices: Vec<String>,
    /// Placeholder text shown in UIs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    /// Format hint (e.g. `"token"`, `"url"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// A remote transport endpoint for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Remote {
    /// Transport type: `"sse"`, `"http"`, etc.
    #[serde(rename = "type", alias = "transport_type")]
    pub transport_type: String,
    /// URL of the remote endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Static HTTP headers to send with every request.
    #[serde(default)]
    pub headers: Vec<Header>,
}

/// Source repository metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repository {
    /// Repository URL (e.g. GitHub URL).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Source host type (e.g. `"github"`, `"gitlab"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// An icon reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Icon {
    /// MIME type hint.
    #[serde(
        rename = "mimeType",
        alias = "mime_type",
        skip_serializing_if = "Option::is_none"
    )]
    pub mime_type: Option<String>,
    /// URL or data URI of the icon.
    #[serde(rename = "src", alias = "url")]
    pub url: String,
}

// ---------------------------------------------------------------------------
// Registry response envelope
// ---------------------------------------------------------------------------

/// Paginated list of MCP servers from the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerListResponse {
    /// Servers in this page.
    pub servers: Vec<ServerResponse>,
    /// Pagination metadata.
    pub metadata: PaginationMetadata,
}

/// Pagination metadata returned with list responses.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PaginationMetadata {
    /// Opaque cursor for fetching the next page, if any.
    #[serde(rename = "nextCursor", alias = "next_cursor")]
    pub next_cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// Response meta (registry-managed extensions)
// ---------------------------------------------------------------------------

/// Registry-managed metadata attached to a `ServerResponse`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResponseMeta {
    /// Official registry extensions (is_latest, status, timestamps).
    #[serde(
        rename = "io.modelcontextprotocol.registry/official",
        alias = "official",
        skip_serializing_if = "Option::is_none"
    )]
    pub official: Option<RegistryExtensions>,
    /// Arbitrary extension metadata keyed by namespace.
    ///
    /// Lab stores its own curation data here under
    /// [`LAB_REGISTRY_METADATA_NAMESPACE`].
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl ResponseMeta {
    /// Return true when no fields carry any data (safe to serialize as `None`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.official.is_none() && self.extensions.is_empty()
    }

    /// Insert or replace an extension value under a given namespace key.
    pub fn insert_extension(&mut self, namespace: &str, value: Value) {
        self.extensions.insert(namespace.to_owned(), value);
    }

    /// Insert or replace Lab-owned registry metadata under the canonical
    /// extension namespace.
    ///
    /// # Errors
    /// Returns a serde error if `metadata` cannot be represented as JSON.
    pub fn insert_lab_registry_metadata(
        &mut self,
        metadata: &LabRegistryMetadata,
    ) -> serde_json::Result<()> {
        self.insert_extension(
            LAB_REGISTRY_METADATA_NAMESPACE,
            serde_json::to_value(metadata)?,
        );
        Ok(())
    }
}

/// Official registry-managed extension fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryExtensions {
    /// Whether this is the latest published version of the server.
    #[serde(rename = "isLatest", alias = "is_latest")]
    pub is_latest: bool,
    /// ISO-8601 timestamp when this version was first published.
    #[serde(rename = "publishedAt", alias = "published_at")]
    pub published_at: String,
    /// Lifecycle status: `"active"`, `"deprecated"`, `"deleted"`, etc.
    pub status: String,
    /// ISO-8601 timestamp when `status` last changed.
    #[serde(rename = "statusChangedAt", alias = "status_changed_at")]
    pub status_changed_at: String,
    /// Human-readable message accompanying a non-active status.
    #[serde(
        rename = "statusMessage",
        alias = "status_message",
        skip_serializing_if = "Option::is_none"
    )]
    pub status_message: Option<String>,
    /// ISO-8601 timestamp of the most recent upstream update.
    #[serde(
        rename = "updatedAt",
        alias = "updated_at",
        skip_serializing_if = "Option::is_none"
    )]
    pub updated_at: Option<String>,
}

// ---------------------------------------------------------------------------
// Validate types
// ---------------------------------------------------------------------------

/// Result from the registry's `/v0.1/validate` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Whether the provided server JSON is valid.
    pub valid: bool,
    /// Validation error messages, if any.
    #[serde(default)]
    pub errors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Query parameters for the `GET /v0.1/servers` list endpoint.
#[derive(Debug, Clone, Default)]
pub struct ListServersParams {
    /// Optional free-text search query.
    pub search: Option<String>,
    /// Maximum number of results per page.
    pub limit: Option<u32>,
    /// Pagination cursor returned by a prior response.
    pub cursor: Option<String>,
    /// Filter to a specific version string.
    pub version: Option<String>,
    /// Filter to entries updated since this ISO-8601 timestamp.
    pub updated_since: Option<String>,
    /// Filter to Lab-featured entries.
    pub featured: Option<bool>,
    /// Filter to Lab-reviewed entries.
    pub reviewed: Option<bool>,
    /// Filter to Lab-recommended entries.
    pub recommended: Option<bool>,
    /// Filter to hidden entries.
    pub hidden: Option<bool>,
    /// Filter to a single Lab curation tag.
    pub tag: Option<String>,
}

impl ListServersParams {
    /// Encode as URL query pairs for `GET /v0.1/servers`, omitting `None` fields.
    ///
    /// Note: Lab-specific filter fields (featured, reviewed, etc.) are client-side
    /// concepts applied against the local store — they are NOT forwarded upstream.
    #[must_use]
    pub fn to_upstream_query_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        if let Some(q) = &self.search {
            pairs.push(("search".to_owned(), q.clone()));
        }
        if let Some(n) = self.limit {
            pairs.push(("limit".to_owned(), n.to_string()));
        }
        if let Some(c) = &self.cursor {
            pairs.push(("cursor".to_owned(), c.clone()));
        }
        pairs
    }
}

// ---------------------------------------------------------------------------
// Lab-specific metadata (stored alongside registry records)
// ---------------------------------------------------------------------------

/// Lab-managed curation metadata attached to a registry record.
///
/// Stored in the local registry SQLite store under the
/// [`LAB_REGISTRY_METADATA_NAMESPACE`] extension namespace. Never accepted from
/// the upstream registry API.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LabRegistryMetadata {
    /// Lab audit trail (populated by the store, read-only for callers).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audit: Option<LabRegistryAudit>,
    /// Curation tags and notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub curation: Option<LabCuration>,
    /// Trust signals (manual review state).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trust: Option<LabTrustMeta>,
    /// Installation quality signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<LabQualityMeta>,
    /// Security review signals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security: Option<LabSecurityMeta>,
    /// UX-level annotations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ux: Option<LabUxMeta>,
    /// Caller-owned metadata that is not part of the first-class Lab contract.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

impl LabRegistryMetadata {
    /// Validate a caller-supplied Lab metadata write payload.
    ///
    /// The upstream registry response shape uses `_meta["dev.labby/registry"]`
    /// for storage. This helper validates only the extension payload inside
    /// that namespace and rejects Lab-managed fields callers are not allowed to
    /// write.
    ///
    /// # Errors
    /// Returns a semicolon-separated validation message when the payload is not
    /// an object, contains unsupported top-level fields, tries to write
    /// `audit`, or violates the documented first-class metadata rules.
    pub fn validate_write_payload(value: &Value) -> Result<Self, String> {
        let Some(object) = value.as_object() else {
            return Err("metadata payload must be a JSON object".to_string());
        };

        let mut errors = Vec::new();
        let allowed_sections =
            BTreeSet::from(["curation", "trust", "quality", "security", "ux", "extra"]);
        for key in object.keys() {
            if key == "audit" {
                errors.push("audit is Lab-managed and cannot be supplied by callers".to_string());
            } else if !allowed_sections.contains(key.as_str()) {
                errors.push(format!(
                    "unknown top-level metadata field `{key}`; put caller-owned metadata under `extra`"
                ));
            }
        }
        validate_section_fields(
            value,
            "curation",
            &[
                "tags",
                "notes",
                "featured",
                "reviewed",
                "recommended",
                "hidden",
            ],
            &mut errors,
        );
        validate_section_fields(
            value,
            "trust",
            &[
                "reviewed",
                "reviewed_at",
                "source_verified",
                "maintainer_known",
            ],
            &mut errors,
        );
        validate_section_fields(
            value,
            "quality",
            &[
                "install_tested",
                "last_install_tested_at",
                "transport_score",
            ],
            &mut errors,
        );
        validate_section_fields(
            value,
            "security",
            &["ssrf_reviewed", "permissions_reviewed", "secrets_reviewed"],
            &mut errors,
        );
        validate_section_fields(
            value,
            "ux",
            &[
                "works_in_lab",
                "recommended_for_homelab",
                "setup_difficulty",
            ],
            &mut errors,
        );
        validate_extra_section(value, &mut errors);

        validate_rfc3339_field(
            value,
            "/trust/reviewed_at",
            "trust.reviewed_at",
            &mut errors,
        );
        validate_rfc3339_field(
            value,
            "/quality/last_install_tested_at",
            "quality.last_install_tested_at",
            &mut errors,
        );
        validate_enum_field(
            value,
            "/quality/transport_score",
            "quality.transport_score",
            &["good", "mixed", "poor"],
            &mut errors,
        );
        validate_enum_field(
            value,
            "/ux/setup_difficulty",
            "ux.setup_difficulty",
            &["easy", "medium", "hard"],
            &mut errors,
        );
        validate_tags(value, &mut errors);

        if !errors.is_empty() {
            return Err(errors.join("; "));
        }

        let mut metadata: Self = serde_json::from_value(value.clone())
            .map_err(|err| format!("metadata payload is invalid: {err}"))?;
        normalize_write_metadata(&mut metadata);
        Ok(metadata)
    }
}

fn validate_section_fields(
    value: &Value,
    section: &str,
    allowed: &[&str],
    errors: &mut Vec<String>,
) {
    let Some(section_value) = value.get(section) else {
        return;
    };
    let Some(section_object) = section_value.as_object() else {
        errors.push(format!("{section} must be a JSON object"));
        return;
    };
    for key in section_object.keys() {
        if !allowed.contains(&key.as_str()) {
            errors.push(format!(
                "unknown metadata field `{section}.{key}`; put caller-owned metadata under `extra`"
            ));
        }
    }
}

fn validate_extra_section(value: &Value, errors: &mut Vec<String>) {
    let Some(extra) = value.get("extra") else {
        return;
    };
    if !extra.is_object() {
        errors.push("extra must be a JSON object".to_string());
    }
}

fn validate_rfc3339_field(value: &Value, pointer: &str, label: &str, errors: &mut Vec<String>) {
    let Some(field) = value.pointer(pointer) else {
        return;
    };
    let Some(raw) = field.as_str() else {
        errors.push(format!("{label} must be an RFC3339 string"));
        return;
    };
    if Timestamp::from_str(raw).is_err() {
        errors.push(format!("{label} must be RFC3339"));
    }
}

fn validate_enum_field(
    value: &Value,
    pointer: &str,
    label: &str,
    allowed: &[&str],
    errors: &mut Vec<String>,
) {
    let Some(field) = value.pointer(pointer) else {
        return;
    };
    let Some(raw) = field.as_str() else {
        errors.push(format!("{label} must be a string"));
        return;
    };
    if !allowed.contains(&raw) {
        errors.push(format!("{label} must be one of: {}", allowed.join(", ")));
    }
}

fn validate_tags(value: &Value, errors: &mut Vec<String>) {
    let Some(tags) = value.pointer("/curation/tags") else {
        return;
    };
    let Some(tags) = tags.as_array() else {
        errors.push("curation.tags must be an array of non-empty strings".to_string());
        return;
    };
    for tag in tags {
        match tag.as_str() {
            Some(raw) if !raw.trim().is_empty() => {}
            _ => errors.push("curation.tags must not contain empty values".to_string()),
        }
    }
}

fn normalize_write_metadata(metadata: &mut LabRegistryMetadata) {
    if let Some(curation) = metadata.curation.as_mut() {
        let mut seen = BTreeSet::new();
        curation.tags = curation
            .tags
            .iter()
            .filter_map(|tag| {
                let trimmed = tag.trim();
                if trimmed.is_empty() || !seen.insert(trimmed.to_string()) {
                    return None;
                }
                Some(trimmed.to_string())
            })
            .collect();
        if curation
            .notes
            .as_deref()
            .is_some_and(|notes| notes.trim().is_empty())
        {
            curation.notes = None;
        }
    }
}

/// Audit trail automatically populated by Lab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabRegistryAudit {
    /// ISO-8601 timestamp of the last metadata write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    /// Agent or user identifier that last wrote the metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

/// Lab curator tags and notes for a server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabCuration {
    /// Curation tags (sorted, deduplicated by the store).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional curator notes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// Whether Lab features this server in curated listings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub featured: Option<bool>,
    /// Whether Lab has reviewed this server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed: Option<bool>,
    /// Whether Lab recommends this server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended: Option<bool>,
    /// Whether this server is hidden from default listings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

/// Trust signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabTrustMeta {
    /// Whether Lab has reviewed this server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed: Option<bool>,
    /// ISO-8601 timestamp when a human last reviewed this server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewed_at: Option<String>,
    /// Whether Lab verified the published source repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_verified: Option<bool>,
    /// Whether Lab recognizes the maintainer identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maintainer_known: Option<bool>,
}

/// Installation quality signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabQualityMeta {
    /// Whether Lab has performed an install smoke test.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_tested: Option<bool>,
    /// ISO-8601 timestamp of the last successful install test.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_install_tested_at: Option<String>,
    /// Observed transport reliability score.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport_score: Option<LabRegistryTransportScore>,
}

/// Security review signals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabSecurityMeta {
    /// Whether SSRF-sensitive fields have been reviewed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssrf_reviewed: Option<bool>,
    /// Whether requested permissions/capabilities have been reviewed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions_reviewed: Option<bool>,
    /// Whether secret handling has been reviewed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secrets_reviewed: Option<bool>,
}

/// UX-level annotations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabUxMeta {
    /// Whether this server works in Lab's gateway/runtime setup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub works_in_lab: Option<bool>,
    /// Whether Lab recommends this server for homelab deployments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_for_homelab: Option<bool>,
    /// Subjective setup difficulty rating.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub setup_difficulty: Option<LabRegistrySetupDifficulty>,
}

/// Transport reliability score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabRegistryTransportScore {
    /// Transport works reliably.
    Good,
    /// Transport has known issues in some configurations.
    Mixed,
    /// Transport is unreliable or broken.
    Poor,
}

/// Subjective setup difficulty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabRegistrySetupDifficulty {
    /// Minimal configuration required.
    Easy,
    /// Some configuration steps required.
    Medium,
    /// Complex configuration or prerequisites required.
    Hard,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_list_response_deserializes_minimal() {
        let json = serde_json::json!({
            "servers": [
                {
                    "server": {
                        "name": "io.example/hello",
                        "description": "A hello world server",
                        "version": "1.0.0"
                    },
                    "meta": null
                }
            ],
            "metadata": {
                "next_cursor": null
            }
        });

        let resp: ServerListResponse = serde_json::from_value(json).expect("should deserialize");
        assert_eq!(resp.servers.len(), 1);
        assert_eq!(resp.servers[0].server.name, "io.example/hello");
        assert!(resp.metadata.next_cursor.is_none());
        assert!(resp.servers[0].server.packages.is_empty());
        assert!(resp.servers[0].server.remotes.is_empty());
    }

    #[test]
    fn server_response_meta_default_is_empty() {
        let meta = ResponseMeta::default();
        assert!(meta.is_empty());
    }

    #[test]
    fn server_response_meta_insert_extension() {
        let mut meta = ResponseMeta::default();
        meta.insert_extension(
            LAB_REGISTRY_METADATA_NAMESPACE,
            serde_json::json!({"curation": {"featured": true}}),
        );
        assert!(!meta.is_empty());
        assert!(
            meta.extensions
                .contains_key(LAB_REGISTRY_METADATA_NAMESPACE)
        );
    }

    #[test]
    fn list_servers_params_to_upstream_query_pairs_omits_lab_fields() {
        let p = ListServersParams {
            search: Some("test".into()),
            limit: Some(25),
            cursor: Some("cur1".into()),
            featured: Some(true), // Lab-only — must NOT appear in upstream pairs
            ..Default::default()
        };
        let pairs = p.to_upstream_query_pairs();
        assert_eq!(pairs.len(), 3);
        assert!(pairs.iter().any(|(k, v)| k == "search" && v == "test"));
        assert!(pairs.iter().any(|(k, v)| k == "limit" && v == "25"));
        assert!(pairs.iter().any(|(k, v)| k == "cursor" && v == "cur1"));
        // Lab-only fields must be absent
        assert!(!pairs.iter().any(|(k, _)| k == "featured"));
    }

    #[test]
    fn lab_registry_metadata_audit_field_roundtrips() {
        let meta = LabRegistryMetadata {
            audit: Some(LabRegistryAudit {
                updated_at: Some("2025-01-01T00:00:00Z".into()),
                updated_by: Some("lab-agent".into()),
            }),
            ..Default::default()
        };
        let v = serde_json::to_value(&meta).unwrap();
        let back: LabRegistryMetadata = serde_json::from_value(v).unwrap();
        assert_eq!(
            back.audit.as_ref().unwrap().updated_at.as_deref(),
            Some("2025-01-01T00:00:00Z")
        );
    }

    #[test]
    fn response_meta_serializes_lab_registry_metadata_under_canonical_namespace() {
        let metadata = LabRegistryMetadata {
            curation: Some(LabCuration {
                tags: vec!["recommended".into(), "stable".into()],
                notes: Some("Works well in small homelab setups.".into()),
                featured: Some(true),
                reviewed: None,
                recommended: None,
                hidden: Some(false),
            }),
            trust: Some(LabTrustMeta {
                reviewed: Some(true),
                reviewed_at: Some("2026-04-23T15:00:00Z".into()),
                source_verified: Some(true),
                maintainer_known: Some(false),
            }),
            quality: Some(LabQualityMeta {
                install_tested: Some(true),
                last_install_tested_at: Some("2026-04-23T15:00:00Z".into()),
                transport_score: Some(LabRegistryTransportScore::Good),
            }),
            security: Some(LabSecurityMeta {
                ssrf_reviewed: Some(true),
                permissions_reviewed: Some(true),
                secrets_reviewed: Some(true),
            }),
            ux: Some(LabUxMeta {
                works_in_lab: Some(true),
                recommended_for_homelab: Some(true),
                setup_difficulty: Some(LabRegistrySetupDifficulty::Easy),
            }),
            extra: BTreeMap::from([(
                "review_source".to_owned(),
                Value::String("manual".to_owned()),
            )]),
            audit: Some(LabRegistryAudit {
                updated_at: Some("2026-04-23T15:00:00Z".into()),
                updated_by: Some("gateway-admin".into()),
            }),
        };
        let mut meta = ResponseMeta::default();
        meta.insert_lab_registry_metadata(&metadata)
            .expect("metadata should serialize");

        let value = serde_json::to_value(&meta).expect("response meta should serialize");
        let lab = &value[LAB_REGISTRY_METADATA_NAMESPACE];
        assert!(value.get("lab").is_none());
        assert_eq!(lab["curation"]["featured"], true);
        assert_eq!(lab["trust"]["reviewed"], true);
        assert_eq!(lab["trust"]["source_verified"], true);
        assert_eq!(lab["quality"]["install_tested"], true);
        assert_eq!(lab["quality"]["transport_score"], "good");
        assert_eq!(lab["security"]["ssrf_reviewed"], true);
        assert_eq!(lab["ux"]["works_in_lab"], true);
        assert_eq!(lab["ux"]["setup_difficulty"], "easy");
        assert_eq!(lab["extra"]["review_source"], "manual");
        assert_eq!(lab["audit"]["updated_by"], "gateway-admin");
    }

    #[test]
    fn lab_registry_metadata_validation_rejects_unknown_top_level_fields() {
        let err = LabRegistryMetadata::validate_write_payload(&serde_json::json!({
            "curation": { "featured": true },
            "unexpected": true
        }))
        .expect_err("unknown top-level fields should be rejected");

        assert!(err.contains("unexpected"));
        assert!(err.contains("extra"));
    }

    #[test]
    fn lab_registry_metadata_validation_rejects_caller_supplied_audit() {
        let err = LabRegistryMetadata::validate_write_payload(&serde_json::json!({
            "audit": { "updated_by": "caller" }
        }))
        .expect_err("audit is Lab-managed");

        assert!(err.contains("audit"));
    }

    #[test]
    fn lab_registry_metadata_validation_rejects_unknown_nested_first_class_fields() {
        let err = LabRegistryMetadata::validate_write_payload(&serde_json::json!({
            "quality": {
                "transportScore": "good"
            },
            "ux": {
                "setupDifficulty": "easy"
            }
        }))
        .expect_err("unknown nested first-class fields should be rejected");

        assert!(err.contains("quality.transportScore"));
        assert!(err.contains("ux.setupDifficulty"));
        assert!(err.contains("extra"));
    }

    #[test]
    fn lab_registry_metadata_validation_rejects_invalid_dates_enums_and_tags() {
        let err = LabRegistryMetadata::validate_write_payload(&serde_json::json!({
            "curation": { "tags": ["stable", " "] },
            "trust": { "reviewed_at": "not-a-date" },
            "quality": {
                "last_install_tested_at": "2026-04-23",
                "transport_score": "excellent"
            },
            "ux": { "setup_difficulty": "trivial" }
        }))
        .expect_err("invalid metadata should be rejected");

        assert!(err.contains("curation.tags"));
        assert!(err.contains("trust.reviewed_at"));
        assert!(err.contains("quality.last_install_tested_at"));
        assert!(err.contains("quality.transport_score"));
        assert!(err.contains("ux.setup_difficulty"));
    }

    #[test]
    fn lab_registry_metadata_validation_accepts_documented_write_shape() {
        let metadata = LabRegistryMetadata::validate_write_payload(&serde_json::json!({
            "curation": {
                "featured": true,
                "hidden": false,
                "tags": ["recommended", "stable"],
                "notes": "Works well in small homelab setups."
            },
            "trust": {
                "reviewed": true,
                "reviewed_at": "2026-04-23T15:00:00Z",
                "source_verified": true,
                "maintainer_known": false
            },
            "quality": {
                "install_tested": true,
                "last_install_tested_at": "2026-04-23T15:00:00Z",
                "transport_score": "good"
            },
            "security": {
                "ssrf_reviewed": true,
                "permissions_reviewed": true,
                "secrets_reviewed": true
            },
            "ux": {
                "works_in_lab": true,
                "recommended_for_homelab": true,
                "setup_difficulty": "easy"
            },
            "extra": {
                "review_source": "manual"
            }
        }))
        .expect("documented shape should validate");

        assert_eq!(
            metadata
                .curation
                .as_ref()
                .expect("curation")
                .tags
                .as_slice(),
            ["recommended", "stable"]
        );
        assert!(metadata.audit.is_none());
    }

    #[test]
    fn package_deserializes_with_defaults() {
        let json = serde_json::json!({
            "registry_type": "npm",
            "identifier": "@example/server",
            "transport": {
                "transport_type": "stdio",
                "headers": []
            },
            "is_required": false,
            "is_secret": false
        });
        let pkg: Package = serde_json::from_value(json).expect("should deserialize");
        assert_eq!(pkg.registry_type, "npm");
        assert!(pkg.runtime_hint.is_none());
        assert!(pkg.environment_variables.is_empty());
        assert!(pkg.runtime_arguments.is_empty());
    }

    #[test]
    fn server_json_accepts_upstream_registry_field_names() {
        let json = serde_json::json!({
            "servers": [{
                "server": {
                    "$schema": "https://static.modelcontextprotocol.io/schemas/2025-07-09/server.schema.json",
                    "name": "io.example/server",
                    "title": "Example",
                    "description": "Example MCP server",
                    "version": "1.2.3",
                    "websiteUrl": "https://example.com",
                    "repository": {},
                    "packages": [{
                        "registryType": "npm",
                        "identifier": "@example/server",
                        "runtimeHint": "npx",
                        "runtimeArguments": ["-y"],
                        "packageArguments": ["--stdio"],
                        "fileSha256": "abc123",
                        "registryBaseUrl": "https://registry.npmjs.org",
                        "transport": {
                            "type": "stdio"
                        },
                        "environmentVariables": [{
                            "name": "EXAMPLE_TOKEN",
                            "description": "Example API token",
                            "isSecret": true
                        }]
                    }],
                    "remotes": [{
                        "type": "streamable-http",
                        "url": "https://example.com/mcp",
                        "headers": [{
                            "name": "Authorization",
                            "description": "Bearer token",
                            "isRequired": true,
                            "isSecret": true
                        }]
                    }],
                    "icons": [{
                        "src": "https://example.com/icon.png",
                        "mimeType": "image/png"
                    }]
                },
                "_meta": {
                    "io.modelcontextprotocol.registry/official": {
                        "isLatest": true,
                        "publishedAt": "2026-01-01T00:00:00Z",
                        "status": "active",
                        "statusChangedAt": "2026-01-01T00:00:00Z",
                        "updatedAt": "2026-01-02T00:00:00Z"
                    }
                }
            }],
            "metadata": {
                "nextCursor": "io.example/server:1.2.3",
                "count": 1
            }
        });

        let response: ServerListResponse =
            serde_json::from_value(json).expect("should deserialize");
        assert_eq!(
            response.metadata.next_cursor.as_deref(),
            Some("io.example/server:1.2.3")
        );
        let first = &response.servers[0];
        let official = first.meta.as_ref().and_then(|meta| meta.official.as_ref());
        assert_eq!(official.map(|meta| meta.is_latest), Some(true));
        assert_eq!(
            official.and_then(|meta| meta.updated_at.as_deref()),
            Some("2026-01-02T00:00:00Z")
        );
        let server = &first.server;
        assert_eq!(server.website_url.as_deref(), Some("https://example.com"));
        assert_eq!(server.packages[0].registry_type, "npm");
        assert_eq!(server.packages[0].runtime_hint.as_deref(), Some("npx"));
        assert_eq!(server.packages[0].transport.transport_type, "stdio");
        assert_eq!(
            server.packages[0].environment_variables[0].name,
            "EXAMPLE_TOKEN"
        );
        assert!(!server.packages[0].environment_variables[0].is_required);
        assert!(server.packages[0].environment_variables[0].is_secret);
        assert_eq!(
            server
                .repository
                .as_ref()
                .and_then(|repo| repo.url.as_deref()),
            None
        );
        assert_eq!(server.remotes[0].transport_type, "streamable-http");
        assert_eq!(
            server.remotes[0].headers[0].description.as_deref(),
            Some("Bearer token")
        );
        assert_eq!(server.remotes[0].headers[0].value, None);
        assert_eq!(server.icons[0].url, "https://example.com/icon.png");
        assert_eq!(server.icons[0].mime_type.as_deref(), Some("image/png"));
    }

    #[test]
    fn server_json_serializes_canonical_registry_field_names() {
        let server: ServerJSON = serde_json::from_value(serde_json::json!({
            "name": "io.example/server",
            "description": "Example MCP server",
            "version": "1.2.3",
            "websiteUrl": "https://example.com",
            "packages": [{
                "registryType": "npm",
                "identifier": "@example/server",
                "runtimeHint": "npx",
                "runtimeArguments": ["-y"],
                "packageArguments": ["--stdio"],
                "fileSha256": "abc123",
                "registryBaseUrl": "https://registry.npmjs.org",
                "transport": {
                    "type": "stdio"
                },
                "environmentVariables": [{
                    "name": "EXAMPLE_TOKEN",
                    "isRequired": true,
                    "isSecret": true
                }]
            }],
            "remotes": [{
                "type": "streamable-http",
                "url": "https://example.com/mcp",
                "headers": [{
                    "name": "Authorization",
                    "isRequired": true,
                    "isSecret": true
                }]
            }],
            "icons": [{
                "src": "https://example.com/icon.png",
                "mimeType": "image/png"
            }]
        }))
        .expect("canonical registry JSON should deserialize");

        let value = serde_json::to_value(&server).expect("server should serialize");
        let package = &value["packages"][0];
        assert_eq!(value["websiteUrl"], "https://example.com");
        assert!(value.get("website_url").is_none());
        assert_eq!(package["registryType"], "npm");
        assert!(package.get("registry_type").is_none());
        assert_eq!(package["runtimeHint"], "npx");
        assert!(package.get("runtime_hint").is_none());
        assert_eq!(package["runtimeArguments"], serde_json::json!(["-y"]));
        assert!(package.get("runtime_arguments").is_none());
        assert_eq!(package["packageArguments"], serde_json::json!(["--stdio"]));
        assert!(package.get("package_arguments").is_none());
        assert_eq!(package["environmentVariables"][0]["name"], "EXAMPLE_TOKEN");
        assert!(package.get("environment_variables").is_none());
        assert_eq!(package["fileSha256"], "abc123");
        assert!(package.get("file_sha256").is_none());
        assert_eq!(package["registryBaseUrl"], "https://registry.npmjs.org");
        assert!(package.get("registry_base_url").is_none());
        assert_eq!(package["transport"]["type"], "stdio");
        assert!(package["transport"].get("transport_type").is_none());
        assert_eq!(value["remotes"][0]["type"], "streamable-http");
        assert!(value["remotes"][0].get("transport_type").is_none());
        assert_eq!(value["icons"][0]["src"], "https://example.com/icon.png");
        assert!(value["icons"][0].get("url").is_none());
        assert_eq!(value["icons"][0]["mimeType"], "image/png");
        assert!(value["icons"][0].get("mime_type").is_none());
        assert_eq!(package["environmentVariables"][0]["isRequired"], true);
        assert!(
            package["environmentVariables"][0]
                .get("is_required")
                .is_none()
        );
        assert_eq!(package["environmentVariables"][0]["isSecret"], true);
        assert!(
            package["environmentVariables"][0]
                .get("is_secret")
                .is_none()
        );
        assert_eq!(value["remotes"][0]["headers"][0]["isRequired"], true);
        assert!(
            value["remotes"][0]["headers"][0]
                .get("is_required")
                .is_none()
        );
        assert_eq!(value["remotes"][0]["headers"][0]["isSecret"], true);
        assert!(value["remotes"][0]["headers"][0].get("is_secret").is_none());
    }

    #[test]
    fn server_list_response_serializes_canonical_metadata_field_names() {
        let response: ServerListResponse = serde_json::from_value(serde_json::json!({
            "servers": [{
                "server": {
                    "name": "io.example/server",
                    "description": "Example MCP server",
                    "version": "1.2.3"
                },
                "_meta": {
                    "io.modelcontextprotocol.registry/official": {
                        "isLatest": true,
                        "publishedAt": "2026-01-01T00:00:00Z",
                        "status": "active",
                        "statusChangedAt": "2026-01-01T00:00:00Z",
                        "statusMessage": "Ready",
                        "updatedAt": "2026-01-02T00:00:00Z"
                    },
                    "dev.labby/registry": {
                        "curation": {
                            "featured": true
                        }
                    }
                }
            }],
            "metadata": {
                "nextCursor": "io.example/server:1.2.3"
            }
        }))
        .expect("canonical registry response should deserialize");

        let value = serde_json::to_value(&response).expect("response should serialize");
        let first = &value["servers"][0];
        let official = &first["_meta"]["io.modelcontextprotocol.registry/official"];
        let lab = &first["_meta"][LAB_REGISTRY_METADATA_NAMESPACE];
        assert!(first.get("_meta").is_some());
        assert!(first.get("meta").is_none());
        assert!(first["_meta"].get("lab").is_none());
        assert_eq!(lab["curation"]["featured"], true);
        assert_eq!(value["metadata"]["nextCursor"], "io.example/server:1.2.3");
        assert!(value["metadata"].get("next_cursor").is_none());
        assert_eq!(official["isLatest"], true);
        assert!(official.get("is_latest").is_none());
        assert_eq!(official["publishedAt"], "2026-01-01T00:00:00Z");
        assert!(official.get("published_at").is_none());
        assert_eq!(official["statusChangedAt"], "2026-01-01T00:00:00Z");
        assert!(official.get("status_changed_at").is_none());
        assert_eq!(official["statusMessage"], "Ready");
        assert!(official.get("status_message").is_none());
        assert_eq!(official["updatedAt"], "2026-01-02T00:00:00Z");
        assert!(official.get("updated_at").is_none());
    }
}
