---
title: "Depot-to-Labby Delivery Protocol v1"
status: proposed-contract
created: "2026-08-22"
---

# Depot-to-Labby Delivery Protocol v1

This document specifies the proposed `dinglebear.depot-delivery/v1` protocol
used when a person sends an authorized Depot Artifact or Loadout to one
explicitly linked Labby target. It is a contract for future conformance work,
not a claim that either backend implements connected delivery today.

The protocol is an independently versioned transport around immutable Artifact
revisions. It does not change `dinglebear.artifact-interchange/v1`, infer that a
Depot identity and a Labby identity are the same, or transfer authority from
either backend to Phabby. Labby remains AGPL-licensed. Depot's licensing is
unresolved and potentially proprietary; implementations may share only this
original wire contract and independently produced fixtures, not Depot source or
documentation prose.

## Authorities and invariants

- Depot authorizes the named account and tenant to distribute the exact
  Artifact or Loadout revision and bytes.
- Labby independently authorizes its local principal to receive, store,
  materialize, expose, and activate on the named target. A successful transfer
  does not imply permission for any later state.
- Phabby coordinates the ceremony and displays truthful receipts. It does not
  proxy artifact bytes, mint backend credentials, or bypass either decision.
- A browser handles only a non-secret, one-use delivery handle. Labby redeems
  that handle server-to-server after authenticating with its connection
  credential. The signed download grant and bytes never cross browser CORS.
- An explicit target is required when more than one target is linked. A visible
  user-selected default may supply that value, but neither backend may guess or
  silently fall back to another target.
- Loss of either backend fails visibly. There is no offline mutation queue.

## Version and compatibility

JSON messages use UTF-8, reject duplicate object keys, and carry the exact
`schemaVersion` value `dinglebear.depot-delivery/v1`. Unknown fields may be
preserved by relays but MUST NOT change authorization or validation. A receiver
MUST reject unknown enum values, a missing required field, and a major schema it
does not support. In-place weakening of v1 is forbidden.

Connections record the supported protocol range and Artifact interchange
versions. Grant issuance selects one mutually supported version and binds it in
the signed claims. A client MUST NOT retry a v1 request as an older or unsigned
protocol. A compatibility error is terminal until an operator changes one side.

Feature rollout is guarded independently on Depot and Labby. Disabling either
gate stops new grants; disabling Labby also cancels active pulls before commit.
Rollback removes the connected-delivery route and workers while preserving
receipts and already committed local revisions. It never deletes an existing
local revision or re-enables the legacy browser byte path.

## Explicit identity-link ceremony

1. An authenticated Labby administrator chooses **Link Depot**. Labby creates a
   ten-minute, single-use challenge containing a random 256-bit nonce, its
   stable `targetId`, display name, exact pinned HTTPS Depot origin, protocol
   range, and Labby signing-key thumbprint. The challenge is stored hashed.
2. The browser navigates to that exact Depot origin. The person authenticates
   to Depot and explicitly chooses a Depot account and tenant. Email addresses
   and display names are presentation only and are never identity keys.
3. Depot displays the named Labby target and requested connection privileges.
   Consent creates a `connectionId` bound to `{depotAccountId, tenantId,
   targetId, depotOrigin, labbyKeyThumbprint}`. Depot signs the challenge
   response; Labby verifies the nonce, expiry, origin, target, and signature.
4. Each backend issues its own narrow, rotatable connection credential and
   stores only the peer credential it needs. Credentials are not browser
   cookies, are never interchangeable, and can be revoked independently.
5. Both backends append correlated `identity_linked` audit events. Either may
   later revoke the connection. Revocation prevents handle redemption and grant
   use immediately; it does not erase immutable audit history.

Relinking after revocation creates a new connection and credentials. It does
not revive grants or transfer history from the old connection.

## Delivery sequence

1. The Depot user selects an immutable revision, a named linked Labby target,
   and an explicit conflict policy: `reject`, `keep_existing`, or
   `create_side_by_side`. V1 has no overwrite policy.
2. Depot authorizes the user and tenant, freezes the revision/digest and bounded
   dependency closure, creates `deliveryId`, and returns a random, single-use,
   two-minute `deliveryHandle`. The handle contains no URL or bearer grant.
3. The browser sends the request fixture shape to the selected Labby. Labby
   authenticates its local user, validates target equality and policy, and
   records `requested`. Duplicate requests with the same `(targetId,
   idempotencyKey)` return the same delivery; reuse with different content is a
   conflict.
4. Labby authenticates to the pinned Depot origin for `connectionId` and
   redeems the handle. Depot reauthorizes the account, tenant, connection,
   revision, and distribution policy, atomically consumes the handle, and
   returns a signed grant plus a signed chunk manifest.
5. Labby validates the signature against the connection's pinned Depot trust
   keys, all claims, the manifest, local policy, compatibility, provenance, and
   license before fetching. Depot records `granted` only after successful
   redemption.
6. Labby fetches chunks server-to-server from relative paths resolved only
   against the pinned Depot origin. Each request presents the signed grant.
   Labby verifies every chunk before placing it in delivery-scoped staging.
7. Labby reconstructs and validates the exact ArtifactInterchange envelope and
   payload digests, then atomically commits immutable CAS objects and the local
   revision. Materialization occurs in a new staging directory and is renamed
   atomically. No visible head changes before commit.
8. Exposure and activation are separate, independently authorized operations.
   Labby emits a signed receipt after every terminal or material state change;
   Depot verifies and retains it. Both audit streams use the same correlation
   fields.

## Signed download grant

The grant is a compact JWS using an allowlisted asymmetric algorithm. V1
requires Ed25519/`EdDSA`; `none`, symmetric MACs, embedded `jwk`/`jku`/`x5u`,
and algorithm substitution are rejected. The protected header is exactly
`{alg:"EdDSA",kid:<pinned-key-id>,typ:"depot-delivery+jwt",v:1}` plus no
security-affecting unknown fields.

The signed claims include:

| Claim | Binding |
| --- | --- |
| `iss`, `dnsPolicyId`, `tenantId`, `sub` | exact Depot origin, immutable approved DNS resolution policy, tenant, and account |
| `aud`, `targetId` | `labby:delivery` and the one linked target |
| `connectionId`, `deliveryId` | explicit connection and request |
| `resourceKind`, `resourceId` | Artifact or Loadout identity |
| `revisionId`, `contentDigest` | immutable revision and SHA-256 digest |
| `manifestDigest` | canonical signed chunk manifest |
| `purpose` | exact literal `depot-to-labby-pull` |
| `protocolVersion`, `artifactSchemaVersion` | downgrade-resistant versions |
| `jti`, `iat`, `nbf`, `exp` | random token identity and short validity |

Grant lifetime is at most five minutes with at most 30 seconds of clock skew.
Expiry is exclusive at the skew-adjusted boundary. `dnsPolicyId` is the SHA-256
identity of the approved origin plus its sorted public resolved-address set; a
private resolution or any resolution-set change requires a new policy identity
and fails the existing connection/grant binding. The transport must validate
the selected socket address as a member of that exact policy while retaining
the bound HTTPS origin for TLS and HTTP authority checks.
Depot stores hashed `jti` state and atomically changes it from `issued` to
`active` on first valid chunk request. Repeated requests for the same delivery
and chunk are permitted while active; a different delivery, target, manifest,
or connection is replay and revokes the grant. Terminal delivery, connection
revocation, user permission loss, or expiry makes every later chunk request
fail. The grant is never placed in a URL, error, trace, receipt, or audit field.

## Bounded chunk manifest and resume

V1 uses a signed chunk manifest, not open-ended HTTP ranges. The manifest
contains a deterministic dependency DAG and archive entries, divided into
fixed chunks of at most 8 MiB. Every chunk has an ordinal, exact byte count,
SHA-256 digest, and relative download path. The manifest itself is canonical
JSON and its digest is grant-bound.

Hard receiver limits are negotiated downward and never exceeded:

- 2,000 components; dependency depth 32; 8,000 dependency edges;
- 2 GiB total uncompressed bytes; 1 GiB total compressed bytes;
- 8 MiB per chunk; 4,096 chunks; 4,096-byte normalized relative paths;
- expansion ratio 20:1; 8 concurrent requests; 3 attempts per chunk;
- 24-hour delivery lifetime, including all resumes.

Labby persists only verified chunk digests in delivery-scoped staging. Resume
re-redeems a fresh grant for the same delivery, revision, manifest, and target;
Depot returns the immutable manifest and Labby requests only missing chunks.
A changed binding creates a new delivery and staging namespace. Completion is
idempotent: an existing identical local revision is reused, never duplicated.
After 24 hours, cancellation, or a terminal policy failure, staging is removed.

Archives reject absolute or traversal paths, links escaping the root, devices,
FIFOs, sockets, duplicate or case-colliding normalized paths, sparse expansion
beyond limits, and bytes not declared by the manifest.

## State machine and receipts

Delivery states are monotonic except that a nonterminal `partial` transfer can
continue. Each component reports its own state and reason.

```text
requested -> granted -> transferred -> verified -> stored -> materialized
                                                       |          |
                                                       +-> exposed -> activated

Any nonterminal state -> partial -> next valid state
Any nonterminal state -> cancelled | failed | incompatible
```

`exposed` means visible to a local surface; `activated` means enabled in a
runtime. Neither is implied by `stored` or `materialized`. A delivery may be
`stored` while activation fails; the receipt must retain the successful storage
state and report activation failure separately. `incompatible` is terminal for
that exact target and revision. Cancellation never rolls back an already
committed immutable revision.

A component in `partial` carries `completedThrough`, naming its last completed
nonterminal stage. Entering partial preserves that stage exactly; resumption may
re-emit the same stage or advance by one valid state-machine edge. This makes
`transferred -> partial(completedThrough=transferred) -> transferred|verified`
truthful without permitting a skipped stage or regression.

Receipts are append-only snapshots with a strictly increasing `sequence`, the
same `deliveryId`, `correlationId`, target, resource, revision, and digest, plus
per-component states. Summary counts are derived from component state and the
explicit failure stage; both counts and component progress are non-regressing.
Aggregate state changes follow the state machine above without skipped or
backward transitions. Depot rejects sequence regression or conflicting content
at the same sequence. The terminal receipt is signed by Labby; intermediate
receipts may be signed or delivered over the mutually authenticated connection.

## Failure and threat controls

| Threat | Required control |
| --- | --- |
| SSRF / DNS rebinding | Connection pins an operator-approved HTTPS origin and resolved policy; only relative manifest paths are accepted; redirects, userinfo, fragments, IP literals, private/link-local/metadata destinations, DNS changes outside the pin policy, and cross-origin fetches fail. |
| Confused deputy | Depot reauthorizes tenant/account/resource on redemption; Labby checks local principal and exact target; all identifiers and purpose are grant-bound. |
| Credential leakage | Secrets are header-only, memory-lifetime bounded, encrypted at rest, redacted by field and value fingerprint, and absent from browser, URLs, receipts, errors, and telemetry. |
| Replay | Handles are atomic single-use; `jti` is tracked; chunk replay is limited to the same active delivery binding; terminal and revoked tokens fail. |
| Downgrade | Signed protocol and Artifact schema versions; mutually recorded minimum; no unsigned fallback. |
| Oversized graph/archive | Preflight counts plus streaming byte/depth/ratio enforcement; breach cancels fetch and deletes staging. |
| Partial writes | Verified delivery-scoped staging, fsync where supported, atomic CAS/revision commit and directory rename; heads and activation change only after commit. |
| Mid-transfer revocation | Depot checks grant/connection/permission on every chunk; Labby stops on denial and retains only bounded resumable staging until expiry. |

Digest, provenance, license, compatibility, or signature failure is never
retryable under the same grant. Uncommitted materialization is removed. Existing
CAS objects may remain only when independently verified and unreachable partial
metadata is garbage-collectable; no local head, exposure, or activation pointer
may reference a failed delivery.

Errors use stable `code`, `stage`, `retryable`, and safe `message` fields. They
never echo credentials or attacker-controlled URLs. Examples are in
[`fixtures/`](./fixtures/).

## Audit and privacy

Both backends emit events for link, unlink, requested, granted, chunk resume,
verification, storage, materialization, exposure, activation, cancellation,
revocation, and failure. Required correlation fields are `correlationId`,
`deliveryId`, `connectionId`, `tenantId`, `targetId`, resource ID, revision ID,
receipt sequence, actor type, event name, outcome, and timestamp.

Audit fields contain stable IDs, not bearer material, raw manifests, content,
email, display names, filesystem paths, source URLs, signed grants, handles, or
connection credentials. Errors record a stable code and safe stage. Operational
metrics use bounded outcome/stage/version labels; resource, tenant, target,
delivery, and correlation identifiers belong in structured events, never metric
labels.

## Conformance and rollback gates

Before enabling connected delivery, implementations must prove:

1. expired, revoked, wrong-target, cross-tenant, wrong-revision, wrong-digest,
   wrong-audience, and replayed grants fail;
2. interruption resumes missing chunks without duplicate revisions;
3. digest/provenance/license failure removes uncommitted materialization;
4. successful storage plus failed activation produces a truthful receipt;
5. permission or connection revocation during transfer safely terminates;
6. target choice is explicit when multiple targets exist;
7. oversized/cyclic graphs, archive traversal, redirects, DNS rebinding, and
   partial-write fault injection fail closed;
8. audit correlation joins both backends while redaction tests find no secret;
9. disabling either feature gate stops new work and preserves committed state;
10. a fresh client requires no CORS credential bridge for bytes.

The fixtures and `validate_fixtures.py` provide syntax and invariant goldens.
They are not substitutes for signature, authorization, storage fault-injection,
fresh-browser, or production-equivalent tests.
