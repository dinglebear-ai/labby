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

This wave intentionally does not read gateway state inside AccessStore. The Wave 11 composition adapter calls `GatewayManager::loadout_get` before the mutation and does not create grants, route exposure, or transport actions as a side effect. Discovery filtering and dispatch authorization remain later gates.

### Wave 10: coherent Project permission snapshot

The crate-private AccessStore facade checks one of the four implemented fixed Project permissions while resolving the canonical identity, active same-Organization membership, fixed role, required Project Loadout mapping, and global revision in one deferred read transaction. It performs no writes or audit events. Missing, inactive, cross-Organization, unmapped, and insufficient-permission cases collapse to the same non-enumerating denial; malformed persisted vocabulary and storage failures remain distinct operator-facing causes.

The returned value is deliberately a project-level snapshot, not a capability or dispatch grant. It contains no exact gateway action/target, catalog generation, expiry, or consume-at-boundary mechanism. A revocation may commit after its SQLite snapshot. Transport enforcement therefore remains off: a later final-dispatch adapter must reauthorize the exact operation immediately before the in-process side-effect boundary and prove the revoke/check/dispatch race contract.

### Wave 11: desired Gateway Loadout admission

The gateway-feature-only, crate-private adapter first authorizes the opaque Project selector for `project.manage`, then validates the canonical Loadout name against desired configuration with `GatewayManager::loadout_get`, and finally invokes the immediate AccessStore mutation, which reauthorizes before writing. This ordering prevents unauthorized Loadout-name probing and closes the permission-revocation window between validation and mutation. Gateway and persistence failures map to bounded redacted composition errors. The adapter is not registered on any transport.

This is point-in-time admission, not cross-store referential integrity. Gateway configuration and AccessStore cannot commit atomically, and a concurrent or later Loadout removal can invalidate the stored symbolic name. Every use-time composition must resolve the name from the coherently published runtime configuration and fail closed. A repair/replacement workflow or immutable Loadout revision identity is required before claiming a durable existence invariant.

### Wave 12: published runtime Loadout snapshot prerequisite

Gateway now owns a coherent runtime-configuration Loadout snapshot paired with an opaque, process-local monotonic publication generation. The snapshot reads the published configuration and generation under the same publication barrier, ignores restart-bound desired configuration, and distinguishes Loadout ABA publications. This is deliberately narrower than a complete gateway catalog generation: pool/catalog-only mutations remain a later boundary. Access composition and transport enforcement remain unimplemented.

### Wave 13: stable Project runtime Loadout context

The gateway-feature-only, crate-private composer brackets two uncached AccessStore Project-permission snapshots around two coherently published runtime Loadout snapshots. It accepts only when the complete Access facts, Access revision, published Loadout, and runtime-configuration generation are identical; mismatches retry at most three times and then fail with a bounded redacted instability error. Authorization precedes every Gateway lookup, ordinary denials remain non-enumerating, and missing published Loadouts fail closed. The returned non-cloneable context is observational evidence only: it is unmounted, binds no exact action or target, and is not a complete catalog generation or dispatch grant.

### Wave 14: published upstream tool catalog prerequisite

`UpstreamPool` now publishes one immutable snapshot of its routable, exposure-filtered tool routes with an opaque process-local generation. The projection and generation are observed coherently under the pool catalog lock, clones share one publication stream, distinct pools cannot collide, and add/remove/identical/ABA transitions retain distinct publication semantics. Projection is bounded and fails closed when route limits or tool invariants are violated.

This prerequisite covers only upstream tools and their routability. It excludes resources, prompts, skills, subject-scoped OAuth catalogs, the built-in service registry, and any claim of a complete gateway catalog generation. It is not composed into the Project runtime context or mounted on discovery, transport, or dispatch enforcement.

### Wave 15 / Milestone 0S: published runtime-pool identity prerequisite

`GatewayRuntimeHandle` now publishes the current optional `UpstreamPool` together with an opaque process-local publication identity in one immutable state. Every swap advances the identity, including publication of the identical pool, repeated `None`, and A-B-A pool transitions. Cloned handles share one publication stream, while separately constructed handles receive distinct identities; readers therefore observe a coherent old or new pool/identity pair rather than a torn combination.

This is a prerequisite for later manager composition of the published runtime Loadout configuration with the selected pool's tool-catalog snapshot. It does not perform that composition, produce a complete catalog generation, bind a Project or exact action, or enable discovery, transport, or dispatch enforcement.

### Wave 16 / Milestone 0T: published Loadout-filtered runtime tool catalog

`GatewayManager` now composes a successful runtime Loadout observation, runtime-pool publication, and bounded upstream tool-catalog publication with a three-attempt G-C-G-C protocol. A returned snapshot binds all three opaque generations and includes only exposure-filtered, routable routes whose exact upstream appears in the runtime Loadout; `expose_tools = false` and an empty upstream set produce an empty projection. Manager, pool, or catalog churn retries and sustained churn fails closed. Missing and invalid publication states remain bounded and redacted.

The snapshot is immutable observational input, not a dispatch grant. It remains unmounted and excludes built-in services, subject-scoped OAuth catalogs, resources, prompts, skills, Code Mode, protected routes, exact actions/targets, and enforcement. Those omitted authority domains still require their own coherent publication identities and final-boundary reauthorization.

### Wave 17 / Milestone 0U: published built-in service registry catalog

`GatewayManager` now atomically publishes the immutable built-in service/action projection, the exact registry object, and an opaque process-local generation. Every initialization and replacement is total-ordered and receives a fresh identity, including identical and A-B-A publications. The owned projection is deterministic, bounded, includes exact `destructive` and `requires_admin` action metadata, and fails closed on duplicate or inconsistent registry vocabulary. Previously returned snapshots remain immutable across replacement.

This publication proves registered service/action metadata only. Replacing the registry does not rebuild the current upstream pool or prove that an in-process peer is routable. It is unmounted observational input, not a service dispatch grant, and is not yet composed with runtime Loadouts, virtual-server policy, protected routes, OAuth scopes, or final-boundary authorization.

### Wave 18 / Milestone 0V: published Loadout-filtered built-in MCP service catalog

`GatewayManager` now composes one running Loadout and its virtual-server MCP policy with one exact immutable built-in service-registry publication through a three-attempt G-S-G-S observation. A successful snapshot binds the runtime-configuration and service-registry generations, selects exact Loadout service names or configured virtual-server ID aliases, requires an enabled MCP surface, applies `expose_tools`, and preserves deterministic service/action ordering plus exact action metadata. Non-empty action allowlists narrow the projection while the generic MCP `help` and `schema` compatibility actions remain available. Missing, disabled, ambiguous-alias, invalid-catalog, and sustained config or registry churn states fail closed with bounded redacted errors; identical and A-B-A registry replacement cannot be mistaken for stability.

The immutable result is unmounted observational input only. It does not rebuild or prove the routability of an in-process peer, authorize a service action, mount discovery or transport, bind a Project or protected route, or include upstream tools, subject-scoped OAuth catalogs, resources, prompts, skills, Code Mode, or a complete catalog generation. Wave 19 next composes this projection with the separately generation-bound upstream-tool projection over one common manager publication interval; Project composition, exact-action final-boundary reauthorization, and the omitted catalog families remain later prerequisites.

### Wave 19 / Milestone 0W: unified published Loadout MCP catalog

`GatewayManager` now publishes the Loadout-filtered upstream-tool, regular Resource, and built-in-service projections over one bounded G-T-R-S-G-T-R-S observation interval. The immutable result nests the existing tool, Resource, and service snapshots, binding one runtime-configuration generation plus the exact pool-publication, tool-catalog, Resource-catalog, and service-registry generations without duplicating filtering policy. Config, pool, tool, Resource, and registry churn retry; identical and A-B-A publications cannot be mistaken for stability; sustained churn and missing or invalid catalogs fail closed through redacted errors.

This unified snapshot is still observational and unmounted. It does not bind a Project, Principal, protected route, OAuth subject, session, exact dispatch action, or final-boundary authorization; it excludes Resource templates, OAuth/UI/local/synthetic Resource families, prompts, skills, Code Mode, MCP Apps, tasks, and pagination. The next boundary is application-layer composition with the stable Project runtime context, followed later by server-owned request/session binding and uncached exact-action reauthorization.

### Wave 20 / Milestone 0X: stable Project runtime MCP catalog context

The gateway-feature-only access composer now performs bounded authorization-first A-M-A-M observation across the AccessStore and Wave 19's unified manager snapshot. It returns only when the complete Project permission snapshots match and all runtime-config, pool, tool-catalog, Resource-catalog, and service-registry publication identities match. Ordinary Project denial reaches no manager lookup; a second authorization denial prevents the second lookup; stable manager failures collapse to one non-enumerating catalog-unavailable result. Three outer attempts cap work at six Access reads and six manager invocations, whose internal publication protocol is independently bounded.

The crate-private, non-cloneable result owns the existing permission snapshot and unified catalog snapshot without copying authority facts. It remains observational and unmounted: no session `BoundAccessContext`, protected-route intersection, discovery filtering, exact action authorization, transport enforcement, caching, or final-boundary revocation check is enabled.

### Wave 21 / Milestone 0Y: Project-bound protected-route publication

`GatewayManager` now publishes an immutable protected-route snapshot from one exact runtime-configuration generation. The publication requires one enabled, uniquely named explicit gateway-subset route, one canonical public resource identity, an exact canonical Project binding, and one uniquely named Access-assigned Loadout from that same configuration. Named route Loadouts must match the assignment; inline route policy may only intersect and narrow the assigned Loadout. A shared runtime intersection helper also drives the mounted MCP route scope so the two policy projections cannot drift.

The bounded observation projects only the selected route and Loadout facts while holding the coherent configuration read. Identical and A-B-A publications advance generation identity, sustained churn fails closed, and missing routes, disabled or ambiguous identities, wrong Project or Loadout bindings, and invalid intersections all collapse to one non-enumerating unavailable error. The optional serialized `project_id` preserves legacy configuration compatibility, but this new Project-bound publication requires it; both host and gateway normalizers trim it and enforce the Access Project-ID bound.

The snapshot remains unmounted, non-cloneable observational evidence. It does not create a server-owned `BoundAccessContext`, filter discovery, authorize an exact action, mount transport, add caching, or perform the final-boundary revocation check. The next dependency is composing this route publication with Wave 20 before binding it to request/session lifecycle.

### Wave 22 / Milestone 0Z: BoundAccessContext core

The crate-private MCP lifecycle kernel now composes Wave 20 Project catalog evidence with Wave 21 protected-route evidence through a bounded C-R-C-R observation. It accepts only identical complete child publications with the same runtime-configuration generation, Project, assigned Loadout, and server-derived canonical route identity. Construction always requests `asset.discover`; stable semantic mismatches fail unavailable while transient cross-publication mismatches retry and sustained churn fails unstable. The result owns both immutable snapshots, a checked process-unique opaque ID, and redacted fingerprints over the binding plus the complete verified identity facts.

This core remains unmounted by itself, not a bearer capability or dispatch grant. MCP params and `_meta` cannot construct it. The protected HTTP shadow mount in Wave 23 wraps it with JWT-instance expiry and binding; reconnect/resume, stateful session validation, and browser-session identity remain deferred. `LabMcpServer` has no field for this context.

### Wave 23 / Milestone 0AA: protected HTTP ownership shadow

Explicit Project-bound gateway-subset routes now derive `VerifiedIdentity` from validated access-token provenance, validate a bounded nonempty token ID and exact expiry before Access/Gateway reads, and attach one request-owned transport binding through HTTP Parts before the scoped stateless MCP service runs. The credential-instance fingerprint is domain-separated and redacted over issuer plus JWT ID; the wrapper owns the Wave 22 core evidence and rejects the exact expiry boundary.

This is ownership-only shadow rollout. Successful binding attaches `Bound`; authorization/catalog failure attaches explicit `Unavailable` and current dispatch continues. Invalid or expired credentials still reject before dispatch. Legacy protected routes without `project_id`, root MCP, stdio and its live bridge, Unix-only identity, and in-process peers remain unchanged and receive no Project observation. No discovery or dispatch enforcement consumes the observation yet, and no `LabMcpServer` field or client-controlled Project selector is introduced.

### Wave 24 / Milestone 0AB: tools/list Project discovery shadow

`tools/list` now consumes the request-owned Wave 23 observation without enforcing it. A live `Bound` observation compares provenance-known built-in services and non-OAuth upstream tool pairs against both the immutable Project catalog and the same-generation protected-route narrowing snapshot. Legacy absence, explicit binding unavailability, and expiry remain distinct shadow states; expiry is rechecked while classifying and immediately before telemetry.

This wave emits only state and aggregate counts. It does not log Project, route, service, upstream, tool, token, JTI, or binding fingerprints. It does not mutate descriptors, pagination, contract hashes, baselines, notifications, Code Mode, OAuth-subject catalogs, MCP Apps, resources, prompts, Skills, or dispatch. Bound differences and unavailable observations therefore preserve the exact current `ListToolsResult`; enforcement remains deferred until direct-invocation authorization and all discovery families share the same proven boundary.

### Wave 25 / Milestone 0AC: built-in action-resource discovery shadow

`resources/list` now classifies only exact canonical `lab://<service>/actions` built-in resources against the request-owned Project binding and its same-generation protected-route effective Loadout. Resource exposure and canonical service membership come from that immutable route publication rather than a potentially older handler route scope. Expiry is rechecked per candidate and immediately before aggregate telemetry; legacy, unavailable, and bound states remain distinct.

This wave is discovery-only and non-enforcing. Live and revision-bound continuation results, cursors, snapshots, revisions, notifications, and resource bytes remain unchanged. Telemetry contains only state and aggregate checked/would-suppress counts. Templates, reads, regular and OAuth upstream resources, MCP Apps, Code Mode, Skills, contracts, gateway synthetic resources, and every direct-dispatch boundary remain unclassified. A generation-bearing regular-upstream resource publication is deferred until connection/catalog incarnation can prevent stale asynchronous list results from being attributed across replacement or ABA.

### Wave 26 / Milestone 0AD: connection/catalog incarnation kernel

The generic upstream pool now assigns an opaque checked incarnation on every structural connection install and records the same identity beside its catalog entry. One stable pool-wide mutex linearizes binding mutations in the audited `binding → connections → catalog` lock order; all guards are acquired before mutation, so cancellation cannot publish half a pair. Observe/apply helpers carry the identity across an asynchronous gap and reject removal, replacement, and A-B-same-connection-A ABA while unrelated catalog mutations do not invalidate the binding. Production discovery, lazy ensure, reprobe, reconciliation/drain, OAuth invalidation, and in-process registration structural paths use the coordinator.

This is an unmounted identity prerequisite only. Existing resources, prompts, Skills, notification, and other asynchronous capability-result consumers do not yet use the observe/apply helpers, so this wave alone does not close stale result attribution and publishes no resource catalog. The next capability-publication wave must acquire an observed peer, perform the RPC without locks, and route every success/failure health, cache, and subscription mutation through checked apply before treating the result as current.

### Wave 27 / Milestone 0AE: incarnation-bound regular resources/list attribution

Regular non-OAuth `resources/list` fanout now acquires routable peers together with their catalog-entry incarnation, performs each RPC without locks, and applies success or failure only if that exact connection/catalog pair remains current. One checked apply owns Resources health and circuit state, last error, unfiltered URI cache, count, and the exposure policy used for the wire result. Stale or missing observations contribute no rows and cannot mutate replacement state. Subscription refresh triggers carry the observation through a final current check; the queued operation intentionally reconciles only the current named connection and carries no result-derived resource facts.

This is still not a resource publication or Project enforcement wave. OAuth/subject-scoped resources, templates, reads, UI and synthetic resources, prompts, Skills, and notifications keep their existing paths. Regular resource wire rewriting, filtering, caps, partial-failure behavior, and ordering are unchanged. A later milestone must build the bounded immutable resource publication before Project discovery can classify regular upstream resources.

### Wave 28 / Milestone 0AF: immutable regular-upstream resource publication

The pool now retains bounded, incarnation-tagged regular non-OAuth `resources/list` source facts inside `CatalogState` and reprojects them atomically whenever the existing `CatalogWriteGuard` closes. The independent resource publication requires the current connection/catalog identity, Resources routability and proxy policy, and the current exposure allowlist. Routes preserve exact upstream/native-URI/Resource provenance, sort deterministically, reject within-owner duplicates and invalid structure, and fail closed at checked row, aggregate retained-byte, published-byte, and route bounds. Every semantic transition, including A-B-A identity replacement, receives a process-unique generation while unchanged facts retain their Arc and generation; older snapshots remain immutable.

This snapshot is observational and unmounted. It is not read authority, authorization evidence, or a grant, and excludes UI resources, templates, OAuth/subject-scoped, synthetic/local, Skills, and application families. Manager Loadout projection, Project composition, handler shadow consumption, reads, and enforcement remain later milestones.

### Wave 29 / Milestone 0AG: Loadout-filtered regular Resource publication

`GatewayManager` now composes one immutable regular Resource view across a bounded G-R-G-R observation of the active runtime configuration, exact pool publication, and pool resource-catalog generation. It filters only exact Loadout upstream membership when `expose_resources` is enabled, preserves deterministic Resource metadata and provenance, retries pool/config/resource ABA, and returns redacted stable missing/catalog errors or bounded `Unstable` under sustained churn. Staged desired configuration is excluded.

The snapshot remains unmounted observation, not read authority, authorization evidence, or a dispatch grant. Project composition, route intersection, handler shadow consumption, OAuth/templates/UI/local/synthetic families, reads, and enforcement remain deferred.

### Wave 30 / Milestone 0AH: unified Project MCP catalog ownership includes Resources

The unified manager snapshot now reads tool and regular Resource publications from the same exact pool plus the built-in service registry inside one bounded G-T-R-S-G-T-R-S common interval. It nests the existing Loadout-filtered tool, Resource, and service children, compares the Resource generation in publication equality, and preserves the shared runtime/pool invariants without copying filter policy. Resource-only ABA retries and sustained Resource churn fails boundedly.

The existing authorization-first A-M-A-M Project composer and C-R-C-R BoundAccessContext kernel inherit the Resource child solely through nested ownership, getters, and publication equality. Denial still reaches no manager, old contexts remain immutable, and no discovery handler, read path, transport, or enforcement boundary consumes the Resource child yet.

### Wave 31 / Milestone 0AI: Project-bound regular Resource discovery shadow

`resources/list` classifies regular non-OAuth upstream Resources by exact `(upstream, native_uri)` provenance retained beside the pagination snapshot. Classification intersects the unified Project Resource publication with the protected route's effective `expose_resources` and upstream narrowing. Cursor pages require the same unexpired, non-sensitive Project/credential/publication snapshot key before replaying aggregate classification; mismatch is reported only as unavailable/zero telemetry. Wire resources, ordering, cursors, revisions, notifications, and legacy behavior remain unchanged. This is shadow observation only: Resources read, templates, OAuth, UI, synthetic/local/Skills/app families, and enforcement remain deferred.

### Wave 32 / Milestone 0AJ: incarnation-bound regular ResourceTemplate listing

Regular non-OAuth `resources/templates/list` fanout now acquires the exact routable connection/catalog incarnation before each RPC and applies Resources health success, unsupported, or failure only through the checked current-incarnation gate. Delayed A results after replacement or same-object A-B-A reinstall contribute no wire templates and cannot mutate the replacement's shared Resources health/error state. Existing pagination, ordering, namespace rewriting, partial-failure behavior, allowlists, and output remain unchanged.

This is an attribution prerequisite only. It adds no template cache, generation-bearing publication, Project catalog child, handler shadow, read authority, or enforcement. OAuth/subject-scoped, UI/local/synthetic/Skills/app families and `resources/read` remain deferred.

### Wave 33 / Milestone 0AK: immutable regular ResourceTemplate pool publication

The pool now publishes a bounded immutable catalog of regular non-OAuth upstream ResourceTemplates from only current-incarnation checked template-list results. The independent generation changes for template content, connection incarnation, Resources routability transitions, proxy enablement, removal, and ABA, while unrelated tool and Resource publications remain stable. Exact native URI-template provenance and metadata are retained; UI, OAuth, local, synthetic, Skills, and app families are excluded.

This pool snapshot is observational and unmounted. It deliberately follows existing template wire semantics: concrete Resource exposure allowlists do not filter URI patterns. It is neither read authority nor a grant, and manager/Project composition, discovery shadowing, reads, and enforcement remain deferred.

### Wave 34 / Milestone 0AL: immutable Loadout ResourceTemplate publication

`GatewayManager` composes one immutable regular ResourceTemplate view across a bounded G-Q-G-Q observation of the active runtime configuration, exact pool publication, and pool template-catalog generation. It filters exact Loadout upstream membership and the Loadout-level `expose_resources` capability while deliberately not applying per-upstream concrete-URI exposure allowlists to URI patterns. Staged desired configuration is excluded; config, pool, and template ABA retry boundedly.

The snapshot is observational and unmounted, not read authority or a grant. Unified MCP catalog ownership, Project/Bound propagation, discovery shadowing, reads, and enforcement remain deferred to later waves.

### Wave 35 / Milestone 0AM: unified Project MCP catalog ownership includes ResourceTemplates

The unified manager snapshot reads tool, regular Resource, and regular ResourceTemplate publications from the same exact pool plus the built-in service registry inside one bounded G-T-R-Q-S-G-T-R-Q-S common interval. It nests all four Loadout-filtered catalog children and compares the ResourceTemplate generation in publication equality, so template-only ABA and sustained churn cannot produce a torn Project context.

The existing authorization-first Project composer and BoundAccessContext kernel inherit the template child mechanically through nested ownership and equality. Cursor shadow identity also carries the template generation, preventing a cached Project snapshot from crossing a template-only publication change. No discovery handler, read path, transport authorization, or enforcement boundary consumes templates yet.

### Wave 36 / Milestone 0AN: Project-bound regular ResourceTemplate discovery shadow

`resources/templates/list` retains exact `(upstream, native_uri_template)` provenance for regular non-OAuth rows and classifies it against the Bound unified template publication plus the protected route's `expose_resources` and upstream narrowing. Continuation snapshots retain provenance and the Q-aware stable Project key; mismatch or expiry reports unavailable/zero shadow telemetry without refanout. UI templates are excluded case-insensitively from classification.

Wire templates, metadata, ordering, pagination revisions/cursors, notifications, and legacy behavior remain unchanged. This is shadow observation only: no filtering, template expansion/read authority, OAuth/UI/local/synthetic classification, or enforcement is added.

### Wave 37 / Milestone 0AO: incarnation-bound regular prompt listing

Regular non-OAuth `prompts/list` fanout acquires the exact routable connection/catalog incarnation before each RPC. Success, unsupported capability, and failure update prompt health, errors, counts, and post-merge ownership names only through checked current-incarnation applies. Accepted rows use the exposure policy captured by that same checked result, so connection replacement or same-object A-B-A cannot reinterpret or attribute stale prompt data.

Delayed stale results contribute no wire prompts and cannot mutate replacement state. Existing pagination, deadlines, deterministic namespacing/collision behavior, exposure filtering, partial results, and output remain unchanged. This is an attribution prerequisite only: no prompt publication, manager/Project catalog ownership, discovery shadow, prompt execution authority, OAuth/subject-scoped changes, or enforcement is added.

### Wave 38 / Milestone 0AP: immutable regular Prompt pool publication

`CatalogState` owns incarnation-tagged raw Prompt source facts populated only by Wave37's checked regular non-OAuth `prompts/list` results. It publishes an independent immutable catalog with exact `(upstream, native_name, Prompt)` provenance and metadata, deterministic ordering, existing `expose_prompts` policy, and process-unique generation identity. Unsupported capability publishes an empty source; current failure removes it; stale results cannot mutate either source or publication.

Invalid/duplicate names and route, row-byte, retained-byte, or published-byte overflow fail closed without retaining oversized payloads. Bind/rebind, removal, Prompt routability, policy, and semantic source changes reproject atomically, while diagnostic prompt count/name hints and unrelated tool/Resource/ResourceTemplate changes preserve the Prompt Arc and generation. This pool-only snapshot is observational, not `prompts/get` authority or a grant. Manager/Project composition, discovery shadow, OAuth/subject/local/synthetic families, execution, and enforcement remain deferred.

### Wave 39 / Milestone 0AQ: immutable Loadout Prompt publication

`GatewayManager` composes one standalone immutable Prompt view through a bounded G-P-G-P observation of the active runtime Loadout, exact pool publication, and pool Prompt-catalog generation. It filters only the Loadout's exact upstream membership and Loadout-level `expose_prompts`; the pool publication already applies each upstream's Prompt exposure policy. Staged desired configuration is excluded, and independently decisive Prompt, pool, and config ABA retry while sustained churn fails boundedly.

The snapshot preserves exact native Prompt provenance, metadata, and deterministic order. Its typed failures are stable and redacted. It is manager-only, observational, and unmounted—not `prompts/get` authority or a grant. Unified MCP catalog ownership, Project/Bound propagation, discovery shadowing, OAuth/subject/local/synthetic prompt families, execution, and enforcement remain deferred.

### Wave 40 / Milestone 0AR: unified Project MCP catalog ownership includes Prompts

The unified manager snapshot now reads tool, regular Resource, regular ResourceTemplate, and regular Prompt publications from the same exact pool plus the built-in service registry inside one bounded G-T-R-Q-P-S-G-T-R-Q-P-S common interval. It nests all five Loadout-filtered catalog children and compares the Prompt generation in publication equality, so prompt-only ABA, stable publication failure, and sustained churn cannot produce a torn Project context.

The existing authorization-first Project composer and BoundAccessContext kernel inherit the Prompt child mechanically through nested ownership and equality. Cursor shadow identity carries the Prompt generation, preventing a cached Project snapshot from crossing a prompt-only publication change. Prompt discovery shadowing, `prompts/get`, OAuth/subject/local/synthetic prompt families, execution authority, and enforcement remain deferred.

### Wave 41 / Milestone 0AS: Project-bound regular Prompt discovery shadow

`prompts/list` retains exact `(upstream, native_name)` provenance for regular non-OAuth rows before gateway namespacing and classifies it against the Bound unified Prompt publication plus the protected route's `expose_prompts` and exact upstream narrowing. Continuation snapshots retain provenance and the Prompt-aware stable Project key; mismatch or expiry reports unavailable/zero aggregate telemetry without refanout.

Wire Prompts, metadata, ordering, pagination revisions/cursors, notifications, and legacy behavior remain unchanged. This is shadow observation only: built-in, OAuth/subject/local/synthetic Prompts remain unclassified, and `prompts/get`, execution authority, filtering, and enforcement are deferred.

### Wave 42 / Milestone 0AT: incarnation-bound exact regular Prompt call kernel

The pool owns a separate, unmounted regular non-OAuth Prompt call kernel that requires the exact current Prompt publication generation and `(upstream, native_name)` route. Acquisition atomically validates the current connection/catalog incarnation, Prompt routability, and compiled exposure policy; every RPC outcome revalidates the same generation, route, and incarnation before returning or changing health. Queue saturation remains local and does not affect upstream health.

This prerequisite does not alter the live `prompts/get` handler, owner resolution, relay, OAuth/subject-scoped behavior, Project authorization, wire errors, execution grants, telemetry/usage accounting, or enforcement. A later mount must bind manager pool identity and current Project/route/access authorization in addition to this pool-local evidence, and the mounted handler remains the single accounting owner.

### Wave 43 / Milestone 0AU: AssetUse-bound exact regular Prompt execution seam

The product owns one unmounted, crate-private resolver/executor for regular non-OAuth Prompts. It derives fresh `AssetUse` authorization and coherent Project route/Loadout/catalog evidence before dispatch, resolves exactly one canonical `upstream/native` Prompt route, rewrites only the outbound upstream name to the native name, and rechecks the same authorization/publication interval before exposing the result. The manager brackets final Prompt outcome attribution with its pool-publication lease, so pool replacement or ABA cannot apply stale health or return stale output.

Authorization is checked when the upstream request starts and again before its result is exposed; it is not a revocation lease and cannot unsend a request already in flight. Pool and Prompt generations remain observations rather than capabilities. This unmounted seam does not bind or validate a protected transport token instance or its expiry. A future handler mount must supply only middleware-derived `VerifiedIdentity` and protected-route facts and must validate the current request's `TransportBoundAccessContext` token instance and expiry before calling this resolver and again before exposing its result. Built-ins, OAuth/subject-scoped Prompts, capability relay, live `prompts/get` mounting, wire-error mapping, and telemetry/usage accounting remain deferred.

### Wave 44 / Milestone 0AV: Project-bound exact regular `prompts/get` mount

The live `prompts/get` handler now consumes Project access observation before every legacy route-scope, builtin, owner-cache, relay, or OAuth decision. A missing observation remains the byte-compatible legacy dispatch tree. Explicitly unavailable Project evidence terminates with a generic existing-shaped error. Bound evidence requires the middleware-owned `VerifiedIdentity` to match the immutable transport binding and requires token expiry validation before exact resolution and again before any result or error is exposed.

The Bound branch supports only the exact regular non-OAuth Prompt route published by its coherent Project/Loadout catalog. It always uses the bounded pooled exact kernel, even when request metadata advertises relay capabilities. Unknown or ambiguous names, builtins, OAuth/subject-scoped targets, and relay execution are terminal in this branch and never fall through. Legacy builtin, regular pooled/relay, OAuth, and local behavior remains unchanged. This is request-start authorization rather than a revocation lease; cancellation and expiry can discard a result but cannot unsend an upstream request already in flight. Resource reads, Prompt completion, other capability families, and broader execution enforcement remain deferred.

### Wave 45 / Milestone 0AW: incarnation-bound exact regular Resource read kernel

Add an unmounted pool-local kernel for regular non-OAuth Resource reads. It acquires the per-upstream permit before observing the target, uses one queue-plus-RPC deadline, and requires the exact current Resource publication generation/route plus connection/catalog incarnation before dispatch and before applying any result or health change. The immutable Resource publication is the authority for routability, proxying, exposure, non-UI classification, and source incarnation; the legacy `resource_upstreams` list is deliberately not consulted. Every returned text/blob URI is overwritten with the canonical gateway URI before the normalized response-size cap is enforced. Typed errors are redacted. Handler/manager/AssetUse mounting, OAuth, relay, UI/local/synthetic/Skills, and ResourceTemplate reads remain deferred.

### Wave 46 / Milestone 0AX: AssetUse-bound exact regular Resource read seam

Add an unmounted server-owned resolver/executor for regular non-OAuth Resources. A fresh `AssetUse` context uniquely resolves the exact canonical gateway URI against the coherent Project/Loadout Resource publication, rewrites only the outbound upstream request URI to its opaque native value, and rebinds the same Access/manager publication before exposing any result or error. The manager brackets Wave 45 outcome attribution with its exact pool-publication lease, so pool replacement or ABA cannot mutate current health or expose stale output. Generations remain observations rather than capabilities. This seam accepts trusted server-derived identity and protected-route facts but does not validate a transport token instance or expiry; a later handler mount must add that middleware boundary. Handlers, OAuth, relay, UI/local/synthetic/Skills, ResourceTemplates, wire mapping, and telemetry remain unchanged.

### Wave 47 / Milestone 0AY: Project-bound exact regular `resources/read` mount

Mount the Wave 46 resolver at the MCP `resources/read` boundary. Immediately after dispatch-start logging, one family-neutral Project observation selects the entire request path: Legacy requests retain the existing Skills/UI/local/synthetic/OAuth/relay/raw dispatch tree byte-for-byte, while Unavailable and Bound observations are terminal. Bound requests revalidate the immutable middleware-owned transport binding's expiry and independently derived identity before and after the exact pooled read; all nonregular URI families and ambiguous or absent routes fail with the same non-enumerating not-found envelope and never fall through. Operational failures use a redacted upstream error, and the terminal branch owns exactly one dispatch notification. This wave does not add OAuth, relay, UI, local, synthetic, Skills, ResourceTemplate, list, or write authority to Project-bound routes.

### Wave 48 / Milestone 0AZ: incarnation-bound exact regular Tool call kernel

Add an unmounted pool-local freshness kernel for regular non-OAuth Tool calls. It acquires the per-upstream permit before observing the exact current Tool publication generation/route and connection/catalog incarnation, uses one queue-plus-RPC deadline, preserves every `CallToolResponse` variant accepted from the peer through checked apply, bounds successful responses and structured MCP errors, and revalidates the exact facts before applying health or exposing an outcome. Valid MCP application errors record connection success; cancellation and exhausted input-required rounds are checked but do not poison health; infrastructure failures retain only a static redacted class. This kernel intentionally performs no AssetUse/admin/destructive authorization or elicitation. SEP-2243 header recovery/cache counters and usage telemetry are compatibility blockers deliberately deferred until their mutations can be exact-publication checked; the kernel must not be mounted before those follow-up contracts are complete. OAuth, relay, builtin/Code Mode, handler, manager, Project, normalization, and dispatch telemetry remain unchanged.

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
