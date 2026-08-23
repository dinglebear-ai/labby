---
title: "Scoped Workspaces and Access Control Specification"
created: "2026-08-22"
updated: "2026-08-22"
status: "design"
---

# Scoped Workspaces and Access Control Specification

## Product boundary

Labby SHALL provide a first-class multi-user authorization and workspace-composition layer for organizations, groups, projects, personal workspaces, shared Artifacts, Loadouts, and gateway capabilities.

Authentication proves who the caller is. OAuth scopes gate coarse transport authority. This subsystem answers what that authenticated principal may discover, use, administer, distribute, and activate inside a selected organizational/project context.

The shared Rust implementation SHALL own policy evaluation and effective-workspace resolution. CLI, API, MCP, Code Mode, web UI, and future surfaces SHALL consume that shared layer rather than maintaining transport-specific ACL logic.

## Goals

1. Scope Loadouts, Skills, prompts, tools, resources, upstreams, and other Artifacts/capabilities by organization, department/team, project, and personal workspace.
2. Support users and service accounts as principals.
3. Model departments, teams, squads, and similar structures with a shared nestable Group primitive.
4. Keep Project as a distinct runtime scope with memberships, Loadouts, credential bindings, sessions, and audit context.
5. Resolve one deterministic EffectiveWorkspace for a principal and active project.
6. Hide unauthorized catalog entries during discovery while re-authorizing direct invocation.
7. Allow organization/group/project baselines to compose with permitted personal additions.
8. Support mandatory assets that cannot be overridden by a lower scope.
9. Support explicitly overridable assignments without copying immutable Artifact payloads.
10. Distinguish discover/use rights from sync/fork/export/reshare rights.
11. Make any available and distributable Artifact sendable to a registered personal Labby destination.
12. Preserve exact Artifact revisions, provenance, license state, and lineage through transfer and fork flows.
13. Make revocation effective across cached catalogs, active workspace resolution, managed mirrors, and runtime credential selection.
14. Produce bounded authorization explanations and append-only audit evidence.

## Non-goals for v1

V1 does not attempt to provide:

- a general-purpose ABAC/Cedar/Rego language;
- arbitrary explicit deny precedence rules;
- cross-organization trust federation between unrelated companies;
- automatic ownership transfer of detached personal forks after a license/takedown event;
- transparent copying of organization secrets or credentials to a personal Labby;
- a replacement for OAuth/OIDC authentication;
- access-control fields inside ArtifactInterchange v1;
- a separate authorization implementation for each protocol surface; or
- perfect remote deletion guarantees for bytes already exported or legitimately forked to an offline machine.

## Delivery milestones

The product goals above describe the complete initiative. They do not all ship in the first enforcement milestone.

### Milestone 1: Project-bound MCP isolation

Milestone 1 proves the smallest security boundary end to end:

- one canonical verified human identity maps browser and OAuth bearer authentication to the same Principal;
- static bearer and Unix credentials map only to explicit bootstrap/service Principals;
- one Organization contains Projects with direct Principal memberships and code-owned fixed roles;
- one Project selects one existing named `GatewayLoadoutConfig`;
- the server binds one authenticated MCP session/request context to one Project;
- discovery intersects the existing route/Loadout scope with Project membership; and
- direct invocation repeats the same current authorization check immediately before dispatch.

Milestone 1 has no authorization cache. It includes explicit single-owner bootstrap, redacted decision logging, storage failure behavior, and opt-in/shadow rollout before enforcement.

### Later milestones

The following remain part of the initiative but are not Milestone 1 dependencies: nested Groups, Group-based Project membership, custom Roles and Grants, temporal membership, generalized Assignments, inheritance/slots/overrides/masks, personal overlay, per-capability assignment, Project credential binding, Artifact distribution, destination pairing/federation, Artifact-backed Loadouts, persistent explanation evidence, public/anonymous policy, and non-MCP surface parity.

Artifact distribution and Personal Labby transfer form a separately gated dependent milestone. No network transfer implementation begins until its pairing, cryptographic identity, endpoint validation, replay, idempotency, and crash-recovery protocol is frozen.

## Scope model

### Organization

An Organization is the top-level administrative boundary. Every Group and Project belongs to exactly one Organization in v1.

Organization assignments may be inherited into its Groups and Projects when the assignment permits inheritance.

### Group

A Group models an organizational unit. A Group has one optional parent Group in v1, creating an acyclic tree inside one Organization.

Examples include department, team, squad, business unit, security group, or other operator-defined kinds. The kind is descriptive metadata and does not create a new policy engine.

A principal may be a direct member of multiple Groups. Effective group membership includes the ancestor closure of each direct membership.

### Project

A Project is a first-class scope, not a Group alias. Projects are where Labby resolves an active workspace and where runtime-specific behavior such as Loadout activation, upstream/credential binding, session context, and project audit happens.

Project membership may be granted directly to a principal or through a Group subject. A Group assigned to a Project gives eligible members the Project role specified by that assignment.

### Personal workspace

Every user principal may have a personal workspace. Personal Artifacts remain private by default. A project may permit personal additions to overlay the project workspace, but the overlay can never broaden project/runtime authority.

## Core user experiences

### Team/project scoping

An administrator can assign an Artifact, Loadout, upstream, tool, resource, prompt, or other stable capability reference to Engineering, Platform, Project Phoenix, or another scope.

A caller who activates Project Phoenix sees only the composition they are authorized to discover:

1. organization assignments that apply to Phoenix;
2. assignments from the caller's applicable Groups;
3. Project Phoenix assignments;
4. permitted personal overlay assignments.

The resolver filters all layers by the caller's effective permissions before exposing the result.

### Personal overlay

A project may allow personal additions by asset kind or slot. A user's personal Skill or prompt can then augment the project workspace without modifying the company-owned Loadout.

Personal overlay MUST NOT:

- make a hidden upstream discoverable;
- grant a tool permission the user lacks in the project;
- replace a mandatory assignment;
- inject credentials not approved for the project runtime; or
- cause a capability denied by the resolved project catalog to become callable through Code Mode or another surface.

### Mandatory baselines

An organization or group assignment may be marked mandatory and not overridable. Examples include security prompts, policy resources, required review Skills, or compliance tooling.

Lower scopes cannot replace or mask a mandatory assignment. Attempts to do so fail validation rather than silently choosing one side.

### Controlled overrides

An inherited assignment may explicitly permit a lower scope to override the same logical slot. A project can then substitute a project-specific Artifact revision or capability reference.

Overrides are explicit assignment relationships. Name collisions alone do not authorize replacement.

### Authorization-aware discovery

When a principal lacks discover permission for an entry, that entry should not appear in:

- MCP tools/list, resources/list, prompts/list, or Skills discovery;
- Code Mode search/catalog surfaces;
- CLI catalog/help surfaces that are principal-scoped;
- command-palette or web search results; or
- Artifact browsing scoped to that principal.

Direct invocation SHALL perform authorization again. Hiding discovery is not the security boundary.

### Add to My Labby

When an Artifact is available and its effective policy permits distribution, a user can choose Add to My Labby or Send to and select a registered destination.

The UI/API SHALL distinguish:

- remote use without local bytes;
- pinned managed mirror of one exact revision;
- followed managed mirror/subscription;
- personal fork with new Artifact identity and preserved lineage;
- detached export when explicitly permitted; and
- reshare/grant to another eligible scope or Labby when permitted.

The action offered to the user is the intersection of authorization grants, publisher policy, Artifact license/redistribution state, takedown state, and target policy.

### Loadout installation/composition

Adding a Loadout to a personal Labby resolves every referenced dependency independently. A Loadout cannot smuggle a dependency past its own access policy.

Each dependency is classified as one of:

- locally mirrorable;
- remote-only;
- forkable;
- unavailable; or
- blocked by license/publisher policy.

A Loadout declares whether inaccessible dependencies are required or optional. Required inaccessible dependencies make the Loadout unavailable. Optional inaccessible dependencies may be omitted with an explicit resolution explanation.

## Visibility presets

Friendly UI presets are conveniences over the underlying access/publication contracts. Private, Organization, Project, and Group map to owner scope plus Assignments/permissions; they are not separate ACL engines.

Expected presets include:

- Private: owner/personal workspace only.
- Organization: eligible organization members may discover/use.
- Project: eligible members of one or more selected Projects may discover/use.
- Group: eligible members of one or more selected Groups may discover/use.
- Public: for Artifact-backed content, request ArtifactPublication visibility Public subject to publisher/license/takedown policy. Public publication does not by itself create or enable a new unauthenticated Labby serving endpoint. If/when such a surface exists, it uses the constrained public-context rules in PERMISSIONS.md rather than a fake organization membership.

Distribution rights are configured separately. Selecting Organization visibility does not automatically allow sync, fork, export, or reshare. Likewise Public visibility does not imply fork/export/reshare rights beyond the Artifact publication/distribution contract.

## Workspace selection

A request that needs project-specific policy SHALL carry or resolve an explicit project context. The selected project becomes part of the authorization/cache/audit key.

The system MUST NOT infer a privileged project merely because the caller belongs to one. If no project is required, the operation executes in organization/personal context according to its own contract.

## Effective workspace

EffectiveWorkspace is the deterministic result of resolving:

- authenticated principal identity;
- organization membership;
- direct and inherited Group memberships;
- direct and Group-based Project memberships;
- role-derived and explicit permissions;
- scope assignments and inheritance;
- assignment overrides/masks;
- Artifact publication/license/distribution constraints;
- Loadout dependency constraints;
- current gateway catalog/exposure policy;
- personal overlay policy; and
- project runtime bindings.

Resolution must be deterministic for the same policy version, catalog generation, Artifact heads/revisions, principal, and project context.

## Runtime credential isolation

Control-plane visibility and runtime authority are separate concerns.

A shared GitHub MCP upstream, for example, may be visible to two projects while each project uses a different project-scoped credential binding. EffectiveWorkspace may reference a runtime binding identifier but MUST NOT expose secret material.

Project context SHALL flow through runtime dispatch so that a capability resolved for one project cannot accidentally execute with another project's credential binding.

## Identity requirements

Stable principal identity SHALL be keyed by trusted issuer plus stable subject identifier. Email may be stored as verified display/contact metadata but SHALL NOT be the durable authorization key.

Browser sessions, OAuth bearer tokens, API keys, and static/service credentials must map to explicit principals. Static bearer access MUST NOT silently become an all-powerful human principal.

## Revocation requirements

Membership, role, grant, assignment, project, destination, or source-policy changes SHALL invalidate affected authorization/cache state.

After revocation:

- newly resolved catalogs omit revoked entries;
- direct invocation is refused;
- new managed-mirror syncs are refused;
- active follow/subscription updates stop;
- runtime credential bindings are no longer selected; and
- revocation is written to audit evidence.

Managed local bytes follow the rules in ARTIFACT_DISTRIBUTION.md. Detached exports/forks cannot be treated as remotely erasable copies.

## Explainability and audit

Administrative tooling SHALL support bounded answers to questions such as:

- Why can this principal use this tool?
- Why is this Artifact unavailable in this project?
- Which scope assigned this Loadout?
- Which role/grant supplied this permission?
- Which inherited assignment was overridden or masked?
- Which policy/license restriction prevented a sync/fork/export?
- Who can execute a selected production capability?

Unauthorized callers SHALL receive non-enumerating refusal responses. Rich explanations are permission-gated and must not reveal hidden asset names or membership details.

## Acceptance criteria

The first production-ready release is complete only when all of the following are true:

1. Two users in different Groups can resolve different MCP/Code Mode catalogs from the same Labby instance.
2. Two Projects can expose different subsets of the same shared upstream/catalog.
3. A Group assignment inherited by a Project is deterministic and tested.
4. A project-specific allowed override replaces an inherited slot; a mandatory assignment cannot be replaced.
5. Personal overlay can add a permitted Skill/prompt without expanding tool/upstream authority.
6. Unauthorized entries are absent from discovery and rejected by direct invocation.
7. Revoking a membership/grant invalidates cached catalog access.
8. The same policy decision is enforced through CLI/API/MCP/Code Mode without transport-specific policy forks.
9. Add to My Labby can pin an exact eligible Artifact revision while preserving provenance/license/lineage.
10. A managed mirror and a personal fork have observably different ownership/revocation semantics.
11. A Loadout with mixed local/remote dependencies resolves each dependency independently and cannot bypass permissions.
12. Runtime credential selection is project-scoped and cannot bleed between projects.
13. Administrative explanation identifies the decision path while ordinary denial does not enumerate hidden assets.
14. ArtifactInterchange v1 conformance remains byte-identical and its existing tests remain green.
15. The adversarial tests in THREAT_MODEL.md pass before the feature is enabled by default.
