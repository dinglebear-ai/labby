---
title: W19 Artifact Gateway Implementation Plan
created: 2026-08-19
updated: 2026-08-19
---

# W19 Artifact Gateway Implementation Plan

Status: active
Branch: `codex/w19-artifact-gateway-20260819`
Worktree: `/home/jmagar/workspace/labby-w19-artifact-gateway`
Baseline: `origin/main` at `0e21d0474` when W19 implementation began

## Guardrails

1. Keep Artifact business logic in a surface-neutral shared Rust layer.
2. Preserve current Gateway, Skills, Loadouts, and Code Mode behavior.
3. Do not restore Stash, Marketplace, Fleet, or Deploy aliases or protocol surfaces.
4. Keep Depot/Bazaar as hosted publication/registry authority and Axon as crawl/enrichment.
5. Require exact ArtifactInterchange v1 parity with Depot G0 commit `25de725`.
6. Prefer explicit roots, bounded I/O, content verification, path containment, safe errors, and per-Artifact locks.
7. Add migrations only when durable existing state requires them. The first local store is new, so no migration is required.
8. Do not create the W19 PR until the first vertical slice passes targeted tests, format, warnings-as-errors, clippy, and adversarial review.

## Phase 0: contract and archaeology

Status: complete and verified

- Inspect current Labby worktrees, branches, and PRs before touching code.
- Confirm W19 isolated worktree and fast-forward it to settled `origin/main`.
- Read repository/runtime guidance and existing Skills/path/redaction implementation.
- Mine retired implementation mechanics from git history without restoring names/surfaces.
- Inspect Phoenix Artifact implementation/docs as migration evidence.
- Freeze Rust structs and validation to Depot's `dinglebear.artifact-interchange/v1`.
- Copy the exact Depot fixture and add byte-canonical parity tests.

Exit gate: the Depot fixture parses, validates, reserializes byte-for-byte, and produces the frozen component-inventory digest.

## Phase 1: first coherent local vertical slice

Status: complete and verified

- Multi-file file/package snapshot model.
- Stable Artifact identity and deterministic component identity.
- Canonically ordered component inventory.
- Immutable SHA-256 revisions.
- Separate provenance, license, lineage, and publication state.
- Explicit-root canonical local store.
- Per-Artifact advisory locking.
- Separate editable workspace.
- Bounded local import with symlink/path/size protections.
- Verified local export with overwrite policy and default secret-content guard.
- Basic fork with exact revision pinning.
- Explicit upstream observation that does not mutate local bytes.
- Agent Skill adapter over existing `ValidatedSkill` and manifest verification.

Exit gate:

- focused Artifact unit tests pass;
- frozen cross-runtime conformance tests pass;
- existing relevant Skills tests pass;
- `cargo fmt --check` passes;
- warnings-as-errors and clippy pass for touched crates;
- adversarial review findings are fixed or documented with evidence.

## Phase 2: local lifecycle and provider seam

Status: planned after Phase 1 checkpoint

- Snapshot an edited workspace into a new revision.
- Diff current/fork/upstream revisions.
- Build explicit update plans; never auto-apply upstream changes.
- Introduce a transport-neutral Artifact provider contract for local, Depot/Bazaar, and future sources.
- Keep provider acquisition separate from Artifact storage and policy.
- Preserve canonical provenance and source revision evidence across provider operations.
- Add transactional/atomic head updates where cross-platform replacement semantics need strengthening.

## Phase 3: thin CLI/API/MCP projections

Status: planned

Expose shared operations through thin adapters only. Candidate operation families include list/get/import/export/fork/status/update-plan/update-apply. Transport DTOs may differ where protocol conventions require it, but all validation and state transitions must route through the shared Artifact implementation.

Skills-over-MCP remains the existing compatibility surface. Artifact-native MCP operations must not make a second Skills implementation. Loadouts continue consuming runtime capability through existing paths until deliberately migrated onto Artifact references.

## Phase 4: runtime placement and deployment mechanics

Status: planned

Mine the useful old deployment mechanics behind modern Artifact terminology:

- deployment target identity and capabilities;
- desired deployment plan;
- explicit stages;
- verification probes/evidence;
- rollback plan and rollback execution;
- remote target adapters;
- idempotency and resumability;
- per-target locking/concurrency;
- safe credential references rather than embedded secrets.

These become new Artifact/runtime contracts. They must not reintroduce retired Deploy or Fleet protocol surfaces.

## Phase 5: hosted and enrichment integration

Status: planned

- Depot/Bazaar provider for exact revision import/export and publication metadata.
- Axon candidate/evidence consumption without giving Axon publication authority.
- Cross-product fixture/contract CI.
- Provenance continuity tests from discovered candidate through personal import and optional hosted publication.

## Verification strategy

Each phase requires focused tests before broad gates. Safety tests must cover traversal, symlinks, duplicate paths/IDs, digest parity for unsorted component input, explicit revision IDs, malformed digests, content corruption, frozen Artifact metadata secret-key semantics, credential-bearing source URIs, secret-like export content, non-empty export targets, immutable revision reuse, fork identity collision, and silent-update prevention.

Before PR creation run formatting, warnings-as-errors, clippy with warnings denied, relevant crate tests, existing Skills tests, repository contract checks, and an adversarial code review of the complete diff.
