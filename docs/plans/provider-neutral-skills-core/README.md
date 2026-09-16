# Provider-neutral Skills core

**Status:** Completed and merged. The provider-neutral Skills core landed in PR
#486 as `625ccf300` on 2026-08-23.

This directory is a historical implementation record. Current product behavior
is documented in [Artifacts And Agent Skills](../../services/SKILLS.md), while
the accepted wire contract remains
[Skills extension](../../contracts/skills-extension.md). The generated
[service](../../generated/service-catalog.md) and
[action](../../generated/action-catalog.md) catalogs are authoritative for
compiled exposure.

## Historical purpose

This work separated Labby's canonical Skill identity, descriptor, availability,
exposure policy, and provider interface from any one discovery or delivery
mechanism. The existing SEP-2640 implementation became one provider adapter,
with bundled and operator-local providers using the same provider-neutral
contract. Compatibility hints such as Agent Skills `compatibility` and
`allowed-tools` remain descriptive requirements, not execution authority.

The shipped implementation preserves the security boundaries developed by this
plan: caller-scoped provider access, fail-closed exposure, bounded discovery and
reads, manifest ownership, lazy resource bodies, and provider-scoped identity.

## Historical artifacts

- [Specification](./SPEC.md) — provider-neutral model and intended behavior.
- [Contract](./CONTRACT.md) — invariants enforced by the implementation.
- [Implementation plan](./IMPLEMENTATION_PLAN.md) — dependency-ordered delivery
  slices and verification gates.
- [Progress log](./PROGRESS.md) — dated implementation and verification evidence.

Any branch, worktree, tracking-bead, or baseline commit named in the progress
record describes the August 2026 implementation session only. It is not current
operational guidance.
