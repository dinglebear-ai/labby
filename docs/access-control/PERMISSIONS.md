---
title: "Permissions, Roles, and Inheritance"
created: "2026-08-22"
updated: "2026-08-22"
status: "design"
---

# Permissions, Roles, and Inheritance

## Principles

Permissions are the authorization API. Roles are named bundles of permissions for humans/operators.

Implementation code MUST check permission identifiers, never role names.

OAuth scopes such as lab:read, lab, and lab:admin remain coarse transport guards. They are not aliases for any permission in this document.

V1 is default-deny and positive-grant only. Assignment inheritance/masking controls workspace composition and must not be confused with a general Deny permission system.

## Permission registry

The initial registry should remain intentionally small. Add a new permission only when it changes an authorization decision that cannot be expressed safely by an existing permission.

Milestone 1 uses code-owned fixed roles and only `project.read`, `project.manage`, `asset.discover`, and `asset.use`. The broader registry below is a roadmap registry introduced feature-by-feature. Generic `asset.*` and kind-specific permissions MUST NOT both gate the same action without one code-owned mapping and differential tests.

### Organization and identity

| Permission | Meaning |
| --- | --- |
| organization.read | Read non-secret organization metadata visible to the caller |
| organization.manage | Change organization policy and administrative metadata |
| principal.read | Read permitted principal/member metadata |
| principal.manage | Create/disable service principals and manage permitted identity links |

### Groups

| Permission | Meaning |
| --- | --- |
| group.read | Discover/read Groups visible in the active organization |
| group.manage | Create/update/reparent/archive Groups |
| group.members.manage | Add/remove/update Group memberships |

### Projects

| Permission | Meaning |
| --- | --- |
| project.read | Discover/read permitted Projects |
| project.create | Create a Project in the organization |
| project.manage | Change Project metadata and workspace policy |
| project.members.manage | Add/remove/update direct or Group-based Project memberships |

### Shared assets and assignments

| Permission | Meaning |
| --- | --- |
| asset.discover | See an assigned asset/capability in discovery surfaces |
| asset.use | Use/read/activate the assigned asset according to kind-specific semantics |
| asset.create | Create an organization/group/project-owned asset where that family supports creation |
| asset.update | Update mutable metadata/head or create a new managed revision through the owning subsystem |
| asset.delete | Remove/retire an owned asset subject to retention/reference rules |
| asset.assign | Assign an existing asset/capability to an allowed scope |
| asset.unassign | Remove an Assignment from an allowed scope |
| asset.publish | Change publication/share visibility through the owning subsystem |
| asset.manage | Administrative asset policy operations not covered by ordinary use |

asset.discover is the single catalog/discovery gate for every AssignmentTarget, including Artifacts and Loadouts. There is no second Artifact-specific discovery permission.

asset.use is the default use gate for target kinds that do not define a stronger kind-specific action. Kind-specific operations use their own permission: Artifact in-place/remote use requires artifact.use, Loadout activation requires loadout.activate, gateway metadata uses gateway.read, and runtime credential selection uses runtime_binding.use plus secret.use where applicable. This mapping prevents two transports from interpreting a generic and kind-specific use permission differently.

If future policy needs separate tool.execute, resource.read, prompt.use, or skill.use permissions, they should explicitly replace asset.use for that operation in the shared permission registry rather than create a transport-local check.

### Artifact use and distribution

| Permission | Meaning |
| --- | --- |
| artifact.use | Use the Artifact in-place/remotely |
| artifact.sync | Materialize an exact managed copy on an eligible Labby destination |
| artifact.follow | Subscribe a managed mirror to source revision observations/approved updates |
| artifact.fork | Create a new independent Artifact identity from an exact permitted revision |
| artifact.export | Detach/export Artifact bytes outside managed Labby federation |
| artifact.reshare | Grant/share an Artifact with another eligible subject/scope/destination |
| artifact.manage | Manage source distribution policy, mirror control metadata, or equivalent administrative state |

Artifact permissions are cumulative only when the Role/Grant explicitly contains them. artifact.use does not imply artifact.sync. artifact.sync does not imply artifact.fork/export/reshare.

All distribution actions are capped by license, publication, takedown, source policy, and destination policy.

### Loadouts and gateway

| Permission | Meaning |
| --- | --- |
| loadout.activate | Activate/resolve an assigned Loadout |
| loadout.manage | Create/update Loadout composition/policy owned by the active scope |
| gateway.read | Read permitted gateway/upstream/runtime metadata |
| gateway.manage | Change gateway/upstream exposure policy within the permitted administrative scope |
| runtime_binding.use | Allow dispatch to select an approved opaque Project runtime binding |
| runtime_binding.manage | Create/change Project-to-target runtime binding metadata |

runtime_binding.use never exposes credential material to the caller.

### Policy, destinations, and audit

| Permission | Meaning |
| --- | --- |
| policy.read | Read permitted access policy configuration |
| policy.manage | Create/change Roles, Grants, Assignments, inheritance/override/mask policy |
| policy.explain | Request rich policy-decision explanations including safe source IDs |
| destination.read | View the caller/organization's permitted paired Labby destinations |
| destination.manage | Pair, rename, disable, or remove eligible Labby destinations |
| audit.read | Read permitted access-control audit records |

### Secrets

| Permission | Meaning |
| --- | --- |
| secret.use | Permit an approved runtime to use a referenced secret without revealing it |
| secret.manage | Create/update/delete secret references through the secret-owning subsystem |

secret.use and runtime_binding.use are both required where a runtime binding points at protected credentials. Neither permission means reveal-secret.

## Built-in role templates

Milestone 1 supports fixed owner/admin/member/viewer templates only. Custom Roles, arbitrary Grants, Group-derived roles, temporal memberships, and distribution permissions are later migrations.

Built-in roles are bootstrap templates. Organizations may eventually define custom roles from the same permission registry.

### Organization Owner

Intended for the explicit bootstrap/local owner and tightly controlled organization owners.

Includes all organization-level permissions necessary to administer members, Groups, Projects, Roles/Grants/Assignments, gateway policy, destinations, runtime bindings, and audit.

The owner role is not created by checking lab:admin. It is an explicit AccessStore role/membership.

### Organization Admin

Typical permissions:

- organization.read;
- principal.read;
- group.read/manage/members.manage;
- project.read/create/manage/members.manage;
- asset.discover/use/create/update/delete/assign/unassign/publish/manage;
- artifact.use/sync/follow/fork/export/reshare/manage subject to source policy;
- loadout.activate/manage;
- gateway.read/manage;
- runtime_binding.use/manage;
- policy.read/manage/explain;
- destination.read/manage;
- audit.read;
- secret.use/manage where explicitly desired.

Organizations MAY split identity/security administration from ordinary organization administration rather than granting every item above.

### Project Admin

Typical Project-scoped permissions:

- project.read/manage/members.manage;
- asset.discover/use/create/update/assign/unassign;
- artifact.use/sync/follow/fork where source policy permits;
- loadout.activate/manage;
- gateway.read;
- runtime_binding.use;
- policy.read plus Project-scope policy.manage;
- policy.explain for that Project;
- audit.read for that Project where desired.

Project Admin does not automatically get organization.manage, group.manage, gateway.manage outside the Project, secret.manage, artifact.export, or artifact.reshare.

### Project Maintainer

Typical permissions:

- project.read;
- asset.discover/use/create/update/assign/unassign within Project;
- artifact.use/sync/follow/fork where allowed;
- loadout.activate/manage;
- gateway.read;
- runtime_binding.use.

### Project Member

Typical permissions:

- project.read;
- asset.discover/use;
- artifact.use;
- loadout.activate;
- gateway.read for permitted runtime entries;
- runtime_binding.use without access to secret contents.

artifact.sync/follow/fork MAY be added to a member role by organization policy but should not be assumed by the template.

### Project Viewer

Typical permissions:

- project.read;
- asset.discover;
- gateway.read where non-sensitive.

Viewer does not imply asset.use.

## Scope applicability

A permission is effective only in a compatible active scope. The code-owned Permission registry SHALL declare the allowed ScopeRef kinds for each permission. Membership/Grant creation validates that compatibility before persistence, and the resolver retains the scope that supplied every effective permission.

Role permissions are anchored to the Membership that applies the Role. They do not float merely because the Principal belongs to the same Organization. A GroupMembership Role applies to its Group context and eligible descendant-Group policy; it does not grant Project permissions. To give a Group Project permissions, the Group is the subject of an explicit ProjectMembership. Organization-level administrative permissions may reach descendant Groups/Projects only where the Permission registry explicitly declares that behavior.

Examples:

- project.manage granted at Project Phoenix does not allow editing Project Soma.
- group.members.manage on Engineering does not allow editing Marketing.
- artifact.sync granted in Project Phoenix applies only to Artifacts available there and still subject to source distribution policy.
- organization.manage may be applied at Organization scope but cannot cross into another Organization.

The policy resolver must retain the source scope of each effective permission for explanation/audit.

## Group inheritance

Direct Group membership contributes the Role assigned to that membership. Ancestor Groups contribute their applicable inherited policy/Assignments according to Group hierarchy rules.

Group Project membership works as follows:

1. Group Engineering is assigned to Project Phoenix with Project Member role.
2. Alice is an active member of Engineering or a descendant Group whose membership closure reaches Engineering.
3. Alice receives the Project Member permissions for Project Phoenix.
4. Removing Alice from the relevant Group or removing the Group-to-Project membership revokes that Project membership after policy invalidation.

V1 should keep this deterministic and avoid arbitrary graph edges between Groups.

## Assignment inheritance

Organization and Group Assignments specify scope_only or descendants.

- scope_only means the Assignment exists only in its exact Scope.
- descendants means it may participate in eligible lower-scope workspace composition.

For Group scope, descendants include descendant Groups and Projects to which the relevant Group/member context applies.

For Organization scope, descendants include its Groups and Projects.

A Project Assignment is terminal for organization hierarchy purposes. It does not automatically propagate to a user's Personal scope.

## Override precedence

Specificity alone is not permission to override.

An override requires:

1. an inherited source Assignment with an explicit slot;
2. mandatory=false;
3. allow_override=true;
4. an explicit child-scope override relation;
5. replacement Assignment with the same slot; and
6. authorization to manage Assignments in the child scope.

When valid, the replacement wins for that child workspace composition only.

This yields intuitive precedence without hidden magic:

Organization baseline
    -> Group specialization when explicitly allowed
    -> Project specialization when explicitly allowed
    -> Personal specialization only when Project overlay policy explicitly allows it

## Masking

Masking removes an inherited Assignment from one child workspace composition.

It requires mandatory=false and allow_mask=true plus an explicit mask relation.

Masking is not a Deny Grant. It does not make the target forbidden everywhere and does not override a separate direct Assignment of that target.

## Mandatory assignments

mandatory=true means:

- cannot be overridden;
- cannot be masked;
- if required by a Loadout/workspace and unavailable due to changed source/gateway policy, resolution fails closed rather than silently dropping it.

Mandatory should be used sparingly for security/compliance baselines.

## Public and anonymous access

Public Artifact publication is separate from organization membership.

If Labby later exposes anonymous/public Artifact discovery/use, anonymous access should map to an explicit constrained anonymous/public policy context, not a fake organization member Principal.

Public distribution still respects Artifact license/publication/distribution policy. Public use never implies export/fork/reshare unless the contract explicitly grants it.

## Permission changes

Removing or changing a permission in a Role/Grant must advance policy epoch and invalidate affected cached workspaces.

Permission renames are contract changes. Prefer adding a new permission and migrating persisted role entries with explicit migration tests over silently changing meaning under an existing identifier.
