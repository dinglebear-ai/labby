//! `skill://` URI parsing for the skills extension (SEP-2640).
//!
//! # The split this parser deliberately does not perform
//!
//! A skill resource URI is `skill://<skill-path>/<file-path>`, where
//! `<skill-path>` may nest to arbitrary depth (`acme/billing/refunds`) and
//! `<file-path>` may itself be multi-segment (`references/FORMS.md`). Given
//! `skill://pdf-processing/references/FORMS.md` in isolation there is no way to
//! know whether the skill is `pdf-processing` (file `references/FORMS.md`) or
//! `pdf-processing/references` (file `FORMS.md`) — both are well-formed. The
//! split is therefore resolved by *manifest lookup*, never positionally, and
//! this parser returns the path as one opaque remainder.
//!
//! The single exception is the canonical `.../SKILL.md` form, which the SEP
//! makes explicit precisely so that "the skill name is always recoverable from
//! the URI alone, without reading frontmatter". [`SkillUri::skill_md_parts`]
//! exposes that case and only that case.
//!
//! # Origin labels
//!
//! Labby prefixes proxied skills with an origin label so two upstreams serving
//! the same skill name can never shadow one another (threat model T8). That
//! label lands in the first segment, which RFC 3986 treats as the authority
//! component; the SEP assigns it no semantics and tells clients not to resolve
//! it, so using it as a routing label is within the convention.

use crate::error::ToolError;
use crate::skills::limits::{MAX_URI_CHARS, MAX_URI_SEGMENT_CHARS};

/// URI scheme prefix for skill resources.
pub const SKILL_URI_SCHEME: &str = "skill://";

/// The file name the SEP requires to be explicit in every skill's root URI.
pub const SKILL_MD_FILE: &str = "SKILL.md";

/// Origin label reserved for skills Labby serves itself. No upstream may claim
/// it; enforced at config-validation time for upstreams with skills proxying on.
pub const FIRST_PARTY_ORIGIN: &str = "labby";

/// A parsed `skill://` URI: an origin label plus an unsplit remainder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillUri {
    origin: String,
    path: String,
}

impl SkillUri {
    /// The first path segment — Labby's origin label for the serving side.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Everything after the origin label, unsplit. Resolve skill-vs-file
    /// against a manifest; do not slice this positionally.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// True when this URI names a skill root document (`.../SKILL.md`).
    #[must_use]
    pub fn is_skill_md(&self) -> bool {
        self.path == SKILL_MD_FILE || self.path.ends_with(concat!("/", "SKILL.md"))
    }

    /// For a canonical `.../SKILL.md` URI, the `(skill_path, name)` pair the SEP
    /// guarantees is readable without fetching frontmatter. `None` for any other
    /// file, where the split requires a manifest.
    ///
    /// `skill_path` is relative to the origin label: for
    /// `skill://acme/billing/refunds/SKILL.md` parsed with origin `acme`, the
    /// skill path is `billing/refunds` and the name is `refunds`.
    #[must_use]
    pub fn skill_md_parts(&self) -> Option<(&str, &str)> {
        let skill_path = self.path.strip_suffix(SKILL_MD_FILE)?;
        let skill_path = skill_path.strip_suffix('/').unwrap_or(skill_path);
        if skill_path.is_empty() {
            // `skill://<origin>/SKILL.md` — the origin label is the whole skill
            // path, so the name is the origin itself.
            return Some(("", self.origin.as_str()));
        }
        let name = skill_path.rsplit('/').next().unwrap_or(skill_path);
        Some((skill_path, name))
    }

    /// Render back to canonical `skill://origin/path` form.
    #[must_use]
    pub fn to_uri(&self) -> String {
        format!("{SKILL_URI_SCHEME}{}/{}", self.origin, self.path)
    }

    /// Re-render this URI under a different origin label, leaving the remainder
    /// byte-identical. Used when minting proxied URIs: the *content* is
    /// untouched, so digests computed by the upstream stay valid.
    #[must_use]
    pub fn with_origin(&self, origin: &str) -> Self {
        Self {
            origin: origin.to_string(),
            path: self.path.clone(),
        }
    }
}

fn invalid(uri: &str, why: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("invalid skill URI `{uri}`: {why}"),
    }
}

/// Reject a path segment that is empty, a dot-segment, or over-long.
///
/// Dot-segments are rejected rather than normalized: a skill URI is a stable
/// identifier that gets compared against a manifest, and silently collapsing
/// `a/../b` would let two different-looking URIs resolve to one manifest entry.
fn check_segment(uri: &str, segment: &str) -> Result<(), ToolError> {
    if segment.is_empty() {
        return Err(invalid(uri, "path contains an empty segment"));
    }
    if segment == "." || segment == ".." {
        return Err(invalid(uri, "path contains a `.` or `..` segment"));
    }
    if segment.chars().count() > MAX_URI_SEGMENT_CHARS {
        return Err(invalid(
            uri,
            &format!("path segment exceeds {MAX_URI_SEGMENT_CHARS} characters"),
        ));
    }
    Ok(())
}

/// True when `label` is usable as an origin label: lowercase ASCII letters,
/// digits, and hyphens, not empty, no leading or trailing hyphen.
///
/// The SEP only asks that the first segment be a valid RFC 3986 `reg-name`;
/// this is deliberately tighter so an origin label cannot vary by case or
/// carry characters that render ambiguously next to another label.
#[must_use]
pub fn is_valid_origin_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_URI_SEGMENT_CHARS
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Parse a `skill://` URI into an origin label and an opaque remainder.
///
/// Rejects: a non-`skill://` scheme, an over-long URI, an origin-only URI with
/// no file part, a malformed origin label, and any empty, dot, or over-long
/// segment.
pub fn parse_skill_uri(uri: &str) -> Result<SkillUri, ToolError> {
    if uri.chars().count() > MAX_URI_CHARS {
        return Err(invalid(uri, &format!("exceeds {MAX_URI_CHARS} characters")));
    }
    let rest = uri
        .strip_prefix(SKILL_URI_SCHEME)
        .ok_or_else(|| invalid(uri, "expected the `skill://` scheme"))?;
    if rest.contains('?') || rest.contains('#') {
        return Err(invalid(uri, "query and fragment components are not allowed"));
    }

    let mut segments = rest.split('/');
    let origin = segments
        .next()
        .ok_or_else(|| invalid(uri, "missing origin segment"))?;
    if !is_valid_origin_label(origin) {
        return Err(invalid(
            uri,
            "origin label must be lowercase alphanumeric with interior hyphens",
        ));
    }

    let path_segments: Vec<&str> = segments.collect();
    if path_segments.is_empty() {
        return Err(invalid(
            uri,
            "URI names only an origin, with no path to a skill file",
        ));
    }
    for segment in &path_segments {
        check_segment(uri, segment)?;
    }

    Ok(SkillUri {
        origin: origin.to_string(),
        path: path_segments.join("/"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_flat_first_party_uri() {
        let uri = parse_skill_uri("skill://labby/using-labby/SKILL.md").expect("valid");
        assert_eq!(uri.origin(), "labby");
        assert_eq!(uri.path(), "using-labby/SKILL.md");
        assert!(uri.is_skill_md());
        assert_eq!(uri.skill_md_parts(), Some(("using-labby", "using-labby")));
    }

    #[test]
    fn parses_nested_file_path() {
        let uri =
            parse_skill_uri("skill://labby/using-labby/references/x.md").expect("valid");
        assert_eq!(uri.path(), "using-labby/references/x.md");
        assert!(!uri.is_skill_md());
        // Deliberately unsplit: only a manifest can say whether the skill is
        // `using-labby` or `using-labby/references`.
        assert_eq!(uri.skill_md_parts(), None);
    }

    #[test]
    fn recovers_name_from_nested_skill_path() {
        // The SEP's own example: prefix `billing`, name `refunds`.
        let uri = parse_skill_uri("skill://acme/billing/refunds/SKILL.md").expect("valid");
        assert_eq!(uri.origin(), "acme");
        assert_eq!(uri.skill_md_parts(), Some(("billing/refunds", "refunds")));
    }

    #[test]
    fn rejects_non_skill_scheme() {
        for uri in [
            "file:///etc/passwd",
            "lab://catalog",
            "https://example.com/SKILL.md",
        ] {
            assert!(parse_skill_uri(uri).is_err(), "should reject {uri}");
        }
    }

    #[test]
    fn rejects_dot_and_empty_segments() {
        for uri in [
            "skill://labby/../etc/SKILL.md",
            "skill://labby/./SKILL.md",
            "skill://labby//SKILL.md",
            "skill://labby/using-labby/",
        ] {
            assert!(parse_skill_uri(uri).is_err(), "should reject {uri}");
        }
    }

    #[test]
    fn rejects_origin_only_uri() {
        assert!(parse_skill_uri("skill://labby").is_err());
    }

    #[test]
    fn rejects_malformed_origin_labels() {
        for uri in [
            "skill://Labby/x/SKILL.md",
            "skill://-labby/x/SKILL.md",
            "skill://labby-/x/SKILL.md",
            "skill://lab_by/x/SKILL.md",
            "skill://lab.by/x/SKILL.md",
        ] {
            assert!(parse_skill_uri(uri).is_err(), "should reject {uri}");
        }
    }

    #[test]
    fn rejects_oversized_segment_and_uri() {
        let long_segment = "a".repeat(MAX_URI_SEGMENT_CHARS + 1);
        assert!(parse_skill_uri(&format!("skill://labby/{long_segment}/SKILL.md")).is_err());

        let many = std::iter::repeat_n("seg", 400).collect::<Vec<_>>().join("/");
        assert!(parse_skill_uri(&format!("skill://labby/{many}/SKILL.md")).is_err());
    }

    #[test]
    fn rejects_query_and_fragment() {
        assert!(parse_skill_uri("skill://labby/x/SKILL.md?a=1").is_err());
        assert!(parse_skill_uri("skill://labby/x/SKILL.md#frag").is_err());
    }

    #[test]
    fn with_origin_rewrites_only_the_label() {
        let uri = parse_skill_uri("skill://upstream-a/billing/refunds/SKILL.md").expect("valid");
        let rewritten = uri.with_origin("gh");
        assert_eq!(rewritten.origin(), "gh");
        assert_eq!(rewritten.path(), uri.path());
        assert_eq!(rewritten.to_uri(), "skill://gh/billing/refunds/SKILL.md");
    }

    #[test]
    fn origin_label_validation_matches_parser() {
        assert!(is_valid_origin_label("labby"));
        assert!(is_valid_origin_label("upstream-2"));
        assert!(!is_valid_origin_label(""));
        assert!(!is_valid_origin_label("Labby"));
        assert!(!is_valid_origin_label("-x"));
        assert!(!is_valid_origin_label("x-"));
        assert!(!is_valid_origin_label("x_y"));
    }
}
