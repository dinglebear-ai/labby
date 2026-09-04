//! Transport-neutral types for the MCP Skills Extension (SEP-2640).
//!
//! # Status
//!
//! SEP-2640 was accepted on 2026-09-03. The vendored contract in
//! `docs/contracts/skills-extension.md` pins the exact accepted revision this
//! module implements; treat any divergence between that document and this code
//! as a bug in one of them.
//!
//! # Scope
//!
//! This module owns the vocabulary — wire DTOs, URI grammar, digest handling,
//! frontmatter rules, manifest validation, and the safety budgets — and nothing
//! that speaks a transport. The `rmcp` conversions live in `labby-gateway` (the
//! client side, calling upstream servers) and `labby` (the server side, answering
//! downstream clients), keeping this crate free of transport dependencies.
//!
//! # Two rules worth stating up front
//!
//! **A digest match is not a trust boundary.** Digests are unsigned and come
//! from the same server as the content; the SEP names gateways explicitly as
//! able to rewrite both together. Labby is such a gateway. Verification catches
//! inconsistency, corruption, and staleness — not a hostile upstream.
//!
//! **A URI does not make something a skill.** Skill identity comes from a
//! `skills/list` entry or a `skills/get` confirmation, never from the `skill://`
//! scheme alone.

pub mod availability;
pub mod core;
pub mod digest;
pub mod frontmatter;
pub mod limits;
pub mod manifest;
pub mod provider;
pub mod requirements;
pub mod uri;
pub mod wire;

pub use availability::{
    SkillAvailabilitySummary, SkillCompatibilityClassification, SkillCompatibilityItem,
};
pub use core::{SkillDescriptor, SkillId, SkillProviderId, SkillProviderKind};
pub use digest::{DIGEST_ALGORITHM, ResourceDigest, parse_digest};
pub use frontmatter::{
    RESERVED_METADATA_PREFIX, compare_frontmatter, is_valid_skill_name, parse_skill_md_frontmatter,
    validate_frontmatter,
};
pub use manifest::{
    SkillRejection, SkillRejectionDetail, ValidatedSkill, validate_skill_entry,
    validate_skill_entry_detailed, verify_manifest_file,
};
pub use provider::{
    SkillDiscoverRequest, SkillDiscoverResult, SkillDiscoverySource, SkillGetRequest,
    SkillGetResult, SkillProvider, SkillProviderDeadline, SkillProviderEntry, SkillProviderError,
    SkillProviderFuture, SkillProviderResource, SkillResourceReadRequest, SkillResourceReadResult,
    SkillResourceRepresentation,
};
pub use requirements::SkillRequirementsSummary;
pub use uri::{
    FIRST_PARTY_ORIGIN, SKILL_MD_FILE, SKILL_URI_SCHEME, SkillUri, is_valid_origin_label,
    parse_skill_resource_uri, parse_skill_uri,
};
pub use wire::{
    RESOURCES_DIRECTORY_READ_METHOD, SKILL_MD_MIME_TYPE, SKILLS_EXTENSION_KEY, SKILLS_GET_METHOD,
    SKILLS_LIST_METHOD, SkillEntry, SkillResource, SkillsCapability, SkillsGetParams,
    SkillsGetResult, SkillsListParams, SkillsListResult,
};

/// Stable error kind for a skill file whose bytes disagree with the digest its
/// entry published, or for a read of a file the manifest does not list — which
/// the SEP treats as the same class of verification failure.
///
/// The recovery is `rediscover`, not `do_not_retry`: the SEP's own prescribed
/// response is to refresh the entry via `skills/get` (or the catalog via
/// `skills/list`) and proceed from the current `resources` set, and it names
/// benign staleness — the skill was updated after the listing was fetched — as a
/// normal cause alongside corruption.
pub const KIND_SKILL_DIGEST_MISMATCH: &str = "skill_digest_mismatch";

/// Stable error kind for a skill URI that no longer resolves against the cached
/// catalog and that `skills/get` does not answer for either.
pub const KIND_SKILL_MANIFEST_STALE: &str = "skill_manifest_stale";
