---
title: "Skills And Skill Library"
created: "2026-08-26"
updated: "2026-08-26"
---

# Skills And Skill Library

The `skills` service combines three related surfaces:

- native SEP-2640 `skills/list` and `skills/get`, plus manifest-bound
  `resources/read`;
- the read-only compatibility actions `skills.list`, `skills.get`, and
  `skills.read`;
- an authenticated MCP App and management actions under `skill_library.*`.

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
destructive catalog operation, but retains immutable revision storage for the
owner or an administrator.

Every successful mutation returns the committed and published library versions,
generation facts, the library digest, and explicit re-list guidance. Labby does
not emit a Skills-specific `list_changed` notification; clients re-run
`skills.list` or native `skills/list`.

## Library Actions

The canonical action catalog is generated from code. The lifecycle groups are:

| Group | Actions | Contract |
| --- | --- | --- |
| Browse | `skill_library.list`, `.get`, `.history`, `.read` | Versioned, bounded, caller-filtered metadata; only `.read` returns one manifest-bound file body |
| Author | `skill_library.validate`, `.create`, `.save` | Logical UTF-8 files; validation is side-effect free; create/save do not activate |
| Publish | `skill_library.activate`, `.deactivate`, `.rollback`, `.refresh` | Exact revision and optimistic library-version preconditions; publication is atomic |
| Acquire | `skill_library.import` | Exact immutable Depot or repository selector through a server-configured connection; no caller-supplied endpoint, path, bytes, or credential |
| Retire | `skill_library.archive` | Hides the record from other readers while retaining immutable owner/admin history |

Mutation requests carry an `expected_library_version` and a bounded
`idempotency_key`. Revision-sensitive operations also carry an
`expected_revision_id`. Stale editors and conflicting replays fail closed rather
than silently overwriting current state.

The MCP App presents the same actions through the existing `skills` tool. It is
a compact conversation card with an expandable, multi-file authoring view. Host
callbacks are the only data path: the app does not fetch arbitrary endpoints,
read local files, store credentials, or decide authorization.

## Visibility And Authorization

New entries are `private` by default for backward compatibility; an authorized
creator may explicitly choose `shared`.

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

## Limits And Reading

Library pages default to 50 entries and accept at most 100. Cursors and
idempotency keys are bounded. History is metadata-only and newest-first; use
`skill_library.read` with an exact Artifact, revision, and manifest path for
content.

Native and compatibility consumers read the same immutable published snapshot.
A reader that began against generation N remains pinned to N while a refresh
publishes N+1, so a manifest and its bytes cannot be mixed across generations.

## Verification

For a release touching the library, prove each boundary independently:

- storage receipt and immutable revision identity;
- materialization and manifest digests;
- activation and committed/published library versions;
- native `skills/list`, `skills/get`, and `resources/read`;
- compatibility `skills.list`, `skills.get`, and `skills.read`;
- restart reconstruction, authorization/non-enumeration, failed-refresh
  rollback, collision handling, and source-offline local serving;
- MCP App console, network, CSP, accessibility, desktop, and mobile behavior.

Do not describe unit tests or a rendered shell alone as fresh-client or
production-equivalent proof.
