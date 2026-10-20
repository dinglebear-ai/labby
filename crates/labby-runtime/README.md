# labby-runtime

`labby-runtime` is the internal, surface-neutral domain and runtime layer shared
by the product, gateway, Code Mode, and authentication crates. It owns stable
contracts and executable behavior: Artifact persistence/materialization, Skill
verification, authority checks, bounded Agent/Task execution, and shared I/O
helpers. It is not limited to data transfer objects.

The dependency direction is toward [labby-primitives](../labby-primitives/Cargo.toml)
and general libraries such as Tokio, Serde, reqwest, and filesystem utilities.
It has no product-crate, Axum, Clap, rmcp, or WebAssembly-engine dependency.
[Cargo.toml](Cargo.toml) declares no crate features. Read the
[crate instructions](AGENTS.md) and [architecture](../../docs/ARCH.md) before
moving behavior across this boundary.

## Module and API map

[src/lib.rs](src/lib.rs) declares the public modules and root re-exports:

| Module or interface | Owner and purpose |
| --- | --- |
| `gateway_config`, root `CodeModeConfig` / `CodeModeResultShapePolicy` | Serialization-stable upstream, OAuth, protected-route, and Code Mode configuration; no configuration loading. |
| `artifacts::{ArtifactInterchange, ArtifactDescriptor, ArtifactRevision, ArtifactStore}` | Portable Artifact content, validation, local immutable revisions, editable workspaces, and lifecycle operations. |
| `artifacts::library` | Durable Skill Library ownership, authorization projections, version checks, idempotency, audit intents, and receipts. |
| `artifacts::provider::{ArtifactProvider, ArtifactAcquisition}` | Verified metadata/payload acquisition, local providers, and guarded remote HTTP acquisition. |
| `skills::{SkillId, SkillDescriptor, SkillProvider, ValidatedSkill}` | Provider-neutral Skill identity, lazy reads, compatibility, and existing SEP-2640 validation. |
| `authority::{AuthorityEpochVector, AuthorityBinding, AuthorityLease}` | Validated authority snapshots and short-lived, resource-bound leases. |
| `agent_runtime::{AgentAuthority, AgentExecutor, ExecutionGuard}` | Injected authority/execution effects, cancellation, resource bounds, and checks at safe boundaries. |
| `task_runtime::{TaskLedger, TaskScheduler}` | Owner-scoped scheduling and durable, fenced attempt settlement/recovery. |
| `dev_container` | Approved-template launch/observation admission checks without executing a host engine. |
| `dev_container_runtime::ContainerRuntime` | Injected container effects, launch validation, and reconciliation against durable intent. |
| `dev_container_image_runtime::{ContainerImageRuntime, ApprovedProvisionCatalog}` | Approved image-build recipes, asynchronous effect handles, and wait/recovery seams. |
| `caller_auth`, `gateway_authority` | Propagated caller facts, upstream restrictions, authority partition keys, and credential-generation bindings. |
| `error::ToolError`, `agent_error` | Shared typed errors, custom serialization, recovery advice, and side-effect metadata. |
| `redact`, `path_safety`, `response_body` | Bounded redaction, filesystem policy, and capped decoded HTTP-body buffering. |
| `secure_atomic_file`, `dotenv` | Atomic file replacement and lossless edits of complete credential assignments. |
| `helpers`, root `lab_home` / `home_dir` / `env_non_empty` | Explicit ambient environment/path helpers; separate from shared data models. |
| `code_mode_app`, `catalog_notify`, `client_registry`, `usage_actor`, `backoff` | Shared app state, notification labels, connected-client observations, attribution, and retry timing. |
| `phabby_delivery` | Versioned delivery wire types and fail-closed validators; no authorization, fetching, or transfer persistence. |

The concrete host supplies identity/authorization records, external executors,
container engines, and durable ledgers. HTTP/MCP/CLI adapters translate the
result without duplicating domain decisions. Shared runtime methods can perform
I/O, while configuration DTOs accept resolved values and do not discover files
or process environment.

## Artifact interchange and local lifecycle

`ArtifactInterchange` uses the frozen schema identifier
`dinglebear.artifact-interchange/v1`. Descriptor, revision, provenance, license,
lineage, publication, downloads, and materialization hints form its portable
envelope. Canonical JSON and component hashing determine stable identities and
revision content digests. Revision IDs default to their content digest; valid
opaque reference IDs remain supported by the interchange contract.

Keep Labby-specific ownership, provider credentials, deployment state, and
library lifecycle metadata outside that envelope. `ArtifactRecord` is a local
mutable head; immutable revision manifests and bytes remain separate. The
[Depot fixture test](tests/artifact_interchange_conformance.rs) requires an exact
canonical byte round trip against [the frozen fixture](tests/fixtures/artifact-interchange-v1.json).

`ArtifactStore::new` takes an explicit absolute root and initializes `artifacts`,
`locks`, and `library` directories. Local operations import files/packages,
export an exact revision, fork, snapshot an editable workspace, compute diffs,
and plan/apply acquired updates. Update plans pin the observed base; persistence
rejects changed heads and inconsistent immutable bytes. Export defaults block
secret-like material, and overwrite/secret exceptions require explicit options.
Store methods use synchronous filesystem I/O; async consumers must admit that
work through their own bounded blocking boundary.

`ArtifactAcquisition::validate` checks the full envelope and one-to-one payload
inventory, including exact paths, sizes, and SHA-256 digests. Remote acquisition
uses configured source policy, bounded transfers, peer/address checks, redirect
rejection, and credential-origin binding. A provider result does not grant local
activation or publication authority.

Skill Library mutations additionally require canonical authorization/ownership
projections, an expected library version, and a bound idempotency request.
Materialized writes stage a recovery intent before coordinating library metadata
with Artifact promotion. `library_snapshot` reconciles pending promotion before
returning state. Preserve `LibraryCorrupt`, `CommittedPending`, and terminal
receipts: a committed operation that needs reconciliation is not an unapplied
request to repeat. Product AccessStore coordination stays in the product layer.

See [Artifacts and Agent Skills](../../docs/services/SKILLS.md) for product
lifecycle behavior and [Artifact distribution](../../docs/access-control/ARTIFACT_DISTRIBUTION.md)
for the separate authority/managed-mirror contract.

## Skill verification and materialization

Skills remain an Artifact family and reuse the `skills` validators. The module
owns frontmatter, resource URI grammar, manifest/digest consistency, availability,
requirements, and safety budgets. Transport conversions belong in the gateway
and product adapters. A `skill://` URI alone does not establish Skill identity,
and an unsigned digest proves consistency rather than source trust.

This example validates authored content and creates an in-memory candidate:

```rust
use labby_runtime::artifacts::{
    ArtifactError, ArtifactProvenance, LogicalSkillFile, MaterializedSkill,
    materialize_logical_skill,
};

fn review_skill() -> Result<MaterializedSkill, ArtifactError> {
    let source = "---\nname: review\ndescription: Review code\n---\n# Review\n";
    materialize_logical_skill(
        "review",
        vec![LogicalSkillFile::new("SKILL.md", source)],
        ArtifactProvenance::default(),
    )
}
```

Creating `MaterializedSkill` neither writes the store nor activates the Skill.
`persist_materialized_skill` is the low-level immutable-byte/head path; managed
library creation/save uses the authorized library transaction instead. Owner
qualification, version comparison, and audit/idempotency binding must survive
that composition. Logical Agent, Hook, and Prompt materializers also project
through the Artifact model without running their content.

The [Skills extension contract](../../docs/contracts/skills-extension.md) and
[conformance tests](tests/skills_contract_conformance.rs) own exact wire rules,
budgets, and rediscovery error guidance.

## Authority, execution, and recovery

Authority leases bind a Principal, owner scope, capability, method/resource,
optional intent, expiration, and epoch fingerprint. Agent execution validates
pinned definitions and bindings, consults the current authority clock/epochs,
and checks cancellation/revocation at safe boundaries. The runtime enforces the
hard execution deadline and final commit check; `AgentExecutor::cancel` must
settle external resources within a bounded cleanup period.

`TaskScheduler` limits attempts per owner. `TaskLedger` implementations own
acquisition, fenced settlement, and expired-attempt recovery. The runtime
revalidates authority before durable success settlement. A valid dispatch-time
snapshot cannot extend a lease or authorize a later commit after revocation.

Dev Container admission checks approved images, resources, and host capabilities.
`ContainerRuntime` performs injected engine operations; reconciliation refreshes
authority/time before inspection and again before mutation. The default disabled
runtime returns typed failures. `ContainerImageRuntime` separates launch,
provision, stop, publish, cleanup/discard, and operation waiting; consumers own
the real engine and durable orchestration around those effects.

See [Agent/Task behavior](../../docs/services/AGENT_TASKS.md),
[Tasks](../../docs/services/TASKS.md), and
[Dev Containers](../../docs/services/DEV_CONTAINERS.md) for product composition.

## I/O, errors, and platform boundaries

`ToolError` preserves stable kinds and structured agent-error metadata through
custom serialization; surface adapters map it only at their boundary. Keep
recovery, retry safety, and partial-side-effect evidence. `redact` removes
secret-like fields/segments and bounds retained text or trace values. Redaction
is not permission to log raw caller payloads.

`path_safety` separates lexical traversal rejection, canonical read/write policy,
and symlink checks. A lexical guard alone does not establish containment.
Artifact validation also caps file counts, bytes, revisions, and metadata;
`read_response_body_capped` enforces per-response and aggregate buffering bounds.
The caller still owns network admission, deadlines, and request authorization.

Ambient helpers deliberately read process state: `lab_home` uses `LABBY_HOME`,
then home conventions, then a system-temp fallback. It is not the product's
validated `InstallationPaths` contract. Pass explicit roots into persistent
stores and resolved values into shared configuration. `dotenv::rewrite_key`
works on supplied text and preserves unrelated bytes; persistence is separate.

Atomic replacement reports whether publication already happened when parent
synchronization fails. Unix paths apply private modes and directory syncing.
The Windows paths validate supported directory/file operations without claiming
Unix-equivalent directory crash durability; callers must supply the appropriate
platform protection. Preserve typed busy/conflict/I/O errors and test native
branches when changing persistence or lifecycle semantics.

## Verification

Run from the workspace root with the pinned toolchain:

```bash
cargo check -p labby-runtime --all-targets
cargo nextest run -p labby-runtime
cargo nextest run -p labby-runtime --lib artifacts::
cargo nextest run -p labby-runtime --lib agent_runtime::
cargo nextest run -p labby-runtime --lib dev_container_runtime::
cargo nextest run -p labby-runtime --test artifact_interchange_conformance
cargo nextest run -p labby-runtime --test skills_contract_conformance --test sep_2640_uri_conformance
cargo nextest run -p labby-runtime --test agent_error_schema --test phabby_delivery_conformance
cargo clippy -p labby-runtime --all-targets -- -D warnings
```

There are no feature slices to select in this crate. Nextest excludes doctests;
`just rustdoc-check` covers workspace Rustdoc and doctests. README Rust fences
are illustrative, not executable doctests. [Testing](../../docs/dev/TESTING.md)
owns broader verification; [Errors](../../docs/dev/ERRORS.md) owns surface mapping.
