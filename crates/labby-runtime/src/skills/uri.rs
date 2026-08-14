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

/// A parsed `skill://` URI.
///
/// Stores the whole `<skill-path>/<file-path>` and derives the first-segment
/// split, because the SEP's `<skill-path>` spans the first segment too: the
/// skill name is the last segment of the path as a whole, and a one-segment
/// path is legal. Keeping only a post-first-segment remainder made the
/// one-segment form unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillUri {
    full: String,
    /// Byte index of the first `/`, or `full.len()` when there is none.
    split: usize,
}

impl SkillUri {
    fn from_full(full: String) -> Self {
        let split = full.find('/').unwrap_or(full.len());
        Self { full, split }
    }

    /// The first path segment.
    ///
    /// For a URI Labby minted this is the host-assigned origin label it
    /// prepended. For a URI as an upstream published it, this is simply the
    /// first `<skill-path>` segment and carries no routing meaning.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.full[..self.split]
    }

    /// Everything after the first segment, unsplit. Resolve skill-vs-file
    /// against a manifest; do not slice this positionally.
    ///
    /// For a URI Labby minted, this equals the owning upstream's own full path
    /// — that is the inverse of the label prepended by [`with_origin`], and the
    /// value a proxied read is routed on.
    #[must_use]
    pub fn path(&self) -> &str {
        if self.split >= self.full.len() {
            ""
        } else {
            &self.full[self.split + 1..]
        }
    }

    /// True when this URI names a skill root document (`.../SKILL.md`).
    #[must_use]
    pub fn is_skill_md(&self) -> bool {
        self.full == SKILL_MD_FILE || self.full.ends_with(concat!("/", "SKILL.md"))
    }

    /// For a canonical `.../SKILL.md` URI, the `(skill_path, name)` pair the SEP
    /// guarantees is readable without fetching frontmatter. `None` for any other
    /// file, where the split requires a manifest.
    ///
    /// `skill_path` spans every segment before the trailing `/SKILL.md`,
    /// including the first. The SEP is explicit that the first segment is part
    /// of `<skill-path>` and "carries no special semantics" — so
    /// `skill://git-workflow/SKILL.md` is a one-segment skill path naming
    /// `git-workflow`, and `skill://acme/billing/refunds/SKILL.md` is
    /// `acme/billing/refunds` naming `refunds`. Computing this over the
    /// remainder alone dropped the first segment and rejected the one-segment
    /// form outright, which is the SEP's own first example.
    #[must_use]
    pub fn skill_md_parts(&self) -> Option<(&str, &str)> {
        let skill_path = self.full.strip_suffix(SKILL_MD_FILE)?;
        let skill_path = skill_path.strip_suffix('/').unwrap_or(skill_path);
        if skill_path.is_empty() {
            // `skill://SKILL.md` names no skill at all.
            return None;
        }
        let name = skill_path.rsplit('/').next().unwrap_or(skill_path);
        Some((skill_path, name))
    }

    /// The whole `<skill-path>/<file-path>` after the scheme.
    ///
    /// This is what a *server* published. For a URI Labby minted, [`path`] is
    /// this same value as the owning upstream serves it — stripping the label
    /// Labby prepended — which is what makes the mapping invertible.
    #[must_use]
    pub fn full_path(&self) -> &str {
        &self.full
    }

    /// Render back to canonical `skill://origin/path` form.
    #[must_use]
    pub fn to_uri(&self) -> String {
        format!("{SKILL_URI_SCHEME}{}", self.full)
    }

    /// Mint this URI under a host-assigned origin label, **prepending** the
    /// label as an additional `<skill-path>` prefix segment.
    ///
    /// Prepending, not replacing. The SEP defines `<skill-path>` as one or more
    /// segments whose *final* segment is the skill's name, with preceding
    /// segments a server-chosen organizational prefix carrying no semantics.
    /// Prepending therefore stays inside the convention, preserves the
    /// name-is-the-last-segment invariant at any depth, and — unlike replacing
    /// the first segment — is lossless: stripping the label recovers the
    /// upstream's URI exactly, which is what lets a read be routed back.
    ///
    /// Replacing silently discarded a real prefix segment:
    /// `skill://acme/billing/refunds/SKILL.md` became
    /// `skill://<label>/billing/refunds/SKILL.md`, losing `acme`.
    ///
    /// Fallible on purpose. The label arrives from upstream configuration, and
    /// an unvalidated one is a namespace-impersonation vector rather than a
    /// cosmetic problem: `with_origin("labby/evil")` would render
    /// `skill://labby/evil/…`, which re-parses with `labby` — the reserved
    /// first-party origin — as the first segment. An empty label would render
    /// `skill:///…`, which does not re-parse at all.
    pub fn with_origin(&self, origin: &str) -> Result<Self, ToolError> {
        if !is_valid_origin_label(origin) {
            return Err(ToolError::Sdk {
                sdk_kind: "invalid_param".to_string(),
                message: format!(
                    "`{origin}` is not a usable skill origin label: expected lowercase \
                     alphanumeric characters with interior hyphens"
                ),
            });
        }
        Ok(Self::from_full(format!("{origin}/{}", self.full)))
    }
}

fn invalid(uri: &str, why: &str) -> ToolError {
    ToolError::Sdk {
        sdk_kind: "invalid_param".to_string(),
        message: format!("invalid skill URI `{uri}`: {why}"),
    }
}

/// True for characters that must never appear inside a skill URI path segment.
///
/// The parser splits only on `/`, so anything else survives inside a segment.
/// Three classes are refused:
///
/// - `\` and `:` — a segment is opaque here but is a plausible path component
///   later. On Windows both are separators, and `Path::join` discards the base
///   entirely when the joined fragment looks absolute or carries a drive prefix,
///   so `..\..\..\Windows\win.ini` inside one "segment" would escape a cache
///   root while satisfying every check in this module.
/// - control characters (C0 and C1) — no legitimate Agent Skills file path
///   contains them, and NUL in particular truncates in C string handling.
/// - bidirectional formatting overrides — byte equality is unaffected, but a
///   path can be made to *render* as a different one in an operator UI.
fn is_forbidden_segment_char(c: char) -> bool {
    matches!(c, '\\' | ':')
        || c.is_control()
        || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
}

/// Reject a path segment that is empty, a dot-segment, over-long, or carries a
/// character that has no place in a skill file path.
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
    if segment.chars().any(is_forbidden_segment_char) {
        return Err(invalid(
            uri,
            "path segment contains a backslash, colon, control, or bidirectional formatting character",
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
        return Err(invalid(
            uri,
            "query and fragment components are not allowed",
        ));
    }

    let mut segments = rest.split('/');
    let origin = segments
        .next()
        .ok_or_else(|| invalid(uri, "missing first path segment"))?;
    // Every segment is checked for the same hostile characters, but the first
    // is NOT held to Labby's own origin-label grammar. The SEP says the first
    // segment is an ordinary `<skill-path>` segment that SHOULD be a valid RFC
    // 3986 reg-name, which permits far more than lowercase-and-hyphens.
    // Enforcing Labby's minting rules on inbound URIs rejected conforming
    // upstreams outright. Labby's own labels are still validated where they are
    // minted, in `with_origin`.
    check_segment(uri, origin)?;

    // A one-segment skill path is legal and is the SEP's primary example:
    // `skill://git-workflow/SKILL.md` names the skill `git-workflow`. Requiring
    // a second segment rejected it.
    for segment in segments {
        check_segment(uri, segment)?;
    }

    Ok(SkillUri::from_full(rest.to_string()))
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
        // `<skill-path>` spans the first segment too, so it is `labby/using-labby`
        // and the name is its final segment.
        assert_eq!(
            uri.skill_md_parts(),
            Some(("labby/using-labby", "using-labby"))
        );
    }

    #[test]
    fn parses_nested_file_path() {
        let uri = parse_skill_uri("skill://labby/using-labby/references/x.md").expect("valid");
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
        // The SEP names this skill path `acme/billing/refunds` — the first
        // segment is part of it, not a label sitting outside it.
        assert_eq!(
            uri.skill_md_parts(),
            Some(("acme/billing/refunds", "refunds"))
        );
    }

    #[test]
    fn a_one_segment_skill_path_names_that_segment() {
        // Per the SEP this is a one-segment `<skill-path>` naming the skill
        // `git-workflow` — its primary example. Labby previously read the first
        // segment as a routing label and rejected the form outright, silently
        // dropping every such skill from a conforming upstream.
        let uri = parse_skill_uri("skill://git-workflow/SKILL.md").expect("parses");
        assert!(uri.is_skill_md());
        assert_eq!(uri.skill_md_parts(), Some(("git-workflow", "git-workflow")));
    }

    #[test]
    fn a_uri_naming_only_skill_md_names_no_skill() {
        // The one genuinely nameless case: no `<skill-path>` at all.
        let uri = parse_skill_uri("skill://SKILL.md").expect("parses");
        assert_eq!(uri.skill_md_parts(), None);
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
    fn a_bare_single_segment_uri_parses_but_names_no_skill_md() {
        // `skill://labby` is a skill *root* (the SEP's directory form), not a
        // `SKILL.md`, so it parses and simply yields no name.
        let uri = parse_skill_uri("skill://labby").expect("parses as a skill root");
        assert!(!uri.is_skill_md());
        assert_eq!(uri.skill_md_parts(), None);
    }

    #[test]
    fn labbys_minting_grammar_binds_minting_not_inbound_parsing() {
        // These are legal for an upstream to serve: the SEP only says the first
        // segment SHOULD be a valid RFC 3986 reg-name. Rejecting them at parse
        // time applied Labby's own label rules to other servers' URIs.
        for uri in [
            "skill://Labby/x/SKILL.md",
            "skill://lab_by/x/SKILL.md",
            "skill://lab.by/x/SKILL.md",
        ] {
            assert!(parse_skill_uri(uri).is_ok(), "should accept {uri}");
        }
        // But Labby still refuses to *mint* under a label of that shape.
        let uri = parse_skill_uri("skill://x/SKILL.md").expect("valid");
        for label in ["Labby", "-labby", "labby-", "lab_by", "lab.by"] {
            assert!(
                uri.with_origin(label).is_err(),
                "should refuse to mint {label}"
            );
        }
    }

    #[test]
    fn rejects_oversized_segment_and_uri() {
        let long_segment = "a".repeat(MAX_URI_SEGMENT_CHARS + 1);
        assert!(parse_skill_uri(&format!("skill://labby/{long_segment}/SKILL.md")).is_err());

        let many = std::iter::repeat_n("seg", 400)
            .collect::<Vec<_>>()
            .join("/");
        assert!(parse_skill_uri(&format!("skill://labby/{many}/SKILL.md")).is_err());
    }

    #[test]
    fn rejects_query_and_fragment() {
        assert!(parse_skill_uri("skill://labby/x/SKILL.md?a=1").is_err());
        assert!(parse_skill_uri("skill://labby/x/SKILL.md#frag").is_err());
    }

    #[test]
    fn with_origin_prepends_and_preserves_the_upstreams_own_prefix() {
        let uri = parse_skill_uri("skill://upstream-a/billing/refunds/SKILL.md").expect("valid");
        let rewritten = uri.with_origin("gh").expect("valid label");
        assert_eq!(rewritten.origin(), "gh");
        // The remainder is the upstream's whole path, so the mapping inverts.
        assert_eq!(rewritten.path(), uri.full_path());
        assert_eq!(
            rewritten.to_uri(),
            "skill://gh/upstream-a/billing/refunds/SKILL.md"
        );
        // Round-trips: minting a proxied URI must not produce something the
        // parser then reads back differently.
        assert_eq!(
            parse_skill_uri(&rewritten.to_uri()).expect("re-parses"),
            rewritten
        );
    }

    #[test]
    fn with_origin_refuses_a_label_that_would_impersonate_another_origin() {
        let uri = parse_skill_uri("skill://upstream-a/x/SKILL.md").expect("valid");
        // Smuggling a separator would render skill://labby/evil/... which
        // re-parses with the reserved first-party origin in front.
        assert!(uri.with_origin("labby/evil").is_err());
        assert!(uri.with_origin("").is_err());
        assert!(uri.with_origin("Upstream").is_err());
        assert!(uri.with_origin("has space").is_err());
    }

    #[test]
    fn rejects_separator_and_formatting_characters_in_segments() {
        // A backslash segment satisfies every other check and prefix test, but
        // encodes traversal on any platform that treats `\` as a separator.
        for uri in [
            r"skill://labby/x/..\..\..\Windows\win.ini",
            "skill://labby/x/C:/evil.md",
            "skill://labby/x/nul\u{0}byte.md",
            "skill://labby/x/spoof\u{202e}dm.md",
        ] {
            assert!(parse_skill_uri(uri).is_err(), "should reject {uri:?}");
        }
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
