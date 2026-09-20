---
title: "ADR 0005: Use the ArtifactStore-Backed Library as Labby's Local Artifact Authority"
created: "2026-09-20"
updated: "2026-09-20"
---

# ADR 0005: Use the ArtifactStore-Backed Library as Labby's Local Artifact Authority

Date: 2026-09-20

Status: Proposed

## Context

Labby presents Skills, Snippets, Prompts, Agents, Hooks, Loadouts, Tools,
Resources, Apps, Plugins, packages, and provider results across Discover, the
Library, editors, and Code Mode. Those surfaces currently use overlapping kind
vocabularies and persistence models.

The general `ArtifactStore` under `$LABBY_HOME/artifacts` already owns arbitrary
Artifact records, immutable revisions, verified bytes, mutable workspaces,
provenance, licensing, lineage, and publication state. Its Library lifecycle
index is still skill-specific: records, active-name indexes, transactions,
receipts, audits, and transport DTOs assume Skills. Snippets are loose files in
a separate directory, and several runtime projections use publication or host
configuration instead of the authenticated local Library record.

Provider discovery also includes results that should remain remote or live.
Showing a Tool or Resource in Library navigation does not mean Labby owns an
immutable local copy of its bytes. Conversely, importing an Artifact must have
one reliable answer for storage, version, ownership, activation, and runtime
projection.

## Decision

The ArtifactStore-backed Library is Labby's local authority for durable owned
Artifacts. It is generalized in place across supported kinds. Remote candidates,
live runtime capabilities, and presentation groupings remain explicit virtual
or provider-backed projections rather than being copied merely because they are
visible.

Depot remains the registry and publication authority described by ADR 0002.
Labby's Library owns the personal or tenant-local lifecycle and runtime
integration.

### Preserve the two durable storage layers

The ArtifactStore continues to own Artifact identity, immutable revisions,
exact files, workspaces, provenance, license, lineage, and publication state.
The Library index continues to own authenticated ownership, private or tenant
visibility, archive state, latest and active revision selection, search
metadata, idempotency receipts, audits, and generation identity.

Generalization replaces skill-only records and indexes with kind-aware forms;
it does not flatten revision bytes into the Library snapshot, expose hashed
implementation paths as contracts, or merge local visibility with remote
publication visibility.

The current crash-safe commit protocol remains an invariant: validate and lock,
write a bounded checksummed intent, commit the Library generation and durable
receipt, promote the exact Artifact revision and workspace, then clear or apply
the recovery marker. New readers retain compatibility with the version 1
snapshot and `pending-skill.json`/`applied-skill.json` journals until a
schema-aware migration is complete. Per-kind and aggregate transaction budgets
bound larger packages.

### Use one canonical kind registry and adapter contract

The backend owns a canonical registry for kind names, aliases, storage class,
validation and materialization, Library lifecycle support, activation support,
required authority, runtime projection, deactivation and rollback behavior, and
supported import or export operations. UI filters, editor choices, Discover
actions, and runtime eligibility derive from this contract instead of separate
Rust and TypeScript lists.

The registry distinguishes portable Artifact kinds from discovery containers,
presentation groups, and live capabilities. Unknown provider kinds may remain
visible as unsupported metadata, but cannot be imported, activated, or executed
without an adapter. The response includes a stable reason and supported next
actions.

### Converge acquisition on one exact lifecycle

“Add to Library,” “Import,” and any retained “Send to Labby” action use this
lifecycle:

```text
discover or author
  -> inspect and validate
  -> acquire exact bytes and provenance
  -> commit an immutable Artifact revision
  -> create or update the owned Library record
  -> optionally activate through an explicit kind adapter
  -> project the exact active revision into the runtime
```

Public JSON may select an exact configured source and revision. It cannot forge
an acquisition receipt, provider endpoint, delegated header, filesystem path,
or acquired bytes. The server returns per-item actions and importability rather
than asking the UI to infer support from a kind string.

Importing, reading, or activating an Artifact never grants filesystem, network,
shell, tool, plugin-installation, hook-execution, or MCP-server-start authority.
An activation adapter can only operate within authority already granted and
revalidated for that kind.

### Make activation kind-specific

“Active” is not a generic boolean with hidden side effects. Skills preserve the
existing prepared-generation and atomic-swap invariant. Prompts, Agents, Hooks,
Snippets, Loadouts, MCP configurations, Apps, Plugins, and packages each receive
an explicit adapter or an unsupported activation status.

Hooks, Plugins, and MCP servers remain inert until a separately authorized
installer or runtime adapter exists. Code Mode and Loadout eligibility derive
from the authenticated Library record, exact active revision, and kind adapter,
except for projections explicitly classified as virtual or live.

### Migrate snippets as versioned Artifacts

Snippets become immutable Artifact revisions with a canonical package entrypoint,
media types, input and dependency metadata, and adapter version. The Code Mode
runner keeps its parsing and execution protocol, but host resolution pins the
exact active Library revision for the whole execution.

Legacy migration validates regular Markdown and JavaScript files from the
verified snippets directory, rejects unsafe entries, records relative legacy
provenance and digests, reports each outcome, and leaves failed files untouched.
It is resumable and idempotent. A verified backup or migration marker precedes
disabling legacy writes; migration does not delete the directory on first
success.

Built-in snippets are embedded or packaged as a versioned read-only provider
with a release digest. A new Labby release may add a built-in revision, but
cannot silently replace a user's active shadow or override.

### Expose one Library aggregation contract

The Library shell pages stable local-owned records from the generalized Library
authority and may join virtual or live projections carrying an explicit storage
class. Cursors bind tenant, query, kind, visibility, storage class, sorting, and
relevant snapshot epochs. Browsers do not merge independently paginated
provider catalogs.

Library detail reuses presentation components where useful but reads local
revisions, receipts, and exact content from the local authority. Metadata export
remains distinct from exporting exact revision bytes.

## Consequences

- Every durable local Artifact has one version, ownership, lifecycle, and
  activation authority.
- The existing ArtifactStore, authorization model, transaction recovery, and
  immutable revision guarantees are extended instead of replaced.
- Some Discover and Library rows remain virtual by design and must say so in
  their storage class and allowed actions.
- Generalization requires compatible schema migration, kind-specific adapters,
  binary-safe component reads, and a full inventory of current kind aliases.
- Runtime eligibility no longer requires making a private local Artifact
  publicly published.
- Built-in and user snippets gain stable packaging and upgrade semantics.

## Alternatives considered

### Create a new universal Library store

Rejected because it would duplicate the general ArtifactStore and jeopardize
its immutable revision, provenance, and crash-recovery guarantees.

### Keep a separate durable store for every kind

Rejected because it preserves the current ambiguity around ownership,
versioning, activation, and runtime resolution.

### Copy every Discover result into the ArtifactStore

Rejected because live Tools and Resources, provider candidates, repositories,
and marketplaces may be references or containers rather than portable owned
packages.

### Treat every Artifact like a Skill during activation

Rejected because activation side effects and authority differ materially by
kind, especially for Hooks, Plugins, and MCP servers.

### Resolve snippets from loose files indefinitely

Rejected because replacement loses revision history and runtime resolution
cannot provide the same exact-revision and tenant-scoped guarantees as the
Library.

## Authority and implementation status

This ADR records the proposed architecture from [GitHub issue
#710](https://github.com/dinglebear-ai/labby/issues/710). It does not claim that
the v2 Library schema, adapters, migration, unified UI contract, or built-in
snippet packaging are implemented. ADR 0002 continues to govern the boundary
between Labby's local product contracts and Depot's registry authority.

## References

- [GitHub issue #710](https://github.com/dinglebear-ai/labby/issues/710)
- [ADR 0002](./0002-labby-product-contracts-over-private-depot.md)
- `docs/artifacts/spec.md`
- `docs/artifacts/contract.md`
- `docs/depot-unified-frontend.md`
- `crates/labby-runtime/src/artifacts/`
- `crates/labby-codemode/src/snippet/`
- `crates/labby/src/dispatch/artifacts.rs`
