//! Manifest-bound, integrity-checked reads for proxied Agent Skill resources.

use std::time::Instant;

use base64::Engine as _;
use labby_runtime::error::ToolError;
use labby_runtime::gateway_config::UpstreamConfig;
use labby_runtime::skills::{SkillResource, ValidatedSkill, parse_skill_resource_uri};

use super::UpstreamPool;
use super::capability_call::{CapabilityCallError, timed_capability_call_with_response_limit};
use super::logging::{UpstreamRequestLog, log_upstream_request_start};

/// Bytes of one proxied skill file, verified against the manifest that published it.
#[derive(Debug, Clone)]
pub(crate) struct VerifiedSkillFile {
    pub(crate) bytes: Vec<u8>,
    pub(crate) is_blob: bool,
    pub(crate) mime_type: Option<String>,
}

struct SkillBinding {
    canonical_uri: String,
    skill: ValidatedSkill,
    resource: SkillResource,
}

struct DecodedSkillContent {
    bytes: Vec<u8>,
    is_blob: bool,
    mime_type: Option<String>,
}

fn integrity(message: impl Into<String>) -> ToolError {
    ToolError::Sdk {
        sdk_kind: labby_runtime::skills::KIND_SKILL_DIGEST_MISMATCH.to_string(),
        message: message.into(),
    }
}

fn stale_skill_binding(upstream: &str, uri: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: labby_runtime::skills::KIND_SKILL_MANIFEST_STALE.to_string(),
        message: format!(
            "`{uri}` on upstream `{upstream}` does not identify exactly one exposed skill file"
        ),
    }
}

fn skill_read_response_size(result: &rmcp::model::ReadResourceResult, content_cap: usize) -> usize {
    let body_exceeds_cap = result.contents.iter().any(|content| match content {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => {
            text.len() > content_cap
        }
        rmcp::model::ResourceContents::BlobResourceContents { blob, .. } => {
            let padding = blob
                .as_bytes()
                .iter()
                .rev()
                .take_while(|byte| **byte == b'=')
                .take(2)
                .count();
            blob.len()
                .checked_div(4)
                .and_then(|groups| groups.checked_mul(3))
                .and_then(|bytes| bytes.checked_sub(padding))
                .is_none_or(|bytes| bytes > content_cap)
        }
        _ => false,
    });
    if body_exceeds_cap {
        usize::MAX
    } else {
        super::helpers::estimate_resource_response_size(result)
    }
}

impl UpstreamPool {
    pub(crate) async fn read_proxied_skill_file(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        upstream_uri: &str,
    ) -> Result<VerifiedSkillFile, ToolError> {
        self.read_proxied_skill_file_inner(config, subject, None, upstream_uri, None)
            .await
    }

    pub(crate) async fn read_proxied_skill_file_for_skill(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        skill_uri: &str,
        upstream_uri: &str,
        max_bytes: usize,
    ) -> Result<VerifiedSkillFile, ToolError> {
        self.read_proxied_skill_file_inner(
            config,
            subject,
            Some(skill_uri),
            upstream_uri,
            Some(max_bytes),
        )
        .await
    }

    async fn read_proxied_skill_file_inner(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        expected_skill_uri: Option<&str>,
        upstream_uri: &str,
        max_bytes: Option<usize>,
    ) -> Result<VerifiedSkillFile, ToolError> {
        let binding = self
            .resolve_skill_binding(config, subject, expected_skill_uri, upstream_uri)
            .await?;
        let decoded = self
            .fetch_skill_content(config, subject, &binding.resource.uri, max_bytes)
            .await?;
        verify_skill_content(config, &binding, decoded, max_bytes)
    }

    async fn resolve_skill_binding(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        expected_skill_uri: Option<&str>,
        upstream_uri: &str,
    ) -> Result<SkillBinding, ToolError> {
        let canonical_uri = parse_skill_resource_uri(upstream_uri)
            .map_err(|error| ToolError::Sdk {
                sdk_kind: "invalid_param".into(),
                message: error.to_string(),
            })?
            .to_uri();
        let (cached, _) = self
            .upstream_skills_snapshot(config, subject)
            .await
            .map_err(|_| ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: "upstream skill catalog is unavailable".to_string(),
            })?;
        let policy = super::entries::resolve_request_skill_exposure_policy(
            &config.name,
            config.expose_skills.clone(),
        );
        let bindings = cached
            .skills
            .resource_index
            .get(&canonical_uri)
            .into_iter()
            .flatten()
            .filter(|binding| policy.matches(&cached.skills.skills[binding.skill].name))
            .filter(|binding| {
                expected_skill_uri.is_none_or(|expected| {
                    cached.skills.skills[binding.skill].entry.uri == expected
                })
            })
            .collect::<Vec<_>>();

        if let [binding] = bindings.as_slice() {
            let skill = cached.skills.skills[binding.skill].clone();
            let Some(resource) = skill
                .entry
                .resources
                .as_ref()
                .and_then(|resources| resources.get(binding.resource))
                .cloned()
            else {
                return Err(stale_skill_binding(&config.name, &canonical_uri));
            };
            return Ok(SkillBinding {
                canonical_uri,
                skill,
                resource,
            });
        }
        if !bindings.is_empty() {
            return Err(stale_skill_binding(&config.name, &canonical_uri));
        }

        let Some(expected) = expected_skill_uri else {
            return Err(stale_skill_binding(&config.name, &canonical_uri));
        };
        let Some(skill) = self.cached_direct_skill(config, subject, expected).await else {
            return Err(stale_skill_binding(&config.name, &canonical_uri));
        };
        let Some(resource) = skill
            .entry
            .resources
            .as_ref()
            .and_then(|resources| {
                resources
                    .iter()
                    .find(|resource| resource.uri == canonical_uri)
            })
            .cloned()
        else {
            return Err(stale_skill_binding(&config.name, &canonical_uri));
        };
        Ok(SkillBinding {
            canonical_uri,
            skill,
            resource,
        })
    }

    async fn fetch_skill_content(
        &self,
        config: &UpstreamConfig,
        subject: Option<&str>,
        upstream_uri: &str,
        max_bytes: Option<usize>,
    ) -> Result<DecodedSkillContent, ToolError> {
        let peer = self
            .acquire_peer(
                &config.name,
                super::super::types::UpstreamCapability::Skills,
                "skill.read",
            )
            .await
            .ok_or_else(|| ToolError::Sdk {
                sdk_kind: "upstream_error".to_string(),
                message: format!("upstream `{}` is not connected", config.name),
            })?;
        let start = Instant::now();
        let redacted = super::helpers::redact_resource_uri_for_logging(upstream_uri);
        let event = UpstreamRequestLog::skill(&config.name, redacted, subject.is_some());
        log_upstream_request_start(event);
        let timeout_ms = self.request_timeout.as_millis();
        let contents = timed_capability_call_with_response_limit(
            self,
            &config.name,
            super::super::types::UpstreamCapability::Skills,
            event,
            start,
            peer.read_resource(rmcp::model::ReadResourceRequestParams::new(upstream_uri)),
            |result| skill_read_response_size(result, max_bytes.unwrap_or(usize::MAX)),
            subject,
            |error| format!("upstream `{}` skill read failed: {error}", config.name),
            format!(
                "upstream `{}` skill read timed out after {timeout_ms}ms",
                config.name
            ),
            super::helpers::max_skill_response_bytes(),
        )
        .await
        .map_err(|error| ToolError::Sdk {
            sdk_kind: if matches!(&error, CapabilityCallError::ResponseTooLarge { .. }) {
                "response_too_large"
            } else {
                "upstream_error"
            }
            .to_string(),
            message: if matches!(&error, CapabilityCallError::ResponseTooLarge { .. }) {
                "upstream skill resource exceeded the response budget".to_string()
            } else {
                "upstream skill read failed".to_string()
            },
        })?;
        decode_single_content(&config.name, contents)
    }
}

fn decode_single_content(
    upstream: &str,
    contents: rmcp::model::ReadResourceResult,
) -> Result<DecodedSkillContent, ToolError> {
    let content_count = contents.contents.len();
    let Ok([content]) = <[_; 1]>::try_from(contents.contents) else {
        return Err(integrity(format!(
            "upstream `{upstream}` returned {content_count} content blocks for one skill file"
        )));
    };
    match content {
        rmcp::model::ResourceContents::TextResourceContents {
            text, mime_type, ..
        } => Ok(DecodedSkillContent {
            bytes: text.into_bytes(),
            is_blob: false,
            mime_type,
        }),
        rmcp::model::ResourceContents::BlobResourceContents {
            blob, mime_type, ..
        } => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(blob)
                .map_err(|_| {
                    integrity(format!(
                        "upstream `{upstream}` returned malformed base64 for a skill resource"
                    ))
                })?;
            Ok(DecodedSkillContent {
                bytes,
                is_blob: true,
                mime_type,
            })
        }
        _ => Err(integrity(
            "upstream returned an unsupported skill resource representation",
        )),
    }
}

fn verify_skill_content(
    config: &UpstreamConfig,
    binding: &SkillBinding,
    decoded: DecodedSkillContent,
    max_bytes: Option<usize>,
) -> Result<VerifiedSkillFile, ToolError> {
    if let Some(limit) = max_bytes.filter(|limit| decoded.bytes.len() > *limit) {
        return Err(ToolError::Sdk {
            sdk_kind: "response_too_large".to_string(),
            message: format!(
                "skill resource from upstream `{}` exceeds the {limit} byte read limit",
                config.name
            ),
        });
    }
    let parsed_digest =
        labby_runtime::skills::parse_digest(&binding.resource.digest).map_err(|_| {
            integrity(format!(
                "upstream `{}` published an unusable digest",
                config.name
            ))
        })?;
    let declared_size = usize::try_from(binding.resource.size).map_err(|_| {
        integrity(format!(
            "manifest size for `{}` from upstream `{}` cannot be represented by this host",
            binding.canonical_uri, config.name
        ))
    })?;
    if decoded.bytes.len() != declared_size {
        return Err(integrity(format!(
            "content of `{}` from upstream `{}` is {} bytes but its entry declared {declared_size}",
            binding.canonical_uri,
            config.name,
            decoded.bytes.len()
        )));
    }
    if !parsed_digest.matches(&decoded.bytes) {
        return Err(integrity(format!(
            "content of `{}` from upstream `{}` does not match the digest its entry published",
            binding.canonical_uri, config.name
        )));
    }
    if is_skill_md(&binding.skill.entry.uri, &binding.canonical_uri) {
        verify_skill_md_frontmatter(config, &binding.skill, &decoded)?;
    }
    Ok(VerifiedSkillFile {
        bytes: decoded.bytes,
        is_blob: decoded.is_blob,
        mime_type: decoded.mime_type,
    })
}

fn verify_skill_md_frontmatter(
    config: &UpstreamConfig,
    skill: &ValidatedSkill,
    decoded: &DecodedSkillContent,
) -> Result<(), ToolError> {
    if decoded.is_blob {
        return Err(integrity(format!(
            "`SKILL.md` from upstream `{}` must be an MCP text resource",
            config.name
        )));
    }
    let text = std::str::from_utf8(&decoded.bytes).map_err(|_| {
        integrity(format!(
            "`SKILL.md` from upstream `{}` is not UTF-8 text",
            config.name
        ))
    })?;
    let served = labby_runtime::skills::parse_skill_md_frontmatter(text).map_err(|_| {
        integrity(format!(
            "`SKILL.md` from upstream `{}` has unparseable frontmatter",
            config.name
        ))
    })?;
    labby_runtime::skills::compare_frontmatter(&skill.entry.frontmatter, &served).map_err(|_| {
        integrity(format!(
            "`SKILL.md` from upstream `{}` disagrees with the frontmatter its entry published",
            config.name
        ))
    })
}

fn is_skill_md(entry_uri: &str, upstream_path: &str) -> bool {
    parse_skill_resource_uri(entry_uri).is_ok_and(|parsed| parsed.to_uri() == upstream_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use labby_runtime::skills::limits;

    #[test]
    fn response_sizing_rejects_content_over_the_caller_cap_without_serializing() {
        let result =
            rmcp::model::ReadResourceResult::new(vec![rmcp::model::ResourceContents::text(
                "12345",
                "skill://demo/SKILL.md",
            )]);
        assert_eq!(skill_read_response_size(&result, 4), usize::MAX);
    }

    #[test]
    fn response_sizing_counts_padded_base64_as_exact_raw_bytes() {
        for bytes in [vec![0_u8; 4], vec![0_u8; 5]] {
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let result =
                rmcp::model::ReadResourceResult::new(vec![rmcp::model::ResourceContents::blob(
                    encoded,
                    "skill://demo/asset.bin",
                )]);
            assert_ne!(skill_read_response_size(&result, bytes.len()), usize::MAX);
            assert_eq!(
                skill_read_response_size(&result, bytes.len() - 1),
                usize::MAX
            );
        }
    }

    #[test]
    fn response_sizing_accepts_the_required_sixteen_mibibyte_binary_boundary() {
        let bytes = vec![0_u8; limits::MAX_SKILL_RESOURCE_BYTES];
        let result =
            rmcp::model::ReadResourceResult::new(vec![rmcp::model::ResourceContents::blob(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                "skill://demo/asset.bin",
            )]);
        assert_ne!(
            skill_read_response_size(&result, limits::MAX_SKILL_RESOURCE_BYTES),
            usize::MAX
        );
    }

    #[test]
    fn response_sizing_streams_the_exact_escape_heavy_serialized_size() {
        let mut result =
            rmcp::model::ReadResourceResult::new(vec![rmcp::model::ResourceContents::text(
                "\"\\\n\r\t".repeat(64),
                "skill://demo/SKILL.md",
            )]);
        result.meta = Some(
            serde_json::from_value(serde_json::json!({
                "nested": { "payload": "\"\\\n\r\t".repeat(64) }
            }))
            .unwrap(),
        );
        let serialized_len = serde_json::to_vec(&result).unwrap().len();
        assert_eq!(
            skill_read_response_size(&result, usize::MAX),
            serialized_len
        );
        assert!(serialized_len > 1_000);
    }
}
