//! Drift protection binding `docs/contracts/skills-extension.md` to the Rust
//! definitions it documents.
//!
//! The contract doc is the vendored, pinned reading of an **unmerged** draft
//! (SEP-2640). Its value depends entirely on the doc and the code saying the
//! same thing, so this test asserts the load-bearing facts appear in both: the
//! pinned commit, the error-kind classifications, the budget values, and the
//! wire constants.
//!
//! Following `agent_error_schema.rs`, the document is read as plain text — no
//! markdown or schema dependency. A test that needs a parser to check a
//! contract tends to stop being run.

use std::path::{Path, PathBuf};

use labby_runtime::agent_error::{
    AgentErrorOrigin, AgentRecoveryAction, AgentSameArgumentsRetry, AgentSideEffectRisk,
    metadata_for_kind, origin_for_kind, recovery_for_kind, side_effects_for_kind,
};
use labby_runtime::skills::{
    KIND_SKILL_DIGEST_MISMATCH, KIND_SKILL_MANIFEST_STALE, SKILLS_EXTENSION_KEY, SKILLS_GET_METHOD,
    SKILLS_LIST_METHOD, limits,
};

/// Mirror commit the contract is pinned to. Changing the pin means re-reading
/// the draft, so it is asserted in both places deliberately.
const PINNED_COMMIT: &str = "9f55cd349932ba00fc18402873c9eb2d2c2e78cb";

/// Upstream commit the mirror synced from.
const UPSTREAM_COMMIT: &str = "0eb05fe9fbd6";

fn contract_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/contracts/skills-extension.md")
        .canonicalize()
        .expect("contract document should exist at docs/contracts/skills-extension.md")
}

fn contract_text() -> String {
    std::fs::read_to_string(contract_path()).expect("contract document should be readable")
}

/// Flatten the document for substring matching: strip markdown blockquote
/// markers and emphasis, then collapse all whitespace runs to single spaces.
///
/// Quoted spec sentences wrap across lines, so a raw substring search would
/// break every time the file is reflowed — which would train people to loosen
/// the assertions rather than fix the doc.
fn flattened_contract_text() -> String {
    let raw = contract_text();
    let unmarked: String = raw
        .lines()
        .map(|line| line.trim_start().trim_start_matches('>').trim_start())
        .collect::<Vec<_>>()
        .join(" ");
    unmarked
        .replace("**", "")
        .replace('*', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn contract_pins_the_revision_the_code_was_written_against() {
    let text = contract_text();
    assert!(
        text.contains(PINNED_COMMIT),
        "contract must pin mirror commit {PINNED_COMMIT}"
    );
    assert!(
        text.contains(UPSTREAM_COMMIT),
        "contract must record upstream provenance {UPSTREAM_COMMIT}"
    );
    assert!(
        text.contains("unmerged"),
        "contract must state that the SEP is an unmerged draft"
    );
}

#[test]
fn contract_documents_the_wire_constants_the_code_uses() {
    let text = contract_text();
    for constant in [
        SKILLS_EXTENSION_KEY,
        SKILLS_LIST_METHOD,
        SKILLS_GET_METHOD,
        "sha256:",
    ] {
        assert!(
            text.contains(constant),
            "contract must document the wire constant `{constant}`"
        );
    }
}

#[test]
fn skills_error_kinds_classify_as_the_contract_documents() {
    for kind in [KIND_SKILL_DIGEST_MISMATCH, KIND_SKILL_MANIFEST_STALE] {
        assert_eq!(
            origin_for_kind(kind),
            AgentErrorOrigin::Validation,
            "{kind} must classify as validation, not the runtime catch-all"
        );
        assert_eq!(
            side_effects_for_kind(kind),
            AgentSideEffectRisk::NoneExpected,
            "{kind} returns no bytes, so it commits nothing"
        );

        let recovery = recovery_for_kind(kind, None, false);
        assert_eq!(
            recovery.action,
            AgentRecoveryAction::Rediscover,
            "{kind} must follow the SEP's prescribed refresh-and-retry recovery"
        );
        assert_eq!(
            recovery.same_arguments,
            AgentSameArgumentsRetry::Never,
            "{kind} must forbid replaying the identical read"
        );
        assert!(
            recovery.guidance.contains(SKILLS_GET_METHOD),
            "{kind} guidance must name skills/get; the generic rediscover text \
             points at actions, tools, prompts, and resources, none of which is \
             the method a caller needs here"
        );
    }
}

#[test]
fn skills_error_kinds_are_not_the_unknown_kind_fallback() {
    // The failure this guards against is silent: an unregistered kind still
    // produces a well-formed envelope, just with catch-all advice. Comparing
    // against a deliberately unknown kind proves registration actually happened.
    let unknown = metadata_for_kind("definitely-not-a-registered-kind", None);
    for kind in [KIND_SKILL_DIGEST_MISMATCH, KIND_SKILL_MANIFEST_STALE] {
        let registered = metadata_for_kind(kind, None);
        assert_ne!(
            registered.recovery.action, unknown.recovery.action,
            "{kind} is falling through to the unknown-kind catch-all"
        );
        assert_ne!(
            registered.origin, unknown.origin,
            "{kind} is falling through to the unknown-kind catch-all"
        );
    }
}

#[test]
fn response_too_large_is_registered() {
    // Predates the classification tables and was never registered, so it
    // silently produced runtime/inspect_and_escalate advice for a plain payload
    // cap. Registered with the payload-limit family it belongs to.
    assert_eq!(
        origin_for_kind("response_too_large"),
        AgentErrorOrigin::Budget
    );
    assert_eq!(
        recovery_for_kind("response_too_large", None, false).action,
        AgentRecoveryAction::ReduceWork
    );
}

#[test]
fn contract_documents_the_error_kinds_it_classifies() {
    let text = contract_text();
    for kind in [KIND_SKILL_DIGEST_MISMATCH, KIND_SKILL_MANIFEST_STALE] {
        assert!(
            text.contains(kind),
            "contract must document the `{kind}` error kind"
        );
    }
    assert!(
        text.contains("rediscover"),
        "contract must document the recovery classification"
    );
}

#[test]
fn contract_documents_the_budget_values_in_force() {
    let text = contract_text();
    for (label, value) in [
        ("skills per upstream", limits::MAX_SKILLS_PER_UPSTREAM),
        ("resources per skill", limits::MAX_RESOURCES_PER_SKILL),
        ("list pages", limits::MAX_LIST_PAGES),
        ("URI segment chars", limits::MAX_URI_SEGMENT_CHARS),
        ("URI chars", limits::MAX_URI_CHARS),
    ] {
        assert!(
            text.contains(&value.to_string()),
            "contract must document the {label} budget ({value})"
        );
    }
    assert!(
        text.contains(&limits::SKILLS_LIST_TIMEOUT.as_secs().to_string()),
        "contract must document the skills/list wall-clock budget"
    );
}

#[test]
fn contract_records_the_non_obvious_spec_requirements() {
    let text = flattened_contract_text();
    for (requirement, needle) in [
        (
            "digests are not a security boundary",
            "MUST NOT treat a digest match as a security boundary",
        ),
        (
            "unlisted skills must still load",
            "hosts MUST support loading a skill given only its URI",
        ),
        (
            "empty listings prove nothing",
            "MUST NOT treat an empty or partial listing as proof",
        ),
        ("frontmatter is cross-verified", "compare it field-by-field"),
        ("per-origin namespacing is host-assigned", "host-assigned"),
        (
            "scheme does not confer skill identity",
            "merely because its URI carries a particular scheme",
        ),
        (
            "T3 residual is documented, not closed",
            "documented, not closed",
        ),
    ] {
        assert!(
            text.contains(needle),
            "contract must record that {requirement} (missing: {needle:?})"
        );
    }
}
