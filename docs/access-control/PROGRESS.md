---
title: "Access Control Progress"
created: "2026-08-22"
updated: "2026-08-22"
status: "design"
---

# Access Control Progress

## Current state

Design packet created. No production access-control implementation, migration, enforcement, personal-Labby pairing, or transfer protocol has been implemented by this worktree yet.

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

- [ ] SQLite migrations.
- [ ] foreign key/transaction policy.
- [ ] identity uniqueness.
- [ ] memberships/Roles/Grants with scope validation.
- [ ] Artifact authority/publisher policy persistence.
- [ ] Assignments/relations + Artifact Assignment distribution persistence.
- [ ] policy epochs.
- [ ] runtime binding metadata.
- [ ] destinations/mirror state.
- [ ] restart/migration/rollback tests.

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

- [ ] ResolutionInput gateway facts.
- [ ] Artifact authority/publisher/Assignment distribution policy facts.
- [ ] current Loadout adapter.
- [ ] filtered workspace output.
- [ ] exact Project + policy epoch/catalog generation cache key.
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

- [ ] explicit local owner bootstrap.
- [ ] preserve private Artifact defaults.
- [ ] preserve existing Loadout behavior.
- [ ] identity setup/doctor checks.
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
