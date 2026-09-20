---
title: "ADR 0002: Labby Product Contracts over Private Depot"
created: "2026-09-20"
updated: "2026-09-20"
---

# ADR 0002: Labby Product Contracts over Private Depot

Date: 2026-09-20

Status: Accepted direction; implementation is partial

## Context

Labby already exposes Artifact, source, ingestion job, upload, and bundle
capabilities through provider-neutral actions. Its Administration web UI also
operates Depot through a server-side catalog adapter. These paths reach the
same backend, but do not yet share all Labby policy and compatibility behavior.
The [capability audit](../design/depot-capability-audit.md) records the current
64-operation denominator, existing shims, and remaining differences.

Depot is a separate private repository and is not intended to be open source.
Moving its user-facing capabilities behind Labby does not move its
implementation, persistent state, or registry authority into Labby. Earlier
descriptions of Public Depot as a separate user-facing product do not express
the integrated product boundary selected here.

## Decision

### Product and implementation ownership

Labby owns the user and agent experience: public names, supported action
contracts, discovery, navigation, workflows, and surface eligibility. Users
work with Labby Artifacts, libraries, sources, jobs, uploads, and bundles.
They do not need to understand Depot to use the integrated product.

Depot remains independently deployed backend software in its own private
repository. It owns registry data, immutable revisions, ingestion execution,
content storage, publication and licensing enforcement, backend resource
authorization, and backend audit truth. Labby does not copy or reimplement
those systems. This decision does not change Depot's source licensing or
grant source distribution rights. Independent deployment and self-hosting do
not imply open-source availability.

Backend-specific identifiers, client types, connection configuration, and
operator diagnostics may retain the Depot name below the product boundary.
Depot's direct APIs and operator tooling remain backend integration and
administration interfaces; their existence does not require a second product UI.

### Shared dispatch shims

Supported Labby operations use surface-neutral dispatch shims in
`crates/labby/src/dispatch/` (or an appropriately extracted runtime). Dispatch
owns Labby's action mapping, connection selection, authority requirements,
parameter translation, compatibility checks, destructive classification,
idempotency orchestration, and structured results/errors. A pure backend HTTP
client may live in `labby-apis`; it must not read ambient configuration or
depend on product transports.

CLI, MCP, HTTP, and web adapters call those shims for each operation they
support. Cookies, CSRF, protocol envelopes, CLI interaction, and browser
confirmation controls remain surface concerns. Browser-only administration
need not become an ordinary MCP tool or a registered multi-surface service
merely because its implementation lives in dispatch.

The existing Administration workflows are retained. Their backend logic is
already largely below HTTP in `dispatch/depot`; completing the boundary must
reuse that implementation. Where Administration and named Labby actions offer
the same capability, both must converge on the same shared operation policy
and execution shim, rather than maintaining independent behavior.

### Behavioral equivalence

A shim translates the public contract and delegates the actual operation to
Depot. Equivalent calls preserve target identity, exact revisions, parameter
meaning (including omission versus null), concurrency preconditions, backend
side effects, job state, cancellation, retry, and idempotency behavior.
Authorization may narrow backend authority, never broaden it. Labby must
preserve meaningful error categories and receipts while enforcing deliberate
secret redaction and transport-specific encoding.

Same behavior does not require byte-identical envelopes or equal visibility
on every surface. Any intentional difference must be explicit and tested.
In particular, an asynchronous ingestion job is not a synchronous ingestion
response, local removal is not remote catalog deletion, and a visible Skill
projection is not automatically the entire remote catalog.

### Administration and catalog ownership

Labby owns supported administration action identities and their policy. Depot's
operation catalog supplies backend schemas, compatibility evidence, and
availability; newly advertised backend operations must not automatically
become supported public Labby actions.

A generic backend catalog adapter may remain an advanced administrator
facility while existing workflows are preserved. It must enforce an explicit
compatibility contract and current authority. Maintenance stays outside normal
model-facing MCP. Token administration remains an explicit backend operator
capability where needed; ordinary managed users are not required to create or
handle Depot service tokens. Removing the existing operator capability is not
part of this decision.

Provider connection management is Labby-owned configuration lifecycle, distinct
from Depot's persisted ingestion sources. Do not merge these concepts merely
to remove a provider name.

### Compatibility and publishing

The exposed `depot_publish` service and `depot.publish_skill_archive` action
must move behind a Labby-owned publishing contract without losing their
protected-route authorization or per-phase revalidation. Reuse the existing
upload-slot, byte-transfer, and ingestion workflow. Final action names and any
compatibility alias lifetime require an explicit implementation contract.

Likewise, public `/depot` and `/v1/depot/*` naming should move behind Labby
concepts with deliberate compatibility handling for callers and links. A route
rename must not alter target selection, authorization, or backend effects.

## Consequences

- The private backend stays private and independently owned.
- Labby can provide a complete experience without duplicating registry logic.
- Existing web administration remains useful during the migration.
- Separate generic-browser and curated-action policies must be reconciled;
  reaching the same backend URL is insufficient proof of parity.
- Every omitted operation needs a recorded disposition, including remote
  deletion, native Skill compatibility, and privileged maintenance.
- Public contract evolution requires compatibility and cross-surface evidence.

## Acceptance evidence for implementation

Implementation qualification must cover the full operation inventory; matched
requests and effects across eligible surfaces; parameter/schema drift;
authorization and revocation; managed-authority readiness; destructive intent;
idempotency and indeterminate outcomes; structured errors and secret handling;
and existing browser workflows. Live deployment and persistence evidence are
separate from unit or mocked transport tests.

The accompanying audit is a source snapshot, not a claim that these gates have
passed. This ADR authorizes the architecture direction; it does not label the
remaining migration implemented.

## Supersession and references

This supersedes the separate user-facing Public Depot interpretation of the
[SaaS North Star](../design/labby-depot-saas-north-star.md), while retaining its
backend authority and deployment separation. It complements the
[dispatch contract](../dev/DISPATCH.md),
[Depot integration contract](../contracts/depot-control-plane.md), and
[Artifacts and Agent Skills](../services/SKILLS.md).
