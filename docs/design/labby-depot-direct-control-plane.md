---
title: "Labby to Depot Direct Control Plane"
status: accepted-implementation-specification
created: "2026-09-20"
updated: "2026-09-20"
tracking: "lab-y7n2a"
---

# Labby to Depot Direct Control Plane

This specification implements the boundary accepted by
[ADR 0002](../adr/0002-labby-product-contracts-over-private-depot.md). Labby
owns the complete user and agent experience. Depot remains the backend authority
for registry data, immutable revisions, ingestion, storage, publication,
licensing, backend authorization, and backend audit truth.

Users and agents must not need to configure Depot as an MCP upstream to use a
supported Labby Artifact workflow. Labby reaches Depot from its server-side
dispatch layer over bounded HTTPS. MCP is one Labby surface and is never the
backend transport between Labby and Depot.

This is a target implementation contract. The
[capability audit](./depot-capability-audit.md) records the current source state;
this document does not claim that the migration or its acceptance gates have
passed.

## Current paths and the convergence problem

Direct server-side Depot access already exists. The missing work is convergence,
not connectivity.

```text
Named Labby actions
  -> shared dispatch
  -> ArtifactControlPlane
  -> ArtifactControlClient
  -> Depot /api/operations/{operation}

Discover
  -> Depot provider manager
  -> ProviderRuntime
  -> pinned NetworkClient
  -> Depot /api/discovery, /list, and /get

Administration
  -> browser authority and CSRF adapter
  -> dispatch/depot.rs
  -> a separate direct Depot HTTPS client
```

The Discover path has the strongest provider topology, immutable credential
snapshot, deployment qualification, health, pagination, DNS pinning, and target
identity behavior. The curated Artifact path has the strongest operation
mapping, schema fingerprint, authority, mutation readiness, canonical request,
delegation, and redaction behavior. Administration has additional operator
delegation, destructive-request, indeterminate-outcome, and one-time-secret
handling that must be preserved.

These are useful implementation pieces, but equivalent operations do not yet
share one connection snapshot, compatibility policy, or execution coordinator.
Reaching the same Depot URL is not enough to establish equivalent behavior.

## Required architecture

### Artifact authority registry

Labby must have one internal registry of admitted Artifact authority
connections. This may evolve the existing Depot provider manager rather than
introducing a new owner. Each immutable connection snapshot is keyed by stable
`connection_id` and contains:

- normalized endpoint and approved redirect policy;
- resolved address and certificate/SPKI pins required by the transport;
- a credential reference and immutable secret-snapshot digest, never a
  serializable secret value;
- qualified Depot deployment identity;
- authority and listing epochs;
- bounded concurrency admission and health state;
- the configured project or tenant binding needed for requests.

The registry must consume the same admitted host sources used by Artifact
discovery. An explicit `connection_id` selects that exact admitted target. When
the caller omits it, execution may proceed only when policy identifies exactly
one eligible connection. It must not silently choose the first provider, fall
back to ambient environment configuration, or substitute a local Depot.

Configuration changes publish a new immutable snapshot. In-flight requests
finish against their captured snapshot or fail with a typed stale-authority
error; they must not mix an old endpoint with new credentials or epochs.

### Bounded Artifact authority transport

One pure HTTP client boundary must cover:

- deployment qualification and discovery metadata;
- bounded discovery list and get;
- operation catalog retrieval;
- operation execution;
- upload byte transfer; and
- exact revision acquisition.

The client belongs in `labby-apis` only if it remains pure: product composition
supplies endpoint, pins, credentials, headers, size bounds, and deadlines. It
must not read environment variables or files, select a connection, perform
Labby authorization, or depend on clap, rmcp, browser cookies, or CSRF.

The implementation should extend the existing pinned client or move Discover
onto an equivalent shared client. It must remove the current per-call ambient
credential selection and the third Administration transport after callers have
migrated. It must retain SSRF protection, DNS and endpoint binding, redirect
restrictions, response-size bounds, and redacted diagnostics.

### Operation policy registry

Labby owns an explicit allowlist of supported backend operations. Each policy
entry defines:

- the public Labby service and action, if any;
- the Depot operation name;
- the pinned contract version and schema fingerprint;
- required Labby permission and backend delegation capability;
- read, write, operator, and destructive classification;
- managed-authority readiness and kill-switch requirements;
- canonical parameter adaptation, result adaptation, and redaction;
- idempotency and confirmation requirements; and
- eligible surfaces.

The runtime Depot catalog supplies availability, the served schema, and backend
annotations. It does not grant public exposure. Every executable entry requires
three-way agreement between Labby's pinned fingerprint, the catalog-declared
fingerprint, and the fingerprint computed from the served schema. Missing or
altered contracts fail as incompatible before execution.

New operations advertised by Depot remain unavailable until Labby adds and
tests a policy entry. Generic Administration must use the same registry. Its
maintenance and token entries can remain browser/operator only without becoming
ordinary CLI or MCP actions.

### Shared execution coordinator

All named actions, generic Administration calls, and publishing phases must use
one coordinator below the surface adapters. In order, it must:

1. Resolve an explicit or unambiguous admitted connection snapshot.
2. Qualify and bind the expected Depot deployment identity.
3. Resolve the operation policy and verify the three-way schema fingerprint.
4. Authorize the caller and revalidate current authority before sending.
5. Enforce managed projection readiness and the mutation kill switch where the
   operation policy requires them.
6. Canonicalize parameters without collapsing omitted values into `null`.
7. Bind the idempotency intent to actor, project, target, operation, and the
   canonical parameter digest.
8. Mint or resolve the narrow, subject-bound backend delegation required for
   this request.
9. Acquire the connection's concurrency permit and apply a bounded deadline.
10. Execute through the shared transport and return a typed result or error.
11. Redact credentials, security metadata, operator-only fields, and provider
    internals before returning to a public surface.

CLI confirmation, MCP elicitation, HTTP cookies and CSRF, browser navigation,
and protocol envelopes remain adapter concerns. They derive destructive and
authority behavior from shared action metadata and operation policy; they do
not recreate it.

## Public workflows and exclusions

The [capability audit](./depot-capability-audit.md) is the complete 64-operation
denominator. The migration must preserve this disposition:

| Disposition | Count | Target contract |
| --- | ---: | --- |
| Named Labby actions | 35 | Retain provider-neutral names and exact parameter/result adapters. |
| Durable ingestion jobs | 8 | Keep `jobs.start(kind = ...)`; do not claim synchronous result parity. |
| Remote read boundary | 4 | Specify remote list/get/load/read pagination, URIs, files, and directories before claiming native Skill parity. |
| Internal exact acquisition | 1 | Keep behind Artifact import; do not expose it as a raw backend operation. |
| Administrator only | 15 | Keep 12 maintenance and 3 token operations out of ordinary model-facing CLI/MCP. |
| Missing public workflow | 1 | Add a Labby-owned remote Skill deletion workflow. |

### Remote deletion

`depot.skills.delete` needs a distinct public contract because deleting a remote
catalog entry is not equivalent to removing a local managed Artifact. The
preferred contract is `artifacts.delete_remote`, with:

- required `connection_id` unless target selection is unambiguous;
- a stable remote Artifact or Skill identifier;
- required `expected_version` or equivalent backend concurrency token;
- a caller-supplied or safely derived idempotency key;
- destructive metadata and surface-appropriate confirmation; and
- a receipt that identifies the target, prior version, backend operation, and
  outcome without leaking provider credentials or internal authorization data.

The exact name may change during implementation only if the service taxonomy
uses a clearer Labby-owned term. It must not expose `depot.skills.delete`
directly as the public product action.

### Publishing

Replace the provider-named `depot_publish` surface with a Labby-owned publishing
workflow, such as `artifacts.publish` or `skills.publish`. Reuse the existing
three-phase sequence:

1. create an authorized upload slot;
2. transfer bounded archive bytes; and
3. start archive ingestion/publication.

Authority, target identity, epochs, and kill-switch readiness must be
revalidated between phases. A successful ingestion receipt is not proof that a
public listing is live. The response must expose the job or publication state
needed for callers to observe completion.

Keep the old service as a hidden, deprecated, telemetry-counted compatibility
alias for a documented release window. It delegates to the same coordinator and
must not retain separate policy or transport code.

### Remote Skill reads

The four Depot Skill read operations remain a formal boundary until Labby
defines their remote semantics. The contract must cover pagination and cursors,
stable remote URIs, exact revisions, file versus directory reads, continuation,
and the relationship to caller-visible local or upstream Skill aggregation.
Until then, related native Skill and Artifact actions must not be described as
complete behavioral equivalents.

### Administrator-only operations

All `depot.maintenance.*` operations and `depot.tokens.{create,list,revoke}`
remain explicit administrator functions. They use the shared registry,
transport, fingerprint check, and coordinator, while retaining:

- browser/operator eligibility;
- `depot:operator` delegation where required;
- destructive intent and idempotency controls;
- uncertain-outcome reconciliation; and
- one-time-secret response handling for token creation.

They must not appear automatically in normal MCP discovery or the ordinary CLI
just because the connected Depot advertises them.

Provider connection administration remains Labby host configuration and is
separate from Depot ingestion sources. It is outside the 64-operation catalog.

## Authority and secret handling

Bootstrap or read credentials may qualify a connection and perform allowed
reads. Mutations require fresh subject-bound delegation with the exact backend
capability, including `skills:write` for supported writes. Operator operations
require `depot:operator`. A static bearer credential never substitutes for the
current user's write or operator authority.

Labby must revalidate authorization after queueing and immediately before a
request leaves the process. Multi-phase uploads revalidate between every phase.
Revocation, project loss, epoch changes, or mutation kill-switch changes fail
closed.

Credential references resolve server-side during connection composition and
rotation. Config, logs, errors, traces, receipts, caches, and serialized
snapshots must never contain bearer values, token secrets, signed delegation,
or credential-bearing URLs. Endpoint, project, credential, and deployment
identity remain bound as one qualified connection; credentials must never be
reused across targets.

## Idempotency, retries, and outcomes

For a mutation, the idempotency identity binds the actor, project, connection,
operation, and canonical parameter digest. Reusing a key with the same identity
returns the prior known receipt. Reusing it with different parameters or target
returns a conflict.

Labby must not blindly retry a mutation after the request may have reached
Depot. A connection loss or timeout after possible acceptance returns
`outcome_indeterminate` with a safe reconciliation path, such as a receipt or
job lookup. Only failures known to precede acceptance may be retried, within a
bounded budget. Read-only discovery may use bounded retries consistent with
cursor and deployment identity binding.

Typed failures must distinguish at least invalid input, unauthorized,
forbidden, incompatible schema, unavailable operation, stale authority,
precondition conflict, rate or concurrency limit, backend rejection, transport
failure before acceptance, outcome indeterminate, and internal redacted error.
Every surface maps these categories without stringifying away recovery metadata.

## Compatibility and migration sequence

Implementation proceeds behind current contracts so that transport convergence
does not become a flag-day user migration:

1. Freeze the 64-operation inventory and pinned fingerprint fixture in a test
   that requires an explicit disposition for every operation.
2. Introduce the shared authority registry and bounded transport behind the
   current Discover and curated adapters.
3. Move the 35 named actions and eight ingestion job workflows to the shared
   coordinator without changing their public contracts.
4. Move generic Administration through the same policy and coordinator while
   preserving operator delegation, destructive tracking, token secrets, and
   browser CSRF behavior.
5. Add the Labby-owned publishing workflow and compatibility alias.
6. Add remote deletion and formalize the four remote read contracts.
7. Introduce Labby-owned Administration routes and navigation. Keep redirects
   or aliases for `/depot` and `/v1/depot/*` for a documented compatibility
   window, backed by the same handlers.
8. Remove the old transports, ambient credential selection, and aliases only
   after call-site migration, telemetry review, and the acceptance gates pass.

Remote deletion changes the served `depot.skills.delete` input schema and its
pinned fingerprint. The paired Depot release **must be deployed before** the
Labby release that pins that fingerprint. Release qualification must fetch the
deployed Depot operation catalog and compare all 64 canonical input schemas
and fingerprints with Labby's pinned fixture before promoting Labby. A mismatch
blocks promotion: generic Administration validates the complete catalog, so a
single stale definition intentionally disables that surface instead of sending
requests under an unknown contract. The safe order is therefore Depot schema
deployment, catalog compatibility receipt, then Labby deployment.

The two known audit defects are prerequisites for claiming convergence:

- `lab-3kip5`: enforce pinned fingerprints in generic Administration;
- `lab-gh311`: keep candidate listing within the backend's page-size contract.

## Required verification

Unit, integration, contract, and live qualification must provide the following
evidence:

- **Inventory:** every operation in the pinned and served catalogs has exactly
  one public, internal, administrator-only, or unsupported disposition.
- **Cross-surface equivalence:** the same eligible action and parameters from
  CLI, MCP, API, and web produce the same canonical coordinator request and
  policy decision. Surface envelopes may differ deliberately.
- **Thin adapters:** adapter tests prove they delegate validation, permission,
  destructive classification, parameter adaptation, and error semantics to
  shared metadata and dispatch.
- **Connection selection:** explicit selection, exactly-one implicit selection,
  ambiguous selection, removed connections, epoch changes, and deployment
  identity changes behave deterministically.
- **Compatibility:** missing, forged, or altered catalog fingerprints and
  schemas fail before backend execution. Release evidence includes a live
  Depot-catalog comparison against Labby's complete pinned fixture.
- **Authority:** permission narrowing, subject binding, revocation between
  queue and send, operator separation, stale projection, and the mutation kill
  switch fail closed.
- **Idempotency:** replay, key conflict, concurrent duplicate calls, failures
  known to precede acceptance, and ambiguous post-send failures preserve the
  documented outcomes.
- **Publishing:** each upload phase revalidates authority and target identity;
  abandoned slots, byte bounds, failed ingestion, and completion observation
  are covered.
- **Network and secrets:** SSRF, redirects, DNS rebinding, pin mismatch,
  credential cross-target reuse, response bounds, and redaction have regression
  coverage.
- **Errors:** all typed categories have stable, secret-free snapshots for every
  eligible surface.
- **Feature slices:** each supported service compiles and tests under its
  documented Cargo feature contract.
- **Depot contract:** tests run against Depot's canonical operation registry and
  validation behavior without copying that authority into Labby.

A self-hosted HTTPS qualification must exercise representative read, write,
destructive, job, upload, and administrator workflows against a real Depot,
including restart and persistence where relevant. This evidence is recorded
separately from mocked transport tests and does not require registering Depot as
an MCP upstream.

## Source anchors

- [Shared source admission](../../crates/labby/src/dispatch/artifact_sources.rs)
- [Discover provider topology](../../crates/labby/src/dispatch/depot/manager.rs)
- [Discover target and cursor binding](../../crates/labby/src/dispatch/depot/discovery.rs)
- [Discover bounded network client](../../crates/labby/src/dispatch/depot/network.rs)
- [Curated execution coordinator](../../crates/labby/src/dispatch/artifact_control.rs)
- [Sealed client and compatibility checks](../../crates/labby-apis/src/artifact_control.rs)
- [Generic Administration dispatch](../../crates/labby/src/dispatch/depot.rs)
- [Administration HTTP adapter](../../crates/labby/src/api/services/depot.rs)
- [Publishing adapter](../../crates/labby/src/api/services/depot_publish.rs)
