---
title: "Access Control Domain Contract v1"
created: "2026-08-22"
updated: "2026-08-22"
status: "design"
---

# Access Control Domain Contract v1

This document is normative for the proposed Labby access-control domain. Capitalized MUST, MUST NOT, SHALL, SHOULD, and MAY define contract requirements.

## Compatibility boundary

This contract is additive beside existing Labby authentication, Artifact, Gateway, Loadout, and MCP contracts.

It MUST NOT change the frozen dinglebear.artifact-interchange/v1 wire envelope. It MUST NOT redefine OAuth scopes as organization/project permissions. It MUST preserve the current property that Gateway Loadouts and route exposure policy can narrow capability exposure but cannot broaden the underlying gateway catalog.

## Principal identity

A Principal is an authenticated actor with:

- principal_id: opaque stable Labby identifier;
- kind: user or service_account;
- status: active, suspended, or disabled;
- display metadata that is non-authoritative for access decisions.

An external identity link SHALL bind a Principal to a trusted issuer plus stable subject identifier. Email MAY be recorded when verified, but email MUST NOT be the durable authorization key.

Every authenticated request that enters domain authorization MUST resolve to exactly one Principal. Unknown or ambiguous identity mappings fail closed.

`AuthContext.issuer` is the issuer of the presented transport credential; it is not necessarily the canonical external identity provider. Browser sessions, Labby-issued JWTs, static credentials, and Unix peer credentials therefore MUST NOT be linked by comparing their literal transport issuer values or by subject/email alone.

The authentication boundary SHALL produce a `VerifiedIdentity` fact containing:

- authentication mechanism and transport credential issuer;
- canonical identity authority/provider issuer when one exists;
- stable provider subject when one exists;
- stable local credential identifier for static/service credentials;
- verification/link generation; and
- safe identity fingerprint for logs.

Browser session persistence MUST retain enough verified provider identity to reproduce the same canonical identity link as a bearer token for that user. Identity issuer normalization uses an explicit trusted-issuer registry and exact canonicalization rules. Identity linking/relinking is an explicit, audited administrative operation; subject-only or email-only fallback is forbidden.

Static bearer/service credentials MUST map to an explicit service_account Principal or explicit local administrative bootstrap identity. They MUST NOT implicitly inherit a human administrator's memberships.

## Scope references

ScopeRef is one of:

- Personal(principal_id)
- Organization(organization_id)
- Group(group_id)
- Project(project_id)

Every Group and Project belongs to exactly one Organization in v1. A Personal scope belongs to exactly one user Principal.

Cross-organization parentage, project ownership, Group nesting, memberships, grants, and assignments are invalid in v1 unless a future federation contract explicitly introduces them.

## Groups

A Group contains:

- group_id;
- organization_id;
- optional parent_group_id;
- kind metadata such as department, team, squad, business_unit, or security_group;
- stable name and display metadata;
- status.

A Group MUST NOT be its own ancestor. Mutations that would create a cycle MUST fail atomically.

Effective Group membership for a Principal is the union of direct Group memberships plus the ancestor closure of those memberships.

## Projects

A Project contains:

- project_id;
- organization_id;
- stable name and display metadata;
- status;
- personal_overlay_policy;
- policy_epoch.

Project is a first-class runtime scope. Selecting a Project changes the authorization/cache/audit context and MAY change Loadouts, visible gateway capabilities, and runtime credential bindings. Project selection and binding MUST follow [PROJECT_CONTEXT.md](./PROJECT_CONTEXT.md): no process-global or principal-global mutable active Project is permitted.

## Memberships

OrganizationMembership binds a Principal to an Organization and a Role.

GroupMembership binds a Principal to a Group and a Role. Group membership does not automatically create Project membership unless the Group is separately assigned to that Project.

ProjectMembership binds a Subject to a Project and a Role. Subject is either Principal or Group.

Memberships MAY have optional activation/expiry timestamps. Expired or inactive memberships MUST NOT contribute permissions.

Removing or changing a membership MUST increment the affected policy epoch/version and invalidate affected cached decisions.

## Permissions and roles

Permission identifiers are stable lower-case dotted strings defined in PERMISSIONS.md.

A Role is a named, versioned bundle of Permission identifiers scoped to an Organization. Built-in role names are conveniences and MUST NOT be hard-coded as authorization checks. Authorization checks ask for permissions, not role names.

Every registered Permission declares the ScopeRef kinds in which it is meaningful. A Role permission is anchored to the Membership scope through which that Role is applied; it never floats to an unrelated scope. Creating a Membership with a Role that contains permissions incompatible with that membership scope MUST fail validation. In particular, a GroupMembership does not manufacture Project permissions: Group members receive Project permissions only through an explicit ProjectMembership whose subject is that Group (or through an applicable Organization-level administrative permission whose registry contract explicitly reaches descendant Projects).

A Grant contains:

- grant_id;
- organization_id;
- subject: Principal or Group;
- scope: ScopeRef;
- one or more permissions;
- optional activation/expiry timestamps;
- creator/audit metadata;
- revision/version.

V1 grants are positive grants only. There is no generic Deny grant.

Effective permissions are the union of active permissions supplied by applicable memberships, roles, and explicit Grants, bounded by the active scope and organization.

## Asset references

Access control intentionally separates Artifact storage from runtime capability references.

AssignmentTarget is one of:

### ArtifactRevision

An exact immutable Artifact reference containing artifact_id and revision_id.

V1 scope assignments SHALL reference an exact revision. A follow/update controller may later replace the assignment through an audited policy mutation, but EffectiveWorkspace resolution MUST NOT silently chase a mutable Artifact head.

### Loadout

A stable Loadout reference. During migration this MAY reference an existing GatewayLoadoutConfig by canonical name. When Loadouts become Artifact-backed, new assignments SHOULD reference exact Loadout Artifact revisions.

### GatewayUpstream

A configured upstream identifier.

### GatewayService

A built-in Labby service identifier.

### McpCapability

A stable capability reference containing source/upstream identity, capability kind, and canonical capability name. Capability kind includes tool, resource, prompt, and other MCP-discoverable families.

A dynamic capability that disappears from the current upstream catalog becomes unavailable even if an Assignment still references it. An Assignment is not authority to invent a capability that the gateway does not currently expose.

## Artifact authority and publisher policy

ArtifactInterchange intentionally does not encode Labby tenancy or access ownership. The access domain therefore maintains a separate local authority record for each locally authoritative Artifact ID.

An ArtifactAuthority contains at least:

- artifact_id;
- owner_scope: Personal, Organization, Group, or Project;
- status; and
- a reference to the current publisher distribution policy.

One locally authoritative Artifact has exactly one owner scope at a time. A managed mirror does not become locally authoritative merely because its bytes exist in the destination store. A personal fork creates a new Artifact identity and a new Personal authority record while ArtifactLineage preserves the source.

Only the owner scope or a Principal with the required artifact.manage/policy authority for that owner scope may widen publisher distribution policy. Assignment-specific sharing policy may only narrow that publisher ceiling. Artifact license/publication/takedown state remains an independent and potentially stricter ceiling.

Publisher distribution policy is default-deny for byte movement and SHALL independently control the maximum allowed set of managed sync, follow, fork, detached export, and reshare operations for that Artifact.

## Assignments

An Assignment makes one AssignmentTarget available in one ScopeRef.

An Assignment contains:

- assignment_id;
- organization_id;
- scope;
- target;
- slot: optional stable logical composition slot;
- required: whether dependent Loadout/workspace resolution may omit it;
- mandatory: whether lower scopes may replace or mask it;
- inheritance: scope_only or descendants;
- allow_override;
- allow_mask;
- optional Artifact assignment distribution policy reference for ArtifactRevision targets;
- optional overlay classification;
- creator/audit metadata;
- revision/version.

Assignments reference shared assets/capabilities. Creating a Project assignment MUST NOT copy immutable Artifact bytes merely to establish scope.

For an ArtifactRevision Assignment, the assignment distribution policy is a share-specific ceiling with default false for sync, follow, fork, export, and reshare. It cannot enable an operation disabled by the Artifact's publisher policy, license/publication/takedown state, or caller permissions. This permits the same Artifact to be remote-use-only in one Project and forkable in another without copying the Artifact or changing its immutable revision.

An Assignment does not itself grant use permission. The caller must also have the required action permission for that target in the active scope.

## Composition slots, overrides, and masks

A slot is an explicit stable identifier used to compose alternatives, such as security-review-prompt or project-default-loadout.

Name collisions do not imply a shared slot.

A lower-scope Assignment may replace an inherited Assignment in the same slot only when the inherited Assignment has allow_override=true and mandatory=false.

A lower scope may mask an inherited Assignment only when allow_mask=true and mandatory=false. Masking affects workspace composition only. It does not create a generic Deny permission and does not revoke unrelated direct access to the same Artifact elsewhere.

A mandatory Assignment MUST reject lower-scope replacement/masking during policy validation.

When multiple applicable non-overriding assignments occupy the same exclusive slot, resolution MUST fail with a policy-conflict diagnostic rather than choose nondeterministically.

## Personal overlay

Personal overlay is disabled unless the active Project explicitly permits it.

PersonalOverlayPolicy SHALL be able to constrain at least:

- allowed asset/capability kinds;
- whether additions are allowed;
- which slots may be overridden, if any;
- maximum number of additions;
- whether runtime capability references are allowed at all.

A personal overlay candidate MUST pass the same authorization and gateway-exposure checks as Project assets.

Personal overlay MUST NOT:

- add a GatewayUpstream not authorized in the Project;
- select a credential binding not approved for the Project;
- broaden capability categories disabled by the resolved Loadout/gateway policy;
- replace or mask a mandatory Assignment; or
- convert a hidden capability into a visible/callable one.

## EffectiveWorkspace contract

The shared resolver SHALL expose one conceptual operation:

resolve_workspace(principal_id, optional organization_id, optional project_id, resolution_context) -> EffectiveWorkspace

EffectiveWorkspace contains at least:

- principal_id;
- organization_id when applicable;
- project_id when applicable;
- effective Group IDs;
- effective permission set relevant to the active context;
- resolved assignments with source scope and resolution status;
- filtered Artifact revision references;
- filtered Loadout references;
- filtered Gateway upstream/service/capability references;
- capability category gates;
- opaque runtime_binding references where applicable;
- policy_epoch;
- gateway_catalog_generation;
- bounded explanation/evidence identifier.

EffectiveWorkspace MUST NOT contain secret credential material.

Every runtime request that consumes an EffectiveWorkspace carries a server-created `BoundAccessContext`. It binds Principal, authenticator/credential identity, route, Organization, Project, policy/access revision, expiry/lifecycle, and a safe binding fingerprint. Callers cannot supply or mutate these authorization facts through request parameters or `_meta`.

For equal inputs and equal policy/catalog/Artifact versions, resolution MUST be deterministic.

## Resolution order

The resolver SHALL evaluate in this order:

1. Verify the request already satisfied required transport authentication/OAuth scope guards.
2. Resolve external identity to one active Principal.
3. Resolve organization and active Project context without privilege inference.
4. Resolve active direct and inherited Group memberships.
5. Resolve applicable Project memberships, including Group subjects.
6. Expand active roles and explicit Grants into effective positive permissions.
7. Collect candidate Assignments from Organization, applicable Groups, Project, and allowed Personal overlay.
8. Apply inheritance boundaries, explicit slot overrides, and narrowly scoped masks.
9. Require discover/use/action permissions for each candidate target.
10. For Artifact targets, intersect caller authorization with Artifact owner/publisher distribution policy, the applicable Assignment distribution ceiling, publication, license/redistribution, takedown/review state, and destination policy where relevant.
11. Intersect runtime targets with the current Gateway catalog, upstream exposure policy, Loadout capability gates, and route policy.
12. Resolve opaque Project runtime bindings without exposing secrets.
13. Emit a deterministic EffectiveWorkspace plus bounded decision evidence.

No later step may broaden the result of an earlier narrowing step.

## Direct action authorization

Discovery filtering is not sufficient authorization.

Every direct execution/read/use/distribution mutation SHALL call the shared authorization layer with the active principal, project/scope, target, and requested action.

An execution path MUST NOT authorize solely because the target appeared in a previously cached catalog. The cached workspace or decision must still match the current policy epoch and relevant catalog generation, or it must be recomputed.

Authorization is linearized at the final in-process boundary before the first external side effect. Milestone 1 performs an uncached AccessStore read for every direct invocation. Revocation committed before that final check MUST deny the action. A revocation that commits after the final check follows explicit start-authorized semantics for that already-started side effect; multi-step or long-running operations MUST re-authorize before each independently avoidable new external side effect. Tests SHALL use barriers to exercise revoke/check/dispatch races and record both decision and execution revisions.

## Cache contract

Milestone 1 has no authorization/workspace cache. Indexed uncached resolution establishes correctness and latency baselines first.

Any later cache design MUST use explicit version domains rather than assuming one epoch is sufficient: Organization emergency revision, Project policy revision, Artifact policy/control-state version, destination policy version, and gateway catalog generation. Cache invalidation is not authorization linearization; direct invocation still follows the contract above.

Authorization/workspace caches MUST include at least:

- principal_id;
- organization/project context, including exact project_id for Project-scoped decisions;
- policy_epoch or equivalent monotonically changing policy version;
- relevant gateway catalog generation;
- any additional source revision necessary for correctness.

Membership, Grant, Role, Assignment, Project policy, destination policy, or runtime binding mutation SHALL invalidate or advance the version used by affected cache keys.

Revocation correctness takes precedence over cache hit rate.

## OAuth and domain permission intersection

Existing OAuth scopes remain independent coarse guards.

Examples:

- lab:admin MAY permit calling an administrative transport endpoint but does not by itself make the caller a Project Admin.
- A Project Admin who lacks the transport scope required by an endpoint still cannot call that endpoint.
- A bearer token scoped to read-only transport access cannot use a domain Grant to manufacture write transport authority.

Effective access is the intersection of required transport authority and required domain permission.

## Artifact distribution action contract

All AssignmentTarget discovery uses the single asset.discover permission. Artifact-specific use and distribution actions are distinct:

- artifact.use;
- artifact.sync;
- artifact.follow;
- artifact.fork;
- artifact.export;
- artifact.reshare;
- artifact.manage.

A caller holding artifact.use does not implicitly hold any distribution action.

Every distribution action SHALL evaluate:

1. caller domain permission;
2. Artifact owner/publisher distribution policy;
3. applicable Assignment distribution policy, default-deny for byte movement/reshare when absent;
4. Artifact publication state;
5. Artifact license redistribution state;
6. takedown/review restrictions;
7. destination eligibility/policy; and
8. exact revision integrity.

The most restrictive applicable condition wins.

## Loadout dependency contract

A Loadout dependency MUST be authorized independently from the Loadout container.

Each dependency is marked required or optional.

If a required dependency is inaccessible, Loadout activation/installation fails closed with a non-enumerating user error and an administrator-visible explanation.

If an optional dependency is inaccessible, it MAY be omitted only when the Loadout contract explicitly permits filtered resolution. Omission is recorded in resolution evidence.

## Runtime binding contract

RuntimeBinding associates an Organization/Project and capability/upstream with an opaque credential/configuration binding.

RuntimeBinding secrets stay in the existing secret/credential ownership layer. Access control stores only the binding reference and policy metadata necessary to select it.

Dispatch SHALL include active Project context when selecting a binding. A binding for Project A MUST NOT be selected for Project B solely because they share the same upstream name.

## Audit and explanation contract

Every policy mutation and sensitive authorization action SHALL produce bounded audit evidence with:

- actor principal;
- active organization/project;
- action;
- target identifier or safe fingerprint;
- decision;
- policy epoch;
- source Grant/Role/Assignment identifiers when safe;
- timestamp/correlation metadata;
- redacted reason code.

Rich explanation requires an administrative permission such as policy.explain or audit.read.

Ordinary unauthorized responses MUST use non-enumerating errors. They MUST NOT reveal whether a hidden target exists, which Group owns it, or which permission would unlock it.

## Stable decision reasons

The implementation SHOULD expose stable internal reason codes including:

- principal_unknown;
- principal_disabled;
- organization_unavailable;
- project_unavailable;
- project_membership_required;
- permission_missing;
- assignment_unavailable;
- inherited_assignment_masked;
- override_forbidden;
- mandatory_assignment;
- policy_conflict;
- gateway_capability_unavailable;
- loadout_dependency_unavailable;
- artifact_distribution_forbidden;
- artifact_license_restricted;
- artifact_takedown_restricted;
- destination_unavailable;
- runtime_binding_unavailable;
- policy_version_stale.

User-facing mapping may intentionally collapse several reasons to not_authorized.

## Contract evolution

V1 persisted/wire records SHALL include a schema version where they may outlive a process release.

Unknown schema versions fail closed. New optional fields require safe defaults. Changes that alter permission meaning, scope precedence, Artifact distribution semantics, or identity binding require a documented contract version review and migration tests.
