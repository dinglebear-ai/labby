//! Route-owned MCP shim for publishing one skill archive to Team Depot.

use labby_primitives::action::{ActionSpec, ParamSpec};
use serde_json::Value;

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

pub const SERVICE: &str = "depot_publish";
pub const ACTION: &str = "depot.publish_skill_archive";
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
    name: "depot.publish_skill_archive",
    description: "Publish a skill archive to the authenticated team's Depot",
    destructive: false,
    requires_admin: false,
    returns: "IngestJobReceipt",
    params: PARAMS,
}];

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
