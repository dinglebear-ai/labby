---
title: Labby Personal Artifacts Specification
created: 2026-08-19
updated: 2026-08-19
---

# Labby Personal Artifacts Specification

Status: W19 implementation specification

## Product boundary

Labby is the AGPLv3 open personal Artifact and MCP/runtime Gateway. It owns a user's local Artifact store, Artifact runtime operations, MCP/runtime integration, Skills and Loadouts compatibility, and thin CLI/API/MCP projections over one shared implementation.

Depot/Bazaar is the independent hosted Artifact registry and publication product. Axon owns crawl, indexing, discovery, and enrichment. Phoenix is a consumer and product integration plane whose prior Artifact work is reference and migration evidence, not Labby's authority.

Retired Stash, Marketplace, Fleet, and Deploy product names and protocol surfaces are not compatibility targets and must not return. Useful mechanics from their history may be reused behind the modern Artifact model.

## Goals

The Artifact subsystem SHALL provide one local domain for multi-file packages and runtime resources. Agent Skills are an Artifact family with `kind = "skill"`, not a separate storage universe. Future families may include prompts, commands, agents, hooks, MCP servers, plugins, bundles, and runtime packages without changing the v1 interchange envelope.

The shared Rust implementation SHALL own validation, identity, revisioning, provenance, license state, lineage, local persistence, import/export, and future provider/deployment semantics. CLI, HTTP/API, MCP, and other transports SHALL adapt to this layer instead of reimplementing behavior.

The first W19 vertical slice provides:

- multi-file Artifact/package snapshots;
- explicit provenance and license/redistribution state;
- deterministic SHA-256 component and revision identity;
- immutable revisions plus a distinct editable workspace;
- bounded, path-safe local import;
- bounded, path-safe and secret-aware local export;
- basic fork lineage and explicit upstream observation;
- Agent Skills projection that reuses existing SEP-2640 verification;
- exact `dinglebear.artifact-interchange/v1` parity with Depot's frozen fixture.

## Prompt artifact materialization

The surface-neutral Artifact runtime can materialize a bounded, inert Prompt from exactly one
`PROMPT.md` file. Prompt frontmatter declares a matching lowercase identifier, a bounded
description, and unique bounded argument names. The remaining Markdown body must be non-empty and
is retained as inert text; materialization does not interpret templates, HTML, commands, or other
instruction-bearing content. The resulting Artifact uses `kind = "prompt"`, content-addresses the
exact source bytes, and records the `labby.prompt/v1` adapter when no adapter was supplied.

Prompt transport and authoring operations use the shared `artifacts` control plane. Prompt
materialization does not introduce a parallel `prompt_library.*` service namespace.

## Agent artifact materialization

The surface-neutral Artifact runtime can materialize a bounded, inert Agent definition from
exactly one `AGENT.md` file. Agent frontmatter declares a matching lowercase identifier, a bounded
description, the `labby` runtime, explicit-only activation, and at most 256 unique capability
references pinned to expected revisions. The remaining Markdown body must be non-empty and is
retained as inert text; materialization does not execute or interpret its instructions.

The resulting Artifact uses `kind = "agent"`, content-addresses the exact source bytes, and records
the `labby.agent/v1` adapter when no adapter was supplied. Agent transport and authoring operations
use the shared `artifacts` control plane; materialization does not introduce a parallel Agent
service namespace or automatic activation behavior.

## Non-goals for the first slice

The first slice does not add hosted registry authority, crawling/enrichment, trust scoring, compatibility aliases for retired products, a new Skills protocol, remote deployment execution, or transport-specific business logic. It does not silently update a fork when an upstream changes.

Provider abstraction, remote target execution, deployment target/plan/stage/verification/rollback, and thin CLI/API/MCP surfaces are subsequent W19 phases built on the same domain.

## Local model

An Artifact has a stable descriptor identity and one mutable local head record. Artifact revisions are immutable and content addressed. Each revision contains a canonical ordered component inventory. File components bind a normalized package path, byte digest, size, optional media type, bounded metadata, requirements, dependencies, and execution-risk classification.

The canonical local layout is implementation-owned and intentionally not a public protocol:

```text
<root>/
  artifacts/<sha256(artifact-id)>/
    artifact.json
    workspace/
    revisions/<sha256(revision-id)>/
      revision.json
      files/...
  locks/<sha256(artifact-id)>.lock
```

Caller-controlled IDs are hashed before becoming storage path segments. The mutable workspace is not the immutable revision store. A new snapshot creates or reuses an immutable revision, then materializes the current workspace from verified bytes.

## Identity and revisions

Artifact identity is deterministic for the tuple `{kind, namespace, name}` using the same canonical JSON seed as Depot. Component identity is deterministic for its normalized package path. File byte digests are `sha256:<64 lowercase hex>`.

Revision content identity is the SHA-256 digest of canonical JSON for the canonical component inventory sorted by `{path, id}`. In v1, the revision ID defaults to that content digest, while the frozen cross-product contract also permits an explicit opaque/reference revision ID. Parent revision, authored timestamp, and message are semantic revision metadata and do not change content identity. Labby's locally created revisions use the content digest as their ID and reuse an existing immutable revision when that identity already exists.

## Provenance, license, and trust

Provenance is evidence about origin, not a trust decision. License/redistribution state is separate from integrity and trust. Unknown redistribution is the safe default. Publication of bytes is invalid unless redistribution is explicitly `redistributable` or `forkable`.

Local import may carry provenance but the portable source URI cannot contain credentials and cannot use active/local schemes such as `file`, `data`, or `javascript`. Secret-shaped metadata keys are rejected recursively.

## Import safety

Local import accepts a regular file or directory only. It rejects symlinks, special files, unsafe path segments, absolute logical package paths, backslashes, NULs, non-UTF-8 logical paths, and canonical path escapes. Directory traversal is deterministic. The Artifact store itself requires an explicit absolute root, validates the nearest existing ancestor before creating directories, rejects symlinked store components, and uses private store/lock permissions on Unix.

The first-slice budgets are:

- 2,000 components per revision;
- 10,000 traversed directory entries per local import;
- 64 directory levels per local import;
- 50 MiB per local imported file;
- 200 MiB aggregate imported package bytes;
- 10,000 revisions referenced by one local head record;
- 2 MiB per serialized local head record;
- 256 MiB per serialized immutable revision manifest;
- 4,096 bytes per logical path;
- metadata depth 8;
- 128 entries per metadata map;
- 256 entries per metadata list;
- 16,384 bytes per metadata string.

These local I/O limits are intentionally stricter than the portable component size ceiling.

## Export safety

Export always reads an exact immutable revision, verifies stored size and SHA-256 digest, revalidates every logical path, and rejects stored symlinks or path escapes. Export requires an explicit absolute destination, refuses any destination that overlaps the canonical Artifact store, and does not overwrite a non-empty destination unless the caller explicitly opts into `force`.

Text content matching Labby's existing secret detector is blocked by default. A caller must explicitly opt into `include_secrets` to export that content. Errors expose only a relative package path, never the detected secret bytes. Executable mode is preserved only through a safe `0o0755` mask.

## Locks and workspaces

Mutating operations use a per-Artifact OS advisory file lock. Lock files may remain on disk; ownership is the live file descriptor, so stale lock-file presence is not a stale lock.

The editable workspace is deliberately separate from revisions. Direct workspace edits do not mutate stored revisions. A later snapshot operation may turn workspace state into a new revision.

## Fork and update semantics

A fork creates a new Artifact identity and copies an exact immutable source revision. Lineage pins both the source Artifact and source revision. `following` records user intent only. Observing a newer upstream revision updates `lastObservedUpstreamRevisionId`; it never silently rewrites local bytes or advances the current revision.

Future update operations must compute an explicit update plan and preserve local divergence.

## Agent Skills compatibility

The Artifact Skill adapter consumes the existing `ValidatedSkill` result and exact resource bytes. Every resource is reverified through the existing SEP-2640 digest path before becoming an Artifact component. Because nested Skill paths can share the same final Skill name, Labby binds the opaque local Artifact ID to the canonical Skill URI while keeping the human namespace/name fields intact.

The adapter must not change:

- Skill URI semantics;
- author frontmatter;
- resource file bytes;
- manifest digest semantics;
- Skills-over-MCP discovery/read behavior;
- current Loadout or Code Mode behavior.

Artifact support therefore unifies persistence and lifecycle without weakening the existing Skills boundary.

## Evidence reused from retired implementations

The modern design mines mechanics without restoring product aliases. Relevant historical Labby commits include `c39bbf451` for the canonical store and advisory locking, `76adeb9f9` for import/workspace mechanics, `f33fbcfda` for SHA-256 revisions, `1564665d1` for safe export and mode handling, `f5134580a` for provider abstraction, and later deployment orchestration work for target/stage/verification/rollback concepts.

Phoenix's current Artifact work contributes additional migration evidence: explicit path validation, bounded writes, content SHA-256, atomic replacement, and versioned local state. Labby extends those proven mechanics to the multi-file package domain required here.


## Local lifecycle and provider semantics

- The editable workspace is mutable working state. Snapshotting it is an explicit mutation that runs under the per-Artifact lock and selects an immutable revision.
- Snapshot content identity is derived before parent linkage. Identical current content is a no-op; content that matches an older revision moves the head back to that exact immutable revision rather than rewriting it. Consequently, a repeated-content snapshot cannot replace the stored message, timestamp, or metadata of an existing revision.
- Revision diffs are deterministic and path ordered, classifying each path as added, removed, or modified.
- Artifact providers acquire exact portable metadata plus revision bytes but do not mutate the personal store or make policy decisions. Acquired files must match the portable component inventory, Labby's local file/package byte budgets, and every declared size and SHA-256 digest.
- Update planning is read-only. A plan records the local target and base revision, exact source Artifact/revision, source provenance, and deterministic diff. Planning never changes bytes, lineage, workspace, or the local head.
- Mutable head transitions compare the expected base revision while the Artifact lock is held. Internal JSON state is published with same-directory atomic replacement rather than a delete-then-rename fallback, and existing symlink targets remain rejected.
