# Skills over MCP compatibility implementation

**Status:** Completed and merged. The shared compatibility facade landed in PR
#456 as `ea07f3609` on 2026-08-19. Transport-preservation hardening later
landed in PR #571 as `b6ad22385` on 2026-09-08.

This directory is a historical implementation record. It is not the source of
truth for current Skills behavior. Use:

- [Artifacts And Agent Skills](../../services/SKILLS.md) for current Labby
  lifecycle and operator behavior;
- [Skills extension contract](../../contracts/skills-extension.md) for the
  accepted SEP-2640 wire contract and pinned upstream revision;
- the generated [service](../../generated/service-catalog.md) and
  [action](../../generated/action-catalog.md) catalogs for current compiled
  surface availability.

## Historical purpose

This work made Labby-hosted Agent Skills usable by MCP clients that do not
natively understand the Skills extension without creating a second Skill model.
It extracted shared Skills semantics, added one fixed compatibility service, and
projected the same caller-scoped registry through CLI, HTTP API, MCP, and Code
Mode adapters.

The core decision remains part of the shipped design: one canonical Skill
registry, multiple projections. Native Skills clients use `skills/list`,
`skills/get`, and manifest-bound `resources/read`; compatibility callers use
the fixed read-only `skills.list`, `skills.search`, `skills.get`, and
`skills.read` actions. Skill cardinality does not increase MCP tool cardinality.

## Historical artifacts

- [Specification](./SPEC.md) — implementation-time product and architecture
  specification.
- [Compatibility contract](./CONTRACT.md) — invariants used while landing the
  compatibility projection.
- [Implementation plan](./IMPLEMENTATION_PLAN.md) — phased delivery plan.
- [Progress log](./PROGRESS.md) — dated implementation, verification, review,
  and rebase evidence.

The branch/worktree references and upstream status snapshots in those records
are historical evidence from August 2026. They must not be interpreted as live
branch state or current SEP status. SEP-2640 was accepted on 2026-09-03; the
current accepted pin is maintained only in
[the Skills extension contract](../../contracts/skills-extension.md).
