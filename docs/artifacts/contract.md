---
title: ArtifactInterchange v1 Contract
created: 2026-08-19
updated: 2026-08-19
---

# ArtifactInterchange v1 Contract

Status: frozen cross-repo contract for W19
Canonical owner: Depot G0 at commit `25de725`
Schema identifier: `dinglebear.artifact-interchange/v1`

## Compatibility rule

Labby SHALL serialize and deserialize the frozen Depot v1 payload without adding, renaming, deleting, default-eliding, or reinterpreting shared wire fields. The copied fixture at `crates/labby-runtime/tests/fixtures/artifact-interchange-v1.json` is a byte-canonical contract fixture and must stay identical to Depot's fixture unless the cross-repo contract version changes.

Labby-specific mutable local state, storage layout, locks, workspaces, provider state, deployment state, and transport DTOs are not additions to the v1 interchange envelope.

## Envelope

The v1 envelope contains exactly these top-level concepts:

- `schemaVersion`: `dinglebear.artifact-interchange/v1`;
- `descriptor`;
- `revision`;
- `provenance`;
- `license`;
- `lineage`;
- `publication`;
- `downloads`;
- `materializationHints`.

## Descriptor v1

Fields: `schemaVersion`, `id`, `kind`, `namespace`, `name`, `title`, `description`, `tags`, and `metadata`.

Artifact IDs are opaque on the wire. Labby's ordinary local identity is `art_` plus SHA-256 of canonical JSON for `{kind, namespace, name}`, matching Depot's v1 adapter behavior. Compatibility adapters may bind that opaque ID to a stronger stable source key when the human namespace/name pair is not globally unique. The Agent Skill adapter uses the canonical Skill URI for that purpose so nested Skills with the same final name cannot collide.

## Component v1

Fields: `schemaVersion`, `id`, `kind`, `path`, `digest`, `size`, `mediaType`, `metadata`, `dependencies`, `requirements`, and `executionRisk`.

Execution risk values are `unknown`, `passive`, `executable`, `privileged`, and `dangerous`.

Logical paths use raw forward-slash relative path semantics. V1 does not URI-decode paths. Reject absolute paths, Windows drive absolutes, empty segments, `.`, `..`, backslashes, NULs, and paths longer than 4,096 bytes.

## Revision v1

Fields: `schemaVersion`, `id`, `contentDigest`, `components`, `parentRevisionId`, `authoredAt`, `message`, and `metadata`.

Components are canonicalized in ascending `{path, id}` order when constructing or hashing a revision. Duplicate component IDs or paths are invalid. A decoder may accept an unsorted component array, but digest verification MUST sort by `{path, id}` before hashing, matching Depot G0. The content digest is SHA-256 of canonical JSON for that ordered component array. V1 defaults the revision ID to the content digest but permits an explicit opaque/reference revision ID. A revision payload is immutable once stored.

## Canonical JSON

Canonical JSON recursively sorts object keys lexicographically, preserves array order, and uses standard compact JSON scalar encoding. No whitespace is emitted. Cross-runtime digests are computed over these canonical bytes.

The frozen fixture revision digest is:

`sha256:feda49490988a21b01ea9d6548f2c893a7cea6c4e9834322985c28d82280c13f`

Labby's conformance test must reproduce that digest and re-encode the entire fixture byte-for-byte.

## Provenance v1

Fields: `schemaVersion`, `provider`, `sourceUri`, `registry`, `repository`, `ref`, `sourcePath`, `sourceDigest`, `observedAt`, `adapter`, `originalFormat`, `originalVersion`, `integrityEvidence`, and `metadata`.

Provenance is evidence only. A matching digest or source reference does not imply trust. Source URIs with embedded credentials are invalid. `file`, `data`, and `javascript` source URI schemes are invalid in the portable contract.

## License v1

Fields: `schemaVersion`, `declared`, `detected`, `notices`, `redistribution`, `reviewState`, `takedownState`, `evidenceAt`, and `metadata`.

Redistribution values: `metadata_only`, `cache_for_index`, `redistributable`, `forkable`, `restricted`, `unknown`. Unknown is the default.

Review values: `unreviewed`, `reviewed`, `disputed`. Takedown values: `none`, `requested`, `restricted`, `removed`.

## Lineage v1

Fields: `schemaVersion`, `upstreamArtifactId`, `upstreamRevisionId`, `forkedFromArtifactId`, `forkedFromRevisionId`, `forkedAt`, `following`, `lastObservedUpstreamRevisionId`, and `metadata`.

Following is intent, not permission for silent updates. A local fork remains pinned until an explicit update action creates a new local state.

## Publication v1

Fields: `schemaVersion`, `state`, `visibility`, `distribution`, `publishedAt`, `withdrawnAt`, and `metadata`.

State values: `draft`, `listed`, `published`, `withdrawn`. Visibility values: `private`, `unlisted`, `public`. Distribution values: `metadata`, `bytes`.

Byte distribution is valid only when license redistribution is `redistributable` or `forkable`.

## Bounded extension data

Portable maps/lists are recursively bounded. W19 enforces the Depot G0 limits used by the shared v1 contract:

- metadata depth: 8;
- entries per map: 128;
- entries per list: 256;
- metadata string bytes: 16,384;
- tags: 64;
- component count: 2,000.

Secret-shaped metadata keys are rejected recursively.

## Skill family rule

An Agent Skill is represented as an Artifact with `descriptor.kind = "skill"`. One component represents each validated Skill resource using the unchanged resource-relative path, byte digest, and size. Existing Skills verification remains authoritative for whether those bytes are admissible.

The adapter may add Artifact metadata and provenance, but it may not rewrite Skill URIs, frontmatter, resource bytes, manifest digest behavior, or Skills-over-MCP semantics.

## Contract changes

Any incompatible change requires a new schema identifier and coordinated fixture/version work across Depot, Labby, Axon, and consuming products. Do not mutate v1 in place.


## Phase 2 local lifecycle contract

1. `snapshot_workspace` MUST snapshot only the canonical editable workspace, honor the existing traversal/component/file/package limits, and run under the Artifact mutation lock.
2. Content identity MUST be computed independently of parent linkage. If content already names an immutable revision, Labby MUST reuse that exact revision and MUST NOT rewrite its metadata.
3. `ArtifactRevisionDiff` MUST be deterministic by normalized path and MUST not mutate either revision.
4. `ArtifactProvider` is an acquisition seam only. A provider MUST NOT write local Artifact state. `ArtifactAcquisition` MUST validate ArtifactInterchange v1, one payload per file component, local byte budgets, exact sizes, and SHA-256 digests before it is trusted by lifecycle operations.
5. `ArtifactUpdatePlan` is evidence and intent, not authority to apply. Creating a plan MUST leave the local head, workspace, lineage, and stored bytes unchanged.
6. Mutable head publication MUST reject an unexpected base revision and MUST use atomic replacement on supported platforms. Existing symlink substitutions remain invalid.
7. These local lifecycle/provider types do not change the frozen `dinglebear.artifact-interchange/v1` wire fields owned by the cross-product contract.
