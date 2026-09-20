---
title: "Depot to Labby Capability Audit"
status: source-audit
created: "2026-09-20"
updated: "2026-09-20"
---

# Depot to Labby Capability Audit

This is a point-in-time source audit supporting [ADR 0002](../adr/0002-labby-product-contracts-over-private-depot.md).
It records implementation evidence and parity limits, not live deployment qualification.
Tracking: Beads `lab-gqryn`.

## Baseline and denominator

- Labby: `132773388b045d996e80f6dc9f47ffde8119eb47`, branch
  `fix/codemode-case-insensitive-namespaces`; clean before this documentation change.
- Depot: `bbce45fcc05a4a24729802a4a5d2353816d1ac6d`; two pre-existing untracked
  documents were left untouched and were not used as implementation evidence.
- Labby's pinned `operations-v1.json` contains 64 operation names. Comparing
  these with `Depot.Operations.Registry` in the separate Depot checkout found
  exactly the same 64 names, with no additions or omissions. This name-set check
  does not assert equality of every current schema or deployed catalog.
- 35 operations have explicit mappings in `remote_control::operation` and the
  sealed SDK `Operation` enum. The remaining 29 comprise eight synchronous
  ingestion variants, four Skill reads, one exact acquisition, twelve maintenance
  operations, three token operations, and one remote Skill deletion.
- The earlier discussion omitted `depot.skills.delete` from its disposition.

## Is Administration implemented in the right place?

Mostly yes. The implementation already delegates backend execution, and much
of the Labby orchestration lives in dispatch. There are multiple paths, including
within the same Administration page:

```text
Administration Sources / Catalog / Access / Operations
  -> depot-client.ts -> /v1/depot/operations
  -> HTTP browser authority + CSRF + intent adaptation
  -> dispatch/depot.rs: policy catalog, delegation, bounded transport,
     destructive request tracking
  -> Depot.Operations -> backend execution

Administration Artifacts and named Labby action surfaces
  -> artifacts / sources / jobs / uploads / bundles
  -> dispatch/remote_control.rs -> dispatch/artifact_control.rs
  -> labby-apis/artifact_control.rs (sealed operation + schema fingerprint)
  -> Depot.Operations -> backend execution

Provider settings
  -> /v1/depot/providers -> dispatch/depot/admin.rs
  -> manager + durable local configuration store
```

The Sources page's convenience helpers call generic `depotCall`, while the
Artifacts page uses `sendControlPlaneAction` for the named services. Therefore
this is not simply a web-versus-MCP distinction. Two browser workflows can reach
separate Labby policy paths for the same backend operation.

Provider connection administration is a different responsibility from Depot
source ingestion. `dispatch/depot/admin.rs` already owns validation, credential
changes, fresh proof, expected-version checks, durable commit, and manager
publication. Its HTTP adapter supplies browser authority and CSRF. The six
ActionSpecs in `dispatch/depot/operations.rs` are not registered through
`registry.rs`; their presence alone does not make provider settings a public
MCP/CLI service.

## Verified strengths

The generic browser path requires durable browser identity and current read
access, obtains a server-cached catalog scoped by actor (five-minute maximum
age), rejects missing/expired catalog entries, and bounds requests/responses.
Write and operator policies require current admin scope, CSRF, project management
permission, and subject-bound delegation. Operator scope is preserved separately
from write scope. A shared static credential cannot substitute for write authority.
Destructive execution additionally requires explicit intent and an idempotency
key; pending or uncertain outcomes are not silently replayed. Backend validation
and resource authorization still run in Depot.

The curated path selects sealed operations, normalizes public parameters,
rejects secret inputs, checks per-operation permission, revalidates authority,
checks managed mutation readiness, binds delegation to the request, verifies the
served schema against a pinned fingerprint, and redacts provider metadata.
Both paths keep actual registry and storage execution in Depot.

## Findings and parity limits

### F1: Generic Administration misses the documented fingerprint gate

Finding tracked in Beads `lab-3kip5`.

`dispatch/depot.rs::CatalogOperation` parses name, annotations, required scope,
transport availability, and authorization metadata. Neither its catalog parser
nor generic execution verifies `contractVersion` or `schemaFingerprint` against
the served schema and a pinned contract. The browser accepts these fields as
optional and validates their shape only. Its comment points to the SDK check,
but this route does not call that SDK.

By contrast, `ArtifactControlClient::execute_with_headers` requires three-way
fingerprint agreement. The integration contract previously described that as a
universal guarantee. The documentation now identifies the implementation gap.
Generic Administration can follow an altered backend input contract rather than
rejecting it as incompatible. Depot still performs its own argument validation;
this finding is contract drift, not proof of an authorization bypass.

### F2: Shared backend execution does not yet mean shared Labby policy

Generic administration uses the host control target and configured publishing
project, browser admin scope, `ProjectManage`, and write/operator delegation.
Curated actions use explicit connection selection, operation-specific
`AssetDiscover` / `AssetUse` / `ProjectManage` checks, authority epochs, and
managed projection-readiness/kill-switch gating. The generic route does not
invoke the curated readiness gate. The backend can still reject a call; local
inspection does not establish whether a particular deployed mode accepts it.

Generic successful envelopes are forwarded with compatibility fields. Curated
successes are unwrapped and recursively redact security/operator metadata.
Error adaptation and destructive request tracking also differ. The generic
adapter's `authorized` catalog field describes the bootstrap principal and is
not an execution grant; requiring it to be true would incorrectly disable
operations that need fresh delegation. Preserve that distinction during unification.

Browser CSRF and cookies properly remain in HTTP. Operation-to-permission,
connection/target selection, compatibility, and backend delegation policy should
be shared below surfaces for equivalent supported operations. A blanket routing
change would lose current operator delegation and token one-time-secret behavior.

### F3: Candidate browsing sends an out-of-contract page size

Finding tracked in Beads `lab-gh311`.

`apps/gateway-admin/components/skills/artifact-control-plane.tsx` selects
`artifacts.list_candidates` with `limit: 50`. The pinned Depot schema and current
`Depot.Operations.Registry` cap this operation at 10. The shim forwards `limit`
without clamping. This is a source-confirmed request mismatch; with the inspected
backend schema it is rejected during validation. It was not exercised on production.

### F4: Publishing remains a protected provider-named shim

`depot_publish` is registered and names `depot.publish_skill_archive`. It is
protected-route functionality; the ordinary dispatcher deliberately rejects
unbound calls, and there is no generic `/v1/depot_publish` action endpoint.
Browser and MCP publishing already reuse the upload/create, byte PUT, and
archive-ingestion workflow in `dispatch/depot.rs`, including revalidation between
phases. Preserve those guarantees when introducing Labby's publishing contract.
The returned ingestion receipt does not itself prove public publication state.

### F5: Missing public mapping is not the same as missing backend capability

Generic Administration can project the catalog, including maintenance, token
administration, compatibility operations, and remote deletion. That is real
existing functionality, conditional on configuration, backend transport support,
and authorization. It is not evidence that all 64 operations have stable Labby
public ActionSpecs or have been exercised successfully.

Native Labby Skill methods operate on the caller-visible facade and can aggregate
local/upstream content. They are not proven equivalent to Depot's complete remote
catalog, URI, pagination, file, and directory contracts. Durable `jobs.start`
covers the eight source kinds but intentionally returns a job rather than the
synchronous ingest result. Remote `depot.skills.delete` mutates Depot's live Skill
store and is not equivalent to removing a local managed Artifact.

## Complete operation disposition

Legend:

- **Shim**: explicit named Labby mapping exists; this is not an end-to-end pass.
- **Job workflow**: source kind is supported through durable ingestion; synchronous
  result parity is intentionally not asserted.
- **Read boundary**: related Labby native/facade capability exists; full remote
  read equivalence needs an explicit contract.
- **Internal**: acquisition primitive already belongs behind import.
- **Admin**: generic browser/backend operator path exists; no curated public shim.
- **Gap**: backend capability exists through generic administration, but a distinct
  supported Labby public mapping/disposition is needed.

All rows refer to backend operations in the 64-name source inventory. Generic
catalog visibility and execution remain authority-dependent. Maintenance and
backend token operations are deliberately excluded from normal model-facing MCP.

| Depot operation | Labby mapping or disposition | Classification |
| --- | --- | --- |
| `depot.acp_registry.list` | `artifacts.list_acp_registry` | Shim |
| `depot.artifacts.exact` | `artifacts.import` via exact provider acquisition | Internal |
| `depot.artifacts.follow` | `artifacts.follow` | Shim |
| `depot.artifacts.fork` | `artifacts.fork` | Shim |
| `depot.artifacts.get` | `artifacts.get_remote` | Shim |
| `depot.artifacts.intake_candidate` | `artifacts.intake_candidate` | Shim |
| `depot.artifacts.list` | `artifacts.list_remote` | Shim |
| `depot.artifacts.list_candidates` | `artifacts.list_candidates` | Shim |
| `depot.artifacts.set_license` | `artifacts.set_license` | Shim |
| `depot.artifacts.set_publication` | `artifacts.set_publication` | Shim |
| `depot.bundles.add_skill` | `bundles.add` | Shim |
| `depot.bundles.create` | `bundles.create` | Shim |
| `depot.bundles.delete` | `bundles.delete` | Shim |
| `depot.bundles.get` | `bundles.get` | Shim |
| `depot.bundles.list` | `bundles.list` | Shim |
| `depot.bundles.publish` | `bundles.publish` | Shim |
| `depot.bundles.remove_skill` | `bundles.remove` | Shim |
| `depot.bundles.set_visibility` | `bundles.set_visibility` | Shim |
| `depot.ingest.cancel` | `jobs.cancel` | Shim |
| `depot.ingest.get` | `jobs.get` | Shim |
| `depot.ingest.list` | `jobs.list` | Shim |
| `depot.ingest.retry` | `jobs.retry` | Shim |
| `depot.ingest.start` | `jobs.start` | Shim |
| `depot.maintenance.cas_audit` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.cas_migration.audit` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.cas_migration.copy` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.cas_migration.cutover` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.cas_migration.end_retention` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.cas_migration.plan` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.cas_migration.rollback` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.cas_migration.status` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.cas_migration.verify` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.gc` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.sidecars` | Administration Operations; retain operator authority | Admin |
| `depot.maintenance.upstream` | Administration Operations; retain operator authority | Admin |
| `depot.mcp_registry.list` | `artifacts.list_mcp_registry` | Shim |
| `depot.skills.delete` | Remote catalog deletion; no equivalent local delete | Gap |
| `depot.skills.get` | Native `skills/get` / Artifact acquisition; scope differs | Read boundary |
| `depot.skills.ingest_acp_registry` | `jobs.start(kind = "acp_registry")` | Job workflow |
| `depot.skills.ingest_ard_catalog` | `jobs.start(kind = "ard_catalog")` | Job workflow |
| `depot.skills.ingest_marketplace` | `jobs.start(kind = "marketplace")` | Job workflow |
| `depot.skills.ingest_mcp` | `jobs.start(kind = "mcp")` | Job workflow |
| `depot.skills.ingest_mcp_registry` | `jobs.start(kind = "mcp_registry")` | Job workflow |
| `depot.skills.ingest_repo` | `jobs.start(kind = "repo")` | Job workflow |
| `depot.skills.ingest_skills_sh` | `jobs.start(kind = "skills_sh")` | Job workflow |
| `depot.skills.ingest_well_known` | `jobs.start(kind = "well_known")` | Job workflow |
| `depot.skills.list` | Native `skills/list` / Artifact discovery; scope differs | Read boundary |
| `depot.skills.load` | Native Skill get/read facade; URI and continuation differ | Read boundary |
| `depot.skills.read` | `resources/read` / Skill facade; remote files and directories need comparison | Read boundary |
| `depot.skills.search` | `artifacts.search_remote` | Shim |
| `depot.skills.search_ard` | `artifacts.search_ard` | Shim |
| `depot.skills.search_marketplace` | `artifacts.search_marketplace` | Shim |
| `depot.skills.search_skills_sh` | `artifacts.search_skills_sh` | Shim |
| `depot.sources.configure` | `sources.configure` | Shim |
| `depot.sources.delete` | `sources.delete` | Shim |
| `depot.sources.list` | `sources.list` | Shim |
| `depot.sources.refresh` | `sources.refresh` | Shim |
| `depot.system.status` | `artifacts.authority_status` | Shim |
| `depot.tokens.create` | Administration Access; backend credential management | Admin |
| `depot.tokens.list` | Administration Access; backend credential management | Admin |
| `depot.tokens.revoke` | Administration Access; backend credential management | Admin |
| `depot.uploads.create` | `uploads.create` | Shim |
| `depot.uploads.delete` | `uploads.delete` | Shim |
| `depot.uploads.get` | `uploads.get` | Shim |

## Explicit adaptation requirements

Existing named shims translate `connection_id` into server-side connection
selection; `id` into `artifactId`, `sourceId`, `jobId`, or `uploadId` as appropriate;
`expected_version` into `expectedVersion`; `idempotency_key` into
`idempotencyKey`; and selected nested ingestion keys into their backend names.
Bundle add/remove map to backend add_skill/remove_skill and therefore do not
prove arbitrary non-Skill bundle membership. Preserve these semantics rather
than merely replacing prefixes.

HTTP upload bytes and authenticated exact component downloads remain transport
steps, not additional JSON catalog operations. Provider configuration, authority
projection, identity/bootstrap, and browser sessions are likewise outside this
64-operation denominator. Their exclusion is not permission to duplicate those
systems in Labby or to remove existing operator controls.

## Source evidence

- [Curated public routing and parameter translation](../../crates/labby/src/dispatch/remote_control.rs)
- [Shared curated authority and execution](../../crates/labby/src/dispatch/artifact_control.rs)
- [Sealed provider client and fingerprint checks](../../crates/labby-apis/src/artifact_control.rs)
- [Pinned operation catalog](../contracts/fixtures/depot-control-plane/operations-v1.json)
- [Generic browser HTTP adapter](../../crates/labby/src/api/services/depot.rs)
- [Browser read authority](../../crates/labby/src/api/services/depot_read_access.rs)
- [Generic dispatch client and tests](../../crates/labby/src/dispatch/depot.rs)
- [Provider administration](../../crates/labby/src/dispatch/depot/admin.rs)
- [Provider ActionSpecs](../../crates/labby/src/dispatch/depot/operations.rs)
- [Service registration](../../crates/labby/src/registry.rs)
- [Administration UI](../../apps/gateway-admin/components/depot/depot-administration-page.tsx)
- [Source administration](../../apps/gateway-admin/components/depot/source-administration.tsx)
- [Artifact control plane UI](../../apps/gateway-admin/components/skills/artifact-control-plane.tsx)
- [Browser client](../../apps/gateway-admin/lib/api/depot-client.ts)
- [Native Skill adapter](../../crates/labby/src/mcp/skills.rs)
- [Exact acquisition provider](../../crates/labby-runtime/src/artifacts/provider.rs)
- [Publishing shim](../../crates/labby/src/dispatch/depot_publish.rs)

Private Depot source inspected separately: `lib/depot/operations/registry.ex`,
`lib/depot/operations.ex`, `lib/depot/operations/executor.ex`,
`lib/depot/ingest/dispatch.ex`, and `docs/transport-parity.md` at the revision above.
No private implementation source was copied into Labby.

## Verification boundary

- Source extraction confirmed 64 unique backend operations, 35 explicit curated
  mappings, and one disposition for each operation.
- `just docs-check` built the documentation binary and verified all 17 generated
  artifacts as fresh; all 641 maintained local links passed. Its product-doc
  phase failed on the pre-existing retired `docs/upstream-api` directory, which
  was left untouched. The compatibility-manifest validator and its five Python
  tests were run separately and passed. `git diff --check` passed.
- Eight standalone operation-form tests passed using Node's TypeScript stripping.
- React Administration and browser client tests could not start: this checkout
  lacks the `tsx` dependency. The combined runner failed before assertions.
- Focused Rust nextest selection could not start: all-feature compilation failed
  in existing Code Mode interfaces (`ToolDescriptor` imports, `semantic_rank`
  signature, and uncovered `CodeModeCaller` variants). No product source was
  modified by this audit. This is not a passing Rust verification receipt.
- Production routes, deployed revisions, backend mutations, and browser
  persistence were not exercised. Existing test source supports the code trace
  but is not reported as a fresh runtime pass.
