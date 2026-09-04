//! Validation of a skill entry's `resources` manifest.
//!
//! The manifest is the unit a host verifies and a user's approval binds to, so
//! it is validated once at ingest and then treated as the authority for which
//! URIs a skill may read. SEP-2640 constrains it tightly:
//!
//! - When present it MUST be complete — every file of the skill, each exactly
//!   once, including an entry whose URI matches the skill's own `uri`, carrying
//!   the digest of `SKILL.md` itself.
//! - Each `uri` MUST be the skill's `SKILL.md` or a file within the skill's
//!   directory.
//! - It MAY be omitted only for dynamically generated skills, which are
//!   consequently unverifiable. Hosts may decline to load those, and Labby does.
//!
//! The second rule is the confused-deputy guard (threat model T5): without it a
//! manifest could name a URI belonging to a different skill, a different
//! upstream, or another scheme entirely, and a host walking the manifest would
//! fetch it on the skill's behalf.

use std::collections::BTreeSet;

use crate::error::ToolError;
use crate::skills::digest::parse_digest;
use crate::skills::frontmatter::validate_frontmatter;
use crate::skills::limits::{MAX_RESOURCES_PER_SKILL, MAX_SKILL_TOTAL_BYTES};
use crate::skills::uri::{SkillUri, parse_skill_resource_uri};
use crate::skills::wire::SkillEntry;

/// Why a skill was rejected at ingest.
///
/// Kept as a distinct enum rather than a bare message so callers can count
/// rejections by cause: the aggregate counts are surfaced to operators in full
/// and to agents as a bare completeness number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillRejection {
    /// The entry's own `uri` is not a well-formed `skill://` URI, or does not
    /// name a `SKILL.md`.
    InvalidSkillUri,
    /// `frontmatter` failed Agent Skills validation, or its `name` disagrees
    /// with the final skill-path segment.
    InvalidFrontmatter,
    /// The manifest is absent. Spec-permitted for generated skills; Labby
    /// declines them because they cannot be content-bound.
    MissingManifest,
    /// A digest was absent, used an unsupported algorithm, or was malformed.
    InvalidDigest,
    /// A manifest URI fell outside the skill's own directory, or used another
    /// origin or scheme.
    ManifestUriOutOfNamespace,
    /// The manifest omits an entry for the skill's own `SKILL.md`.
    ManifestMissingSkillMd,
    /// The same URI appears more than once in one manifest.
    ManifestDuplicateUri,
    /// The manifest exceeds the per-skill resource cap.
    ManifestTooLarge,
    /// The manifest declares more total bytes than a conforming host must accept.
    ManifestBytesTooLarge,
}

/// Operator-safe detail for one rejected skill candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRejectionDetail {
    pub reason: SkillRejection,
    pub detail: String,
}

impl SkillRejection {
    /// Short, stable, log-safe reason code.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSkillUri => "invalid_skill_uri",
            Self::InvalidFrontmatter => "invalid_frontmatter",
            Self::MissingManifest => "missing_manifest",
            Self::InvalidDigest => "invalid_digest",
            Self::ManifestUriOutOfNamespace => "manifest_uri_out_of_namespace",
            Self::ManifestMissingSkillMd => "manifest_missing_skill_md",
            Self::ManifestDuplicateUri => "manifest_duplicate_uri",
            Self::ManifestTooLarge => "manifest_too_large",
            Self::ManifestBytesTooLarge => "manifest_bytes_too_large",
        }
    }
}

/// A skill entry that passed ingest validation, with its URI already parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSkill {
    /// Parsed form of the entry's own `uri`.
    pub uri: SkillUri,
    /// The skill's `name`, equal to the final skill-path segment.
    pub name: String,
    /// The entry as received, unmodified.
    pub entry: SkillEntry,
}

/// Validate one skill entry for ingest.
///
/// Returns the rejection cause rather than a `ToolError`: a single bad skill
/// must never sink the whole upstream, so callers exclude the skill, count the
/// cause, and carry on.
pub fn validate_skill_entry(entry: &SkillEntry) -> Result<ValidatedSkill, SkillRejection> {
    validate_skill_entry_detailed(entry).map_err(|rejection| rejection.reason)
}

/// Validate one skill while retaining the exact schema rule that failed.
pub fn validate_skill_entry_detailed(
    entry: &SkillEntry,
) -> Result<ValidatedSkill, SkillRejectionDetail> {
    let reject = |reason, detail: String| SkillRejectionDetail { reason, detail };
    let uri = parse_skill_resource_uri(&entry.uri).map_err(|_| {
        reject(
            SkillRejection::InvalidSkillUri,
            "skill URI does not satisfy the required structure".into(),
        )
    })?;
    let (skill_path, name) = uri.skill_md_parts().ok_or_else(|| {
        reject(
            SkillRejection::InvalidSkillUri,
            "URI must identify a SKILL.md file".into(),
        )
    })?;
    let skill_path = skill_path.to_string();
    let name = name.to_string();

    validate_frontmatter(&entry.frontmatter, Some(&name)).map_err(|error| {
        let detail = match error {
            ToolError::Sdk { message, .. } => message,
            other => other.to_string(),
        };
        reject(SkillRejection::InvalidFrontmatter, detail)
    })?;

    let resources = entry.resources.as_ref().ok_or_else(|| {
        reject(
            SkillRejection::MissingManifest,
            "resources manifest is missing".into(),
        )
    })?;
    if resources.len() > MAX_RESOURCES_PER_SKILL {
        return Err(reject(
            SkillRejection::ManifestTooLarge,
            format!("resources manifest exceeds {MAX_RESOURCES_PER_SKILL} entries"),
        ));
    }
    let total_size = resources
        .iter()
        .try_fold(0_u64, |total, resource| total.checked_add(resource.size))
        .ok_or_else(|| {
            reject(
                SkillRejection::ManifestBytesTooLarge,
                "resources manifest byte total overflows u64".into(),
            )
        })?;
    if total_size > MAX_SKILL_TOTAL_BYTES {
        return Err(reject(
            SkillRejection::ManifestBytesTooLarge,
            format!("resources manifest exceeds {MAX_SKILL_TOTAL_BYTES} total bytes"),
        ));
    }

    // Everything in the manifest must sit under the skill's own directory, which
    // is the entry URI with the trailing `/SKILL.md` removed.
    let skill_root = format!("{skill_path}/");
    let canonical_entry_uri = uri.to_uri();

    let mut seen = BTreeSet::new();
    let mut has_skill_md = false;
    for resource in resources {
        parse_digest(&resource.digest).map_err(|_| {
            reject(
                SkillRejection::InvalidDigest,
                "manifest resource digest must be canonical sha256".into(),
            )
        })?;

        // Reject before the prefix test so a malformed URI cannot slip through
        // on a lucky string match.
        let resource_uri = parse_skill_resource_uri(&resource.uri).map_err(|_| {
            reject(
                SkillRejection::ManifestUriOutOfNamespace,
                "manifest resource URI does not satisfy the required structure".into(),
            )
        })?;
        // Now that any scheme is accepted, a manifest could otherwise name a
        // file under a *different* scheme and still satisfy the prefix test on
        // some other axis. Every file of a skill lives in that skill's
        // directory, which means one scheme per skill.
        if resource_uri.scheme() != uri.scheme() {
            return Err(reject(
                SkillRejection::ManifestUriOutOfNamespace,
                "manifest resource uses a different URI scheme".into(),
            ));
        }
        if !resource_uri.full_path().starts_with(&skill_root) {
            return Err(reject(
                SkillRejection::ManifestUriOutOfNamespace,
                "manifest resource is outside the skill directory".into(),
            ));
        }
        let canonical_resource_uri = resource_uri.to_uri();
        if !seen.insert(canonical_resource_uri.clone()) {
            return Err(reject(
                SkillRejection::ManifestDuplicateUri,
                "manifest contains a duplicate resource URI".into(),
            ));
        }
        if canonical_resource_uri == canonical_entry_uri {
            has_skill_md = true;
        }
    }
    if !has_skill_md {
        return Err(reject(
            SkillRejection::ManifestMissingSkillMd,
            "manifest does not include the skill's own SKILL.md".into(),
        ));
    }

    Ok(ValidatedSkill {
        uri,
        name,
        entry: entry.clone(),
    })
}

impl ValidatedSkill {
    /// Look up a file URI in this skill's manifest, returning its digest.
    ///
    /// A URI absent from the manifest is, per the SEP, a change to the skill and
    /// a verification failure equivalent to a digest mismatch — not a mere
    /// lookup miss. Callers must not fall back to reading it.
    #[must_use]
    pub fn digest_for(&self, uri: &str) -> Option<&str> {
        self.entry
            .resources
            .as_ref()?
            .iter()
            .find(|resource| resource.uri == uri)
            .map(|resource| resource.digest.as_str())
    }

    /// Look up the complete manifest binding for one resource URI.
    #[must_use]
    pub fn resource_for(&self, uri: &str) -> Option<&crate::skills::wire::SkillResource> {
        self.entry
            .resources
            .as_ref()?
            .iter()
            .find(|resource| resource.uri == uri)
    }
}

/// Verify fetched bytes against the digest the manifest published for `uri`.
///
/// A match proves the file and the entry are consistent with each other. It
/// proves nothing about whether either is trustworthy — the SEP is explicit
/// that an intermediary can rewrite both together.
pub fn verify_manifest_file(
    skill: &ValidatedSkill,
    uri: &str,
    bytes: &[u8],
) -> Result<(), ToolError> {
    let Some(resource) = skill.resource_for(uri) else {
        return Err(ToolError::Sdk {
            sdk_kind: "validation_failed".to_string(),
            message: format!(
                "`{uri}` is not listed in the manifest for skill `{}`; an unlisted file is a change to the skill",
                skill.name
            ),
        });
    };
    if u64::try_from(bytes.len()).ok() != Some(resource.size) {
        return Err(ToolError::Sdk {
            sdk_kind: "validation_failed".to_string(),
            message: format!(
                "content length of `{uri}` does not match the size published for skill `{}`",
                skill.name
            ),
        });
    }
    let digest = parse_digest(&resource.digest)?;
    if !digest.matches(bytes) {
        return Err(ToolError::Sdk {
            sdk_kind: "validation_failed".to_string(),
            message: format!(
                "content of `{uri}` does not match the digest published for skill `{}`",
                skill.name
            ),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::digest::ResourceDigest;
    use crate::skills::wire::SkillResource;
    use serde_json::json;

    fn entry_with(uri: &str, name: &str, resources: Option<Vec<SkillResource>>) -> SkillEntry {
        SkillEntry {
            uri: uri.to_string(),
            frontmatter: json!({ "name": name, "description": "d" })
                .as_object()
                .expect("object")
                .clone(),
            resources,
            meta: None,
        }
    }

    fn resource(uri: &str, bytes: &[u8]) -> SkillResource {
        SkillResource {
            uri: uri.to_string(),
            digest: ResourceDigest::of_bytes(bytes).to_wire(),
            size: bytes.len() as u64,
        }
    }

    fn valid_entry() -> SkillEntry {
        entry_with(
            "skill://labby/using-labby/SKILL.md",
            "using-labby",
            Some(vec![
                resource("skill://labby/using-labby/SKILL.md", b"body"),
                resource("skill://labby/using-labby/references/x.md", b"ref"),
            ]),
        )
    }

    #[test]
    fn accepts_a_well_formed_entry() {
        let validated = validate_skill_entry(&valid_entry()).expect("valid");
        assert_eq!(validated.name, "using-labby");
        assert_eq!(validated.uri.origin(), "labby");
    }

    #[test]
    fn accepts_equivalent_scheme_spellings_within_one_manifest() {
        let entry = entry_with(
            "GitHub://owner/refunds/SKILL.md",
            "refunds",
            Some(vec![resource("github://owner/refunds/SKILL.md", b"body")]),
        );

        validate_skill_entry(&entry).expect("URI schemes are case-insensitive");
    }

    #[test]
    fn accepts_nested_skill_path_naming() {
        let entry = entry_with(
            "skill://acme/billing/refunds/SKILL.md",
            "refunds",
            Some(vec![resource(
                "skill://acme/billing/refunds/SKILL.md",
                b"x",
            )]),
        );
        assert_eq!(validate_skill_entry(&entry).expect("valid").name, "refunds");
    }

    #[test]
    fn rejects_missing_manifest_as_unverifiable() {
        let entry = entry_with("skill://labby/gen/SKILL.md", "gen", None);
        assert_eq!(
            validate_skill_entry(&entry),
            Err(SkillRejection::MissingManifest)
        );
    }

    #[test]
    fn rejects_manifest_without_skill_md_entry() {
        let entry = entry_with(
            "skill://labby/x/SKILL.md",
            "x",
            Some(vec![resource("skill://labby/x/other.md", b"o")]),
        );
        assert_eq!(
            validate_skill_entry(&entry),
            Err(SkillRejection::ManifestMissingSkillMd)
        );
    }

    #[test]
    fn rejects_cross_origin_and_foreign_scheme_manifest_uris() {
        for foreign in [
            "skill://other-origin/x/leak.md",
            "skill://labby/different-skill/leak.md",
            "file:///etc/passwd",
            "lab://catalog",
            "https://example.com/x.md",
        ] {
            let entry = entry_with(
                "skill://labby/x/SKILL.md",
                "x",
                Some(vec![
                    resource("skill://labby/x/SKILL.md", b"body"),
                    resource(foreign, b"leak"),
                ]),
            );
            assert_eq!(
                validate_skill_entry(&entry),
                Err(SkillRejection::ManifestUriOutOfNamespace),
                "should reject manifest URI {foreign}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_manifest_uris() {
        // The SEP requires each file exactly once, so a repeat is invalid even
        // when both copies agree.
        let entry = entry_with(
            "skill://labby/x/SKILL.md",
            "x",
            Some(vec![
                resource("skill://labby/x/SKILL.md", b"body"),
                resource("skill://labby/x/SKILL.md", b"body"),
            ]),
        );
        assert_eq!(
            validate_skill_entry(&entry),
            Err(SkillRejection::ManifestDuplicateUri)
        );
    }

    #[test]
    fn rejects_duplicate_uri_with_conflicting_digests() {
        let mut entry = entry_with(
            "skill://labby/x/SKILL.md",
            "x",
            Some(vec![resource("skill://labby/x/SKILL.md", b"body")]),
        );
        entry
            .resources
            .as_mut()
            .expect("manifest")
            .push(SkillResource {
                uri: "skill://labby/x/SKILL.md".to_string(),
                digest: ResourceDigest::of_bytes(b"different").to_wire(),
                size: b"different".len() as u64,
            });
        assert_eq!(
            validate_skill_entry(&entry),
            Err(SkillRejection::ManifestDuplicateUri)
        );
    }

    #[test]
    fn rejects_bad_digests() {
        for bad in ["sha1:abc", "notadigest", "sha256:XYZ"] {
            let entry = entry_with(
                "skill://labby/x/SKILL.md",
                "x",
                Some(vec![SkillResource {
                    uri: "skill://labby/x/SKILL.md".to_string(),
                    digest: bad.to_string(),
                    size: 0,
                }]),
            );
            assert_eq!(
                validate_skill_entry(&entry),
                Err(SkillRejection::InvalidDigest),
                "should reject digest {bad}"
            );
        }
    }

    #[test]
    fn rejects_frontmatter_name_disagreeing_with_path() {
        let entry = entry_with(
            "skill://labby/using-labby/SKILL.md",
            "something-else",
            Some(vec![resource("skill://labby/using-labby/SKILL.md", b"x")]),
        );
        assert_eq!(
            validate_skill_entry(&entry),
            Err(SkillRejection::InvalidFrontmatter)
        );
    }

    #[test]
    fn accepts_claude_compatible_allowed_tools_list() {
        let mut entry = valid_entry();
        entry.frontmatter.insert(
            "allowed-tools".to_string(),
            serde_json::json!(["Read", "Write"]),
        );

        validate_skill_entry_detailed(&entry).expect("bounded string lists are compatible");
    }

    #[test]
    fn detailed_rejection_does_not_echo_hostile_uri_or_digest_input() {
        let secret_uri = "skill://labby/secret-token?credential=hunter2";
        let mut entry = valid_entry();
        entry.uri = secret_uri.to_string();

        let rejection = validate_skill_entry_detailed(&entry).unwrap_err();
        assert_eq!(rejection.reason, SkillRejection::InvalidSkillUri);
        assert!(!rejection.detail.contains(secret_uri));
        assert!(!rejection.detail.contains("hunter2"));

        let mut entry = valid_entry();
        entry.resources.as_mut().unwrap()[0].digest = "secret-algorithm:hunter2".into();
        let rejection = validate_skill_entry_detailed(&entry).unwrap_err();
        assert_eq!(rejection.reason, SkillRejection::InvalidDigest);
        assert!(!rejection.detail.contains("hunter2"));

        let mut entry = valid_entry();
        entry
            .frontmatter
            .insert("name".into(), serde_json::json!("hunter2!"));
        let rejection = validate_skill_entry_detailed(&entry).unwrap_err();
        assert_eq!(rejection.reason, SkillRejection::InvalidFrontmatter);
        assert!(!rejection.detail.contains("hunter2"));

        let mut entry = valid_entry();
        entry.frontmatter.insert(
            "metadata".into(),
            serde_json::json!({"hunter2": ["not", "a", "string"]}),
        );
        let rejection = validate_skill_entry_detailed(&entry).unwrap_err();
        assert_eq!(rejection.reason, SkillRejection::InvalidFrontmatter);
        assert!(!rejection.detail.contains("hunter2"));
    }

    #[test]
    fn rejects_oversized_manifest() {
        let mut resources = vec![resource("skill://labby/x/SKILL.md", b"body")];
        for index in 0..MAX_RESOURCES_PER_SKILL {
            resources.push(resource(&format!("skill://labby/x/f{index}.md"), b"f"));
        }
        let entry = entry_with("skill://labby/x/SKILL.md", "x", Some(resources));
        assert_eq!(
            validate_skill_entry(&entry),
            Err(SkillRejection::ManifestTooLarge)
        );
    }

    #[test]
    fn rejects_manifest_over_total_byte_limit() {
        let mut entry = valid_entry();
        entry.resources.as_mut().expect("manifest")[0].size = MAX_SKILL_TOTAL_BYTES + 1;
        assert_eq!(
            validate_skill_entry(&entry),
            Err(SkillRejection::ManifestBytesTooLarge)
        );
    }

    #[test]
    fn manifest_total_size_accepts_limit_and_rejects_cumulative_and_integer_overflow() {
        let mut exact = valid_entry();
        let resources = exact.resources.as_mut().expect("manifest");
        resources[0].size = MAX_SKILL_TOTAL_BYTES;
        for resource in &mut resources[1..] {
            resource.size = 0;
        }
        assert!(validate_skill_entry(&exact).is_ok());

        let mut cumulative = valid_entry();
        let resources = cumulative.resources.as_mut().expect("manifest");
        resources[0].size = MAX_SKILL_TOTAL_BYTES;
        for resource in &mut resources[1..] {
            resource.size = 0;
        }
        resources.push(resource("skill://labby/using-labby/extra.md", b"x"));
        assert_eq!(
            validate_skill_entry(&cumulative),
            Err(SkillRejection::ManifestBytesTooLarge)
        );

        let mut overflow = valid_entry();
        let resources = overflow.resources.as_mut().expect("manifest");
        resources[0].size = u64::MAX;
        for resource in &mut resources[1..] {
            resource.size = 0;
        }
        resources.push(resource("skill://labby/using-labby/extra.md", b"x"));
        assert_eq!(
            validate_skill_entry(&overflow),
            Err(SkillRejection::ManifestBytesTooLarge)
        );
    }

    #[test]
    fn verifies_listed_file_and_rejects_mismatch_and_unlisted() {
        let skill = validate_skill_entry(&valid_entry()).expect("valid");

        verify_manifest_file(&skill, "skill://labby/using-labby/SKILL.md", b"body")
            .expect("matching bytes verify");

        let err = verify_manifest_file(&skill, "skill://labby/using-labby/SKILL.md", b"tampered")
            .expect_err("mismatch");
        assert!(err.to_string().contains("does not match the size"));

        let err = verify_manifest_file(&skill, "skill://labby/using-labby/SKILL.md", b"evil")
            .expect_err("same-size digest mismatch");
        assert!(err.to_string().contains("does not match the digest"));

        let err = verify_manifest_file(&skill, "skill://labby/using-labby/unlisted.md", b"x")
            .expect_err("unlisted");
        assert!(err.to_string().contains("not listed in the manifest"));
    }

    #[test]
    fn rejection_reasons_are_stable_and_unique() {
        let all = [
            SkillRejection::InvalidSkillUri,
            SkillRejection::InvalidFrontmatter,
            SkillRejection::MissingManifest,
            SkillRejection::InvalidDigest,
            SkillRejection::ManifestUriOutOfNamespace,
            SkillRejection::ManifestMissingSkillMd,
            SkillRejection::ManifestDuplicateUri,
            SkillRejection::ManifestTooLarge,
            SkillRejection::ManifestBytesTooLarge,
        ];
        let unique: BTreeSet<_> = all.iter().map(|reason| reason.as_str()).collect();
        assert_eq!(unique.len(), all.len());
    }
}
