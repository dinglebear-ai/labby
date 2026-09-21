//! Route-owned MCP shim for publishing one skill archive through Labby.

use labby_primitives::action::{ActionSpec, ParamSpec};
use serde_json::Value;
use std::future::Future;

use super::depot::{DepotClient, DepotError};
use labby_auth::depot_delegation::BrowserDepotAuthorization;
use labby_primitives::product_credential::BoundAccessGrant;

// Base64 plus the JSON-RPC envelope must fit the HTTP MCP transport's 4 MiB cap.
pub(crate) const MAX_ARCHIVE_BYTES: usize = 3_000_000;
pub(crate) const MAX_BASE64_BYTES: usize = MAX_ARCHIVE_BYTES.div_ceil(3) * 4;

/// Package a single composer Skill without allowing caller-controlled paths.
pub fn skill_archive(name: &str, source: &str) -> Result<Vec<u8>, super::error::ToolError> {
    let invalid = || super::error::ToolError::InvalidParam {
        param: "source".into(),
        message: "Provide a Skill name and non-empty SKILL.md source (at most 1 MB)".into(),
    };
    if name.is_empty() || name.len() > 128 || source.trim().is_empty() || source.len() > 1_000_000 {
        return Err(invalid());
    }
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut archive = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(source.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "SKILL.md", source.as_bytes())
        .map_err(|_| invalid())?;
    archive
        .into_inner()
        .and_then(flate2::write::GzEncoder::finish)
        .map_err(|_| invalid())
}

/// Labby-owned public service name. Depot remains an implementation detail.
pub const SERVICE: &str = "artifact_publish";
/// Labby-owned public action name.
pub const ACTION: &str = "artifacts.publish_skill_archive";
/// Hidden compatibility name accepted for callers from before the Labby contract.
pub const LEGACY_SERVICE: &str = "depot_publish";
/// Hidden compatibility action accepted with [`LEGACY_SERVICE`].
pub const LEGACY_ACTION: &str = "depot.publish_skill_archive";
pub const REQUIRED_UPSTREAM: &str = "team-depot";
pub const UPLOAD_CREATE_OPERATION: &str = "depot.uploads.create";
pub const UPLOAD_PUT_OPERATION: &str = "depot.uploads.put";
pub const INGEST_START_OPERATION: &str = "depot.ingest.start";

const PARAMS: &[ParamSpec] = &[
    ParamSpec {
        name: "filename",
        ty: "string",
        required: true,
        description: "Archive filename ending in .zip, .tar.gz, or .tgz",
    },
    ParamSpec {
        name: "archive_base64",
        ty: "string",
        required: true,
        description: "Base64 encoded skill archive bytes",
    },
    ParamSpec {
        name: "namespace",
        ty: "string",
        required: false,
        description: "Optional Team Depot namespace",
    },
];

pub const ACTIONS: &[ActionSpec] = &[ActionSpec {
    // Keep this literal visible to the architecture catalog scanner. Runtime
    // dispatch uses `ACTION`; the scanner intentionally validates catalog
    // declarations without evaluating Rust constants.
    name: "artifacts.publish_skill_archive",
    description: "Publish a skill archive to the authenticated team's Artifact authority",
    destructive: false,
    requires_admin: false,
    returns: "IngestJobReceipt",
    params: PARAMS,
}];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishRequest {
    pub filename: String,
    pub archive: Vec<u8>,
    pub namespace: Option<String>,
}

impl PublishRequest {
    pub fn from_wire(params: &Value) -> Result<Self, super::error::ToolError> {
        use base64::Engine as _;
        let object = params
            .as_object()
            .ok_or_else(|| super::error::ToolError::InvalidParam {
                message: "Artifact publish parameters must be an object".into(),
                param: "params".into(),
            })?;
        if !object
            .keys()
            .all(|key| matches!(key.as_str(), "filename" | "archive_base64" | "namespace"))
        {
            return Err(super::error::ToolError::InvalidParam {
                message: "Artifact publish accepts only filename, archive_base64, and namespace"
                    .into(),
                param: "params".into(),
            });
        }
        let filename = object
            .get("filename")
            .and_then(Value::as_str)
            .filter(|name| {
                !name.is_empty()
                    && name.len() <= 255
                    && !name.contains(['/', '\\'])
                    && !name.chars().any(char::is_control)
                    && {
                        let lower = name.to_ascii_lowercase();
                        lower.ends_with(".zip")
                            || lower.ends_with(".tar.gz")
                            || lower.ends_with(".tgz")
                    }
            })
            .ok_or_else(|| super::error::ToolError::InvalidParam {
                message: "filename must name a supported archive".into(),
                param: "filename".into(),
            })?
            .to_owned();
        let encoded = object
            .get("archive_base64")
            .and_then(Value::as_str)
            .ok_or_else(|| super::error::ToolError::InvalidParam {
                message: "archive_base64 is required".into(),
                param: "archive_base64".into(),
            })?;
        if encoded.len() > MAX_BASE64_BYTES {
            return Err(super::error::ToolError::InvalidParam {
                message: "archive_base64 is too large".into(),
                param: "archive_base64".into(),
            });
        }
        let archive = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| super::error::ToolError::InvalidParam {
                message: "archive_base64 is invalid".into(),
                param: "archive_base64".into(),
            })?;
        if archive.is_empty() || archive.len() > MAX_ARCHIVE_BYTES {
            return Err(super::error::ToolError::InvalidParam {
                message: format!("archive must contain between 1 and {MAX_ARCHIVE_BYTES} bytes"),
                param: "archive_base64".into(),
            });
        }
        let namespace = object
            .get("namespace")
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| {
                        !value.is_empty()
                            && value.len() <= 128
                            && value.bytes().all(|byte| {
                                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                            })
                    })
                    .map(str::to_owned)
                    .ok_or_else(|| super::error::ToolError::InvalidParam {
                        message: "namespace must be a non-empty string no longer than 128 bytes"
                            .into(),
                        param: "namespace".into(),
                    })
            })
            .transpose()?;
        Ok(Self {
            filename,
            archive,
            namespace,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallResolution {
    Canonical,
    Legacy,
    NotPublish,
}

#[must_use]
pub fn is_publish_service(service: &str) -> bool {
    service == SERVICE || service == LEGACY_SERVICE
}

#[must_use]
pub fn resolve_call(service: &str, action: &str) -> CallResolution {
    match (service, action) {
        (SERVICE, ACTION) => CallResolution::Canonical,
        (LEGACY_SERVICE, LEGACY_ACTION) => CallResolution::Legacy,
        _ => CallResolution::NotPublish,
    }
}

#[must_use]
pub fn is_publish_action(action: &str) -> bool {
    action == ACTION || action == LEGACY_ACTION
}

#[must_use]
pub fn is_publish_call(service: &str, action: &str) -> bool {
    resolve_call(service, action) != CallResolution::NotPublish
}

pub async fn publish<F, Fut>(
    depot: &DepotClient,
    request: PublishRequest,
    authorize: F,
) -> Result<Value, DepotError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<BoundAccessGrant, DepotError>>,
{
    depot
        .publish_skill_archive_revalidated(
            &request.filename,
            request.archive,
            request.namespace.as_deref(),
            authorize,
        )
        .await
}

pub async fn publish_for_browser<F, Fut>(
    depot: &DepotClient,
    request: PublishRequest,
    authorize: F,
) -> Result<Value, DepotError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<BrowserDepotAuthorization, DepotError>>,
{
    depot
        .publish_skill_archive_for_browser_revalidated(
            &request.filename,
            request.archive,
            request.namespace.as_deref(),
            authorize,
        )
        .await
}

pub async fn dispatch(_action: &str, _params: Value) -> Result<Value, super::error::ToolError> {
    Err(super::error::ToolError::Sdk {
        sdk_kind: "route_scope_denied".into(),
        message: "Depot publishing is available only on a protected team route".into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    #[test]
    fn composer_archive_contains_only_skill_source() {
        let source = "---\nname: review\ndescription: Review code\n---\n# Review\n";
        let bytes = skill_archive("review", source).unwrap();
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_slice()));
        let mut entries = archive.entries().unwrap();
        let mut entry = entries.next().unwrap().unwrap();
        assert_eq!(entry.path().unwrap().to_str(), Some("SKILL.md"));
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        assert_eq!(text, source);
        assert!(entries.next().is_none());
        assert!(skill_archive("review", " ").is_err());
        assert!(skill_archive("", source).is_err());
        assert!(skill_archive("review", &"x".repeat(1_000_001)).is_err());
    }
}
