---
title: "Artifacts And Agent Skills"
created: "2026-08-26"
updated: "2026-11-26"
---

# Artifacts And Agent Skills

Labby models a Skill as one kind of durable Artifact. The product boundary has
two related surfaces:

- native SEP-2640 `skills/list` and `skills/get`, plus manifest-bound
  `resources/read`;
- an authenticated MCP App and lifecycle actions under `artifacts.*`.

There is no `skill_library.*` management alias. Durable lifecycle operations live only
under `artifacts.*`. Labby still ships the legacy compatibility service actions
`skills.list`, `skills.search`, `skills.get`, and `skills.read` for existing CLI/API
consumers; they are read-only projections over the same canonical registry and are not
a second storage or management namespace. Native `skills/list` and `skills/get` remain
the SEP-2640 protocol methods.

The native extension contract is pinned and documented in
[Skills extension](../contracts/skills-extension.md). This document owns the
Labby product lifecycle and operator behavior.

## Bundled Skills Over MCP

Labby embeds `using-labby`, `using-codemode`, and `using-snippets` in the
binary and serves them under `skill://labby/<name>/SKILL.md`. Every product
build advertises the `io.modelcontextprotocol/skills` extension. The root
route lists these skills by default over stdio `labby mcp`, the stdio bridge to
a running daemon, and HTTP `/mcp`. A protected route lists them only when its
loadout enables Skills. Operator skills in `$LABBY_HOME/skills` share the
`labby` origin and follow the same rules.

Claude Code reads skills through this extension and names each one
`<server>:<skill>`. It does not merge an MCP skill with a local skill of the
same name. The Labby plugin installs the same three skills locally and also
starts `labby mcp`. Without an opt-out, the client would load each skill twice.

`labby mcp --no-bundled-skills`, or `LABBY_MCP_NO_BUNDLED_SKILLS=1`, removes
the embedded skills from `skills/list` on that connection. `skills/get` and
`resources/read` still serve them by URI, as SEP-2640 requires. Operator,
Artifact, Depot, and upstream skills stay listed. The generated plugin MCP
configurations set the environment variable, because those plugins already
install the skills. Older binaries ignore an unknown variable, so the plugin
works with any release. Connections without the plugin, such as VML or a
hand-added `claude mcp add`, keep the default listing.

Labby cannot see what a client has installed. SEP-2640 has no capability or
identity field for "the client already has this skill", and Claude Code
documents no de-duplication. For a local copy that did not come from the
plugin, such as a file under `~/.claude/skills/using-labby`, choose one of
these client-side options:

- set `LABBY_MCP_NO_BUNDLED_SKILLS=1` in that client's Labby server entry;
- deny the MCP copy with a Claude Code permission rule such as
  `Skill(labby:using-labby)` (use the server name from `/mcp`);
- remove the local copy and use the MCP skill.

HTTP has no per-client switch. A client that installs the plugin connects
through its stdio entry, which carries the setting.

## Catalog Performance And Freshness

Native pagination reuses a bounded, immutable aggregate catalog across requests
with the same caller scope. Before reuse, Labby checks the local generation,
upstream catalog revisions and expiry, and the current exposure configuration.
Changes rebuild the aggregate and retain the existing cursor validation rules.
Incomplete upstream discovery is not admitted to this cache. Cold upstreams
use the ordinary listing attempt; cache validation never initiates network I/O.

On a cold upstream cache, `skills/get` overlaps the target request with catalog
discovery. The catalog still validates ownership and collisions before an
unlisted target can be returned. Listed entries can finish without waiting for
the speculative target request; credential or catalog invalidation rejects late
unlisted results.

The local ArtifactStore shares a bounded parsed-head cache across clones. Each
query still enumerates current heads and checks Unix file identity, change time,
modification time, size, and mode. Unchanged heads avoid repeated JSON reads,
validation, and sorting. External edits and deletions invalidate reuse; platforms
without Unix change metadata read and validate every head. Debug logs report
elapsed time, cache reuse, and parsed-head counts without record contents.

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
| Managed distribution | `artifacts.transfer_options`, `.pin`, `.follow_managed`, `.follow_update`, `.fork_personal` | Inspect transfer rights, materialize or update an authorized mirror, or create a personal fork |
| Remote discovery | `artifacts.search_remote`, `.list_remote`, `.get_remote`, `.list_candidates`, `.search_skills_sh`, `.search_ard`, `.search_marketplace`, `.list_mcp_registry`, `.list_acp_registry`, `.authority_status` | Provider-neutral views over configured and public discovery authorities |
| Remote Skill reads | `artifacts.list_remote_skills`, `.get_remote_skill`, `.load_remote_skill`, `.read_remote_skill` | Explicit reads against one configured remote authority; separate from the local and aggregated native Skills facade |
| Remote lifecycle | `artifacts.intake_candidate`, `.follow`, `.fork`, `.set_publication`, `.set_license` | Candidate evidence, lineage, publication, redistribution, and takedown policy remain enforced by the remote authority |
| Retire | `artifacts.archive` | Hides the record from other readers while retaining immutable owner/admin history |

To save an edit, send the `expected_revision_id` returned when you opened the
skill. Another tenant can update its own skills while your edit is open.
Callers that require a particular whole-library state can send
`expected_library_version` instead. When a save supplies both, both must match.
Each mutation requires a bounded `idempotency_key`. Retrying a committed
request with the same key and input returns its original receipt.

Every Artifact action, local or remote, passes one transport gate: scope,
audience, and product-credential binding to the requested Project. On a
Project-bound route the requested Project must be the route's own, and on a
Team-bound route the Team is the route's Team; `x-labby-project-id` and
`x-labby-team-id` may restate them but never redirect a call. The owned
`artifacts` and Depot publish tools also honor a loadout that disables Tools.
`artifacts.transfer_options` performs a server-credentialed exact acquisition,
so it requires the same write scope and `AssetUse` permission as
`artifacts.import`.

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

### Audit correlation

Every Skill Library authorization decision receives a server-minted request correlation
id. A client-supplied `x-request-id` is transport metadata/correlation context only; it
is never trusted as the audit deduplication identity. Retained audit-ring entries may
coalesce exact replay events, but every authorization decision is still emitted to the
trace stream. Ring evictions are counted and surfaced in tracing so audit loss is
operator-visible.

## Storage And Recovery

Managed Skill Artifacts live beneath `$LABBY_HOME/artifacts`; operator-provided
directory Skills remain a separate startup input under `$LABBY_HOME/skills`.
The Artifact store uses immutable revisions plus a durable, checksummed library
authority. Publication commits durable authority before installing the exact
candidate generation. On startup, Labby reconstructs the committed active
generation before serving Skills. During reconstruction it reports unreadable
revisions, colliding names, and entries that exceed admission limits. Other
verified records remain available under their original ownership policies.

Each tenant can activate 256 managed Skills across its owners, Teams, and
Projects. Activating a different revision of an active Skill uses the same
slot. The process also limits the entire first-party generation to 16,384
Skills, 64 MiB of content, and 131,072 resources across all tenants and
local providers.

An interrupted or failed refresh leaves the last published generation readable.
`artifacts.refresh` republishes the current durable Artifact active set through the
single activation/publication composer; it does **not** rescan `$LABBY_HOME/skills`.
Operator directory Skills are startup input, so changing them still requires a restart.
The published generation is composed from bundled Skills, the startup directory snapshot,
and active Artifact Skills, and only that composer writes the process generation cell.
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
Artifact Skill URIs derive from the name alone, so same-name Artifact records
from different owners, in any Project or tenant, collide: a mutation that would
make a second one active is refused with `active_skill_collision`, and the
first stays served under its own access policy. The refusal reveals only that
some active Skill already uses the name. If duplicates are already active (for
example from an older release), neither is served and the gateway logs the
conflicting Skill name so an operator can rename or deactivate all but one.

The remote control plane extends `artifacts.*` and exposes four supporting
HTTP-context-only service families (not MCP or in-process Code Mode tools):

- `sources.*` controls persisted, refreshable ingestion sources;
- `jobs.*` starts and observes durable repository, registry, MCP, marketplace,
  and archive ingestion;
- `uploads.*` creates and inspects short-lived upload slots, while raw bytes use
  the bounded authenticated `PUT /v1/uploads/{id}` route;
- `bundles.*` curates and publishes immutable Artifact collections.

The authenticated Administration browser surface also projects the actor-filtered
canonical Depot operation catalog, including token administration, garbage
collection, sidecar repair, migration, and other maintenance operations that
Depot advertises to the configured credential. These operations remain excluded
from model-facing MCP. Labby revalidates current browser authority, requires
`lab:admin` plus session CSRF for mutations, binds execution to the validated
catalog/intent, and Depot independently enforces its write scope and resource
policy. Callers cannot select arbitrary endpoints, headers, or credentials.
The `artifacts` service exposes explicit remote Skill reads without publishing
provider operation names. `list_remote_skills` returns canonical summaries with
an opaque continuation cursor. `get_remote_skill` accepts the canonical
`SKILL.md` URI and returns its manifest metadata and exact resource inventory.
`load_remote_skill` reads that manifest with byte-offset continuation, while
`read_remote_skill` reads a file or lists one direct-child directory page with
an opaque directory cursor. The caller may select `connection_id`; omission is
valid only when exactly one Artifact authority is configured. URI and cursor
values are authority-owned opaque identities and must be passed back unchanged.

These actions use the same direct HTTPS authority client as the rest of the
remote Artifact control plane. Labby verifies the operation schema fingerprint,
revalidates project authority after connection admission, delegates only
`skills:read`, and redacts provider security metadata. They do not connect to a
Depot MCP server and they do not change the semantics of native `skills/list`,
`skills/get`, or local `artifacts.*` reads. Durable remote ingestion continues
through `jobs.start`.

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
