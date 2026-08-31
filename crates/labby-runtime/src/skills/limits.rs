//! Hard safety caps for the skills extension (SEP-2640).
//!
//! These combine SEP-2640's required per-skill interoperability limits with
//! stricter host-chosen catalog and parsing budgets. They live here as
//! `pub const` rather than configuration because they exist to bound a
//! *hostile* upstream — an operator-tunable ceiling is a ceiling an operator can
//! be talked into raising.
//!
//! Callers in `labby-gateway` MUST apply the count caps *incrementally, per
//! page*, never by accumulating every page and truncating afterwards. rmcp's
//! own `list_all_resources()` helper accumulates without any page cap, so
//! copying that shape would let a single upstream stream unbounded pages inside
//! the wall-clock budget before any limit engaged.

use std::time::Duration;

/// Maximum skills retained from one upstream's `skills/list`, across all pages.
pub const MAX_SKILLS_PER_UPSTREAM: usize = 256;

/// Maximum skill candidates validated from one upstream's `skills/list`, across all pages.
///
/// This is deliberately higher than [`MAX_SKILLS_PER_UPSTREAM`]: malformed or
/// policy-rejected entries still consume validation work and rejection memory,
/// so an invalid-skill flood needs its own bound instead of relying on the
/// retained-valid-skill ceiling.
pub const MAX_SKILL_CANDIDATES_PER_UPSTREAM: usize = 1024;

/// Maximum entries in a single skill's `resources` manifest.
pub const MAX_RESOURCES_PER_SKILL: usize = 512;

/// Maximum total raw bytes declared across one skill's resource manifest.
pub const MAX_SKILL_TOTAL_BYTES: u64 = 16 * 1024 * 1024;

/// Maximum bytes returned for one Skill resource.
///
/// This matches the operator-local snapshot cap and applies to every provider,
/// so switching source types cannot silently widen the memory budget.
pub const MAX_SKILL_RESOURCE_BYTES: usize = MAX_SKILL_TOTAL_BYTES as usize;

/// Maximum `skills/list` pages traversed for one upstream before the walk stops.
///
/// Terminates both a self-referencing `nextCursor` and the subtler case of a
/// cursor that never repeats but never ends.
pub const MAX_LIST_PAGES: usize = 16;

/// Wall-clock budget for a full `skills/list` traversal against one upstream.
pub const SKILLS_LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// Maximum serialized bytes of one skill entry's `frontmatter` object.
///
/// Applied *before* YAML/JSON parsing, so a frontmatter bomb is rejected on
/// size rather than after a parser has already expanded it.
pub const MAX_FRONTMATTER_BYTES: usize = 16 * 1024;

/// Maximum bytes of a `SKILL.md` frontmatter block parsed for the field-by-field
/// comparison the SEP requires against an entry's `frontmatter`.
pub const MAX_SKILL_MD_FRONTMATTER_BYTES: usize = MAX_FRONTMATTER_BYTES;

/// Maximum characters in any single `skill://` path segment.
pub const MAX_URI_SEGMENT_CHARS: usize = 128;

/// Maximum characters in a whole skill or resource URI.
pub const MAX_URI_CHARS: usize = 1024;

/// Maximum `name` length, per the Agent Skills specification.
pub const MAX_NAME_CHARS: usize = 64;

/// Maximum `description` length, per the Agent Skills specification.
pub const MAX_DESCRIPTION_CHARS: usize = 1024;

/// Maximum `compatibility` length, per the Agent Skills specification.
pub const MAX_COMPATIBILITY_CHARS: usize = 500;
