---
title: "Access Control Data Model"
created: "2026-08-22"
updated: "2026-08-22"
status: "design"
---

# Access Control Data Model

## Storage direction

The preferred v1 implementation is a dedicated SQLite-backed AccessStore owned by the shared access-control layer. It should follow Labby's existing SQLite durability/migration conventions while remaining logically separate from OAuth tokens/sessions and Artifact payload storage.

The Milestone 1 database is `access.db` under Labby's configured state directory (`LABBY_HOME`, otherwise `$HOME/.labby`). The resolver must produce an absolute path; absence of a usable configured/home state root fails explicitly rather than falling back to a relative working-directory database. The initial secure opener may create the final state-directory leaf with mode `0700`, but its parent must already exist and pass symlink-safety checks; recursively creating missing ancestors is deferred until it has a descriptor-relative implementation.

SQLite foreign keys MUST be enabled. Mutations that change policy and the corresponding policy-epoch increment MUST commit atomically in one transaction.

No table defined here stores raw passwords, OAuth refresh tokens, upstream API keys, or runtime secret values.

### AccessStore security and durability profile

AccessStore is authorization-critical, not best-effort telemetry. Its implementation contract includes an owner-only state directory and database/WAL/SHM files, symlink- and hardlink-safe creation, WAL with `synchronous=FULL`, a five-second busy timeout, foreign-key verification, serialized versioned migrations, startup integrity and tenant checks, WAL-consistent backup/restore, and explicit disk-full/locked/corrupt/read-only/newer-schema behavior. Owner-only permissions must be established before schema or policy data is written. Any unavailable or unsafe state fails closed as service-unavailable/setup-required; compatibility-owner fallback is forbidden after enforcement.

The initial Milestone 1 implementation deliberately owns one mutex-serialized SQLite connection. This is the bounded concurrency model for the first correctness baseline, not an implicit connection pool. A multi-connection pool is deferred until measured contention requires it and must preserve one-transaction snapshots, per-connection foreign keys/pragmas, migration serialization, and the same failure contract.

Policy mutations and their mandatory audit event commit in the same SQLite transaction. Authorization reads use one read transaction and bounded set-based snapshot queries. AccessStore, rather than `AppState`, owns this coherence boundary; a later runtime may retain a cloneable AccessStore handle, but it must not split one authorization decision across independently opened or differently versioned reads.

## Identifier rules

All durable domain entities use opaque stable IDs. Human names are mutable display/lookup data and MUST NOT be foreign keys.

Recommended prefixes are descriptive but not semantically required by policy logic:

- prn_ for Principal;
- org_ for Organization;
- grp_ for Group;
- prj_ for Project;
- rol_ for Role;
- grn_ for Grant;
- asn_ for Assignment;
- dst_ for Labby destination;
- mir_ for managed mirror/subscription state.

Artifact IDs/revision IDs remain owned by the Artifact subsystem and are stored as validated references.

## Core tables

### access_metadata

Schema v2's STRICT singleton table has `singleton = 1` as its constrained primary key plus non-null `schema_version`, `schema_fingerprint`, `global_revision`, `updated_at`, and `bootstrap_generation` columns, with a nullable `bootstrap_identity_fingerprint`. The metadata carries exactly schema identity, the singleton global AccessStore revision, and explicit bootstrap generation/identity state; it is not an open-ended key/value surface. `global_revision` starts at zero and increments monotonically with every authorization-affecting mutation. Bootstrap generation is either zero with no fingerprint or one with a non-empty safe identity fingerprint. SQLite `user_version`, `application_id`, the compiled schema fingerprint, and the recorded schema version must agree before the store is accepted.

Unknown/newer schema versions fail closed.

### principals

Fields:

- principal_id primary key;
- kind: user or service_account;
- status: active, suspended, disabled;
- display_name optional;
- created_at;
- updated_at.

Authorization never keys on display_name.

### principal_links

Fields:

- link_id primary key;
- principal_id foreign key;
- link_kind: external or local_credential;
- issuer and subject for an external link;
- credential_id for a local-credential link;
- status: active or revoked;
- verification_generation and link_generation;
- created_at;
- updated_at.

Constraints:

- unique issuer + subject;
- unique credential_id for local credentials;
- each identity maps to exactly one Principal;
- deleting a Principal must not leave a usable orphan identity.
- transport sentinel issuers such as `browser-session` and `local` are not canonical provider issuers;
- subject-only and email-only linking are forbidden; and
- relinking requires an explicit audited compare-and-set mutation.

Verified email, last-seen time, and explicit revocation time are optional future metadata. They are not authorization keys and are not columns in the Milestone 1 v1 schema.

### Milestone 1 schema subset

Milestone 1 schema v2 contains exactly `access_metadata`, `organizations`, `principals`, `principal_links`, `projects`, `project_memberships`, `project_loadouts`, and `access_audit`. `principal_links` stores both canonical external issuer/subject links and stable local-credential links with an exactly-one-kind constraint. Project membership is direct Principal membership only and persists exactly the fixed `owner`, `admin`, `member`, or `viewer` role. `project_loadouts` has one Organization-qualified row per Project and references one existing named Loadout. The metadata table carries schema identity, the singleton global AccessStore revision, and bootstrap generation/safe identity fingerprint.

Fresh stores create the canonical v2 schema directly in one transaction. A store with the exact canonical v1 manifest migrates transactionally to v2, preserving `global_revision` and starting at bootstrap generation zero. Malformed v1 and unknown/newer schemas fail closed; migration does not silently repair them.

Groups, custom Roles/Grants, generalized Assignments, distribution, destinations, mirrors, runtime bindings, and their tables are broader future design and require later versioned migrations.

### organizations

Fields:

- organization_id primary key;
- name;
- status;
- policy_epoch monotonically increasing;
- created_at;
- updated_at.

### groups

Fields:

- group_id primary key;
- organization_id foreign key;
- parent_group_id nullable foreign key;
- kind;
- name;
- status;
- created_at;
- updated_at.

Constraints:

- parent must belong to the same Organization;
- parent cycles are forbidden;
- name uniqueness may be scoped to Organization + parent if product UX requires it, but IDs remain authoritative.

### projects

Fields:

- project_id primary key;
- organization_id foreign key;
- name;
- status;
- project_policy_epoch monotonically increasing;
- created_at;
- updated_at.

Project policy is separate from Group hierarchy.

### project_policies

Fields:

- project_id primary/foreign key;
- personal_overlay_enabled;
- allowed_overlay_kinds encoded in a versioned bounded representation;
- overlay_max_items;
- runtime_capability_overlay_allowed;
- additional versioned policy fields;
- updated_at.

Policy representation MUST be bounded and schema-versioned. Avoid an unvalidated arbitrary JSON policy language.

## Membership tables

### organization_memberships

Fields:

- membership_id primary key;
- organization_id;
- principal_id;
- role_id;
- status;
- valid_from optional;
- valid_until optional;
- created_by;
- created_at;
- updated_at.

Unique active membership should normally be organization_id + principal_id unless multi-role membership is deliberately introduced. Multiple effective roles can be expressed through Grants without duplicating membership.

### group_memberships

Fields mirror organization membership with group_id + principal_id + role_id.

Group ancestor permissions are resolved from the hierarchy; ancestor rows are not materialized as duplicate membership records.

### project_memberships

Fields:

- membership_id;
- project_id;
- subject_kind: principal or group;
- subject_id;
- role_id;
- status;
- valid_from/valid_until;
- created_by;
- timestamps.

The subject must belong to the same Organization as the Project.

Milestone 1 permits only `principal` subjects and code-owned fixed roles. Group subjects and custom role foreign keys are introduced only with their later milestone.

## Role and permission tables

### roles

Fields:

- role_id;
- organization_id nullable only for built-in template roles;
- name;
- description optional;
- system_template boolean;
- version;
- created_at;
- updated_at.

Role names never appear in authorization conditionals.

### role_permissions

Fields:

- role_id;
- permission;

Composite unique role_id + permission.

Only registered permission identifiers from PERMISSIONS.md may be persisted once registry validation is enabled.

Permission scope compatibility is code-owned metadata, not free-form database policy. Each PermissionSpec declares allowed scope kinds and any explicitly permitted descendant reach. Membership and Grant writes validate the selected Role/permission against that metadata. A GroupMembership using a Role that contains Project-only permissions is invalid; Project permissions for a Group come from a ProjectMembership whose subject is that Group.

### grants

Fields:

- grant_id;
- organization_id;
- subject_kind;
- subject_id;
- scope_kind;
- scope_id;
- permission;
- status;
- valid_from/valid_until;
- created_by;
- created_at;
- updated_at.

A Grant is positive-only in v1. There is no effect=deny column.

Polymorphic subject/scope identifiers cannot be protected by ordinary SQLite foreign keys. Before these later tables ship, their migration MUST choose typed join tables with real foreign keys or exhaustive tenant-qualified triggers plus deferred transaction validation. Every lookup and mutation includes `organization_id`. Corruption tests bypass application constructors and prove startup/doctor fail closed.

## Artifact authority and publisher policy

ArtifactInterchange remains tenancy-neutral. AccessStore therefore records who is authoritative for locally owned Artifact identities without modifying the Artifact wire envelope.

### artifact_authorities

Fields:

- artifact_id primary key/reference to ArtifactStore;
- owner_scope_kind: personal, organization, group, or project;
- owner_scope_id;
- status;
- created_by;
- created_at;
- updated_at.

Constraints:

- one locally authoritative Artifact ID has exactly one owner scope;
- owner scope must exist and be internally consistent;
- a managed remote mirror does not receive a local authority row merely because bytes are materialized;
- a personal fork receives a new Artifact ID and a Personal authority row while ArtifactLineage remains canonical for source lineage.

ArtifactStore is filesystem-backed, so `artifact_id` is not a SQLite foreign key. Rows include `operation_id` and lifecycle state (`pending`, `committing`, `active`, `failed`) and become usable only in `active`. Startup and doctor reconcile missing/orphan content and policy rows without granting authority.

### artifact_publisher_policies

Fields:

- artifact_id primary/foreign key to artifact_authorities;
- allow_sync boolean default false;
- allow_follow boolean default false;
- allow_fork boolean default false;
- allow_export boolean default false;
- allow_reshare boolean default false;
- managed_revocation_mode: disable_only or purge_when_reachable;
- version;
- updated_by;
- updated_at.

This policy is the owner's maximum distribution ceiling. It can never override Artifact license/publication/takedown restrictions. Only owner-scope artifact.manage/policy authority can widen it.

## Assignment tables

### assignments

Fields:

- assignment_id;
- organization_id;
- scope_kind;
- scope_id;
- target_kind;
- target_key;
- target_revision optional but required for ArtifactRevision targets;
- slot optional;
- required boolean;
- mandatory boolean;
- inheritance: scope_only or descendants;
- allow_override boolean;
- allow_mask boolean;
- status;
- created_by;
- created_at;
- updated_at.

Target encoding MUST be canonical and validated per target_kind. Do not persist an unchecked arbitrary URI as a capability identity.

Artifact assignments MUST persist exact artifact_id + revision_id.

### artifact_assignment_distribution

Optional one-to-one policy for an ArtifactRevision Assignment. Absence means all byte-movement/reshare operations are disabled for that share.

Fields:

- assignment_id primary/foreign key;
- allow_sync boolean default false;
- allow_follow boolean default false;
- allow_fork boolean default false;
- allow_export boolean default false;
- allow_reshare boolean default false;
- updated_by;
- updated_at.

An assignment policy may only narrow the matching artifact_publisher_policies ceiling. The evaluator still intersects it with caller permissions, Artifact publication/license/takedown state, and destination policy. This allows the same exact Artifact revision to be remote-use-only in one Project and sync/fork-enabled in another without duplicating bytes.

### assignment_relations

Represents explicit replacement or masking relationships.

Fields:

- relation_id;
- source_assignment_id;
- child_scope_kind;
- child_scope_id;
- relation_kind: override or mask;
- replacement_assignment_id nullable for mask and required for override;
- created_by;
- created_at.

Validation rules:

- source and child scope must share one Organization;
- source must be inherited into child scope;
- source mandatory=true rejects both relation kinds;
- override requires source allow_override=true;
- mask requires source allow_mask=true;
- replacement must use the same explicit slot as source;
- one child scope cannot create contradictory active relations for the same source.

Keeping relations explicit prevents accidental override from name collisions.

## Runtime binding table

### runtime_bindings

Fields:

- runtime_binding_id;
- organization_id;
- project_id;
- target_kind;
- target_key;
- secret_ref opaque;
- status;
- created_by;
- created_at;
- updated_at.

secret_ref identifies an entry in the actual secret/credential owner. AccessStore MUST NOT dereference or serialize the secret into EffectiveWorkspace/audit output.

Target + Project uniqueness SHOULD prevent ambiguous credential selection unless the runtime explicitly supports named alternatives.

For the supported single-binding form this uniqueness is REQUIRED. A runtime-binding migration cannot ship until the credential owner and all subject/Project-sensitive connection-pool keys are identified and tested.

## Personal Labby destinations

### destinations

Fields:

- destination_id;
- owner_principal_id;
- display_name;
- canonical_endpoint or gateway identity;
- trust/pairing state;
- destination_capabilities;
- credential_ref opaque;
- status;
- last_verified_at optional;
- created_at;
- updated_at.

A destination belongs to a user Principal in v1. Arbitrary unpaired URLs are not persisted as trusted destinations.

## Managed Artifact state

Artifact payloads remain in ArtifactStore. AccessStore tracks only distribution/control state.

### artifact_mirrors

Fields:

- mirror_id;
- owner_principal_id;
- destination_id optional for local instance;
- source_organization_id;
- source_scope_kind/source_scope_id;
- source_artifact_id;
- source_revision_id;
- local_artifact_id;
- local_revision_id;
- mode: pinned or followed;
- status: active, access_revoked, source_withdrawn, removed, error;
- last_authorized_policy_epoch;
- created_at;
- updated_at.

A managed mirror retains source authority. local_artifact_id MAY equal source_artifact_id when the local store supports a faithful mirror identity; a personal fork must use a distinct identity.

Mirror rows also carry a durable `operation_id` plus pending/committing/active/revoked/failed reconciliation state. Filesystem and AccessStore changes use idempotent stage/finalize/compensate operations; no cross-store mutation is described as atomic.

### artifact_subscriptions

If follow behavior grows beyond a mirror mode flag, persist it separately with:

- mirror_id;
- update_policy: notify, auto_approved, pinned;
- last_observed_revision_id;
- last_applied_revision_id;
- last_checked_at;
- status.

Following alone never grants authorization. Each check/apply revalidates access and source policy.

## Personal forks

A personal fork is represented primarily by the existing ArtifactRecord/ArtifactLineage in ArtifactStore.

AccessStore MAY keep an audit/link record for source authorization evidence, but it MUST NOT become the owner of fork lineage. ArtifactLineage remains canonical.

The fork record should retain the authorization decision/audit identifier that permitted the fork, without embedding a reusable bearer credential.

## Audit tables

### access_audit

Recommended fields:

- event_id;
- occurred_at;
- correlation_id optional;
- actor_principal_id;
- organization_id optional;
- project_id optional;
- action;
- target_kind;
- target_fingerprint or safe identifier;
- decision: allow or deny;
- reason_code;
- policy_epoch;
- explanation_ref optional;
- redacted metadata.

Audit records must follow docs/dev/OBSERVABILITY.md redaction rules.

### decision_evidence

Optional bounded evidence storage for administrator explanation.

It may reference:

- membership IDs;
- role IDs;
- Grant IDs;
- Assignment IDs;
- override/mask relations;
- Artifact policy fact reason;
- gateway catalog generation.

Evidence retention must be bounded. It MUST NOT store secret material or full hidden catalog payloads.

## Policy epoch rules

Milestone 1 uses one monotonically increasing AccessStore revision and performs uncached authorization reads. This revision supports snapshot stability and audit correlation, not caching. Later caching may introduce explicit Organization emergency, Project, Artifact-policy, destination-policy, and gateway-catalog version domains so unrelated mutations do not invalidate every workspace.

Every authorization-affecting mutation increments the owning Organization policy_epoch in the same transaction.

Project-local policy mutations also increment project_policy_epoch when present.

At minimum, epoch changes are required for:

- Principal status changes affecting an Organization;
- membership changes;
- Group hierarchy changes;
- Role/permission changes;
- Grants;
- Artifact authority/publisher policy changes affecting an Organization/Project scope;
- Assignments/relations and Artifact assignment distribution;
- Project overlay policy;
- RuntimeBinding selection metadata;
- destination trust/eligibility when it affects transfer; and
- managed source revocation metadata.

Artifact license/publication/head changes keep their own Artifact revisions and are included separately in distribution/workspace input where relevant.

## Migration/bootstrap

The first migration on an existing single-user Labby must be fail-safe.

The implemented store-only bootstrap transaction:

1. create one local owner Principal;
2. create one local Organization;
3. create an explicit owner Project membership;
4. associate the configured local authentication identity with that Principal where a stable identity exists;
5. record the bootstrap audit event and atomically advance the global revision and bootstrap generation.

The transaction does not create Loadout mappings, Artifact assignments, organization-visible/public grants, or a compatibility projection. Those remain later integration work, and existing private Artifacts must remain private.

Bootstrap is an explicit crate-internal AccessStore operation reached only through the authenticated browser owner-bootstrap adapter; it is not invoked by startup, AppState, setup, or doctor. It is one-time, compare-and-set, idempotent across restarts, and stores bootstrap generation one plus a safe identity fingerprint. It accepts only a pristine generation-zero business state. Ambiguous or absent canonical identity requires the explicit browser workflow; changed identity or bootstrap naming is never auto-promoted. Concurrent attempts produce exactly one owner or fail closed.

Integrity validation protects the reserved bootstrap Organization, Principal, canonical identity link, default Project, owner membership, and audit record. It intentionally does not require global table counts to remain one: later legitimate principals, projects, memberships, Loadout mappings, and audit events may coexist without invalidating bootstrap state. At generation zero, unrelated valid migrated data may be opened, but partial use of reserved bootstrap identifiers fails integrity validation and explicit bootstrap refuses any non-pristine business state.

Cross-store operations use durable pending state, idempotent finalize/compensate functions, operation IDs, and startup/runtime reconciliation. SQLite foreign keys never imply atomicity with ArtifactStore, gateway configuration, or secret storage.

## Query/index requirements

Indexes should cover:

- external identity issuer + subject;
- memberships by principal/group/project;
- Group parent traversal;
- grants by subject + scope;
- Artifact authority/publisher policy by Artifact ID;
- assignments by scope + status;
- Artifact assignment distribution by assignment ID;
- assignment relations by source/child scope;
- runtime bindings by project + target;
- destinations by owner + status;
- mirrors by owner/source/status; and
- audit by actor/project/time.

Performance work must preserve deterministic resolution and revocation correctness. Materialized membership/permission caches are optional optimizations, not sources of truth.

Milestone 1 resolution has a fixed query-count budget independent of result count. Tests assert query counts/plans and benchmark cold authorization, contention, unstable-snapshot retries, and storage failure before enforcement.

### Wave 6 Milestone 1 read snapshot

The first coherent read projections accept a canonical `VerifiedIdentity`. Listing resolves the unique active identity link and Principal, then returns active same-Organization direct Project memberships at one singleton AccessStore revision; the persisted Loadout name is optional in this discovery result. Explicit selection additionally accepts a Project ID and, within one SQLite read transaction, requires the active Project, direct membership and fixed role, and Project Loadout row. The selected result is one immutable Principal/Project/role/Loadout snapshot at that revision.

Identity resolution and explicit selection fail closed for missing, inactive, revoked, disabled, cross-Organization, or ambiguous required rows; no partial selected snapshot is returned. Listing a valid active Principal may return no Projects, and a listed Project may have no persisted Loadout mapping. This projection does not read gateway catalog state and does not prove that the named Loadout currently exists. Gateway fact capture, runtime ownership of the snapshot, capability intersection, and direct-invocation enforcement are separate next steps and are not implemented by the database read itself.

### Process runtime lifecycle

Normal process initialization does not create or migrate `access.db`. A process-scoped `AccessRuntime` classifies missing and uninitialized stores as setup-required, classifies insecure or unusable stores as blocked, and opens only an exact-current bootstrapped WAL store as Ready. Explicit owner bootstrap is the sole lifecycle path that may create or migrate the store, and successful completion atomically promotes the runtime to Ready. Ready handles remain valid for the process lifetime; persistent health is re-observed at process restart, while individual store operations continue to fail closed on storage errors.
