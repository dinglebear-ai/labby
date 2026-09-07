---
title: "Labby and Depot SaaS North Star"
status: accepted-target
created: "2026-09-07"
updated: "2026-09-07"
---

# Labby and Depot SaaS North Star

This document selects the target product and deployment architecture for hosted
Labby and Depot. It is a direction and migration contract, not a claim that the
target is already implemented. Current behavior remains defined by the live
code, generated catalogs, and the implementation status in the access-control
packet.

The first tenant is the Lime team. Shipping that private tenant is the first
production slice of the same architecture, not a bespoke internal fork.

## Decision

Labby is the user-facing runtime, MCP gateway, and workspace control plane.
Depot is the Artifact registry, publication authority, and immutable content
source. They remain independently deployable and fully self-hostable.

The hosted product has four related surfaces:

1. **Public Depot** provides public Artifact discovery, public Artifact detail,
   creation and publication for authenticated users, personal libraries, and
   policy-permitted forks of exact immutable revisions.
2. **Personal Labby** runs for one user on a laptop, workstation, server, or
   other user-controlled environment. It can use remote Artifacts, materialize
   managed copies, or own explicit forks according to distribution policy.
3. **Hosted Labby** provides a durable personal or team MCP gateway without
   requiring the customer to operate it. It owns runtime configuration,
   upstream connections, Loadouts, sessions, and local Artifact state for that
   hosted runtime.
4. **Teams and paid runtimes** add organization-scoped membership, policy,
   shared libraries, managed gateways, runtime capacity, usage accounting, and
   administration. A subscription buys service and capacity; it does not alter
   Artifact integrity or authorization rules.

```text
                         public discovery and publication
                   +-----------------------------------------+
                   |              Public Depot               |
                   | metadata, revisions, lineage, policies  |
                   +--------------------+--------------------+
                                        |
                              exact immutable Artifacts
                                        |
          +-----------------------------+-----------------------------+
          |                                                           |
 +--------v---------+                                       +---------v--------+
 | Personal Labby   |                                       | Hosted Labby     |
 | user-operated    |                                       | personal or team |
 | local gateway    |                                       | managed runtime  |
 +------------------+                                       +---------+--------+
                                                                       |
                                                        organization projects,
                                                        Loadouts and upstreams
```

## Product ownership

### Depot owns the Artifact registry

Depot is authoritative for:

- Artifact identity and immutable revisions;
- component manifests, content digests, and provenance;
- public and tenant-private discovery metadata;
- draft, listed, published, withdrawn, and takedown state;
- license and redistribution ceilings;
- fork lineage and upstream-revision observation;
- publisher namespaces and publication policy;
- exact Artifact acquisition; and
- registry-side quotas and publication abuse controls.

Public discovery is readable without organization membership only through an
explicit constrained public policy context. Creating, publishing, changing,
withdrawing, or forking requires an authenticated principal and an exact policy
decision. Discovery or use never implies permission to copy, fork, export, or
reshare bytes.

Every eligible team member may create and publish Skills to the Lime tenant.
That statement means the default Lime member role will receive the necessary
Artifact create and publish grants; it does not bypass validation, namespace,
license, secret scanning, audit, quota, or takedown policy.

### Labby owns execution and workspace composition

Labby is authoritative for:

- MCP gateway and protocol behavior;
- upstream server connections and upstream credential lifecycle;
- Loadouts and capability exposure;
- runtime sessions, health, and failure isolation;
- personal and team workspace composition;
- local Artifact materialization and activation;
- destination pairing and Send to Labby delivery state; and
- runtime usage facts needed for operations and metering.

Labby consumes exact Depot revisions through the Artifact provider boundary.
It does not reimplement Depot publication or registry policy. Depot does not
become an MCP gateway or execute a customer's upstream tools.

An imported managed mirror remains source-controlled. An explicit fork gets a
new Artifact identity and preserved exact lineage. Neither operation silently
activates content or follows a newer revision.

## Identity and authorization

One account may participate in multiple organizations and own multiple Labby
runtimes. Authentication establishes a principal; it does not select a tenant
or confer organization permissions.

Authorization is evaluated against an explicit organization, project, personal
workspace, or public context. The shared model uses the existing access-control
vocabulary: Principal, Organization, Group, Project, Role, Grant, Assignment,
Artifact authority, and EffectiveWorkspace.

Required properties are:

- default deny and organization-qualified lookup at every protected boundary;
- no caller-controlled tenant, role, owner, or billing identity;
- authorization-aware discovery plus reauthorization at direct invocation;
- separate permissions for discover, use, create, publish, sync, fork, export,
  reshare, administer, and bill;
- publisher and Assignment distribution policy can narrow but never broaden
  license or takedown constraints;
- tenant, principal, project, and credential identity participate in cache,
  connection-pool, session, cursor, and background-job keys; and
- revocation invalidates active serving paths without falling back to another
  subject, project, organization, or public context.

Hosted Labby and Depot may share an identity provider and account linking, but
each service authorizes its own operation. Service-to-service assertions are
short-lived, audience-bound, subject-bound, and scoped. Reusable end-user or
organization bearer credentials are not forwarded as an integration shortcut.

## Tenant isolation and data planes

### Immediate Lime deployment

The first private tenant uses two Depot processes:

- the existing public Depot process for the public registry; and
- a separately configured Lime Depot process for the Lime private library.

Each process has distinct runtime configuration, credentials, metadata store,
R2 scope, health, deployment, and rollback. This temporary process boundary
reduces first-tenant blast radius while tenant-aware persistence and operations
are qualified. It is not the long-term scaling model and must not grow into one
Depot process per customer.

Lime receives a hosted team Labby connected to the Lime Depot authority. Team
members can discover, create, publish, acquire, and fork Skills according to
the default Lime member policy. Personal Labby remains a supported destination
for exact managed copies and permitted forks.

### Long-term shared Depot metadata plane

The long-term hosted service uses a tenant-aware shared Depot metadata plane,
with horizontal application workers and hard tenant isolation. Organization or
publisher scope is part of every authoritative metadata key and relationship.
Tenant isolation is enforced at multiple layers:

- authenticated tenant context is resolved server-side;
- database schemas, keys, constraints, and queries require tenant identity;
- cross-tenant relationships fail validation;
- jobs and queues carry integrity-protected tenant context;
- caches, cursors, idempotency keys, search indexes, and rate-limit keys are
  tenant-qualified;
- object capabilities are issued for an exact tenant, Artifact, revision, and
  bounded operation;
- audit and metering records carry the authoritative tenant and principal; and
- adversarial tests cover cross-tenant IDOR, cache confusion, job replay,
  connection reuse, search leakage, and backup/restore boundaries.

An R2 prefix alone is not a security boundary. Database policy and application
authorization remain mandatory even when object keys are tenant-qualified.
Where the selected database supports row-level security, it should provide an
independent enforcement layer rather than replace explicit application checks.

### Content plane: R2

Cloudflare R2 is the hosted immutable component-byte store. Objects are
addressed by verified content identity and written through a bounded storage
adapter. Metadata records bind an exact revision manifest to its components;
clients never infer authority from knowing an object key.

The hosted storage contract requires:

- tenant-qualified access capabilities and object paths;
- digest and size verification before publication and after retrieval;
- immutable-write or compare-existing semantics for content identities;
- no public bucket listing;
- bounded signed acquisition rather than durable bucket credentials;
- lifecycle rules that respect retained revisions, forks, takedowns, legal
  holds, and rollback windows;
- inventory, backup, restore, and streamed-integrity qualification; and
- a storage adapter that preserves S3-compatible self-hosted alternatives such
  as MinIO.

R2 is the hosted choice, not a product requirement. Self-hosted Depot remains
able to use supported S3-compatible storage or filesystem storage according to
its documented backend contract.

## Control-plane boundaries

The shared hosted control plane owns account, organization, entitlement,
deployment, and billing orchestration. Domain services remain authoritative for
their resources.

| Concern | Authority | Boundary |
| --- | --- | --- |
| Sign-in and account linking | identity service | produces verified principals; does not grant tenant access |
| Membership and RBAC | access-control authority | organization/project grants consumed by both products |
| Artifact metadata and publication | Depot | validates every registry mutation and distribution decision |
| Artifact bytes | Depot storage adapter and R2 | exact immutable components; no policy inferred from keys |
| MCP runtime and upstream credentials | Labby | per-runtime/per-subject connections and execution |
| Runtime provisioning | hosted control plane | creates, upgrades, suspends, and deletes paid runtimes through explicit desired state |
| Billing and entitlements | billing authority | maps subscriptions to bounded plans, quotas, and features |
| Audit | append-only audit pipeline plus domain emitters | records authoritative actor, tenant, operation, outcome, and correlation without secrets |

The control plane passes opaque resource identities and scoped desired state. It
does not open Labby or Depot databases directly, duplicate their business
rules, or silently retry non-idempotent mutations. Every cross-service command
uses an idempotency identity, deadline, authenticated service identity, bounded
payload, and observable terminal outcome.

## Billing and paid runtime model

Billing is separate from authentication and authorization. A paid entitlement
may permit a hosted runtime, more storage, more seats, higher request limits, or
specific managed features. It never grants access to an Artifact, organization,
project, upstream, or credential by itself.

Metering facts are produced by the service that performed the work, then
aggregated by the billing plane. They include stable tenant/runtime dimensions,
quantity, period, event identity, and correction semantics, but no prompts,
Artifact bodies, credentials, or raw tool parameters. Duplicate delivery is
expected and deduplicated by event identity. Plan downgrade, payment failure,
and quota exhaustion have explicit grace and recovery behavior; they do not
silently delete immutable Artifacts or personal data.

Hosted runtimes are isolated execution units even when the control plane is
shared. Runtime credentials, process/container identity, durable state,
network policy, resource quotas, backups, and logs are tenant-bound. A noisy or
compromised runtime must not gain another runtime's credentials or storage.

## Audit contract

Sensitive reads, mutations, publication, withdrawal, takedown, forks,
distribution, membership/RBAC changes, entitlement changes, runtime lifecycle,
and service-to-service delegation emit structured audit events.

Each event records the authoritative principal/service, tenant, project or
personal context, target ID, operation, policy/entitlement version, outcome,
correlation and idempotency IDs, and bounded reason codes. Secrets, OAuth
material, signed URLs, prompts, component bytes, and raw sensitive parameters
are excluded. Tenant administrators can read only their authorized tenant audit
view; platform security access is separately privileged and audited.

## Existing Skills repositories

The existing `limetech-ai-skills` and `limetech-elixir-skills` repositories
remain readable during migration but stop being an active publication path.
They are locked against ordinary content updates after Depot authoring and Lime
team publication pass acceptance.

Their retained purposes are:

- historical source and review provenance;
- rollback reference during the transition;
- compatibility for consumers not yet migrated; and
- an explicit, time-bounded export or archival source.

New Skills and updates are authored and published through Depot and made
available through team Labby. A repository is not kept synchronized as a second
mutable authority. Any emergency repository exception requires a named owner,
an exact revision, and an explicit reconciliation into Depot.

## Migration

### Phase 0: freeze contracts

- Preserve ArtifactInterchange v1 and the existing access-control vocabulary.
- Define stable tenant, publisher namespace, runtime, entitlement, audit, and
  metering identities.
- Qualify R2 inventory, integrity, backup, restore, and rollback without
  deleting the existing filesystem authority.

### Phase 1: Lime private tenant

- Deploy the isolated Lime Depot process and hosted Lime team Labby.
- Configure separate metadata, credentials, R2 scope, observability, and
  rollback from the public Depot process.
- Give the default Lime member role bounded create and publish rights.
- Prove discovery, authoring, validation, publication, withdrawal, exact
  acquisition, fork lineage, MCP consumption, and Personal Labby delivery.
- Retain the old Skills repositories as read-only migration inputs.

### Phase 2: public product completion

- Complete public discovery, authenticated creation/publication, personal
  library, namespace ownership, abuse response, takedown, and permitted forks.
- Ship hosted personal Labby and paid team runtime provisioning.
- Keep product-facing public and private contexts explicit and non-fallback.

### Phase 3: shared tenant-aware Depot plane

- Introduce tenant-qualified metadata persistence, search, caches, jobs, audit,
  quotas, and service capabilities behind the same domain contracts.
- Run public and Lime traffic in shadow/read-compare or other non-authoritative
  qualification before writes move.
- Migrate one bounded tenant at a time with census, digest, authorization,
  audit, failure, restart, backup/restore, rollback, and no-cross-tenant proof.
- Retire the temporary Lime Depot process only after the shared plane is
  authoritative and the rollback window has elapsed.

### Phase 4: repository lock and normal operations

- Lock the two legacy Skills repositories against ordinary content updates.
- Publish all subsequent Skills through Depot and expose them through team
  Labby.
- Operate shared release, incident, billing, abuse, backup, restore, and tenant
  offboarding procedures with periodic isolation tests.

## Rollback principles

- Process-isolated public and Lime Depot deployments remain independently
  reversible during the first-tenant phase.
- R2 migration never requires source filesystem deletion.
- Metadata cutovers retain a durable checkpoint and an explicit prior authority;
  no component silently reads from a stale backend after cutover.
- New writes created during a retention window are included in rollback proof.
- Hosted Labby upgrades preserve durable runtime state and have an independently
  deployable previous release.
- A failed tenant migration returns only that tenant to its prior authority; it
  never falls through to public or another tenant's data.

## Explicit non-goals

- One Depot process per long-term customer.
- Combining Labby and Depot into one service or database.
- Requiring the hosted control plane for self-hosted Labby or Depot.
- Making R2 the only supported storage backend.
- Inferring authorization from bucket paths, object knowledge, email domains,
  billing status, or MCP route names.
- Treating public visibility as permission to fork, export, or reshare.
- Silent activation or silent update of imported Skills.
- Copying organization secrets or upstream credentials into Artifacts or a
  Personal Labby transfer.
- Preserving Git repositories as a second mutable publication authority.
- Cross-company trust federation in the initial SaaS release.
- Perfect remote deletion claims for legitimately exported or forked bytes on
  offline user-controlled devices.

## Consequences

The temporary two-process deployment ships Lime sooner and creates a clean
blast-radius boundary, at the cost of short-lived operational duplication. The
shared metadata plane then avoids per-customer process sprawl, but demands
tenant identity in every persistence and asynchronous boundary plus sustained
adversarial isolation testing.

Labby and Depot retain clear ownership and self-hostability. Customers can use
public Depot without buying a hosted runtime, run Personal Labby against public
or authorized private Artifacts, or buy managed personal/team Labby capacity.
The same immutable Artifact and authorization contracts apply in every mode.

## Related contracts

- [Access Control, Workspaces, and Artifact Distribution](../access-control/README.md)
- [Artifact Distribution and Personal Labby Sync](../access-control/ARTIFACT_DISTRIBUTION.md)
- [Artifact contract](../artifacts/contract.md)
- [Artifact specification](../artifacts/spec.md)
- [Phabby shared control plane](./phabby-control-plane.md)
- [Durable-state disaster recovery](../runtime/DISASTER_RECOVERY.md)
