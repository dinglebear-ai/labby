---
title: "Multi-user ownership migration and recovery"
created: "2026-09-05"
updated: "2026-09-07"
status: "design"
---

# Multi-user ownership migration and recovery

This runbook freezes the rehearsal and activation contract for moving an
existing Labby AccessStore installation (schema v1 through v6; v5 is the last
released schema) to the multi-user ownership schema v7. The rehearsal proves
that the input can be inventoried, backed up, reopened, restored, and
deterministically classified before the ownership migration is allowed to
exist.

## Activation boundary

The point of no return is the first successful scoped write after multi-user
enforcement is enabled. Before that boundary, rollback restores the complete
pre-migration checkpoint. After it, rollback means forward repair or restoring
the complete checkpoint and losing all later writes; an old binary must never
open the new store and reinterpret scoped rows as globally owned.

Ordinary startup never crosses a schema boundary implicitly: every legacy
version (v1 through v6) is refused with `MigrationApprovalRequired` until the
operator sets `LABBY_ACCESS_MIGRATION_EVIDENCE` to the owner-controlled
approval document described in [ENV.md](../runtime/ENV.md). Bootstrap of a
never-initialized legacy store is gated the same way. Labby binds that
document to an independent checkpoint, the exact source/target schema pair, a
durable operation ID, and an explicit activation decision before opening an
exclusive migration transaction.

Checkpoint verification is logical, not byte-identical. The checkpoint file's
digest is streamed and must match the document; the live store is then
compared with the checkpoint by schema manifest plus every table's
primary-key-ordered content digest. A WAL-mode source whose committed frames
have not been checkpointed into its main file therefore still verifies
against its consolidated `VACUUM INTO` / backup-API checkpoint. A prepared
sidecar marker makes an interrupted attempt replay only with the same
evidence; after the migration transaction commits, the complete marker is
published, and a marker write failure is logged rather than failing the
already-durable open.

The migration is split into separately observable phases:

1. `inventory`: open v5 read-only, run quick/FK/schema/bootstrap validation,
   and produce the signed inventory report;
2. `checkpoint`: stop writers, checkpoint WAL, create an SQLite-consistent
   backup, copy required sidecars/keys/configuration, and hash the restore set;
3. `expand`: create only the next-version structures in one exclusive
   transaction;
4. `classify`: attach exactly one typed owner to every durable resource;
5. `verify`: compare counts, stable IDs, logical content digests, references,
   quarantine counts, and audit/outbox facts;
6. `reopen`: close and reopen twice using the new binary, repeating integrity
   and inventory verification each time;
7. `shadow`: compare old and new authorization decisions without enforcing;
8. `activate`: persist the enforcement generation and emit its audit/outbox
   event in one transaction; and
9. `contract`: remove obsolete compatibility fields only in a later release.

Every phase has a durable operation ID and completion marker. Repeating a
completed phase verifies its recorded input/output digests and returns the
prior result. A changed replay fails closed.

## Offline operator activation

`labby state migrate-access` is the installation-owner entry point. Stop the
Labby daemon before running it. The command acquires the same installation
lifecycle lock as the daemon and refuses to run while that lock is held.
It requires an existing, initialized AccessStore; it does not bootstrap an
owner or issue credentials.

First rehearse against an independent, SQLite-consistent copy of the source
store in an owner-only installation directory. Select that directory with
`LABBY_HOME`. Create the approval document described in
[ENV.md](../runtime/ENV.md), bound to a separate checkpoint of that rehearsal
store. Do not point rehearsal approval at the live installation. Run:

```sh
LABBY_HOME=/absolute/rehearsal-installation \
LABBY_ACCESS_MIGRATION_EVIDENCE=/absolute/rehearsal-approval.json \
labby --json state migrate-access
```

The command applies the existing approval and logical checkpoint checks,
migrates through the existing transaction, then closes and opens the store
twice. Its result reports `schema_version` and `verified_reopens`. Repeating
the command on a valid current-schema store verifies the reopens without a
new schema migration. An error after the migration transaction commits does
not imply rollback; preserve the checkpoint and inspect the store before
retrying.

After reviewing the rehearsal evidence, prepare a separate approval bound to
the quiesced live source and its independent checkpoint. With the daemon
still stopped, run the same command using the live installation's `LABBY_HOME`
and its approval document. Keep the checkpoint and approval until the
post-migration authorization and inventory checks in this runbook pass.
Ordinary startup continues to refuse legacy stores; setting the evidence
variable alone does not activate migration through the daemon.

## Production-shaped v5 inventory

The rehearsal fixture is non-empty and includes:

- the canonical bootstrap Organization, Principal, identity link, default
  Project, owner membership, and audit record when bootstrap generation is one;
- multiple active and inactive Principals with unique verified identity links;
- multiple Projects, all four Project roles, optional Loadout mappings, and
  audit rows;
- current credential/proof, tombstone, policy-publication, admission, and
  security tables, including empty-table counts where emptiness is meaningful;
- WAL mode with committed rows still represented through the WAL checkpoint
  path; and
- IDs and text containing realistic maximum-safe lengths and Unicode.

The report records the exact v5 application ID, user version, schema
fingerprint, bootstrap generation, global revision, table/index manifest
digest, table row counts, primary-key set digest per table, logical content
digest per table, `quick_check`, `foreign_key_check`, and source file/sidecar
digests. File-byte equality is not expected after a valid SQLite migration;
logical digests are canonical encodings ordered by primary key.

`scripts/ci/validate-multi-user-migration-rehearsal.py` is an
**operator-run** tool: it generates and verifies a provenance-bound rehearsal
manifest for stores the operator supplies, and CI runs only its unit tests
against synthetic databases. It does not itself execute a migration. The
executable migration evidence CI does run is the Rust
`access::migrations` test module in full: production-shaped v4 and v5 fixtures
are migrated, reopened twice, and restored from their checkpoints; every
legacy version is proven to refuse without evidence; and the approval gate is
exercised end to end on a WAL-diverged v5 file, including the checkpoint
mismatch refusal.

## Ownership classification

The only automatic owner seed is the canonical verified bootstrap Principal.
Existing private user material becomes that Principal's Personal scope. A
migrated store that was never bootstrapped receives no platform administrator,
no Team, and no Team-Project assignment; pre-existing direct Project
memberships survive unchanged and receive authority epoch 1.
For a store with bootstrap generation one, the existing migration grants
PlatformAdministrator authority to the canonical bootstrap Principal, creates
one `Initial Team`, and records that same Principal as its owner. It adds two
audit rows for these authority seeds. It preserves the original Organization,
Principal, Project, identity-link, and direct-membership IDs; it does not
replace the owner, issue credentials, or assign existing Projects to the new
Team. Bootstrap generation remains unchanged.

Installation configuration, host filesystem operations, raw logs, recovery,
and provider credentials become Installation-owned. Email, display name,
namespace, directory name, creator string, and OAuth scope are never used to
infer a Team or PlatformAdmin.

Rows whose owner cannot be proven, whose identifiers collide after canonical
normalization, or whose references disagree are copied without mutation to a
PlatformAdmin-only quarantine ledger. The ledger records source store/table,
stable source key, safe digest, reason code, discovery phase, and resolution
state. It does not contain secret material. Quarantine is never included in
ordinary listing or compatibility-owner fallback.

Activation requires:

- every source row accounted for as classified or quarantined;
- zero duplicate target owner rows;
- zero dangling references or unclassified usable resources;
- unchanged stable IDs and logical content digests for non-policy payloads;
- exactly one PlatformAdministrator derived from the canonical bootstrap
  Principal; and
- independently reproducible before/after reports.

## Failure injection

Before production activation, a separate executable rehearsal must interrupt
every phase and inject the following failures. This proof is not provided by
`validate-multi-user-migration-rehearsal.py`:

- busy/locked, read-only, disk-full, I/O, corrupt/not-a-database, and truncated
  SQLite inputs;
- invalid application ID, user version, schema fingerprint/manifest,
  bootstrap shape, and foreign keys;
- process termination with non-empty WAL/SHM;
- duplicate IDs, canonical-name collisions, dangling owner candidates, and
  invalid UTF-8 at external inventory boundaries;
- missing/truncated backup members and mismatched restore-set generations; and
- failure to append audit/outbox/quarantine records.

Before activation, each failure leaves the source v5 store byte/logically
recoverable and the enforcement generation unchanged. Transactional failures
must leave the schema version and logical inventory unchanged. After restart,
the reconciler either resumes the exact operation or rejects changed inputs; it
does not skip ahead.

## Backup and restore

The operator blocks writers and records a maintenance lease before backup.
SQLite backup uses its online backup API or `VACUUM INTO` from a validated
connection after an explicit WAL checkpoint. Copying `access.db` alone while a
WAL may contain committed state is invalid.

The restore set contains:

- AccessStore database and required WAL/SHM state or a verified consolidated
  SQLite backup;
- schema and capability registry generations;
- signing private/public keys and active/overlap key generations;
- authority outbox head, Depot acknowledged watermark, and snapshot digest;
- bootstrap and enforcement generations; and
- configuration that selects standalone/managed authority mode.

Restore occurs into a new owner-only directory. The operator verifies every
manifest digest before atomically selecting it, starts without traffic, opens
and validates twice, verifies counts/digests/quarantine, and only then removes
the maintenance lease. A partial or cross-generation restore enters
recovery-required mode.

## Old-binary proof

The checkpoint retains an exact v5 database that the current v5 reader opens
and validates after restore. The future ownership database advertises a newer
user version and fingerprint; v5 must return `UnsupportedSchema` without
mutation. This is the rollback proof: restore the v5 checkpoint first, then run
the old binary. Never point the old binary at a post-activation database.

## Evidence retained

Release evidence includes source and target binary commits/digests, fixture
seed, phase operation IDs, start/end times, pre/post reports, checkpoint and
restore-set digests, injected-failure matrix, reopen results, quarantine
inventory, shadow-decision differences, and the activation audit/outbox event.
Secrets and raw identity assertions are excluded.
