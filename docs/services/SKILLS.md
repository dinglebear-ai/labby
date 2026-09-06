---
title: "Artifacts And Agent Skills"
created: "2026-08-26"
updated: "2026-09-03"
---

# Artifacts And Agent Skills

Labby models a Skill as one kind of durable Artifact. The product boundary has
two related surfaces:

- native SEP-2640 `skills/list` and `skills/get`, plus manifest-bound
  `resources/read`;
- an authenticated MCP App and lifecycle actions under `artifacts.*`.

There are no `skills.*` or `skill_library.*` management aliases. The native
`skills/list` and `skills/get` names remain because they are the Agent Skills
protocol, not a second Labby storage namespace.

The native extension contract is pinned and documented in
[Skills extension](../contracts/skills-extension.md). This document owns the
Labby product lifecycle and operator behavior.

## Lifecycle

Labby keeps four facts separate:

1. **Stored**: an immutable Artifact revision is durable.
2. **Materialized**: its logical files passed canonical Skill validation.
3. **Active**: a particular revision belongs to the committed active set.
4. **Published**: that committed set has been installed as the process-wide
   Skills generation visible to readers.

Create, save, and import store a revision but never activate it. Activate and
rollback publish an exact revision as a new generation. Deactivate removes a
Skill from the active generation without deleting its revisions. Archive is a
recoverable catalog state transition and retains immutable revision storage for
the owner or an administrator.

Every successful mutation returns the committed and published library versions,
generation facts, the library digest, and explicit re-list guidance. Labby does
not emit a Skills-specific `list_changed` notification; clients re-run
`artifacts.list` or native `skills/list`.

## Library Actions

The canonical action catalog is generated from code. The lifecycle groups are:

| Group | Actions | Contract |
| --- | --- | --- |
| Browse | `artifacts.search`, `.list`, `.get`, `.history`, `.read` | Versioned, bounded, caller-filtered metadata; only `.read` returns one manifest-bound file body |
| Author | `artifacts.validate`, `.create`, `.save` | Logical UTF-8 files; validation is side-effect free; create/save do not activate |
| Publish | `artifacts.activate`, `.deactivate`, `.rollback`, `.refresh` | Exact revision and optimistic library-version preconditions; publication is atomic |
| Acquire | `artifacts.import`, `.import_batch` | Exact immutable selectors through server-configured connections; no caller-supplied endpoint, path, bytes, or credential |
| Remote discovery | `artifacts.search_remote`, `.list_remote`, `.get_remote`, `.list_candidates`, `.search_skills_sh`, `.search_ard`, `.search_marketplace`, `.list_mcp_registry`, `.list_acp_registry`, `.authority_status` | Provider-neutral views over configured and public discovery authorities |
| Remote lifecycle | `artifacts.intake_candidate`, `.follow`, `.fork`, `.set_publication`, `.set_license` | Candidate evidence, lineage, publication, redistribution, and takedown policy remain enforced by the remote authority |
| Retire | `artifacts.archive` | Hides the record from other readers while retaining immutable owner/admin history |

Mutation requests carry an `expected_library_version` and a bounded
`idempotency_key`. Revision-sensitive operations also carry an
`expected_revision_id`. Stale editors and conflicting replays fail closed rather
than silently overwriting current state.

The MCP App presents these actions through the `artifacts` tool. It is
a compact conversation card with an expandable, multi-file authoring view. Host
callbacks are the only data path: the app does not fetch arbitrary endpoints,
read local files, store credentials, or decide authorization.

## Visibility And Authorization

New entries are `private` by default; an authorized creator may explicitly
choose `shared`.

- Private Skills are discoverable and readable only by their owner and current
  administrators.
- Shared Skills become company-readable only while active.
- The owner and current administrators may mutate a record. Another member may
  not mutate it.
- Role and membership are evaluated from verified request identity at the final
  operation boundary. Client-supplied owner, role, tenant, email, subject, or
  `_meta` claims are never authority.
- Unauthorized reads are non-enumerating.

List and item responses expose privacy-preserving relationships and source
categories, not principal identifiers, source URLs, repository names, paths,
credentials, or file bodies. Their `allowed_actions` fields are authoritative
for the current caller and record state.

## Storage And Recovery

Managed Skill Artifacts live beneath `$LABBY_HOME/artifacts`; operator-provided
directory Skills remain a separate startup input under `$LABBY_HOME/skills`.
The Artifact store uses immutable revisions plus a durable, checksummed library
authority. Publication commits durable authority before installing the exact
candidate generation. On startup, Labby reconstructs the committed active
generation before serving Skills.

An interrupted or failed refresh leaves the last published generation readable.
Idempotent receipt replay returns the terminal outcome without applying a second
mutation. A configured but unavailable Depot or repository source cannot prevent
local create, save, activate, restart recovery, list, get, or read.

## Acquisition Paths

Repository packs and Depot are optional acquisition paths into the same local
library; neither is a runtime dependency for serving an already imported Skill.
Imports select one exact remote Artifact and revision via a named, server-owned
connection and do not implicitly activate the result.

For the concrete Depot/notification-Worker connection, request shape, bearer
placement, and address-pinning requirements, see
[Runtime Configuration](../runtime/CONFIG.md#durable-depot-skill-imports).

Operator directory Skills under `$LABBY_HOME/skills` coexist with managed
Artifacts. Active first-party names are globally unique in Labby's
`skill://labby/...` namespace; a conflicting activation has exactly one winner.

The remote control plane extends `artifacts.*` and exposes four supporting
service families:

- `sources.*` controls persisted, refreshable ingestion sources;
- `jobs.*` starts and observes durable repository, registry, MCP, marketplace,
  and archive ingestion;
- `uploads.*` creates and inspects short-lived upload slots, while raw bytes use
  the bounded authenticated `PUT /v1/uploads/{id}` route;
- `bundles.*` curates and publishes immutable Artifact collections.

These are fixed Labby actions mapped to a server-held authority. Callers cannot
select arbitrary provider operations, endpoints, headers, or credentials.
Depot's token administration, maintenance/garbage collection, sidecar repair,
and capacity benchmark operations stay authority-internal and are not projected
through Labby. Direct provider `skills.*` read/ingest methods are likewise not
forwarded: Labby uses native Agent Skills reads, local Artifact lifecycle
actions, and durable `jobs.start` ingestion instead.

One Depot connection can provide exact acquisition and the remote control plane,
but those URLs are separate contracts:

```toml
[[artifacts.sources]]
id = "primary"
kind = "depot"
endpoint = "https://depot.example/api/artifacts/exact"
control_plane_url = "https://depot.example"
pinned_addresses = ["93.184.216.34"]
bearer_token_env = "LABBY_DEPOT_TOKEN"
```

`endpoint` receives the exact-revision acquisition request used by local import.
`control_plane_url` must be a path-free public HTTPS origin; Labby appends only
its sealed operation and upload paths. Every resolved address is operator-pinned
and public, redirects are disabled, and `bearer_token_env` names a server-side
environment variable rather than containing a secret.

## Remote Metadata Boundary

Depot retains the complete authority record. Labby passes the operator-facing
subset through after redaction: stable identity, kind, descriptive fields,
revision and content digests, license and publication state, source refresh
state, job progress, upload state, bundle membership, drift, pagination, and
timestamps. On import, Labby's local Artifact Library persists the immutable
revision contents and its smaller source-provenance projection.

The provider adapter removes authorization values, bearer/access tokens,
credential fields, raw internal errors, stack traces, internal implementation
state, and operator-only notes recursively before a result reaches API, MCP, or
the WebUI. Provider errors are normalized to Labby's stable error taxonomy; raw
provider error bodies are not returned to callers. Successful provider results
are passed through after recursive sensitive-field redaction.

## Limits And Reading

Library pages default to 50 entries and accept at most 100. Cursors and
idempotency keys are bounded. History is metadata-only and newest-first; use
`artifacts.read` with an exact Artifact, revision, and manifest path for
content.

Native Skill consumers read the same immutable published snapshot.
A reader that began against generation N remains pinned to N while a refresh
publishes N+1, so a manifest and its bytes cannot be mixed across generations.

## Verification

For a release touching the library, prove each boundary independently:

- storage receipt and immutable revision identity;
- materialization and manifest digests;
- activation and committed/published library versions;
- native `skills/list`, `skills/get`, and `resources/read`;
- restart reconstruction, authorization/non-enumeration, failed-refresh
  rollback, collision handling, and source-offline local serving;
- MCP App console, network, CSP, accessibility, desktop, and mobile behavior.

Do not describe unit tests or a rendered shell alone as fresh-client or
production-equivalent proof.
