---
title: "Artifact Distribution and Personal Labby Sync"
created: "2026-08-22"
updated: "2026-08-22"
status: "design"
---

# Artifact Distribution and Personal Labby Sync

## Purpose

Any Artifact a user can legitimately access should be able to participate in a consistent Labby distribution flow when policy and license permit it.

The primary UX is Add to My Labby or Send to. The implementation must preserve the difference between remote use, managed local materialization, following upstream changes, creating an independent fork, detached export, and resharing.

Availability is not ownership. Use permission is not copy permission.

This is a dependent future milestone, not part of the first Project-bound MCP enforcement release. Implementation cannot begin until canonical identity, the authorization kernel, application-owned ArtifactStore wiring, and the cross-store recovery state machine are complete.

## Protocol freeze gate

Before remote transfer code lands, a versioned contract MUST freeze source/destination cryptographic identity and key rotation; authenticated pull-versus-push direction; endpoint/address revalidation including DNS rebinding, private/link-local/Unix targets and redirects; one-time exact-revision capability fields; atomic nonce consumption; destination idempotency; expiry/skew; payload/decompression/time/concurrency bounds; and crash/timeout reconciliation.

The preferred threat-minimal starting point is destination-authenticated pull using a capability bound to source instance, destination public key, actor, mode, exact digest/length, policy revision, nonce, issuance, and expiry. Conceptual redirect/replay tests are not sufficient until this protocol is frozen.

## Relationship to ArtifactInterchange v1

ArtifactInterchange v1 remains the content/metadata interchange contract. This document does not add ACL, recipient, bearer token, destination, or revocation fields to that envelope.

Transfer consists of two planes:

1. Artifact content: exact ArtifactInterchange v1 revision plus verified component bytes using the existing acquisition/validation rules.
2. Access/control: separate Labby authorization stating who requested what transfer mode, for which exact revision, to which paired destination, under which current policy decision.

The receiving Labby validates Artifact content independently. A source authorization decision cannot make invalid bytes valid.

## User-visible transfer modes

### Use remotely

No Artifact bytes are installed into the personal Artifact store.

The user's workspace contains a remote/reference capability to the organization/project source. Each use re-enters normal source authorization.

Revocation naturally stops future use because the source remains authoritative.

### Pin to My Labby

Pin creates a managed mirror of one exact immutable Artifact revision.

Properties:

- exact revision is recorded;
- source Organization/scope and Artifact identity are retained;
- provenance/license/publication/lineage remain intact;
- local bytes are content-verified;
- no automatic revision advance occurs;
- source remains authoritative for whether the managed mirror may continue to be used/synced under managed policy.

Pin requires artifact.sync plus all source/license/destination checks.

### Follow in My Labby

Follow is a managed mirror plus a subscription to observe eligible upstream revisions.

Following is intent, not standing permission to obtain arbitrary future bytes. Every observation/apply cycle revalidates membership, artifact.follow/artifact.sync, source distribution policy, license/takedown state, and destination eligibility.

The existing ArtifactLineage following flag remains intent/evidence. It does not by itself authorize or apply an update.

Update policy may be:

- notify: observe and present a deterministic update plan;
- auto_approved: an explicit user/admin subscription policy pre-authorizes applying updates that still pass all current checks;
- pinned: observe nothing/apply nothing until the user changes mode.

An auto-approved update is not a silent consequence of ArtifactLineage. It is an explicit higher-level managed-subscription policy and MUST create audit evidence for every applied revision.

### Fork to Personal

Fork creates a new personal Artifact identity from one exact permitted source revision.

Properties:

- new Artifact ID/namespace ownership;
- exact source bytes are copied and verified;
- ArtifactLineage records forked-from Artifact/revision;
- future source updates do not mutate the fork;
- the fork may diverge through normal local Artifact revisions;
- source access revocation does not magically erase a legitimately detached fork;
- license/takedown/redistribution obligations remain applicable according to the Artifact contract and law/policy.

Fork requires artifact.fork and source license/publisher policy that permits forking.

### Export

Export moves Artifact bytes outside Labby's managed source/destination relationship.

It uses the existing safe export path and requires artifact.export plus license/publisher permission. Secret-detection and path/executable safety rules from the Artifact subsystem remain mandatory.

Export is the clearest point where source Labby loses technical control of a copy. UI and audit should communicate that distinction.

### Reshare

Reshare grants or transfers availability to another eligible Subject/scope/destination without claiming new authorship.

Reshare requires artifact.reshare and must respect the source's maximum distribution boundary. A recipient cannot reshare merely because the source allowed the recipient to use or sync.

## Add to My Labby UX

When a user selects an Artifact, Labby computes TransferOptions for the selected destination.

Example options may include:

- Use remotely;
- Pin exact revision;
- Follow approved updates;
- Fork to Personal;
- Export;
- Send to another paired Labby.

Unavailable options should be absent or disabled with a safe reason. Rich policy detail is shown only to callers with policy.explain.

A default personal destination may make Add to My Labby a single primary action. Send to opens the destination chooser when multiple eligible Labby instances exist.

## Destination model

A user may pair/register multiple Labby destinations, for example:

- Personal Labby;
- Laptop Labby;
- Devbox Labby.

Pairing establishes an authenticated destination identity and the ownership/trust relationship. The destination advertises supported transfer capabilities/versioning.

A transfer request MUST target a known paired destination. An arbitrary caller-provided URL is not sufficient evidence that the endpoint belongs to the user.

Long-lived destination credentials are stored in the appropriate secret/credential layer. Transfer records contain only opaque credential references.

## Transfer authorization

For each operation the source authorizes the tuple:

- actor Principal;
- source Organization/scope;
- exact Artifact ID/revision ID;
- requested transfer mode;
- destination ID when applicable;
- current policy epoch;
- current Artifact distribution facts.

Authorization is short-lived/single-operation in spirit. The protocol MUST NOT send a reusable organization bearer token to the destination just to make transfer convenient.

A future network protocol may use signed or capability-style transfer grants, but its token shape is intentionally not frozen by this design packet.

## Effective distribution policy

The effective transfer right is the intersection of six layers:

1. **Caller authorization:** artifact.sync/follow/fork/export/reshare as appropriate.
2. **Artifact publisher policy:** the maximum distribution rights authorized by the Artifact's authoritative owner scope.
3. **Assignment distribution policy:** the narrower rights granted for this specific Organization/Group/Project share; absent policy means byte movement/reshare is disabled.
4. **Artifact publication state:** whether metadata/bytes are currently distributable.
5. **License/redistribution/takedown state:** hard cap from Artifact evidence/state.
6. **Destination policy:** whether that paired Labby may accept the requested mode/artifact family.

No layer can broaden another layer.

## License/redistribution mapping

The existing Artifact license model remains authoritative.

Safe default behavior:

| Redistribution state | Managed byte sync | Fork | Detached export | Reshare bytes |
| --- | --- | --- | --- | --- |
| metadata_only | no | no | no | no |
| cache_for_index | no user materialization | no | no | no |
| redistributable | allowed only if source/caller policy allows | only if source policy separately allows and license semantics permit | allowed only if source/caller policy allows | allowed only if source/caller policy allows |
| forkable | allowed if source/caller policy allows | allowed if source/caller policy allows | allowed if source/caller policy allows | allowed if source/caller policy allows |
| restricted | no by default | no | no | no |
| unknown | no by default | no | no | no |

ArtifactPublication must also permit byte distribution. Takedown restricted/removed state wins over ordinary transfer permission.

This table is a safe implementation floor, not legal advice. Source-specific policy may be more restrictive.

## Managed mirror state

ArtifactStore and AccessStore cannot commit atomically. Every local operation therefore uses a durable operation ID and explicit staged/pending/committing/active/failed/revoked states with idempotent finalize, compensate, garbage-collect, and restart reconciliation behavior. A mirror is unusable until both stores agree on `active`.

A managed mirror tracks at least:

- source Artifact/revision;
- local Artifact/revision;
- source Organization/scope;
- destination/owner;
- pin/follow mode;
- last policy epoch under which source authorization succeeded;
- subscription/update state;
- revocation/withdrawal status.

The managed mirror is visibly labeled as managed/source-controlled. It must not look identical to a user-owned fork.

## Revocation semantics

### Remote references

Revocation stops future source use immediately after policy invalidation/re-resolution.

### Pinned/followed managed mirrors

When source authorization is revoked:

- future execution/use through managed policy is disabled;
- future sync/follow checks stop;
- the mirror is marked access_revoked or equivalent;
- source runtime bindings are unavailable;
- source-required purge policy is attempted on the next reachable destination interaction where supported;
- audit evidence records the transition.

The UI must not claim remote deletion succeeded when a destination is offline/unreachable.

A source policy may choose managed-cache retention behavior such as disable-only or purge-managed-bytes. The initial implementation SHOULD default organization-managed mirrors to disable use and purge bytes when safely possible, while retaining minimal non-secret tombstone/audit metadata.

### Personal forks

A fork made while artifact.fork and redistribution policy allowed it becomes a distinct Artifact. Later membership revocation does not convert it back into a managed mirror or provide a fictional remote-delete guarantee.

Future distribution of that fork remains bounded by its license/takedown state and any legally durable restrictions represented by the Artifact contract.

### Detached exports

Labby can revoke future export permission but cannot claim it can remotely erase a detached export already delivered outside managed control.

## Source withdrawal/takedown

When a source revision becomes withdrawn/restricted/removed:

- new transfers fail closed;
- followed mirrors stop applying revisions;
- managed mirrors enter the appropriate restricted state;
- destinations are notified/updated when reachable and protocol support exists;
- personal forks/exports follow their own independent Artifact license/takedown handling and cannot be described as remotely controlled copies unless they actually are.

## Loadout transfer

A Loadout is a composition container. Transferring/adding it never bypasses dependency policy.

Before Add Loadout to My Labby, Labby resolves each dependency against the chosen destination:

| Dependency result | Behavior |
| --- | --- |
| mirrorable | exact revision may be pinned/followed |
| remote_only | keep a source reference, no local bytes |
| forkable | user may choose managed mirror or personal fork according to policy |
| unavailable_required | whole Loadout install/activation fails |
| unavailable_optional | omit only if Loadout permits filtered resolution |
| license_blocked | treat unavailable; explanation records license/publisher reason |

The resulting personal Loadout resolution records which dependencies are local, remote, omitted, or forked. It never lies by showing a dependency as locally owned when it remains organization-managed.

## Personal overlay after transfer

A pinned/followed managed Artifact may participate in a user's Personal workspace if policy permits. When overlaying an Organization Project, the Project's overlay rules still apply.

A personal fork may also be proposed as an overlay candidate, but it cannot broaden Project upstream/tool/runtime-binding authority.

Transfer is therefore separate from Project authorization: possessing local bytes does not imply the current Project may execute/use them.

## Bidirectional/future flows

The same primitives can later support:

- My Labby -> Project submission;
- My Labby -> another user's Labby where sharing policy permits;
- Project Labby -> Personal Labby;
- personal fork -> request publication to Group/Organization;
- Labby -> Depot/Bazaar publication through the Artifact provider boundary.

These flows should reuse Artifact identity/revision/provenance/lineage and the same distribution permissions instead of creating a second sharing protocol.

## Audit requirements

Record at least:

- requested transfer mode;
- actor Principal;
- source scope;
- exact revision;
- destination safe ID;
- allow/deny reason code;
- source policy epoch;
- result status;
- for applied follow updates, old and new exact revision IDs.

Never record Artifact secret contents, destination credentials, or reusable transfer bearer material.

## Required tests

At minimum:

1. artifact.use without artifact.sync cannot pin bytes.
2. artifact.sync without artifact.fork cannot create a detached fork.
3. unknown/restricted redistribution blocks byte transfer.
4. a paired destination succeeds while an arbitrary unpaired URL fails.
5. exact revision digest mismatch fails before local state mutation.
6. revocation prevents a followed mirror from obtaining the next revision.
7. auto_approved follow still rechecks current authorization/license policy for every update.
8. personal fork receives a new identity with exact fork lineage.
9. Loadout transfer cannot smuggle a forbidden required dependency.
10. project overlay refuses a locally owned Artifact when the Project policy forbids that overlay kind/runtime authority.
11. ArtifactInterchange v1 conformance fixture remains byte-identical.
12. secret-safe export behavior remains enforced for detached export.
