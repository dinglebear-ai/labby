---
title: "Access Control Implementation Plan"
created: "2026-08-22"
updated: "2026-08-22"
status: "planned"
---

# Access Control Implementation Plan

## Implementation posture

Implementation SHALL follow test-driven development. Each slice begins with failing contract/unit/integration tests that describe the externally meaningful behavior, then adds the minimum shared implementation, then refactors only after the tests pass.

No transport-specific authorization shortcut is accepted as a temporary implementation if it changes policy semantics. CLI/API/MCP/Code Mode/web adapters may land incrementally, but they must all call the same shared access layer.

The first implementation should be opt-in/shadowed until migration, uncached direct-invocation enforcement, storage/session failure behavior, and adversarial gates are proven. Cache invalidation becomes a gate only when a later measured cache is introduced.

## Engineering-review delivery order

The numbered phases below are the complete initiative roadmap, not one release. Engineering review established this binding delivery order:

### Milestone 0: prerequisites

1. Add canonical `VerifiedIdentity` facts to the authentication boundary. Browser and bearer auth for one provider identity must converge; static/Unix credentials use explicit stable credential IDs.
2. Freeze the AccessStore security profile, tenant-integrity strategy, coherent snapshot protocol, typed failures, and single-owner bootstrap/migration.
3. Implement bootstrap before shadow resolution or enforcement. No AccessStore failure may fall back to owner compatibility after activation.
4. Add minimal redacted decision logs and transactional policy-mutation audit from the first mutation.

### Milestone 1: Project-bound MCP isolation

Implement only principals, external identities, Organizations, Projects, direct Principal membership with the code-owned Project roles Owner/Admin/Member/Viewer, and Project-to-existing-named-Loadout selection. Project Owner is Project-scoped and does not imply Organization Owner. Start in a private surface-neutral `crates/labby/src/access/` module; extract `labby-access` only after a second consumer or an architecture/dependency test proves the need.

Bind each MCP request/session to a server-owned `BoundAccessContext`; intersect discovery with the existing route/Loadout scope; repeat an uncached authorization read at final direct dispatch. Include reconnect, tasks, pagination, Code Mode, MCP Apps, in-process, stdio/Unix, and revoke/check/dispatch race tests. Ship shadowed/opt-in only.

### Milestone 2: policy expansion

Only after Milestone 1 evidence may the project add Groups, Group Project membership, custom Roles/Grants, temporal membership, generalized Assignments, inheritance, slots, overrides/masks, per-capability policy, personal overlays, or non-MCP parity. Each addition requires a concrete user case and preserves the same shared check.

### Milestone 3: runtime credentials

First map existing credential owners, OAuth refresh, connection-pool keys, reconnect, and invalidation. Then add Project bindings with no fallback and Project/binding identity in every credential-retaining cache key.

### Milestone 4: Artifact application wiring and local policy

Before Artifact ACL/distribution, make one canonical ArtifactStore root application-owned with startup/doctor/reconciliation and safe versioned fact adapters. Cross-store operations use pending/finalize/compensate/reconcile states and operation IDs.

### Milestone 5: distribution/federation

Freeze the protocol in ARTIFACT_DISTRIBUTION.md before pairing or transfer code. Pin/follow/fork/export/reshare, destination federation, purge acknowledgement, and Loadout dependency transfer are separate dependent work, not Milestone 1.

### Milestone 6: caching, persistent explanation, and scale

Milestone 1 is uncached. Add version-domain caching only after cold-resolution and contention benchmarks require it. Rich persistent explanation evidence is separate from minimal logs/transactional mutation audit.

## Required failure contract for every phase

Every new codepath must state and test: realistic failure; fail-open/fail-closed behavior; transaction/linearization boundary; retry/idempotency/compensation; stable user-facing error kind; redacted log/audit event; operator recovery; and crash/restart behavior. A phase cannot exit with an unrescued, untested, silent failure.

Minimum required cases include AccessStore locked/corrupt/newer/disk-full; unknown/ambiguous identity; unstable external-fact snapshot; stale/corrupt cache when caching exists; missing Project binding; epoch change between list and dispatch; credential refresh/cache-key failure; cross-store partial commit; audit sink failure; background follow revoke/offline; transfer timeout/redirect/ambiguous commit; and interrupted bootstrap.

## Existing code integration points

Current code already provides several useful seams and several prerequisites:

- crates/labby-auth/src/auth_context.rs: authenticated issuer/subject/scopes/session facts;
- crates/labby-runtime/src/artifacts/model.rs and artifacts/: ArtifactInterchange and an explicit-root store/lifecycle library; the product does not yet own a canonical ArtifactStore application handle;
- crates/labby-runtime/src/gateway_config.rs: GatewayLoadoutConfig and protected gateway subset configuration;
- crates/labby/src/mcp/route_scope.rs: current Loadout/capability narrowing for MCP routes;
- crates/labby-gateway/src/gateway/: current catalog, Loadout management, upstream dispatch;
- existing observability/error/testing docs and generated catalogs.

The implementation should extend these seams rather than adding a parallel gateway or Artifact store.

## Phase 0: Freeze design vectors and architecture tests

### Red

Add test fixtures/vectors that encode the core contract before implementation:

1. stable identity uses issuer + subject, not email;
2. default deny with no membership/grant;
3. direct and Group-based Project membership;
4. Group ancestor closure;
5. Organization -> Group -> Project Assignment inheritance;
6. explicit allowed override;
7. mandatory override/mask rejection;
8. personal overlay cannot broaden upstream/tool authority;
9. artifact.use does not imply artifact.sync/fork/export/reshare;
10. transport scope and domain permission intersection;
11. Role/Grant permissions are anchored to compatible membership/scope kinds;
12. Project A/B runtime binding isolation;
13. two concurrent sessions for one Principal cannot share mutable Project context;
14. Artifact publisher policy plus per-Assignment distribution policy is default-deny and can only narrow;
15. managed mirror retention never becomes Artifact ownership, while fork creates a new owner authority;
16. ArtifactInterchange fixture remains byte-identical.

Add architecture/layering tests that keep policy code out of UI/MCP handlers. Do not require a new crate until the extraction gate is met.

### Green

No product behavior yet. Land only test fixtures/scaffolding that can compile against placeholder domain structures if necessary.

### Exit

Normative vectors reviewed against SPEC.md, CONTRACT.md, PERMISSIONS.md, and THREAT_MODEL.md.

## Phase 1: Introduce the access domain boundary

Start private and surface-neutral under `crates/labby/src/access/`. Extract to `crates/labby-access` when a second concrete consumer or architecture/dependency test demonstrates the need.

### Red

Unit tests first for:

- opaque ID validation;
- Principal/Organization/Group/Project validation;
- same-Organization constraints;
- Group cycle rejection;
- ScopeRef/SubjectRef validation;
- Permission registry validation including allowed scope kinds/descendant reach;
- Role expansion anchored to Membership scope;
- positive Grant applicability;
- ArtifactAuthority and publisher distribution policy validation;
- Assignment target canonicalization plus Artifact assignment distribution ceilings;
- explicit slot override/mask rules;
- PersonalOverlayPolicy bounds;
- stable reason-code serialization.

Property tests SHOULD generate Group trees/invalid cycles and Assignment conflict combinations.

### Green

Implement pure domain models and validation with no network I/O and no secret ownership.

### Refactor

Keep public API small. Do not expose raw SQL rows as domain types. Add Rustdoc for every public contract type.

### Exit

cargo test -p labby-access and clippy -D warnings pass.

## Phase 2: AccessStore SQLite persistence and migrations

This phase belongs to Milestone 0 and MUST complete before any shadow/enforcement work. It includes the full security/durability profile, concrete tenant-integrity enforcement, single-snapshot set-based reads, fixed query-count tests, and storage failure matrix.

### Red

Migration/repository tests first for every DATA_MODEL.md invariant:

- foreign keys enabled;
- issuer+subject uniqueness;
- cross-Organization relations rejected;
- Group cycle mutation rollback;
- assignment relation validation;
- no generic Deny persistence field;
- exact Artifact revision required for Artifact assignments;
- one local owner authority per locally authoritative Artifact;
- managed mirrors do not acquire local ownership;
- publisher distribution defaults false and per-Assignment distribution can only narrow it;
- Role/Membership and Grant scope compatibility is rejected before persistence;
- policy_epoch increments atomically with mutations;
- failed mutations do not advance partial state;
- unknown/newer schema fails closed.

Add restart/durability tests and migration-from-empty tests.

### Green

Implement AccessStore and versioned migrations, beginning with one mutex-serialized SQLite connection. Do not introduce a multi-connection pool until contention evidence justifies it and snapshot/pragmas/failure behavior are re-proven.

Use transactions for policy mutation + epoch increment. Add indexes only after correctness queries exist, then verify query plans where useful.

### Exit

Persistence tests prove deterministic round-trip and rollback behavior.

### Current implementation status

The private AccessStore currently creates schema v2 directly for fresh stores and transactionally migrates only the exact canonical v1 manifest, preserving its global revision. Schema v2 metadata carries exact schema identity, global revision, bootstrap generation, and a safe bootstrap identity fingerprint. The crate-private explicit bootstrap operation creates the reserved local Organization, owner Principal and canonical identity link, default Project owner membership, and audit record in one immediate compare-and-set transaction. Repeated calls with the same input are idempotent; drift and non-pristine generation-zero business state fail closed. Integrity checks protect the reserved bootstrap records while allowing later legitimate rows and audit growth.

The store operation is now reachable through one explicit authenticated browser endpoint. It is not invoked by startup, setup, or doctor, and Loadout compatibility and transport enforcement have not landed. Those capabilities must not be inferred from the bootstrap implementation. The earlier idea that only `AppState` could own access reads is superseded: the AccessStore is the surface-neutral owner of its connection and transaction boundary, while application state may later hold a cloneable handle as runtime wiring requires.

## Phase 3: Identity mapping from current AuthContext

Do not map literal `AuthContext.issuer + sub`. First extend the authentication boundary to produce `VerifiedIdentity`, including canonical provider authority/subject or stable local credential ID. Browser sessions and Labby-issued bearer tokens for one human must resolve the same link; static and Unix credentials remain explicit service/bootstrap identities.

### Red

Tests first for:

- known issuer+subject -> exact Principal;
- same email/different subject does not inherit identity;
- disabled Principal denied;
- unknown mapping fails closed;
- service/static credential maps to explicit service Principal;
- browser session and bearer token for same external identity resolve same Principal;
- OAuth scope is preserved as a transport fact but does not create a domain Role.

### Green

Add the minimal adapter between labby-auth AuthContext and AccessStore identity mapping.

Do not key authorization on AuthContext.email.

### Exit

Authentication regressions remain green and no existing OAuth flow is reimplemented in labby-access.

### Wave 6: coherent access snapshot reads

The current implementation wave joins the completed `VerifiedIdentity` authentication fact to the Milestone 1 store subset. Project listing and explicit Project selection each use one AccessStore read transaction. Selection given a caller-supplied `VerifiedIdentity` and Project ID must:

1. resolve exactly one active `principal_links` row and active Principal;
2. resolve exactly one active Project in the same Organization;
3. resolve exactly one active direct Principal membership and its fixed Project role;
4. resolve exactly one Project Loadout mapping; and
5. return one immutable snapshot containing the store revision, Principal, Project, role, and Loadout name.

Listing returns only active same-Organization direct memberships and includes an optional persisted Loadout name so callers can distinguish discoverable Projects from selectable Projects; a valid Principal may receive an empty list. Identity resolution and explicit selection fail closed through typed redacted errors for missing or unusable required records. The implementation issues no per-result queries, and the revision plus all returned facts come from the same SQLite read transaction; fixed query-count instrumentation remains an enforcement-readiness gate. This wave is read-only: it does not install the snapshot into MCP/API/CLI runtime state, select gateway capabilities, authorize dispatch, or activate enforcement. Those runtime ownership and enforcement adapters are the next implementation boundary and remain unimplemented until their focused tests and repository gates are green.

### Wave 7: AccessRuntime lifecycle core

`AccessRuntime` is the process-scoped lifecycle authority for the AccessStore. Normal initialization is observational: missing or uninitialized state remains setup-required, unsafe or unusable state is typed blocked, and only a bootstrapped exact-current WAL store can become Ready. The Ready open path never creates or migrates the database, validates schema/integrity/bootstrap facts in one read transaction, and applies only connection-local operational pragmas. Explicit bootstrap is serialized in an owned task so request cancellation cannot commit authority without completing the in-memory Ready transition.

Wave 8 attaches exactly one runtime allocation, after the live-daemon bridge early return, to hosted AppState, standalone stdio, and every root or protected HTTP/Unix MCP handler. The authenticated owner-bootstrap endpoint uses this owner rather than reopening persistence per request. Delegated in-process built-in peers receive an explicit blocked non-authoritative runtime because policy decisions belong at the root boundary. Path-resolution failure remains a redacted blocked runtime while enforcement is disabled, preserving existing serve availability. This ownership wiring still does not bind a Project, filter discovery, authorize dispatch, or enable enforcement.

### Wave 9: audited Project Loadout compatibility assignment

The crate-private AccessStore mutation accepts an exact `VerifiedIdentity`, Project ID, and canonical Loadout name that the caller has already validated against desired gateway configuration. In one immediate transaction it re-resolves the active Principal, requires an active same-Organization Project and direct membership with `project.manage`, inserts the sole Project Loadout mapping, advances the global revision plus owning Organization and Project policy epochs, and writes redacted audit evidence. Exact replay returns `AlreadyApplied` without writes; a different existing mapping returns a conflict; inaccessible Projects are non-enumerating.

This wave intentionally does not read gateway state inside AccessStore. The next composition adapter must call `GatewayManager::loadout_get` before the mutation and must not create grants, route exposure, or transport actions as a side effect. Discovery filtering and dispatch authorization remain later gates.

### Wave 10: coherent Project permission snapshot

The crate-private AccessStore facade checks one of the four implemented fixed Project permissions while resolving the canonical identity, active same-Organization membership, fixed role, required Project Loadout mapping, and global revision in one deferred read transaction. It performs no writes or audit events. Missing, inactive, cross-Organization, unmapped, and insufficient-permission cases collapse to the same non-enumerating denial; malformed persisted vocabulary and storage failures remain distinct operator-facing causes.

The returned value is deliberately a project-level snapshot, not a capability or dispatch grant. It contains no exact gateway action/target, catalog generation, expiry, or consume-at-boundary mechanism. A revocation may commit after its SQLite snapshot. Transport enforcement therefore remains off: a later final-dispatch adapter must reauthorize the exact operation immediately before the in-process side-effect boundary and prove the revoke/check/dispatch race contract.

## Phase 4: Membership, role, and permission resolver

### Red

Pure resolver tests first for:

- direct Organization/Group/Project memberships;
- Group ancestor closure;
- Group-as-Project-member behavior;
- expired/inactive membership exclusion;
- scoped Role permissions;
- explicit Grant union;
- no cross-Project permission leakage;
- no cross-Organization permission leakage;
- deterministic ordering/evidence output.

### Green

Implement bounded iterative membership closure and effective permission resolution.

### Performance gate

Benchmark representative organizations with many users/groups/projects. Establish a baseline before caching.

### Exit

Resolver correctness is independent of transport and database ordering.

## Phase 5: Assignment composition and personal overlay

### Red

Tests first for:

- Organization/Group/Project inheritance;
- scope_only boundaries;
- explicit slot override;
- override forbidden without allow_override;
- mask forbidden without allow_mask;
- mandatory assignment immutability;
- same-name/no-slot collision does not override;
- conflicting exclusive slots fail deterministically;
- Project overlay disabled by default;
- allowed personal Skill/prompt addition;
- personal hidden-upstream injection rejected;
- personal runtime-binding injection rejected;
- overlay item/kind bounds.

### Green

Implement candidate collection, explicit relation application, and overlay filtering.

### Exit

Composition does not require gateway/network calls and emits stable evidence.

## Phase 6: EffectiveWorkspace and current Gateway catalog integration

### Red

Integration tests construct current Gateway/Loadout facts and verify:

- workspace can only narrow configured gateway exposure;
- Loadout expose_tools/resources/prompts/skills/code_mode gates remain authoritative;
- hidden upstream/service omitted;
- hidden individual capability omitted;
- same-name capabilities from different upstreams remain distinct;
- disappeared upstream capability becomes unavailable;
- policy epoch/catalog generation and exact Project context are included in workspace cache key;
- Artifact policy facts include owner authority plus publisher/Assignment distribution versions where relevant;
- Project A workspace cache is never reused for Project B;
- stale cache rejected after any relevant version changes.

### Green

Build ResolutionInput adapters from existing Artifact/Gateway/Loadout state and produce EffectiveWorkspace/compiled filter.

Prefer passing explicit snapshots/facts into labby-access. Do not add gateway network dependencies to the pure resolver.

### Exit

Existing single-user Loadout route-scope tests remain green.

## Phase 7: Cross-surface discovery and direct-invocation enforcement

Implement one surface at a time while preserving shared semantics.

Milestone 1 implements MCP only and splits this phase into three ordered slices: (1) server-owned BoundAccessContext creation/lifecycle across stateful/stateless/stdio/Unix/in-process paths; (2) discovery projection; (3) uncached final-boundary direct dispatch authorization. Code Mode, tasks, pagination, reconnect/resume, notifications, and MCP Apps are part of the MCP context slice. CLI/API/web parity is a later milestone.

### MCP and Code Mode Red

Tests first:

- tools/list, resources/list, prompts/list, Skills discovery omit forbidden entries;
- Code Mode search/catalog omits forbidden entries;
- direct tool/resource/prompt call by hidden name still fails;
- stale pre-revocation catalog cannot authorize call;
- route/loadout policy plus workspace policy is an intersection;
- two simultaneous MCP sessions for the same Principal can bind different Projects without catalog/context bleed;
- Code Mode inherits its owning session Project and cannot switch it through nested calls.

### CLI/API/web Red

Tests first:

- principal-scoped listing agrees with MCP result for same workspace;
- direct object/capability operations re-authorize;
- foreign Project/Organization IDs fail non-enumerating;
- command palette/search does not reveal hidden assets;
- errors map through existing error/output conventions.

### Green

Thread Principal/Project context and shared authorization calls through each adapter.

Do not duplicate permission matrices in frontend/transport code. UI capability hints are presentation only.

### Exit

Differential tests show equivalent policy results across transports.

## Phase 8: Project runtime bindings and credential isolation

### Red

Tests first for:

- Project A/B same upstream selects different binding IDs;
- missing Project context fails for Project-bound execution;
- missing runtime_binding.use or secret.use fails;
- same upstream name never falls back to another Project binding;
- EffectiveWorkspace serialization/logging contains no secret value;
- revoking Project membership prevents further binding selection.

### Green

Implement opaque RuntimeBinding selection metadata in AccessStore and integrate with the existing secret/upstream credential path at dispatch time.

### Exit

Adversarial cross-project credential test proves no bleed.

## Phase 9: Artifact distribution policy and managed local state

### Red

Implement the Required tests from ARTIFACT_DISTRIBUTION.md before network transfer:

- independent use/sync/follow/fork/export/reshare permissions;
- owner ArtifactAuthority is explicit and managed mirrors cannot claim it;
- publisher policy is a maximum ceiling and defaults distribution false;
- per-Assignment distribution policy can only narrow the publisher ceiling and defaults distribution false when absent;
- license/publication/takedown intersection;
- exact revision only;
- managed mirror vs personal fork state;
- follow reauthorization on every revision;
- revoked mirror state;
- Artifact digest/size validation;
- existing secret-safe export tests remain active.

### Green

Implement local distribution policy evaluation and managed mirror/subscription records without network federation first.

Use existing Artifact APIs for content validation/materialization/fork/export. Do not implement a new raw filesystem copy path.

### Exit

Local pin/follow/fork semantics are proven before remote pairing is added.

## Phase 10: Personal Labby destination pairing and transfer

### Red

Tests first for:

- explicit authenticated pairing;
- unpaired arbitrary URL rejected;
- destination ownership/status/capability checks;
- transfer authorization binds exact revision/mode/destination/policy version;
- replay to another revision/destination rejected;
- redirect/endpoint substitution does not bypass pairing;
- partial/failed transfer leaves no trusted local head;
- offline purge remains pending, not falsely successful.

Use local/mock test servers only in CI-safe tests. Live homelab smokes remain opt-in.

### Green

Add the smallest versioned transfer protocol/control envelope necessary. Keep reusable credentials in secret storage and use short-lived/single-operation authorization semantics.

### Exit

Source and destination both independently validate content and policy.

## Phase 11: Loadout dependency graph and Add Loadout to My Labby

### Red

Tests first for:

- required inaccessible dependency fails;
- optional dependency omitted only when explicitly allowed;
- remote-only dependency remains remote;
- local mirror/fork dependencies preserve ownership/source distinction;
- dependency cycle and graph-size bounds;
- a Loadout cannot grant a dependency permission;
- Project overlay still intersects the resolved personal Loadout.

### Green

Add explicit dependency resolution output and UI/API transfer plan.

### Exit

A mixed local/remote Loadout has deterministic resolution evidence.

## Phase 12: Audit and explanation

### Red

Tests first for:

- policy mutations produce audit evidence;
- allow/deny use/distribution events produce safe reason codes;
- ordinary denial does not enumerate hidden target existence;
- policy.explain reveals only authorized scoped evidence;
- audit.read boundaries;
- token/secret/runtime binding values never appear;
- bounded retention/evidence payload limits.

### Green

Implement append-only audit/explanation storage/adapters using existing observability redaction conventions.

### Exit

THREAT_MODEL explanation/audit cases pass.

## Phase 13: Single-user migration and rollout

The minimal owner bootstrap, identity linking, compatibility projection, setup/doctor recovery, and idempotent restart behavior move to Milestone 0 immediately after persistence. This later phase retains only final staged rollout/default-on validation.

### Red

Migration tests first from representative current configs:

- existing private Artifacts remain private;
- current owner retains equivalent local gateway behavior;
- current Loadouts still resolve;
- no new public/organization-wide grant appears implicitly;
- ambiguous external identity mapping fails with actionable setup state;
- rollback/restart safe;
- disabled access-control flag leaves current behavior intact during staged rollout.

### Green

The persistence portion creates the explicit local owner Principal/Organization, canonical identity link, default Project owner membership, and audit event. Fresh schema v2 creation and exact canonical v1 migration are implemented. The explicit operator workflow is `POST /v1/access/bootstrap-owner`, mounted only with OAuth browser state and restricted to a CSRF-validated session whose middleware-derived `VerifiedIdentity`, `lab:admin` scope, and configured admin email all agree. It returns only `created` or `already_applied`; failures use the canonical agent error envelope; no CLI/MCP/stdio/bearer or loopback bypass exists. Without OAuth the route is absent and returns `404` before body validation. Retain the following work in this phase: preserve current Loadout behavior without implicit broad grants and stage enforcement only after health checks and identity verification.

Add doctor/setup checks for:

- identity mapping health;
- policy DB/migration health;
- orphan/cross-tenant references;
- stale/broken Artifact assignments;
- runtime binding ambiguity;
- destination pairing health.

### Rollout

1. disabled/bootstrap-only;
2. shadow resolver;
3. opt-in discovery filtering;
4. opt-in direct enforcement;
5. runtime binding enforcement;
6. distribution/pairing;
7. default-on after migration and adversarial review.

## Phase 14: Performance and scale hardening

Benchmarks should measure:

- identity lookup;
- Group closure;
- permission resolution;
- Assignment composition;
- full EffectiveWorkspace cold resolve;
- warm cache resolve;
- cache invalidation after membership/Grant mutation;
- filtered gateway catalog construction;
- large Loadout dependency plan;
- audit write overhead.

Test realistic and adversarial sizes. Define explicit bounds rather than accepting unbounded graphs.

Optimize with indexes, compact snapshots, and version-keyed caches only after correctness baselines exist.

## Phase 15: Documentation and generated surfaces

Before implementation is called complete, update all affected canonical docs, including at minimum:

- docs/README.md;
- docs/ARCH.md;
- docs/guides/SKILLS_AND_LOADOUTS.md;
- docs/runtime/OAUTH.md for authentication-vs-domain-authorization boundary;
- docs/runtime/CONFIG.md and ENV.md for any new configuration;
- docs/services/GATEWAY.md;
- docs/surfaces/MCP.md;
- docs/surfaces/CLI.md;
- API/OpenAPI docs for new access/project/artifact-transfer endpoints;
- docs/dev/DISPATCH.md if shared operation boundaries change;
- docs/dev/OBSERVABILITY.md for audit fields/redaction;
- docs/dev/TESTING.md for authorization/adversarial gates;
- docs/artifacts/spec.md and contract.md only for cross-links/clarification, never by changing ArtifactInterchange v1 without a separate contract version;
- public Rustdoc for labby-access and changed integration APIs;
- generated action/service/route/help/catalog references when metadata/actions change.

Run just docs-generate when code-owned metadata changes and just docs-check before merge.

## Required final verification

At production-readiness stage run:

- formatter;
- changed-crate clippy with warnings denied;
- labby-access unit/property/persistence tests;
- labby-auth identity integration tests;
- gateway/Loadout/MCP/Code Mode targeted tests;
- ArtifactInterchange conformance tests;
- Artifact import/export/security tests;
- API/CLI differential authorization tests;
- migration tests;
- threat-model adversarial suite;
- relevant full workspace tests;
- rustdoc correctness/audit gates;
- docs generation/check;
- repository architecture/layering gates.

Then conduct an explicit adversarial code review and record every surfaced issue/resolution in PROGRESS.md before default-on rollout.
