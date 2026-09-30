---
title: "File Stash"
created: "2026-09-05"
updated: "2026-09-30"
---

# File Stash

File Stash is Labby's built-in service for personal- or Team-scoped arbitrary
files owned by an authenticated principal or one explicitly selected Team. Labby owns the local metadata and
blob lifecycle, so this capability meets the built-in-service exception. Depot
is not a dependency and an explicitly configured remote target never falls back
to File Stash.

This document is the normative File Stash contract. File Stash is registered on Linux
and is available through authenticated HTTP, generic service
dispatch, MCP resources, and the web UI. Unsupported platforms omit the service
rather than advertising handlers that cannot honor its filesystem contract.

## Boundary and non-goals

File Stash stores principal-owned files organized in virtual folders. It is not the retired Agent
Artifact Manager. V1 has no components, revisions, workspaces, component kinds,
providers, push/pull, deployment targets, Marketplace forks, drift detection,
directory import/export, or implicit synchronization. The archived Stash docs
are historical evidence only and must not be used as an implementation pattern.

## Identity and isolation

Network and stdio operations consume the middleware-derived canonical
`VerifiedIdentity` and ask the existing AccessStore to resolve its
`PrincipalLink` to the durable AccessStore `PrincipalId`. File Stash does not
derive, hash, cache, or migrate a parallel principal namespace. The private
in-process peer is the sole exception: its host-controlled metadata may carry a
pre-resolved `access_principal_id`, which AccessStore must confirm is still
active before Stash authorization. Network and stdio transports never trust a
serialized principal ID. MCP must preserve caller
authorization through `resolve_caller_authorization`; it must never fall back to
trusted-local identity. Static bearer, Unix-peer, and trusted-local stdio
credentials work only when their stable local `PrincipalLink` is explicitly
mapped to an active service/bootstrap Principal in AccessStore. Missing,
ambiguous, inactive, or unavailable resolution fails closed before any filename,
object, grant, quota, or recipient lookup. Observability actor keys are not
authorization identities.

Personal is the compatibility default and retains the historical durable
principal key. Team operations require explicit `owner_kind=team` plus an
opaque Team ID (HTTP uses the corresponding `X-Labby-Owner-*` headers; browser
downloads use same-origin query parameters because links cannot attach
headers). Labby resolves current Team capability before lookup, and mutations
and opened downloads re-observe authority at their final boundary. Team members
may read Team files; Team administrators may manage them according to the fixed
role templates. Removing membership blocks new opens without affecting the
caller's personal files. Durable Team keys are type-prefixed, so no principal
or Team identifier can alias another owner's quota or objects. The
per-surface selector list, including the MCP `_meta` owner selection, is in
[Selecting the authority context](../access-control/MULTI_USER_AUTHORITY.md#selecting-the-authority-context).

Grant recipients are selected by a validated opaque AccessStore `PrincipalId`
from an authoritative, non-enumerating identity-selection surface, never by
email or display name. The owner and grantee IDs are in the same AccessStore
namespace. V1 rejects self-grants and duplicate active grants. Credential
rotation that keeps the same resolved Principal preserves access; a link change,
issuer change, or deleted/deactivated recipient denies access and never silently
retargets a grant.

## Object and filename contract

Each upload receives a random opaque file ID. The canonical reference is:

```text
stash://me/files/{opaque_file_id}
```

`me` is a fixed authorization-context authority label, not an ownership
namespace embedded in the URI. The opaque ID selects one object; each read then
authorizes the resolved caller as its owner or an active grantee. The exact same
URI therefore works for owner and grantee. It stays bound to that one object
until deletion; rename and move preserve it, and a later upload never reuses or retargets it.
Owner and shared files with equal display names remain unambiguous.

MCP clients select a Team-owned resource view per request with the
`ai.dinglebear.labby/stashOwner` request `_meta` object, for example
`{"kind":"team","id":"team-id"}`. Omitting it (or selecting
`{"kind":"personal"}`) uses the caller's Personal Stash. This is only a
scope selector: Labby resolves the verified identity and rechecks current Team
membership and read capability before listing or opening any resource.

The client filename is display metadata only and never becomes a storage path.
For multipart input, take only the final path component after splitting the raw
filename on both `/` and `\\`, in that order before Unicode normalization; this
turns browser values such as `C:\\fakepath\\report.pdf` into `report.pdf`.
Normalize that component to Unicode NFC, then reject an empty value, `.`/`..`,
controls, remaining separators, NUL, invalid UTF-8, or more than 255 UTF-8 bytes.
An owner cannot have two live files with the same case-folded normalized name
in the same folder. Pending uploads also reserve that name. The same filename
may exist in different folders. A collision returns `conflict`; saves, uploads,
renames, and moves never overwrite.

## Context documents and repo folders

Use `stash.save_text` to save exact UTF-8 Markdown or plain text from a ChatGPT
conversation. It returns file metadata and the stable MCP resource URI. Documents
are private in the caller's Personal Stash by default; an explicit Team selector
or grant uses the same existing Stash authorization rules.

```json
{
  "action": "stash.save_text",
  "params": {
    "filename": "handoff.md",
    "folder": "dinglebear-ai/labby",
    "format": "markdown",
    "content": "# Handoff\nDecisions, requirements, and next steps.\n"
  }
}
```

Pass the returned `stash://me/files/{id}` URI to Codex or another agent using the
same Labby identity. MCP `resources/read` returns saved Markdown as native text
with `text/markdown`, or plain text with `text/plain`. `stash.read_text` is a tool
fallback: it returns at most 16 KiB of UTF-8 content plus `next_cursor`; continue
with the same URI and that cursor until it is null. Every continuation rechecks
file access. Ordinary uploaded files retain binary MCP resources regardless of
filename extension; saving text explicitly selects text resource behavior.

Documents are capped at 512 KiB and also consume ordinary Stash quota. The save
result contains metadata, not an echo of the content. To save a revision, choose
a new filename or folder: Stash has no document version history or overwrite.
Content remains untrusted passive context, not executable instructions or a
rendered inline preview. If a request is interrupted, list/search before retrying;
a save may have completed even when the client did not receive its result.

`folder` is optional display metadata. Omit it when saving to use Unfiled (`""`).
Use repo names such as `dinglebear-ai/labby`, or arbitrary folders such as
`research`. Slash-separated segments support organization without creating host
directories. Paths normalize to Unicode NFC, are case-sensitive, and reject
controls, backslashes, empty segments, `.`/`..`, or more than 1,024 UTF-8 bytes
(255 bytes per segment). A folder does not change ownership, grants, or the URI.
Empty folders are not persisted; `stash.folders` lists only folders with visible
files and their file counts, with a bounded continuation cursor.

`stash.list` and `stash.search` accept an exact `folder` filter. Omission searches
all visible folders; `""` selects only Unfiled. `stash.move` accepts `file_id` and a
destination `folder`, including `""`. The web UI offers folder selection, uploads
into the selected folder, and moving owned files through their Manage dialog.
HTTP listing accepts `?folder=...`; folder enumeration is `GET /v1/stash/folders`.
Binary HTTP uploads accept the percent-encoded `X-Labby-Stash-Folder` header.

Labby does not infer a current local repository from a ChatGPT connection and
does not write into a Git checkout. An agent that knows its checkout can identify
the repository from its Git remote and pass that folder when saving/searching;
ChatGPT can use a repository or folder named in the conversation. Without that
context, use Unfiled and move the document later. Folder organization and saved
content survive a Labby restart. The schema v3 migration retains existing files,
reservations, quotas, and grants as Unfiled binary objects; older binaries cannot
open the upgraded database, so rollback requires a matching backup.

### Upgrading existing Stash state

Before upgrading, block client traffic, let uploads finish, and stop the owning
Labby service. Check that `pending_uploads` is empty, then copy the **entire**
Stash root with ownership and permissions preserved: `metadata.sqlite3` and any
WAL/SHM files, `blobs/`, `tmp/`, and `snapshot-id`. Retain the previous binary.
Copying the database alone does not provide a restorable Stash snapshot.

Start the qualified new binary while client traffic remains blocked and wait
for Stash readiness. Run the following read-only comparison, replacing both
absolute paths with the upgraded and preserved roots. This procedure requires
zero pending uploads so restart recovery cannot change the baseline. Every
difference count must be zero; version must be `3`, integrity must be `ok`, and
the foreign-key check must return no rows.

```sh
sqlite3 -bail -readonly /absolute/stash/metadata.sqlite3 <<'SQL'
ATTACH DATABASE 'file:/absolute/backup/metadata.sqlite3?mode=ro' AS prior;
BEGIN;
PRAGMA user_version;
PRAGMA integrity_check;
PRAGMA foreign_key_check;
SELECT COUNT(*) AS prior_pending FROM prior.pending_uploads;
SELECT COUNT(*) AS current_pending FROM main.pending_uploads;
SELECT COUNT(*) AS snapshot_difference FROM main.stash_metadata m
JOIN prior.stash_metadata p USING(singleton) WHERE m.snapshot_id <> p.snapshot_id;
SELECT COUNT(*) AS missing_or_changed_files FROM (
  SELECT file_id,owner_principal_id,display_name,collision_key,size_bytes,
         blob_key,ready,created_at,updated_at FROM prior.files
  EXCEPT
  SELECT file_id,owner_principal_id,display_name,collision_key,size_bytes,
         blob_key,ready,created_at,updated_at FROM main.files
);
SELECT (SELECT COUNT(*) FROM main.files) -
       (SELECT COUNT(*) FROM prior.files) AS file_count_difference;
SELECT COUNT(*) AS incorrect_legacy_defaults FROM main.files f
JOIN prior.files p USING(file_id)
WHERE f.folder <> '' OR f.content_type <> 'application/octet-stream';
SELECT COUNT(*) AS missing_grants FROM (
  SELECT * FROM prior.grants EXCEPT SELECT * FROM main.grants
);
SELECT COUNT(*) AS added_grants FROM (
  SELECT * FROM main.grants EXCEPT SELECT * FROM prior.grants
);
SELECT COUNT(*) AS missing_claims FROM (
  SELECT * FROM prior.name_claims EXCEPT SELECT * FROM main.name_claims
);
SELECT COUNT(*) AS added_claims FROM (
  SELECT * FROM main.name_claims EXCEPT SELECT * FROM prior.name_claims
);
-- Compute expected counters from files: schema v1 had no usage tables.
SELECT COUNT(*) AS missing_owner_usage FROM (
  SELECT owner_principal_id FROM prior.files WHERE ready=1
  EXCEPT SELECT owner_principal_id FROM main.stash_usage
);
SELECT COUNT(*) AS incorrect_owner_usage FROM main.stash_usage u
WHERE u.committed_bytes <> COALESCE((SELECT SUM(f.size_bytes) FROM prior.files f
  WHERE f.ready=1 AND f.owner_principal_id=u.owner_principal_id),0)
OR u.live_files <> (SELECT COUNT(*) FROM prior.files f
  WHERE f.ready=1 AND f.owner_principal_id=u.owner_principal_id)
OR u.reserved_bytes <> 0 OR u.pending_files <> 0;
SELECT ABS(1-COUNT(*)) AS missing_instance_usage FROM main.stash_instance_usage;
SELECT COUNT(*) AS incorrect_instance_usage FROM main.stash_instance_usage
WHERE committed_bytes <> COALESCE((SELECT SUM(size_bytes) FROM prior.files WHERE ready=1),0)
OR live_files <> (SELECT COUNT(*) FROM prior.files WHERE ready=1)
OR reserved_bytes <> 0 OR pending_files <> 0;
ROLLBACK;
SQL
cmp /absolute/stash/snapshot-id /absolute/backup/snapshot-id
```

Verify authenticated listing and an existing binary resource before reopening
client traffic. If verification fails, stop the new service, preserve the
upgraded root separately, restore the complete matching backup, restore the
previous binary, then start it and verify readiness. Never point an older
binary at v3 state. Once client writes resume, restoring the pre-upgrade backup
discards subsequent changes; preserve the upgraded state for recovery.

## Operations and metadata

Stash supports upload, text save/read, folder enumeration and moves, list/search,
metadata read, download, delete, grant create, grant list, and grant revoke. Lists are cursor-paginated in stable
`created_at DESC, file_id DESC` order. Search applies a bounded case-insensitive
substring filter over normalized display names on each returned page; clients
continue with the ordinary page cursor to search later pages. The stats response defines:

- `owned_file_count`: committed, non-deleted files owned by the caller;
- `owned_shared_file_count`: those owned files with at least one currently
  active, non-expired grant (one file counts once regardless of grant count);
- `owned_committed_bytes`: persisted bytes of committed, non-deleted owned files;
- `owned_reserved_bytes`: declared bytes held by the caller's pending uploads.

The mock's **Shared** summary card is `owned_shared_file_count`, not grant count
or files shared with the caller. All stats come from one authoritative snapshot,
not UI aggregation.

Rename, move, delete, grant creation, and grant revocation stage their metadata
inside an unpublished transaction and revalidate the caller's selected owner
authority immediately before commit. Rejection or an authority-check timeout
rolls back metadata, name claims, grants, and quota changes. The owned operation
finishes that check even if its requesting transport disconnects. Authority
validation spans separate Access and Stash stores; this is not a cross-database
transaction.

Delete is destructive and atomically removes the file metadata row and its
cascade-owned grants in one metadata transaction. Delete and revoke prevent all new opens immediately.
Authorization is snapshot-on-open: a stream whose authorized regular-file handle
was already opened may finish, while later opens fail. Physical reclamation may
be asynchronous, but a deleted object never becomes newly readable. Upload and
grant creation mutate state but are not destructive. Shared action metadata is the only source for
`requires_admin` and `destructive`; surface adapters must not reclassify them.

Grants are explicit, read-only, and bind one file to one grantee AccessStore
`PrincipalId`. Revocation is effective for the next open, including an already
discovered URI; it does not abort a stream opened under an earlier valid
snapshot. V1 does not expire grants, mint bearer share links, or permit
re-sharing.

## Limits and backpressure

Defaults are intentionally conservative for a local operator service:

| Limit | Default | Configurable maximum |
| --- | ---: | ---: |
| file bytes | 100 MiB | 1 GiB |
| principal committed plus reserved bytes | 1 GiB | 100 GiB |
| instance committed plus reserved bytes | 10 GiB | 1 TiB |
| live files per principal | 1,000 | 100,000 |
| live files per instance | 100,000 | 1,000,000 |
| list page | 50 | 200 |
| search query | 128 UTF-8 bytes | 1,024 bytes |
| request-header bytes | 16 KiB | 64 KiB |
| grant recipients returned per page | 50 | 200 |
| MCP resource read | 10 MiB | 25 MiB |

The runtime also bounds concurrent uploads per principal (2) and instance (8),
instance downloads/disk reads (16), MCP resource reads (4), and database blocking
work (one 64-entry queue). Defaults may be lowered. Implementations must provide
bounded maxima for every exposed override and reject invalid startup config.

Uploads require exactly one valid `Content-Length`, reject any unsupported
`Transfer-Encoding`, and require absent or `identity` `Content-Encoding`.
Absent, malformed, duplicate, understated, overstated, or body-mismatched lengths
are rejected. The runtime reserves exactly the declared bytes transactionally,
checks the incremental byte count while reading, and compares the final persisted
byte count exactly to `Content-Length` before publication. Upload and download
idle and total deadlines default to 30 seconds and 10 minutes, respectively.
Pending reservations expire after 30 minutes. A
bounded janitor processes at most 100 expired items per pass with exponential
backoff capped at five minutes; the cap cannot be shorter than the normal
janitor interval. Permit or queue saturation returns a retryable
`busy` error; quota exhaustion returns `quota_exceeded` and is not retried until
state or limits change.

## Storage, durability, and content safety

Metadata is durable SQLite state under Labby's configured state root. Blobs use
opaque storage names beneath a dedicated root. Startup validates the root and
every existing ancestor without following links, rejects symlinks/reparse points
and insecure ownership or permissions, and creates missing descendants with
owner-only permissions. All create/read/delete/reconcile operations are
descriptor-relative beneath the validated root and no-follow. They accept only
regular-file handles, create temporary and final blob names exclusively, and
reject links, reparse points, devices, sockets, FIFOs, or other special files at
every lifecycle stage.

SQLite and the filesystem cannot commit atomically. Upload therefore uses an
explicit crash-consistent state machine: (1) commit `pending` metadata and quota
reservation; (2) stream to an exclusively created temp file, verify the exact
length, fsync the file, exclusively publish the opaque blob, then fsync the blob
directory; (3) commit metadata as `committed`; and (4) release the reservation.
Only `committed` rows with a verified regular blob are readable. Restart recovery
reconciles pending publication state before readiness: it completes or rolls back
pending rows, releases orphaned reservations, and fails closed on ambiguous state.
After readiness, a cancellation-aware background scrub checks committed blobs and
removes unreferenced temp/blob files in bounded batches. Reads independently verify
the selected blob before opening it, so a database/blob mismatch remains an
`integrity_error`, never an empty or missing file, while the scrub is in progress.

Startup opens and validates the root and database, then performs crash-critical
pending-upload reconciliation in a background lifecycle task. Readiness and
every Stash operation fail closed with a recovering/unavailable state until
that pass finishes. Its cursor is durably checkpointed after each bounded batch,
so an interrupted process resumes without replaying completed entries. Once the
runtime becomes ready, committed-blob verification and orphan hygiene continue
as non-critical background work; opened blobs still verify their expected size.
Synchronous descriptor-relative filesystem batches run on the blocking pool.
A recovery error changes the runtime to a blocked state and prevents the janitor
from starting.

SQLite mutations retain one serialized transactional writer. Query-only work is
distributed across a bounded four-connection read pool, with shared queue and
deadline admission, so unrelated reads can overlap without weakening write
ordering or overload behavior.

File content is untrusted and plaintext at rest in v1. Operators must include
both database and blob directories in a consistent backup and restrict host
access; restore must preserve their shared generation. Encryption at rest,
malware scanning, content preview/rendering, deduplication, version history, and
remote replication are deferred. HTTP downloads use attachment disposition with
an escaped ASCII-safe quoted `filename` fallback plus an RFC 5987 `filename*`,
`X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, a Content Security
Policy of `default-src 'none'; sandbox`, private no-store caching, and a generic
binary media type. Header construction replaces control characters, quotes, and
path separators that cannot be represented safely. The web UI never executes,
frames, or previews uploaded content inline.

## Surfaces and errors

File Stash is default `gateway-host` functionality on supported platforms with
authenticated HTTP, generic service action metadata/tool dispatch, MCP resource
reads, and the web UI. It has no bespoke clap tree. Feature slices agree: a
surface is absent when its owning runtime is not compiled or available, rather
than advertising a handler that fails later.

The v1 durable runtime is available only where Labby can anchor SQLite and blob
operations to verified directory handles. Linux is the currently qualified
target. Android, macOS, Windows, and other unsupported targets fail initialization
closed and must not register or advertise File Stash until a sanctioned
handle-relative implementation exists; the rest of `gateway-host` remains
available.

Every `/v1/stash/*` route remains behind the ordinary `/v1` authentication,
host/origin, and authorization middleware. Cookie-authenticated mutations also
require the shared CSRF validation. No loopback, multipart, download, or MCP
adapter bypass is permitted. HTTP streams large bodies without JSON/base64 wrapping. MCP resource reads apply
the lower MCP ceiling and return `quota_exceeded` when the object is too large;
clients use HTTP download for larger files. Stable agent error kinds are
`invalid_param`, `not_found`, `conflict`, `quota_exceeded`, `busy`,
`service_unavailable`, and `integrity_error`. Authorization failures and probes
of another principal's object or grant use the same non-enumerating `not_found`
shape. Authentication failures occur before service dispatch. Every error keeps
the shared agent error envelope and HTTP mapping; raw paths, identity material,
filenames, authorization values, and file content are excluded from logs.
