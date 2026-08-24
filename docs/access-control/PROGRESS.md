---
title: "Access Control Progress"
created: "2026-08-22"
updated: "2026-08-23"
status: "implementation"
---

# Access Control Progress

## Current state

The design packet and initial authentication/domain/persistence foundation are implemented in this worktree. The AccessStore now has explicit owner bootstrap, an authenticated browser-only bootstrap endpoint, and read-only doctor/setup health projection. Project-bound protected Streamable HTTP requests can carry a server-owned shadow binding that composes stable Access, Loadout-filtered MCP catalog, and protected-route narrowing evidence. `tools/list` and the built-in action-resource portion of `resources/list` consume that binding for aggregate shadow telemetry only. No discovery result or direct dispatch is authorization-enforced, and legacy transports remain unchanged. Startup bootstrap, personal-Labby pairing, and transfer protocols also remain unimplemented.

Branch/worktree for the design packet:

- branch: codex/access-control-workspaces-docs
- base at creation: origin/main 176495de6
- worktree: /home/jmagar/workspace/labby/.worktrees/access-control-workspaces-docs

## Design deliverables

- [x] Create docs/access-control/ as one canonical design folder.
- [x] Product/specification document.
- [x] Normative domain contract.
- [x] Current-code-grounded architecture document.
- [x] Proposed SQLite/data-model document.
- [x] Permission/role/inheritance model.
- [x] Project context/session binding contract.
- [x] Personal Labby Artifact distribution/sync/fork model.
- [x] Threat model and adversarial test matrix.
- [x] TDD implementation plan.
- [x] Progress tracker.
- [x] Add canonical docs/README.md index link.
- [x] Run `git diff --check`, local-link validation, and canonical product-doc validation.
- [x] Run generated-doc freshness check via the already-built Labby binary: `checked 17 docs artifacts: fresh`. (The equivalent Cargo wrapper previously hit a DOOKIE Claude bridge TaskGroup error during build execution.)
- [x] Adversarially review the design packet for contradictions/gaps.
- [x] Resolve every surfaced design-review finding before implementation begins; DREV-01 through DREV-06 are recorded below.

## Locked decisions

### D01: Shared authorization layer

Status: accepted.

CLI, API, MCP, Code Mode, web UI, and future transports use one shared domain authorization layer. No surface-specific ACL semantics.

### D02: Authentication and domain authorization are separate

Status: accepted.

Current AuthContext/OAuth scopes remain coarse authentication/transport authority. Organization/Group/Project permissions are a separate layer. lab:admin is not Organization Owner.

### D03: Principal stable identity uses issuer + subject

Status: accepted.

Verified email is metadata, never the durable authorization key.

### D04: Group is the common department/team primitive

Status: accepted.

Department, team, squad, business unit, and similar labels are Group kinds. V1 Group nesting is a same-Organization single-parent acyclic tree.

### D05: Project is first-class

Status: accepted.

Project is not a Group alias. It owns active workspace/runtime context, Project memberships, overlay policy, runtime binding selection, and audit context.

### D06: Default deny, positive grants, no generic Deny in v1

Status: accepted.

Workspace composition supports explicit inherited-assignment masks/overrides, but v1 does not add a general policy-language Deny effect.

### D07: Authorization-aware discovery plus direct re-authorization

Status: accepted.

Unauthorized catalog entries are omitted where possible, but every direct use/execute/read/distribution action is authorized again.

### D08: Assignments reference shared assets

Status: accepted.

Project/Group scoping does not copy Artifact bytes. Artifact Assignments reference exact immutable Artifact ID + revision ID.

### D09: Exact revisions in scoped Assignments

Status: accepted.

Workspace resolution never silently chases a mutable Artifact head. Follow/update policy replaces an Assignment only through an explicit audited update flow.

### D10: Personal overlay cannot broaden Project authority

Status: accepted.

Project policy must opt in. Personal additions cannot expose hidden upstreams/tools, select unapproved credentials, replace mandatory assignments, or broaden disabled capability families.

### D11: Managed mirror and personal fork are distinct states

Status: accepted.

Pin/follow creates source-authoritative managed state. Fork creates a new personal Artifact identity with exact ArtifactLineage.

### D12: Artifact distribution rights are separate

Status: accepted.

Use, sync, follow, fork, export, and reshare are independent permissions. Artifact owner publisher policy sets a maximum ceiling, each Assignment may narrow it for that share, and Artifact license/publication/takedown plus destination policy can narrow it further.

### D13: ArtifactInterchange v1 stays frozen

Status: accepted.

Access/recipient/destination/revocation policy is not added to dinglebear.artifact-interchange/v1. Transfer control uses a separate future Labby contract.

### D14: Paired destinations only for managed Send to

Status: accepted.

Normal managed transfer targets a known paired Labby destination, not an arbitrary URL supplied during a transfer request.

### D15: Project-scoped runtime bindings

Status: accepted.

Same upstream may use different credentials per Project. EffectiveWorkspace carries only opaque binding IDs and no secret values.

### D16: Policy epoch invalidates caches

Status: accepted.

Authorization-affecting mutations atomically advance a policy version/epoch. Cached workspaces/catalog projections include this version plus current gateway catalog generation.

### D17: Preferred new shared crate is labby-access

Status: revised after engineering review.

The target extracted boundary remains `labby-access`, but Milestone 1 begins in a private surface-neutral `crates/labby/src/access/` module. Extraction occurs only after a second concrete consumer or architecture/dependency test demonstrates the need. Contract fixtures must make extraction behavior-preserving.

### D18: Loadouts migrate gradually toward Artifact-backed composition

Status: accepted direction, not required for initial access-control slice.

Current GatewayLoadoutConfig remains supported as the runtime projection. Access control first assigns/restricts existing named Loadouts; later Loadout Artifacts can compile to the same runtime shape.

### D19: One universal discovery permission

Status: accepted after design review.

asset.discover is the single discovery/catalog gate for all AssignmentTarget kinds. Artifact-specific permissions begin at artifact.use and distribution actions; there is no competing artifact.discover interpretation.

### D20: Artifact authority and two-level distribution ceilings

Status: accepted after design review.

AccessStore records the authoritative owner scope for locally authoritative Artifact IDs. Owner publisher policy sets a default-deny maximum for sync/follow/fork/export/reshare, and each Artifact Assignment can only narrow those rights for a particular share. Managed mirrors do not become owners; forks create new Artifact identities/authority.

### D21: Permission scope compatibility is explicit

Status: accepted after design review.

The code-owned Permission registry declares valid scope kinds/descendant reach. Role permissions are anchored to the Membership that applies them. A GroupMembership cannot manufacture Project permissions; Group-to-Project authority requires an explicit ProjectMembership whose subject is that Group.

### D22: Project context is request/route/session scoped

Status: accepted after design review.

There is no process-global or principal-global mutable active Project. MCP binds Project no later than session establishment and keeps it immutable for that session; Code Mode inherits that context. Other surfaces resolve explicit request/session context. Concurrent clients may use different Projects safely.

### D23: Public publication is not fake membership

Status: accepted after design review.

Private/Organization/Group/Project visibility is modeled by ownership/Assignments/permissions. Public Artifact visibility uses the existing ArtifactPublication contract and does not implicitly create an unauthenticated local serving endpoint. Any future anonymous surface uses an explicit constrained public context.

## Implementation phase tracker

The phase tracker below covers the complete roadmap. The authoritative execution grouping is the milestone order in IMPLEMENTATION_PLAN.md; Phases 0-7 are narrowed for Milestones 0-1, while advanced policy, credentials, distribution, federation, persistent explanation, and caching remain dependent work.

### Phase 0: contract vectors/architecture tests

- [ ] Add failing contract fixtures for identity, default deny, scope-compatible permissions, inheritance, override/mask, overlay, Project session isolation, Artifact authority/distribution ceilings, OAuth/domain intersection, and runtime-binding isolation.
- [ ] Protect ArtifactInterchange byte-canonical fixture.
- [ ] Add architecture/layering expectations for access-control ownership.

### Phase 1: labby-access domain

Milestone 1 starts as a private `crates/labby/src/access/` kernel; extraction remains evidence-gated.

- [x] Minimal typed Principal/Organization/Project IDs and entities.
- [x] Direct Organization-qualified Project membership with fixed owner/admin/member/viewer roles.
- [x] One Organization-qualified Project-to-existing-named-Loadout mapping.
- [x] Uncached fail-closed resolution with stable Milestone 1 reason codes and cross-tenant collision tests.
- [ ] AccessStore-backed callable facade and transport integration.

Later roadmap domain work:

- [ ] Create crate.
- [ ] IDs/entities/scopes/subjects.
- [ ] Permission registry with scope compatibility/Roles/Grants.
- [ ] ArtifactAuthority + publisher distribution policy.
- [ ] Assignment targets/slots/relations + per-Assignment Artifact distribution policy.
- [ ] PersonalOverlayPolicy.
- [ ] stable reason codes.
- [ ] unit/property tests.
- [ ] comprehensive Rustdoc.

### Phase 2: AccessStore persistence

Wave 2's private persistence kernel and Wave 3's store-only explicit owner bootstrap are implemented. The kernel provides the exact eight-table schema (`access_metadata`, `organizations`, `principals`, `principal_links`, `projects`, `project_memberships`, `project_loadouts`, and `access_audit`), a singleton global AccessStore revision, an absolute configured-state `access.db` path, owner-only symlink/hardlink-safe storage, one mutex-serialized SQLite connection, exact schema-identity validation, and fail-closed typed storage errors. Schema v2 adds constrained bootstrap generation and safe identity fingerprint metadata. Fresh stores create v2 directly; exact canonical v1 stores migrate transactionally while preserving global revision.

- [x] Exact schema-v2 SQLite creation, canonical v1-to-v2 migration, and canonical reopen validation.
- [x] Composite tenant foreign keys and authorization-grade connection profile.
- [x] Canonical external/local identity shape and global uniqueness.
- [x] Secure absolute path, owner-only creation, NOFOLLOW, hardlink/sidecar checks, and integrity validation.
- [x] Singleton global revision initialized for later transactional mutations.
- [x] Explicit one-time owner bootstrap transaction with canonical identity link, default Project owner membership, audit record, compare-and-set metadata, restart idempotence, and concurrent-writer safety.
- [x] Reserved bootstrap-record integrity that permits later legitimate store growth and rejects partial reserved state.
- [x] Audited Project Loadout compatibility assignment with exact identity re-resolution, `project.manage`, same-Organization qualification, no-write idempotence, conflict detection, and atomic global/Organization/Project revision advancement.
- [ ] memberships/Roles/Grants with scope validation.
- [ ] Artifact authority/publisher policy persistence.
- [ ] Assignments/relations + Artifact Assignment distribution persistence.
- [ ] general policy-epoch mutation coverage; Project Loadout assignment already advances the global, owning Organization, and Project revisions atomically.
- [ ] runtime binding metadata.
- [ ] destinations/mirror state.
- [x] bootstrap restart/concurrency/rollback and v1 migration safety tests.

The bootstrap facade remains crate-private and is not called by product startup. One narrow `POST /v1/access/bootstrap-owner` adapter is mounted only with OAuth browser state and invokes it only after browser session, CSRF, middleware-derived `VerifiedIdentity`, `lab:admin`, and configured-admin-email gates. It returns only `created` or `already_applied`, uses canonical agent errors, and has no MCP/CLI/stdio/bearer/loopback bypass; without OAuth the route is absent and returns `404` before body validation. Doctor `access.check` and `audit.full`, plus setup check/repair reports, project observational AccessStore health without creating, migrating, bootstrapping, chmodding, checkpointing, or repairing the database. Missing/uninitialized stores remain advisory while enforcement is disabled; unsafe or unusable states are blocking. The stale AppState-only ownership assumption is superseded: AccessStore owns its connection/transaction boundary, and future application/runtime state may carry a cloneable store handle without owning policy semantics. The Loadout assignment and desired-config admission adapter are crate-private and unmounted; no API, CLI, MCP, or automatic compatibility projection invokes them. Broader mutations and transport enforcement remain unimplemented, so access control is not active merely because the store/bootstrap/health kernel exists.

### Phase 3: AuthContext identity integration

- [x] Canonical verified-identity fact keyed by issuer+subject for browser sessions and OAuth bearer tokens.
- [x] Explicit local-credential identity facts for static bearer and kernel-derived Unix peer credentials.
- [ ] disabled/unknown fail closed.
- [x] Email excluded from the Principal-link type and covered by a non-authority contract test.
- [ ] OAuth scope/domain permission separation tests.

Milestone 0A implementation evidence: `labby-auth` now emits one transport-independent `VerifiedIdentity` request extension alongside the existing `AuthContext`. Labby-issued JWTs carry signed, exactly-one identity provenance for Google, configured enterprise issuers, or machine-client local credentials; pathful HTTPS enterprise issuers remain distinct and allowlisted. Verification/link generations are explicit schema versions and fingerprints are redacted correlation values only. This establishes authentication facts only; durable Principal mapping and disabled-Principal enforcement remain AccessStore work.

### Phase 4: membership/permission resolver

- [ ] Organization/Group/Project membership.
- [ ] Group ancestor closure.
- [ ] Group-to-Project membership.
- [ ] effective Role/Grant union.
- [ ] scope boundary tests.
- [ ] baseline benchmark.

### Phase 5: Assignment composition/personal overlay

- [ ] inheritance.
- [ ] explicit override.
- [ ] explicit mask.
- [ ] mandatory baseline.
- [ ] collision/conflict handling.
- [ ] overlay policy and privilege-expansion tests.

### Phase 6: EffectiveWorkspace/Gateway integration

- [x] **Wave 6:** coherent AccessStore snapshot read from canonical `VerifiedIdentity` plus explicit Project ID through active Principal, same-Organization Project membership, fixed role, and Project Loadout, all at one store revision in one read transaction. Focused read tests cover transport convergence, exact local identity, inactive and malformed facts, cross-Organization isolation, deterministic listing, missing Loadouts, selection, revision, and restart.
- [x] **Wave 7 core:** process-scoped `AccessRuntime` lifecycle with observational startup, typed setup/blocked states, exact-current non-migrating store open, cancellation-safe serialized explicit bootstrap, and atomic promotion to Ready.
- [x] **Wave 8 ownership:** one `AccessRuntime` allocation is created after the live-daemon bridge early return and shared by hosted AppState, HTTP/Unix root and protected MCP handlers, and standalone stdio. The owner-bootstrap endpoint now mutates through that live runtime; delegated in-process peers are explicitly non-authoritative.
- [x] **Wave 9 store mutation:** explicit Project Loadout assignment re-resolves identity and direct Project membership in one immediate transaction, requires the fixed-role `project.manage` permission, advances global/Organization/Project revisions, and commits redacted audit evidence atomically. Exact replay is a zero-write success and a different existing mapping conflicts.
- [x] **Wave 10 project permission snapshot:** an uncached crate-private facade resolves `VerifiedIdentity`, same-Organization Project membership, fixed role, required Loadout mapping, requested project permission, and global revision in one read transaction. Ordinary denials collapse to one non-enumerating result while malformed/storage failures remain typed.
- [x] **Wave 11 desired-Loadout admission:** an unmounted gateway-feature adapter authorizes `project.manage` before consulting desired Gateway configuration, admits only a current exact name through `GatewayManager::loadout_get`, then reauthorizes in the audited immediate AccessStore mutation. All composition errors are bounded and redacted.
- [x] **Wave 12 published Loadout prerequisite:** Gateway exposes an exact runtime-configuration Loadout snapshot with an opaque monotonic publication generation, read under one publication barrier. Staged desired configuration is excluded and Loadout ABA produces distinct generations. This is not yet a complete pool/catalog generation or an Access runtime context.
- [x] **Wave 13 stable runtime context:** an unmounted composer performs bounded A-G-A-G reads across AccessStore and Gateway, accepts only identical authorization and published runtime-Loadout facts, and fails closed on denial, absence, or sustained churn. The result is deliberately not a dispatch grant.
- [x] **Wave 14 published upstream tool catalog prerequisite:** `UpstreamPool` exposes an immutable, generation-bearing snapshot of bounded routable and exposure-filtered tool routes. Publication is coherent across clones, process-unique across pools, distinguishes ABA changes, and fails closed on projection-limit or invariant violations. The snapshot excludes resources, prompts, skills, subject OAuth catalogs, and the built-in service registry; it remains uncomposed and does not enable discovery, transport, or dispatch enforcement.
- [x] **Wave 15 / Milestone 0S published runtime-pool identity prerequisite:** `GatewayRuntimeHandle` publishes its optional current `UpstreamPool` with an opaque process-local identity in one immutable state. Every swap advances the identity, including identical pool, repeated `None`, and A-B-A publications; clones share a stream and distinct handles do not collide.
- [x] **Wave 16 / Milestone 0T published Loadout-filtered runtime tool catalog:** `GatewayManager` uses a bounded G-C-G-C observation to bind runtime-configuration, runtime-pool, and successful upstream tool-catalog generations, then applies exact runtime Loadout `expose_tools` and upstream filtering. Missing, invalid, and sustained-churn states fail closed with redacted errors. The immutable result is unmounted observational input, not a grant, and excludes built-in services, subject OAuth catalogs, resources/prompts/skills, Code Mode, protected routes, exact targets, and enforcement.
- [x] **Wave 17 / Milestone 0U published built-in service registry catalog:** `GatewayManager` atomically publishes a bounded deterministic owned service/action projection, the exact immutable registry object, and an opaque generation. Every identical/ABA replacement advances; duplicate, inconsistent, and oversized vocabulary fails closed; `destructive` and `requires_admin` remain exact. The snapshot does not prove in-process peer routability and is not composed with Loadouts or dispatch enforcement.
- [x] **Wave 18 / Milestone 0V published Loadout-filtered built-in MCP service catalog:** `GatewayManager` uses bounded G-S-G-S observation to bind runtime-configuration and service-registry generations, then applies exact Loadout service or virtual-server ID-alias selection, `expose_tools`, enabled MCP-surface policy, deterministic action allowlists, and implicit `help`/`schema` compatibility. Missing, disabled, ambiguous, invalid, and sustained config/registry-churn states fail closed. The immutable result remains unmounted, proves neither in-process peer routability nor authorization, and excludes Project binding, protected routes, OAuth scopes, upstream tools, other MCP capability families, transport, and enforcement.
- [x] **Wave 19 / Milestone 0W unified Loadout MCP catalog prerequisite:** `GatewayManager` composes the existing Loadout-filtered upstream-tool and built-in-service snapshots across one bounded common publication interval and binds the runtime-config, pool, tool-catalog, and service-registry generations. It remains unmounted, Project-unbound observational input and is not a dispatch grant.
- [x] **Wave 20 / Milestone 0X stable Project runtime MCP catalog context:** the crate-private access composer brackets unified manager snapshots with uncached Project authorization reads, accepts only identical complete access facts and all four manager publication identities, preserves authorization-before-lookup non-enumeration, and fails closed under bounded access or catalog churn. The result remains unmounted observational evidence rather than a session binding or dispatch grant.
- [x] **Wave 21 / Milestone 0Y Project-bound protected-route publication:** `GatewayManager` publishes one canonical enabled gateway-subset route, exact Project binding, and same-generation Access-assigned Loadout with named-policy equality or inline narrowing intersection. Duplicate route/resource/Loadout identities, mismatches, and sustained churn fail closed through one non-enumerating unavailable boundary; legacy routes without `project_id` remain loadable but cannot produce this Project-bound snapshot. The immutable result remains unmounted and is not a dispatch grant.
- [x] **Wave 22 / Milestone 0Z BoundAccessContext core:** an unmounted crate-private MCP kernel binds stable Project catalog and protected-route publications over bounded C-R-C-R observation, requiring the same Project, assigned Loadout, runtime-config generation, and server-derived canonical route. It owns both non-cloneable snapshots plus checked opaque/redacted binding identity. Transport mounting, expiry, resume/session and credential-instance validation, discovery filtering, and exact-action authorization remain deferred.
- [x] **Wave 23 / Milestone 0AA protected HTTP ownership shadow:** explicit Project-bound gateway-subset requests derive verified identity from signed claim provenance, reject invalid JWT-instance identity or expiry before policy reads, and carry a request-owned `Bound` or redacted `Unavailable` observation through HTTP Parts into rmcp. Bind failure remains shadow-only and does not yet change dispatch. Legacy/root/stdio/bridge/Unix-only/in-process paths remain unbound; no handler enforces the observation yet.
- [x] **Wave 24 / Milestone 0AB tools/list Project discovery shadow:** `tools/list` consumes the request-owned Project observation and compares only provenance-known built-in services and non-OAuth upstream tool pairs against the immutable Project catalog plus protected-route narrowing. Expiry is revalidated through the listing; explicit unavailable and legacy absence remain distinct. Telemetry is aggregate-only, and descriptors, pagination, hashes, notifications, OAuth/synthetic families, and dispatch remain byte-for-byte non-enforcing.
- [x] **Wave 25 / Milestone 0AC built-in action-resource discovery shadow:** `resources/list` classifies only exact canonical `lab://<service>/actions` resources against the bound protected-route effective Loadout. Live and continuation-page outputs, cursors, snapshots, revisions, and notifications remain unchanged; telemetry is aggregate-only. Templates, reads, upstream/OAuth/UI/Skills/contract/synthetic resource families, and dispatch remain unclassified. Regular-upstream publication is deferred pending safe connection/catalog incarnation binding for asynchronous list results.
- [x] **Wave 26 / Milestone 0AD connection/catalog incarnation kernel:** every coordinated generic connection install receives a checked opaque identity mirrored beside its catalog entry; structural replacement/removal/drain paths are linearized in one audited cancellation-safe lock order. Observe/apply rejects removal and same-connection ABA without invalidating unrelated entries. This kernel is deliberately unmounted from capability-list consumers and publishes no resources; stale resource/prompt/Skills/notification result attribution remains open until each consumer routes all result-derived state through checked apply.
- [x] **Wave 27 / Milestone 0AE incarnation-bound regular resources/list attribution:** regular non-OAuth Resources fanout now acquires a routable peer and catalog identity together, then commits success/failure health, circuit/error state, unfiltered URI cache/count, exposure policy, and subscription trigger only through checked current-incarnation gates. Stale results are omitted from the wire and cannot mutate replacements. This preserves existing output behavior and does not yet publish resources or cover OAuth, templates/read, UI/synthetic families, prompts, Skills, or notifications.
- [x] **Wave 28 / Milestone 0AF immutable regular-upstream resource publication:** `CatalogState` now owns bounded incarnation-tagged regular Resource source facts and independently publishes a deterministic immutable upstream/native-URI/Resource projection on every guarded mutation. Removal, replacement/ABA, health, proxy, and exposure changes reproject atomically; duplicates, invalid facts, and route/retained/published byte overflow fail closed without retaining oversized payloads. The snapshot remains pool-only, observational, and unmounted; UI/templates/OAuth/local/synthetic/Skills/apps, manager/Project composition, reads, and enforcement are excluded.
- [x] **Wave 29 / Milestone 0AG Loadout-filtered regular Resource publication:** `GatewayManager` uses bounded G-R-G-R observation to bind active runtime-config, exact pool-publication, and immutable resource-catalog generations, then filters exact Loadout upstream membership plus `expose_resources` without changing metadata or order. Stable failures are redacted and sustained config/pool/resource churn fails boundedly. The result remains unmounted, non-grant observation; staged desired config, Project/route composition, other resource families, reads, and enforcement are excluded.
- [x] **Wave 30 / Milestone 0AH unified Project MCP catalog ownership includes Resources:** the unified manager common interval now nests Loadout-filtered tools, regular Resources, and built-in services across bounded G-T-R-S-G-T-R-S observation and includes the Resource generation in equality. Existing authorization-first Project and BoundAccessContext kernels inherit the child through nested ownership without extra reads or grants. Resource ABA/churn fails closed; discovery, reads, transport consumption, and enforcement remain unmounted.
- [x] **Wave 31 / Milestone 0AI Project-bound regular Resource discovery shadow:** regular non-OAuth upstream `resources/list` rows retain exact provenance for aggregate Project shadow classification, including continuation pages. A stable credential/Project/route/catalog key prevents a cursor snapshot from being interpreted under a different binding. Output remains unchanged; reads, templates, OAuth/UI/local/synthetic families, and enforcement remain deferred. Implemented under `lab-0hdn9`.
- [x] **Wave 32 / Milestone 0AJ incarnation-bound regular ResourceTemplate listing:** regular non-OAuth template fanout now carries the exact connection/catalog incarnation through RPC completion and gates shared Resources health plus wire rows on a checked current apply. Delayed success/failure and same-object ABA are discarded without replacement mutation. No template cache/publication, Project shadow, read authority, or enforcement is added. Implemented under `lab-whfad`.
- [x] **Wave 33 / Milestone 0AK immutable regular ResourceTemplate pool publication:** current-incarnation checked template-list results now feed an independent, bounded, immutable pool catalog with exact upstream/native-template provenance, metadata, deterministic ordering, and ABA-safe generation identity. UI/OAuth/local/synthetic families are excluded; manager/Project composition, shadow consumption, reads, and enforcement remain deferred. Implemented under `lab-6vn76`.
- [x] **Wave 34 / Milestone 0AL immutable Loadout ResourceTemplate publication:** the manager now composes a bounded active-runtime G-Q-G-Q ResourceTemplate snapshot for one Loadout, filtered by exact upstream membership and Loadout-level `expose_resources` while leaving concrete-URI allowlists out of URI-pattern semantics. Unified Project/Bound ownership and consumption remain deferred. Implemented under `lab-aztyp`.
- [x] **Wave 35 / Milestone 0AM unified Project MCP catalog ownership includes ResourceTemplates:** the bounded unified manager interval now owns tool, Resource, ResourceTemplate, and service children and compares template generation in publication equality. Project/Bound contexts and the stable cursor shadow key inherit that generation mechanically; template discovery/read/enforcement consumption remains deferred. Implemented under `lab-1bv7v`.
- [x] **Wave 36 / Milestone 0AN Project-bound regular ResourceTemplate discovery shadow:** regular non-OAuth template rows now retain exact upstream/native-template provenance for Bound route/catalog classification, including Q-aware continuation snapshots. Output remains unchanged; template reads/expansion, OAuth/UI/local/synthetic families, and enforcement remain deferred. Implemented under `lab-0q4eh`.
- [x] **Wave 37 / Milestone 0AO incarnation-bound regular prompt listing:** regular non-OAuth prompt fanout now carries the exact connection/catalog incarnation through RPC completion and gates prompt health/error/count, accepted exposure policy, post-merge owner cache, and wire rows on checked current applies. Delayed success/failure and same-object ABA are discarded without replacement mutation. Prompt publication, Project shadow, OAuth/subject-scoped changes, execution authority, and enforcement remain deferred. Implemented under `lab-y73k5`.
- [x] **Wave 38 / Milestone 0AP immutable regular Prompt pool publication:** checked regular non-OAuth prompt-list results now feed an independent bounded immutable pool catalog with exact upstream/native-name provenance, Prompt metadata, existing exposure policy, deterministic ordering, and ABA-safe generation identity. Stale results and diagnostic cache hints cannot perturb publication; manager/Project composition, discovery shadow, OAuth/subject/local/synthetic prompts, `prompts/get`, and enforcement remain deferred. Implemented under `lab-3ixqe`.
- [x] **Wave 39 / Milestone 0AQ immutable Loadout Prompt publication:** the manager now composes a bounded active-runtime G-P-G-P Prompt snapshot for one Loadout, filtered by exact upstream membership and Loadout-level `expose_prompts` while reusing the pool's per-upstream policy. Native provenance/metadata/order and exact config/pool/Prompt generations are preserved; unified Project ownership, discovery shadow, OAuth/subject/local/synthetic prompts, `prompts/get`, and enforcement remain deferred. Implemented under `lab-hypy6`.
- [x] **Wave 40 / Milestone 0AR unified Project MCP catalog ownership includes Prompts:** the unified manager common interval is now G-T-R-Q-P-S-G-T-R-Q-P-S and owns the exact Loadout Prompt child alongside tools, Resources, ResourceTemplates, and services. Prompt generation participates in Project/Bound publication equality and the stable cursor shadow key; handler shadowing, `prompts/get`, execution authority, and enforcement remain deferred. Implemented under `lab-j44gn`.
- [x] **Wave 41 / Milestone 0AS Project-bound regular Prompt discovery shadow:** regular non-OAuth `prompts/list` rows now retain exact pre-namespace upstream/native-name provenance and receive aggregate-only classification against the Bound Prompt publication and route narrowing. Prompt-aware cursor snapshots reject changed or expired binding identity as unavailable/zero without refanout; wire results and legacy behavior remain unchanged, and `prompts/get`/enforcement remain deferred. Implemented under `lab-yqj1h`.
- [x] **Wave 42 / Milestone 0AT incarnation-bound exact regular Prompt call kernel:** implemented an unmounted pool-local call primitive that validates exact current Prompt generation/route/policy/routability and token-checks every outcome. Live owner resolution, handler mounting, Project authorization, relay/OAuth, and enforcement remain deferred. Tracked under `lab-hwwv9`.
- [x] **Wave 43 / Milestone 0AU AssetUse-bound exact regular Prompt execution seam:** implement an unmounted server-owned resolver/executor that derives fresh `AssetUse` evidence, uniquely resolves the canonical regular Prompt route, and atomically gates pool outcome attribution against the exact manager publication. Live handler mounting, relay/OAuth, wire mapping, telemetry, and enforcement remain deferred. Tracked under `lab-4r7gt`.
- [x] **Wave 44 / Milestone 0AV Project-bound exact regular `prompts/get` mount:** mount the exact AssetUse resolver for middleware-bound Project requests with pre/post transport identity and expiry validation. Bound/Unavailable observations are terminal and cannot fall through to builtins, OAuth, relay, or legacy owner resolution; requests without Project observation retain the existing dispatch tree. Tracked under `lab-g035s`.
- [x] **Wave 45 / Milestone 0AW incarnation-bound exact regular Resource read kernel:** add an unmounted pool-local read primitive with exact current Resource publication and connection-incarnation validation, one queue-plus-RPC deadline, checked outcome attribution, canonical URI normalization, bounded normalized output, and redacted typed errors. Manager/AssetUse/handler mounting and all OAuth/UI/local/synthetic/Skills/ResourceTemplate paths remain deferred. Tracked under `lab-krdum`.
- [x] **Wave 46 / Milestone 0AX AssetUse-bound exact regular Resource read seam:** add an unmounted server-owned AssetUse resolver/executor with unique canonical URI matching, fresh Access/manager pre/post coherence, exact pool/Resource generation binding, native outbound rewrite, and manager-leased Wave 45 outcome attribution. Transport/handler mounting and OAuth/relay/UI/local/synthetic/Skills/ResourceTemplate paths remain deferred. Tracked under `lab-ug8ye`.
- [x] **Wave 47 / Milestone 0AY Project-bound exact regular `resources/read` mount:** select Project execution immediately after dispatch-start, preserve the complete Legacy branch, and make Bound/Unavailable observations terminal with transport expiry/identity revalidation, exact pooled regular reads, non-enumerating errors, and one notification. Project-bound OAuth/relay/UI/local/synthetic/Skills/ResourceTemplate/list/write paths remain unsupported. Tracked under `lab-nnv4t`.
- [ ] ResolutionInput gateway facts.
- [ ] Artifact authority/publisher/Assignment distribution policy facts.
- [x] crate-private desired Gateway Loadout admission adapter; `GatewayManager::loadout_get` is the point-in-time desired-config authority, but the adapter remains unmounted.
- [x] use-time Gateway Loadout resolution into an unmounted, bounded stable Project context. Gateway and AccessStore remain separate stores, so the context is optimistic evidence rather than a durable invariant or dispatch capability.
- [ ] Loadout repair/replacement workflow for invalid symbolic mappings.
- [ ] filtered workspace output.
- [ ] exact-action final dispatch authorization. Wave 10 is only a project-level snapshot: it binds no gateway target/action or catalog generation and must never be reused as a dispatch grant. Revocation can commit after its read snapshot; enforcement remains disabled until the final in-process boundary rechecks the exact operation and the revoke/check/dispatch race tests pass.
- [ ] exact Project + policy epoch/complete catalog generation cache key. Waves 19-21 provide stable Project catalog context plus a separately generation-bound exact protected-route/Loadout narrowing publication, but they are not yet composed into one request/session binding and still omit subject OAuth catalogs, other MCP capability families, exact actions, and the final dispatch boundary.
- [ ] stale-cache invalidation tests.

### Phase 7: Cross-surface enforcement

- [ ] MCP discovery filtering.
- [ ] MCP direct invocation authorization.
- [ ] Code Mode search/catalog filtering.
- [ ] Code Mode direct call authorization.
- [ ] concurrent MCP Project-session isolation + immutable Code Mode Project inheritance.
- [ ] API integration.
- [ ] CLI integration.
- [ ] web/command palette integration.
- [ ] differential cross-surface tests.

### Phase 8: Project runtime bindings

- [ ] persistence/management.
- [ ] opaque selection integration.
- [ ] runtime_binding.use + secret.use checks.
- [ ] no Project fallback.
- [ ] cross-Project credential isolation adversarial test.

### Phase 9: Local Artifact distribution semantics

- [ ] TransferOptions intersection: caller + owner publisher ceiling + Assignment ceiling + Artifact state + destination.
- [ ] managed mirror preserves source authority and never acquires ownership.
- [ ] managed pin.
- [ ] follow/subscription state.
- [ ] auto-approved update reauthorization.
- [ ] personal fork.
- [ ] detached export authorization.
- [ ] reshare authorization.
- [ ] license/publication/takedown intersection.
- [ ] revocation states.

### Phase 10: Personal Labby pairing/remote transfer

- [ ] versioned pairing contract.
- [ ] paired destination management.
- [ ] short-lived exact transfer authorization.
- [ ] replay protection/idempotency.
- [ ] endpoint/redirect/SSRF protection.
- [ ] source + destination independent validation.
- [ ] offline purge acknowledgement semantics.

### Phase 11: Loadout dependency transfer

- [ ] dependency graph model.
- [ ] required/optional semantics.
- [ ] remote/local/forked status.
- [ ] bounded/cycle-safe resolution.
- [ ] Add Loadout to My Labby transfer plan.

### Phase 12: audit/explanation

- [ ] access audit schema.
- [ ] decision evidence.
- [ ] policy.explain.
- [ ] audit.read.
- [ ] non-enumerating denial.
- [ ] redaction/bounds tests.

### Phase 13: migration/rollout

- [x] explicit store-only local owner bootstrap transaction.
- [x] authenticated browser-only owner-bootstrap endpoint with CSRF, canonical VerifiedIdentity, `lab:admin`, and configured-admin-email gates.
- [ ] preserve private Artifact defaults.
- [ ] preserve existing Loadout behavior.
- [ ] identity setup/doctor checks.
- [ ] AppState/startup bootstrap (intentionally absent; any future behavior requires a separate reviewed design).
- [ ] shadow resolver.
- [ ] opt-in enforcement flags.
- [ ] staged default-on decision.

### Phase 14: performance/scale

- [ ] cold/warm EffectiveWorkspace benchmark.
- [ ] large Group/membership benchmark.
- [ ] large Assignment/Loadout graph benchmark.
- [ ] invalidation latency benchmark.
- [ ] database query-plan/index review.
- [ ] explicit resource bounds.

### Phase 15: documentation/release gates

- [ ] docs/ARCH.md.
- [ ] docs/guides/SKILLS_AND_LOADOUTS.md.
- [ ] docs/runtime/OAUTH.md.
- [ ] docs/runtime/CONFIG.md and ENV.md.
- [ ] docs/services/GATEWAY.md.
- [ ] docs/surfaces/MCP.md and CLI.md.
- [ ] API/OpenAPI docs.
- [ ] docs/dev/DISPATCH.md if needed.
- [ ] docs/dev/OBSERVABILITY.md.
- [ ] docs/dev/TESTING.md.
- [ ] docs/artifacts cross-links without changing v1 wire contract.
- [ ] public Rustdoc.
- [ ] regenerate code-owned docs when actions/routes/metadata change.
- [ ] just docs-check.

## Adversarial review tracker

### Design review completed 2026-08-22

The initial packet was adversarially reviewed for privilege ambiguity, cross-surface divergence, Artifact ownership/distribution gaps, Project-context races, and public-access semantics. The following design findings were surfaced and resolved in the packet before implementation:

- **DREV-01 — competing discovery permissions:** generic asset.discover and artifact.discover left room for transports to disagree about Artifact visibility. **Resolved:** asset.discover is the one universal catalog gate; Artifact-specific permissions begin at artifact.use/distribution.
- **DREV-02 — Artifact ownership/distribution authority was under-specified:** the first draft referenced source policy without a persisted authoritative owner or a per-share ceiling. **Resolved:** ArtifactAuthority + default-deny owner publisher policy + default-deny Artifact Assignment distribution policy, with each layer only able to narrow. Managed mirrors never become owners; forks receive new Artifact identity/authority.
- **DREV-03 — Role permissions could float outside membership scope:** a Role bundle was not sufficiently constrained from accidentally granting Project authority through a GroupMembership. **Resolved:** code-owned PermissionSpec scope compatibility/descendant reach and Membership-anchored Role semantics; Group Project authority requires explicit ProjectMembership subject=Group.
- **DREV-04 — ambient active Project race:** the first draft required Project context but did not contract how simultaneous clients bind it. **Resolved:** PROJECT_CONTEXT.md; no process/principal-global active Project, MCP session binding is immutable for that session, Code Mode inherits it, and caches/runtime bindings include exact Project.
- **DREV-05 — Public preset conflicted with authenticated Principal semantics:** treating public as an ordinary grant could imply a fake member/anonymous Principal or accidental public server. **Resolved:** Public Artifact visibility maps to ArtifactPublication; a future anonymous serving surface uses an explicit constrained public context and does not create membership.
- **DREV-06 — transfer policy wording could conflate owner and share policy:** a Project share needed to be able to narrow an otherwise forkable Artifact without changing the immutable Artifact or publisher ceiling. **Resolved:** every transfer intersects caller permission, owner publisher policy, the specific Assignment distribution policy, Artifact publication/license/takedown state, destination policy, and exact revision integrity.

### Engineering review completed 2026-08-23

Epic `lab-mh3rs` received architecture, simplicity, security, and performance review. The complete initiative was incorrectly shaped as one v1 milestone, and several described seams were not implementation-ready.

Resolved in this packet: canonical VerifiedIdentity; a narrow Project-bound MCP Milestone 1; early bootstrap, authorization-grade SQLite, logging and mutation audit; server-owned BoundAccessContext; coherent bounded snapshot assembly; uncached query-bounded initial enforcement; explicit ArtifactStore wiring and cross-store recovery prerequisites; credential seam mapping before runtime bindings; a protocol freeze before federation; evidence-gated crate extraction; and an operational failure contract for every phase.

The full report is ENGINEERING_REVIEW.md. No critical/high review finding remains unaddressed in the plan; implementation evidence is still absent.

### Future implementation review

No implementation review has occurred yet because this worktree contains design/docs only. When implementation begins, every finding receives an ID, severity, evidence, owner/slice, resolution, and verification test. Do not close findings based only on code inspection when a regression test can demonstrate the fix.

## Open design questions

These are intentionally not blockers for the initial domain/resolver slice unless implementation reaches the affected feature.

1. **Transfer protocol shape:** exact signed/capability token format for remote Personal Labby transfer remains unfrozen until pairing/transport research is complete.
2. **Destination identity transport:** choose the canonical pairing identity mechanism compatible with Labby's OAuth/gateway model without handing destinations reusable source credentials.
3. **Managed mirror purge default:** decide final operator/user default between disable-only and purge-managed-bytes, while preserving honest offline acknowledgement semantics.
4. **Custom roles UX:** domain supports Role bundles; decide how much custom-role editing ships in the first UI versus built-in templates.
5. **Multiple organizations:** domain permits explicit Organization membership; first UX may optimize for one active Organization while keeping IDs/storage multi-org safe.
6. **Public anonymous consumption:** keep out of initial enforcement unless a concrete hosted/public use case requires it.
7. **Artifact-backed Loadouts:** determine exact Loadout Artifact manifest/dependency schema separately, then preserve compilation into current GatewayLoadoutConfig semantics.
8. **Policy bounds:** choose concrete maximum Group depth, assignments per resolution, dependency graph size, and explanation evidence after baseline benchmarks.

## Completion definition

This initiative is not complete until the implementation satisfies SPEC.md/CONTRACT.md, every threat-model mandatory test passes, ArtifactInterchange conformance remains unchanged, all relevant transports use the same shared authorization layer, migration is proven from current single-user Labby, docs/generated/Rustdoc are updated, and an adversarial review has no unresolved release-blocking findings.
