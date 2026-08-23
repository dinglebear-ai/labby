---
title: "Access Control Architecture"
created: "2026-08-22"
updated: "2026-08-22"
status: "design"
---

# Access Control Architecture

## Current-code grounding

The design intentionally builds on current Labby boundaries rather than replacing them.

- labby-auth already produces AuthContext with issuer, subject, OAuth scopes, browser-session state, optional actor key, and optional verified email.
- labby-runtime already implements ArtifactInterchange v1 and an explicit-root Artifact store/lifecycle library. The current product does not yet own and open one canonical ArtifactStore as application state; product wiring is a prerequisite, not an existing enforcement seam.
- GatewayLoadoutConfig already selects upstreams/services and gates tools, resources, prompts, skills, and Code Mode.
- MCP route scoping already computes capability gates from a Loadout and narrows the gateway view.
- labby-gateway already depends on labby-runtime and labby-auth, so the authorization design must not introduce a dependency cycle between those crates.

The new subsystem should reuse those mechanisms as inputs/output adapters. It should not duplicate them.

## Proposed component boundary

### labby-access

Target shared boundary: labby-access, after an extraction gate.

Milestone 1 starts as a private, surface-neutral `crates/labby/src/access/` module so one complete policy path can prove its types and dependency direction. Extract it to `crates/labby-access` before a second concrete consumer or when architecture tests demonstrate that keeping it in the product crate would create transport coupling or a dependency cycle. The extraction must preserve behavior through contract fixtures; this packet does not pre-commit a broad public API before that evidence exists.

It owns:

- Principal, external identity link, Organization, Group, Project;
- Membership, Role, Permission, Grant;
- Assignment and composition-slot policy;
- Artifact ownership authority plus publisher/assignment distribution ceilings;
- Project personal-overlay policy;
- destination/mirror authorization metadata;
- policy epochs/versioning;
- access-control persistence/migrations;
- deterministic policy evaluation;
- Decision/Explanation models; and
- EffectiveWorkspace domain types that contain identifiers and safe policy facts, never secrets.

labby-access SHOULD depend on labby-primitives and common workspace libraries, but SHOULD NOT depend on labby-auth, labby-gateway, or concrete HTTP/MCP surfaces.

The core resolver should operate on explicit resolution inputs/snapshots rather than reaching into gateway/network services itself. This keeps policy tests deterministic and avoids trait-object service graphs.

### labby-auth

labby-auth remains authentication/OAuth/session authority.

Its integration responsibility is to provide verified authentication facts that can be mapped to an access Principal. AuthContext remains a request-authentication structure rather than becoming an ACL bag.

The access integration adapter maps `VerifiedIdentity` to a Principal. `AuthContext.issuer` remains a transport-credential fact and is not blindly used as the provider identity issuer. Browser sessions must preserve/recover canonical provider issuer and subject; static/service credentials use stable credential IDs. Email is display metadata only.

### labby-runtime Artifact subsystem

The existing Artifact subsystem remains authoritative for:

- Artifact identity;
- immutable revisions/digests;
- component bytes;
- provenance;
- license/redistribution state;
- publication state;
- lineage;
- local workspaces;
- import/export safety; and
- provider acquisition/update planning.

Access control references Artifact ID + exact revision ID and consumes safe policy facts derived from Artifact state. It does not duplicate Artifact payload metadata into ACL tables.

Before Artifact Assignments or distribution are implemented, the top-level application must own one configured canonical ArtifactStore root and handle startup, doctor, restart reconciliation, and fact projection. SQLite cannot foreign-key into the filesystem store; every reference is application-validated and orphan state is fail-closed.

### labby-gateway

labby-gateway remains authoritative for the current runtime catalog and upstream execution.

Gateway integration consumes an EffectiveWorkspace or a compiled workspace filter to:

- filter upstreams/services;
- filter tools/resources/prompts/skills;
- preserve Loadout capability gates;
- constrain Code Mode catalog/search;
- re-authorize direct invocation; and
- select only an opaque runtime-binding reference valid for the active Project.

The current Loadout/route-scope model is therefore a natural enforcement adapter. EffectiveWorkspace narrows it; it never broadens it.

### labby top-level application

The top-level application composes authentication facts, access persistence, Artifact facts, current gateway catalog, Project context, and runtime bindings into one resolution input. Project context is request/route/session scoped according to [PROJECT_CONTEXT.md](./PROJECT_CONTEXT.md); there is no shared mutable active Project.

Transport handlers remain thin:

1. authenticate;
2. enforce coarse transport scope;
3. resolve Principal and Project context;
4. ask shared access layer for workspace/decision;
5. call the existing shared business/runtime operation with the authorized projection.

## Resolution data flow

The conceptual request flow is:

Authenticated request
    -> AuthContext
    -> external identity mapping
    -> Principal
    -> requested Organization/Project context
    -> AccessStore snapshot
    -> Artifact policy facts + current Gateway catalog facts
    -> shared policy resolver
    -> EffectiveWorkspace / AuthorizationDecision
    -> surface-specific projection
    -> runtime dispatch with opaque Project binding
    -> audit evidence

No surface may skip the shared decision because another surface already filtered the catalog.

## Resolution input

To keep labby-access independent of concrete gateway/runtime crates, higher layers construct a bounded ResolutionInput containing:

- Principal record/status;
- requested Organization/Project;
- memberships/roles/grants relevant to that Principal;
- applicable Assignments;
- personal overlay candidates/policy;
- safe ArtifactPolicyFact records for referenced revisions;
- GatewayCatalogFact records for referenced upstreams/services/capabilities;
- Loadout dependency/capability-gate facts;
- opaque RuntimeBindingFact records; and
- policy/catalog revision numbers.

The resolver does no network I/O. It produces deterministic output from this snapshot.

### Coherent snapshot protocol

The top-level adapter assembles facts with a bounded optimistic snapshot protocol:

1. capture the AccessStore revision inside one read transaction;
2. load all relevant membership/policy rows with bounded set-based queries from that transaction;
3. capture Artifact policy/control-state fingerprints and gateway/loadout catalog generation;
4. assemble the resolution input;
5. reread the AccessStore revision and external generations;
6. accept only when the before/after tuple matches; otherwise retry a bounded number of times and then fail closed as `authorization_snapshot_unstable`.

Artifact revision identity alone is not a policy version: publication, takedown, license, and authority policy facts require an explicit version/fingerprint. Every fact provider contract states its bound, version semantics, and failure behavior.

AccessStore resolution must use a bounded number of set-based queries; per-membership, per-role, per-assignment, or per-capability N+1 queries are prohibited. Query-count and query-plan tests are required before enforcement.

## Bound access context

The top-level application creates a `BoundAccessContext` after authentication and Project selection. Stateful MCP stores it in the server-owned session; stateless HTTP constructs it per request; stdio and in-process peers receive an explicit local/service binding. Pagination cursors, tasks, notifications, MCP Apps, reconnect/resume, and Code Mode nested calls retain or reference the same server-owned binding.

Caller-controlled `_meta`, nested MCP App input, or Code Mode code cannot choose Principal, Organization, Project, policy revision, or runtime binding. Where serialization is unavoidable, only an opaque server-side context ID or integrity-protected internal envelope is accepted. Missing context never becomes bootstrap-owner authority.

## EffectiveWorkspace projection

EffectiveWorkspace is a control-plane projection, not a secret-bearing runtime object.

It contains:

- active Principal/Organization/Project IDs;
- effective Group IDs;
- permissions relevant to the context;
- resolved Assignment IDs and provenance scopes;
- exact Artifact revision references;
- allowed upstream/service/capability references;
- resolved Loadout references and dependency statuses;
- capability-category gates;
- opaque runtime binding IDs;
- policy/catalog versions; and
- bounded explanation reference.

Transport/UI formatting stays outside the access crate.

## Catalog enforcement

### Discovery

The gateway derives a caller-specific catalog from the intersection of:

1. compiled/current gateway catalog;
2. route/Loadout exposure policy;
3. EffectiveWorkspace assignments;
4. caller permissions; and
5. Project/runtime constraints.

This filtered catalog feeds MCP list operations, Code Mode search, web command palette/search, and principal-scoped CLI/API catalog views.

### Direct invocation

Direct invocation repeats authorization using target identity and requested action. It may reuse a cached workspace only when the policy epoch and catalog generation are still current.

This prevents a stale list result or manually constructed tool call from bypassing revocation.

## Loadout architecture

Current GatewayLoadoutConfig remains a runtime projection with upstream/service selections and capability-family gates.

V1 access control SHOULD introduce an adapter that treats a named GatewayLoadoutConfig as an assignable target without adding ACL fields to it.

Longer term, Loadouts SHOULD become Artifact-backed composition manifests. An Artifact-backed Loadout can reference exact immutable dependencies and compile to the existing GatewayLoadoutConfig/runtime projection.

The migration sequence must preserve existing Loadout behavior until Artifact-backed Loadouts reach parity.

## Runtime binding architecture

Project-specific runtime binding is a later milestone. Before it begins, the implementation must map the real current credential owners, upstream OAuth refresh lifecycle, connection-pool keys, reconnect behavior, and invalidation path. Project/binding identity must participate in every connection/cache key that can retain credentials. Missing, ambiguous, expired, or failed secret resolution never falls back to another Project or subject; it returns a stable redacted unavailable error.

Credentials are not assets distributed through the Artifact system.

A RuntimeBinding associates a Project and target with an opaque reference into the appropriate secret/credential owner. The access layer decides whether the binding may be selected; the secret-owning layer returns/uses credential material only at dispatch time.

Cache keys and dispatch context include Project identity. A shared upstream name is insufficient to select a credential.

## Artifact distribution architecture

Artifact distribution is a dependent milestone, not part of Milestone 1. It cannot begin until application-owned ArtifactStore wiring and a cross-store operation state machine are complete.

Artifact transfer uses two independent planes:

### Content plane

ArtifactInterchange v1 plus exact component bytes, verified through the existing Artifact provider/acquisition rules.

### Authorization/control plane

A separate Labby transfer/sync request contains source Principal/scope, exact Artifact revision, requested transfer mode, destination identity, and policy evidence/authorization. Source authorization intersects the Artifact owner's publisher distribution ceiling with the narrower Assignment distribution policy before license/publication/takedown and destination checks. It MUST NOT modify ArtifactInterchange v1.

Receiving Labby validates the Artifact using existing Artifact rules and records local mirror/fork state separately. A managed mirror retains remote source authority; only an explicitly permitted fork creates a new locally authoritative Artifact identity.

## Destination registration

A user may register one or more Labby destinations such as Personal Labby, Laptop Labby, or Devbox Labby.

Destination records contain identity/routing/trust metadata, capabilities, ownership, and status. Long-lived transfer credentials are stored through the normal secret/credential layer and referenced opaquely.

The initial implementation should require an explicit authenticated pairing/authorization flow. Do not treat an arbitrary URL supplied at transfer time as a trusted destination.

## Policy/version invalidation

Milestone 1 maintains one monotonically increasing AccessStore revision for snapshot stability and audit correlation, not caching. A later cache design uses explicit Organization-emergency, Project, Artifact-policy, destination-policy, and gateway-catalog version domains; no optimization may weaken revocation correctness.

Gateway catalog changes carry an independent catalog generation.

A cached workspace is valid only while all relevant versions still match.

## Audit architecture

Policy mutation and sensitive use/distribution events append structured evidence to an audit sink with existing observability redaction rules.

Audit payloads use IDs/fingerprints where names may leak hidden resources. Rich human explanations are generated only after checking policy.explain/audit.read.

## Deployment/compatibility strategy

Bootstrap/migration precedes enforcement. It is one-time, compare-and-set, idempotent, and explicit when identity is ambiguous. Once enforcement is activated, an unavailable/corrupt AccessStore never falls back to compatibility-owner behavior.

The access subsystem should initially be disabled or single-owner compatible when no multi-user policy database exists.

Migration MUST preserve today's personal/single-user behavior without accidentally making a formerly local gateway publicly accessible. Bootstrap migration should create an explicit local owner Principal/Organization and grants rather than rely on implicit superuser fallbacks.

Feature rollout should proceed in stages:

1. domain and persistence with no enforcement;
2. shadow resolution/explanation telemetry in local logs without secret/name leakage;
3. principal-scoped catalog filtering behind an opt-in flag;
4. direct-invocation enforcement;
5. Project runtime binding enforcement;
6. Artifact distribution/personal-Labby transfer;
7. default-on multi-user behavior only after migration/adversarial gates pass.
