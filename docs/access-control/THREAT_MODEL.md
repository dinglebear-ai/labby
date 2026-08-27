---
title: "Access Control Threat Model"
created: "2026-08-22"
updated: "2026-08-22"
status: "design"
---

# Access Control Threat Model

## Security objective

An authenticated Labby principal must be able to discover, use, administer, or distribute only the assets/capabilities authorized for the active organization/project context, and runtime execution must use only credentials/bindings approved for that context.

The system must remain fail-closed under stale caches, malformed policies, missing upstream capabilities, revoked memberships, destination failures, and Artifact policy uncertainty.

## Trust boundaries

### Authentication boundary

labby-auth establishes request identity and coarse OAuth/session authority. Access control trusts only verified issuer/subject facts supplied by that boundary.

### Access policy boundary

AccessStore owns memberships, Roles, Grants, Assignments, composition policy, Project overlay policy, destinations, and policy epochs.

### Artifact boundary

ArtifactStore/ArtifactInterchange owns content integrity, revision identity, provenance, license/publication/takedown state, and lineage. Access control consumes validated Artifact facts but cannot override them.

### Gateway/runtime boundary

The gateway owns the current compiled/discovered capability catalog and actual dispatch. Access policy cannot make a nonexistent/hidden upstream capability real.

### Secret boundary

Credential/secret owners keep raw secret values. AccessStore/EffectiveWorkspace carries only opaque binding references.

### Destination boundary

A paired personal Labby is a separate administrative/runtime instance. Transfer authorization must not assume the destination is always online, uncompromised, or able to honor a later purge request.

## Primary threats and mitigations

### T1: Discovery filtering used as the only authorization check

Attack: A caller manually invokes a hidden tool/resource by name even though it was absent from tools/list or Code Mode search.

Mitigations:

- direct invocation always calls shared authorization;
- cached workspace must match current policy epoch/catalog generation;
- target identity is canonicalized before checking;
- tests invoke hidden targets directly through every supported surface.

### T2: Cross-project confused deputy

Attack: A user authorized for a GitHub tool in Project A selects Project B but dispatch accidentally uses Project A's broader credential because both projects reference the same upstream name.

Mitigations:

- Project context is request/route/session scoped under PROJECT_CONTEXT.md; there is no process-global or principal-global mutable active Project;
- Project ID is mandatory runtime-binding selection input;
- an MCP session binds one Project no later than establishment and cannot be switched by another client, nested Code Mode call, or MCP App;
- RuntimeBinding uniqueness includes Project + target;
- workspace/cache keys include the exact Project;
- secret values never live in EffectiveWorkspace;
- dispatch rejects missing/ambiguous binding instead of falling back;
- tests create concurrent sessions plus identical upstream names with different credentials and prove catalog/context/credential isolation.

### T3: OAuth scope/domain role confusion

Attack: lab:admin token is treated as Organization Owner or a Project Admin role is used to bypass a missing transport scope.

Mitigations:

- transport scopes and domain permissions remain separate checks;
- no role is inferred from OAuth scope;
- code review lint/tests forbid known shortcut checks where feasible;
- matrix tests cover each intersection of read/admin transport scope and domain role.

### T4: Email-based identity takeover/rebinding

Attack: authorization is keyed by mutable/case-insensitive email and a new identity obtains another person's grants.

Mitigations:

- durable link key is trusted issuer + subject;
- email is verified metadata only;
- identity-link changes require explicit administrative authorization/audit;
- uniqueness prevents one external identity mapping to multiple Principals.
- transport issuers are distinct from canonical identity authorities;
- browser sessions persist canonical provider identity facts;
- issuer normalization is allowlisted and subject-only fallback is forbidden;
- static credentials use explicit stable credential IDs.

### T5: Stale-cache access after revocation

Attack: removed Group/Project membership continues to authorize because a caller-specific catalog or workspace is cached.

Mitigations:

- policy mutations atomically advance epoch;
- cache keys include policy epoch and catalog generation;
- direct action checks reject stale versions;
- revocation test holds a pre-revocation catalog then attempts direct execution after mutation.

### T6: Group hierarchy cycle or cross-organization edge

Attack: malformed Group graph causes infinite recursion, unexpected inherited privileges, or cross-tenant access.

Mitigations:

- one optional parent per Group in v1;
- same-Organization parent constraint;
- transaction-time cycle validation;
- bounded iterative ancestor resolution, not unbounded recursion;
- database and property tests generate cycles/deep chains.

### T7: Role/grant escalation by assignment administration

Attack: a Project Owner or Admin assigns a privileged runtime capability and believes assignment itself grants execution.

Mitigations:

- Assignment availability and Permission authority are separate;
- asset.assign does not imply asset.use/runtime_binding.use/secret.use;
- target is rechecked against current gateway exposure;
- policy mutation authorization is scope-bound.

### T8: Accidental override of mandatory controls

Attack: a Project/personal Artifact with the same name shadows an Organization security prompt or required Skill.

Mitigations:

- overrides require explicit slots and relations;
- name collision never means override;
- mandatory assignments reject override/mask;
- conflicting exclusive slots fail instead of choosing by ordering.

### T9: Personal overlay privilege expansion

Attack: user adds a personal Skill/Loadout that references a hidden upstream or production tool and thereby exposes it inside a Project.

Mitigations:

- overlay is Project-opt-in;
- candidates are intersected with Project gateway/catalog authority;
- personal content cannot introduce runtime binding/secret authority;
- every transitive Loadout dependency is independently authorized.

### T10: Loadout dependency smuggling

Attack: authorized Loadout references forbidden tool/resource/Artifact and gateway trusts container authorization.

Mitigations:

- dependency authorization is recursive/independent;
- required inaccessible dependency fails closed;
- optional omission must be explicitly allowed and audited;
- dependency graph is bounded and cycle-checked.

### T11: Capability-name collision/confusion

Attack: two upstreams expose a tool with the same human name and an Assignment or invocation resolves to the wrong one.

Mitigations:

- canonical capability key includes upstream/source identity + capability kind + canonical capability name;
- UI may show friendly names but persistence/authorization uses canonical keys;
- ambiguous unqualified references are rejected.

### T12: Managed Artifact sync becomes data exfiltration

Attack: ordinary artifact.use or organization visibility lets a user copy proprietary bytes to a personal device.

Mitigations:

- artifact.sync/fork/export/reshare are separate permissions;
- distribution intersects the Artifact owner's publisher ceiling, the specific Assignment's default-deny distribution ceiling, caller permissions, and Artifact license/publication/takedown state;
- default unknown/restricted redistribution blocks byte transfer;
- paired destination required for managed sync.

### T13: ArtifactInterchange ACL smuggling

Attack: a transferred Artifact includes forged metadata claiming wider access rights and receiving Labby trusts it.

Mitigations:

- ArtifactInterchange v1 carries no access grant authority;
- transfer authorization is a separate source decision;
- receiving Labby validates its own destination/local policy;
- unknown extension metadata cannot grant permissions.

### T14: Follow subscription continues after revocation

Attack: user loses Project membership but a background subscription pulls the next proprietary revision.

Mitigations:

- every observation/apply rechecks current authorization and source policy;
- subscription stores last policy epoch only as evidence, not authority;
- revocation marks mirror/subscription unavailable;
- auto_approved update is still a fresh authorization operation.

### T15: Fork/mirror semantic confusion

Attack: managed source-controlled content is presented as user-owned, or a detached fork is treated as remotely revocable when it is not.

Mitigations:

- distinct persisted states and UI labels;
- fork always has new Artifact identity + ArtifactLineage;
- managed mirror preserves source authority/revocation state;
- audit distinguishes pin/follow/fork/export.

### T16: Remote purge overclaim

Attack: source UI claims revoked bytes were deleted from an offline destination, creating a false compliance assertion.

Mitigations:

- purge request and purge acknowledgement are separate states;
- unreachable/offline destination remains pending/unverified;
- UI/audit never report successful deletion without destination evidence.

### T17: Destination SSRF/arbitrary exfiltration

Attack: Send to accepts an arbitrary URL and source Labby pushes Artifact bytes/tokens to attacker-controlled network locations.

Mitigations:

- only paired destination IDs are accepted by normal transfer API;
- pairing validates canonical endpoint/identity under existing network/SSRF controls;
- transfer does not accept redirect-based endpoint substitution without validation;
- no reusable organization bearer token is sent to destination.

### T18: Artifact byte/digest substitution during transfer

Attack: source/destination gets metadata for allowed revision but bytes are swapped in transit/cache.

Mitigations:

- existing ArtifactAcquisition size and SHA-256 verification is mandatory;
- exact revision digest is authorized;
- local state mutation occurs only after full validation;
- failed transfer leaves no partially trusted revision head.

### T19: License/publisher policy race

Attack: transfer is authorized, then source is withdrawn/restricted before bytes are applied.

Mitigations:

- short-lived operation authorization;
- apply step revalidates relevant source policy/version where practical;
- exact policy epoch/revision recorded;
- transactional local commit only after final validation.

### T20: TOCTOU on policy mutation and execution

Attack: permission is revoked between workspace resolution and tool dispatch.

Mitigations:

- dispatch performs/validates a current decision immediately before crossing sensitive runtime boundary;
- policy epoch mismatch forces recomputation;
- long-running operations define whether authorization is checked at start only or at safe phase boundaries; no silent privilege extension for new side effects.
- authorization is linearized at the final in-process dispatch boundary;
- Milestone 1 performs an uncached current AccessStore read;
- race tests record decision and execution revisions using barriers;
- multi-step operations re-authorize before each independently avoidable external side effect.

### T21: Explanation endpoint leaks hidden assets/groups

Attack: unauthorized caller probes why access failed to enumerate project names, Group membership, or hidden tool names.

Mitigations:

- ordinary denial is non-enumerating;
- rich explanation requires policy.explain or audit.read as appropriate;
- target identifiers are fingerprinted/redacted in general audit logs;
- explanation lookup itself checks scope authorization.

### T22: Audit log secret leakage

Attack: runtime binding, destination credential, prompt/resource contents, token, or Artifact secret bytes land in authorization logs.

Mitigations:

- audit model stores IDs/fingerprints/reason codes;
- follows docs/dev/OBSERVABILITY.md redaction contract;
- no EffectiveWorkspace field contains secret values;
- redaction tests include known credential patterns and Artifact secret detector fixtures.

### T23: Service account becomes ambient superuser

Attack: static bearer or service Principal inherits implicit owner behavior because no human session is present.

Mitigations:

- every credential maps to explicit Principal;
- service account has explicit memberships/grants only;
- single-user migration owner is explicit, not fallback behavior;
- missing mapping fails closed.

### T24: Cross-tenant IDOR

Attack: caller supplies Group/Project/Assignment/Artifact ID from another Organization directly to update/read endpoints.

Mitigations:

- every referenced object is checked against active Organization/scope;
- persistence queries prefer organization-qualified lookups;
- cross-organization relations fail validation;
- test suite exercises valid-looking foreign IDs for every mutation type.

### T25: Policy explosion/resource exhaustion

Attack: extremely deep Group tree, huge membership set, giant Assignment graph, or cyclic Loadout dependencies exhausts resolver memory/CPU.

Mitigations:

- explicit bounds on hierarchy depth, members/assignments returned per resolution, dependency graph size, and explanation evidence;
- iterative bounded algorithms;
- indexed database queries;
- property/load tests at expected large organization sizes plus above-limit rejection.

### T26: Reused stale transfer authorization

Attack: one successful Add to My Labby authorization is replayed for many revisions/destinations.

Mitigations:

- transfer authorization binds actor, exact revision, mode, destination, policy version, and short validity window/nonce when protocolized;
- destination/source record idempotency/replay state;
- no wildcard future revision grant for ordinary sync.

### T27: Local Artifact path/secret safety regression

Attack: access-control transfer bypasses existing Artifact safe import/export and introduces symlink traversal, oversized package, executable-mode, or secret-export regressions.

Mitigations:

- all local materialization/export routes through existing Artifact APIs;
- no new raw filesystem copy path in access layer;
- Artifact traversal/size/digest/secret tests remain mandatory gates.

### T28: Authorization database corruption or permissive fallback

Attack: disk-full, corruption, a newer schema, unsafe permissions, or migration failure causes compatibility-owner fallback or partial revocation.

Mitigations:

- authorization-grade SQLite profile, integrity/tenant checks, restrictive permissions, serialized migration, and explicit durability;
- unsafe state fails closed as service-unavailable/setup-required;
- compatibility fallback is impossible after activation;
- crash, corruption, disk-full, busy, permission, backup/restore, and downgrade tests.

### T29: Cross-store split brain

Attack: Artifact, secret, gateway, or AccessStore state commits without the other side, creating ghost authority or unsafe usable content.

Mitigations:

- durable operation IDs and pending/committing/active/failed states;
- idempotent finalize/compensate/reconcile paths;
- only active reconciled state is authorizable;
- crash-point and restart matrices cover every boundary.

### T30: Bootstrap or audit failure creates silent privilege

Attack: concurrent bootstrap creates multiple owners, configuration drift auto-promotes another identity, or an administrative mutation succeeds without durable audit.

Mitigations:

- one-time compare-and-set bootstrap with generation and safe fingerprint;
- ambiguous identity requires explicit setup and config changes never auto-promote;
- policy mutations and audit commit atomically;
- successful sensitive mutations cannot silently lose required evidence.

## Security invariants

1. No authorization decision uses mutable display name/email as the stable identity key.
2. No transport surface implements a private policy fork.
3. No workspace/catalog cache survives a relevant policy version change.
4. No direct runtime action trusts discovery filtering alone.
5. No Project runtime credential is selected without Project context.
6. No access policy can broaden current gateway exposure.
7. No personal overlay can broaden Project runtime authority.
8. No Loadout/container authorization bypasses dependency authorization.
9. No Artifact use grant implies byte transfer/fork/export/reshare.
10. No Artifact access field is added to ArtifactInterchange v1.
11. No managed transfer trusts arbitrary destination URLs.
12. No EffectiveWorkspace/audit record contains secret material.
13. No rich denial explanation is available without explanation/audit permission.
14. No generic Deny precedence exists in v1; masks are composition-specific and explicit.

## Mandatory adversarial test matrix

Before default-on enforcement, tests must cover at least:

- direct hidden tool invocation after filtered discovery;
- Code Mode search/call bypass attempts;
- API and CLI direct object ID access from foreign Project/Organization;
- lab:admin without domain permission;
- domain admin without required OAuth scope;
- revoked Group membership with cached workspace;
- Group cycle/cross-Organization parent attempt;
- mandatory Assignment override/mask attempt;
- personal overlay hidden upstream injection;
- Loadout required dependency smuggling;
- same-name capability collision across upstreams;
- Project A credential binding used from Project B;
- artifact.use to sync/fork/export escalation attempts;
- unknown/restricted license transfer;
- unpaired destination URL SSRF attempt;
- replayed transfer authorization for different revision/destination;
- digest mismatch/partial transfer rollback;
- follow update after membership/source-policy revocation;
- rich explanation enumeration without policy.explain;
- audit redaction for tokens/secrets/emails where policy requires fingerprinting;
- existing Artifact path/symlink/size/secret conformance suite;
- ArtifactInterchange byte-canonical fixture unchanged.

## Review gate

Implementation is not security-complete when unit tests merely demonstrate expected happy paths. Before enabling multi-user enforcement by default, perform an explicit adversarial review of:

- identity mapping;
- persistence constraints/migrations;
- resolver precedence;
- cache invalidation;
- gateway/Code Mode enforcement;
- runtime credential selection;
- transfer/pairing networking;
- Artifact distribution/license intersection;
- explanation/audit redaction; and
- single-user migration/bootstrap behavior.

All findings are tracked in PROGRESS.md until resolved or explicitly deferred with rationale and containment.
