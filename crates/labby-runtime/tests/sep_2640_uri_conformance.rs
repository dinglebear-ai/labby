//! Conformance for the `skill://` URI structure defined by SEP-2640.
//!
//! Every case here is transcribed from the SEP's own "Resource Mapping →
//! Examples" table and its `skills/list` example result. They are the spec's
//! examples, not ones chosen to suit the implementation — which is the point:
//! Labby's original model treated the first segment as a routing authority with
//! semantics, and the SEP says the opposite:
//!
//! > Per RFC 3986, the first segment of `<skill-path>` occupies the authority
//! > component. This carries no special semantics under this convention.
//!
//! Under the old model `skill://git-workflow/SKILL.md` — the SEP's first
//! example, and the first entry of its `skills/list` example — was rejected at
//! ingest, so a conforming upstream's skills were silently dropped.

use labby_runtime::skills::{FIRST_PARTY_ORIGIN, parse_skill_uri};

/// The spec's examples table, as `(uri, skill_path, name)`.
const SPEC_EXAMPLES: &[(&str, &str, &str)] = &[
    (
        "skill://git-workflow/SKILL.md",
        "git-workflow",
        "git-workflow",
    ),
    (
        "skill://acme/billing/refunds/SKILL.md",
        "acme/billing/refunds",
        "refunds",
    ),
    (
        "skill://pdf-processing/SKILL.md",
        "pdf-processing",
        "pdf-processing",
    ),
];

#[test]
fn every_spec_example_skill_md_resolves_to_its_declared_name() {
    for (uri, expected_path, expected_name) in SPEC_EXAMPLES {
        let parsed = parse_skill_uri(uri).unwrap_or_else(|e| panic!("{uri} must parse: {e}"));
        let (skill_path, name) = parsed
            .skill_md_parts()
            .unwrap_or_else(|| panic!("{uri} must yield a skill path and name"));
        assert_eq!(&skill_path, expected_path, "skill path for {uri}");
        assert_eq!(&name, expected_name, "name for {uri}");
    }
}

#[test]
fn a_one_segment_skill_path_is_legal() {
    // Regression: this exact URI was rejected, because the parser required a
    // segment *after* an assumed origin label.
    let parsed = parse_skill_uri("skill://git-workflow/SKILL.md").expect("legal per the SEP");
    assert_eq!(parsed.skill_md_parts().expect("resolves").1, "git-workflow");
}

#[test]
fn non_skill_md_files_parse_without_being_split_positionally() {
    // The SEP notes the skill/file boundary is not recoverable from the URI
    // alone; only the `.../SKILL.md` form is. These must parse, and must not
    // claim a name.
    for uri in [
        "skill://pdf-processing/references/FORMS.md",
        "skill://pdf-processing/scripts/extract.py",
        "skill://acme/billing/refunds/examples/email.md",
    ] {
        let parsed = parse_skill_uri(uri).unwrap_or_else(|e| panic!("{uri} must parse: {e}"));
        assert!(parsed.skill_md_parts().is_none(), "{uri} is not a SKILL.md");
    }
}

#[test]
fn minting_prepends_and_is_exactly_invertible() {
    // Prepending keeps `final segment == name` at any depth and lets a proxied
    // read recover the upstream's URI. Replacing the first segment silently
    // discarded a real prefix (`acme` below) and was not invertible.
    for (upstream_uri, expected_minted, expected_name) in [
        (
            "skill://git-workflow/SKILL.md",
            "skill://up/git-workflow/SKILL.md",
            "git-workflow",
        ),
        (
            "skill://acme/billing/refunds/SKILL.md",
            "skill://up/acme/billing/refunds/SKILL.md",
            "refunds",
        ),
    ] {
        let upstream = parse_skill_uri(upstream_uri).expect("parses");
        let minted = upstream.with_origin("up").expect("valid label");
        assert_eq!(minted.to_uri(), expected_minted);
        // The name survives relabelling — it is the last segment either way.
        assert_eq!(minted.skill_md_parts().expect("resolves").1, expected_name);
        // Invertible: the remainder is the upstream's own full path verbatim.
        assert_eq!(minted.path(), upstream.full_path());
    }
}

#[test]
fn two_origins_publishing_one_name_stay_distinct() {
    // Why Labby prepends at all: the SEP requires a host to resolve skill names
    // in a per-origin namespace and forbids one origin's skill shadowing
    // another's. Passing both through unchanged would publish one URI twice.
    let upstream = parse_skill_uri("skill://git-workflow/SKILL.md").expect("parses");
    let a = upstream.with_origin("alpha").expect("label");
    let b = upstream.with_origin("beta").expect("label");
    assert_ne!(a.to_uri(), b.to_uri());
    assert_eq!(a.skill_md_parts().unwrap().1, b.skill_md_parts().unwrap().1);
}

#[test]
fn the_first_party_origin_cannot_be_impersonated_by_a_label() {
    let upstream = parse_skill_uri("skill://refunds/SKILL.md").expect("parses");
    assert!(
        upstream
            .with_origin(&format!("{FIRST_PARTY_ORIGIN}/evil"))
            .is_err(),
        "a label containing a separator would re-parse under the reserved origin"
    );
    assert!(
        upstream.with_origin("").is_err(),
        "an empty label breaks round-tripping"
    );
}

#[test]
fn an_upstream_label_labby_would_reject_is_still_parseable_inbound() {
    // Labby's minting grammar is lowercase-and-hyphens, but the SEP only says
    // the first segment SHOULD be a valid RFC 3986 reg-name. Holding inbound
    // URIs to Labby's stricter minting rules rejected conforming upstreams.
    for uri in [
        "skill://Acme/refunds/SKILL.md",
        "skill://team_billing/refunds/SKILL.md",
        "skill://v2.1/refunds/SKILL.md",
    ] {
        assert!(
            parse_skill_uri(uri).is_ok(),
            "{uri} is legal for an upstream to serve"
        );
    }
}

// ── Native schemes ───────────────────────────────────────────────────────────
//
// > A server MAY serve skills under another scheme native to its domain (e.g.,
// > `github://owner/repo/skills/refunds/SKILL.md`). No scheme is privileged:
// > the structural constraints above — `<skill-path>` ending in the skill name,
// > `SKILL.md` explicit in the URI — apply regardless of scheme.

#[test]
fn a_native_scheme_parses_and_yields_the_same_structure() {
    // The SEP's own native-scheme example. Requiring `skill://` here excluded
    // every skill from a conforming upstream that used its own scheme.
    let uri = parse_skill_uri("github://owner/repo/skills/refunds/SKILL.md")
        .expect("a native scheme is legal");
    assert_eq!(uri.scheme(), "github");
    let (skill_path, name) = uri.skill_md_parts().expect("structure applies regardless");
    assert_eq!(skill_path, "owner/repo/skills/refunds");
    assert_eq!(name, "refunds");
}

#[test]
fn structural_constraints_still_bind_under_a_native_scheme() {
    // "No scheme is privileged" cuts both ways: a native scheme buys no
    // exemption from the rules a `skill://` URI must satisfy.
    assert!(parse_skill_uri("github://owner/repo/../../etc/passwd/SKILL.md").is_err());
    assert!(parse_skill_uri("github://owner//SKILL.md").is_err());
}

#[test]
fn a_malformed_scheme_is_still_rejected() {
    for uri in [
        "notascheme/refunds/SKILL.md",
        "1github://owner/refunds/SKILL.md",
        "git hub://owner/refunds/SKILL.md",
    ] {
        assert!(parse_skill_uri(uri).is_err(), "{uri} should be refused");
    }
}

#[test]
fn a_native_scheme_skill_is_published_in_labbys_own_namespace() {
    // Labby is the server downstream, so it publishes under `skill://` whatever
    // the upstream used. The native URI is not reconstructed from this string —
    // it is recovered from the cached manifest, which is what keeps the read
    // routable.
    let native = parse_skill_uri("github://owner/repo/skills/refunds/SKILL.md").expect("parses");
    let minted = native.with_origin("gh").expect("valid label");
    assert_eq!(minted.scheme(), "skill");
    assert_eq!(
        minted.to_uri(),
        "skill://gh/owner/repo/skills/refunds/SKILL.md"
    );
    // The name survives, and the remainder is the upstream's own path, which is
    // what the read matches against.
    assert_eq!(minted.skill_md_parts().expect("resolves").1, "refunds");
    assert_eq!(minted.path(), native.full_path());
}
