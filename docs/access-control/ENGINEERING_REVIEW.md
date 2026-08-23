---
title: "Access Control Engineering Review"
created: "2026-08-23"
updated: "2026-08-23"
status: "reviewed"
---

# Access Control Engineering Review

## Review target and outcome

Epic `lab-mh3rs` was reviewed for architecture, simplicity, security, and performance against the complete packet and current code at `daf9caa48`.

The separation between authentication, policy, Artifact content, gateway execution, and secrets is directionally sound. Implementation was blocked by canonical identity ambiguity, oversized initial scope, missing Project/session ownership, incomplete storage/snapshot contracts, and assumptions about Artifact and credential product wiring. The plan now starts with a small Project-bound MCP isolation milestone.

## Architecture

Strengths include shared policy below adapters, frozen ArtifactInterchange v1, narrowing-only Gateway/Loadout integration, explicit Project context, and no required dependency cycle.

Findings applied:

1. `AuthContext.issuer + sub` is not canonical across browser/JWT/static transports; define `VerifiedIdentity` and explicit relinking.
2. ArtifactStore is a library seam, not canonical product state; add application wiring/reconciliation first.
3. static `McpRouteScope` is not Principal/Project session state; add server-owned `BoundAccessContext`.
4. Access, Artifact, and gateway facts need capture/gather/re-read/retry/fail-closed snapshot assembly.
5. separate stores need operation state machines and reconciliation.
6. bootstrap must precede shadow/enforcement.
7. runtime binding requires a current credential/cache seam map.

## Simplicity

The complete initiative is no longer treated as one release. Milestone 1 uses direct Principal Project membership, the fixed Project roles Owner/Admin/Member/Viewer, one existing named Loadout, MCP discovery filtering, and direct reauthorization. Project Owner is Project-scoped in this kernel and is not Organization Owner or a projection of `lab:admin`. Groups, custom Roles/Grants, Assignment precedence, overlays, federation, persistent explanation, and caching are later milestones. `labby-access` is an extraction target, not a precommitted public API.

## Security

The revised plan adds canonical provider/credential identity; concrete tenant-integrity requirements; final-boundary authorization linearization; immutable authenticated session ownership; authorization-grade SQLite with no permissive fallback; compare-and-set bootstrap; transactional mutation audit; protocol freeze before transfer; and cross-store crash recovery.

## Performance

Milestone 1 is uncached, uses a fixed set-based query-count budget and one AccessStore read snapshot, retries unstable snapshots only within a bound, and must pass latency/contention/storage-failure gates before enforcement. Later caches use explicit version domains rather than Organization-wide invalidation as the only mechanism.

## Failure modes

| Codepath | Production failure | Required rescue | Test | User sees | Logged/audited |
| --- | --- | --- | --- | --- | --- |
| AccessStore open/migrate | locked, corrupt, newer schema, disk full | fail closed; setup/doctor recovery | crash/storage matrix | unavailable/setup-required | safe startup alert |
| Identity resolution | missing/ambiguous canonical link | deny; explicit linking setup | browser/JWT/static matrix | non-enumerating setup state | fingerprint + reason |
| Tenant mutation | cross-Organization edge | transaction rejection/quarantine | storage-bypass/concurrency | validation/unavailable | mutation audit |
| Snapshot assembly | policy/Artifact/catalog changes mid-read | bounded retry then fail closed | generation barriers | temporary unavailable | before/after versions |
| MCP binding | reconnect/context substitution | reject binding/session | reconnect, `_meta`, task | safe Project error | binding fingerprint |
| Direct invocation | revoke commits before dispatch | uncached final check denies | revoke/check/dispatch barrier | not authorized | decision/execution revisions |
| Runtime binding | wrong/missing secret or cache key | no fallback | reconnect/concurrency | runtime unavailable | redacted reason |
| Cross-store Artifact op | crash between filesystem and SQLite | reconcile/compensate | crash-point/restart matrix | truthful pending/failed | operation ID |
| Audit mutation | audit write fails | mutation rolls back | sink-full/locked | mutation unavailable | storage alert |
| Background follow | source offline/revoked | retain trusted revision, stop update | revoke/offline/idempotency | degraded state | bounded retry event |
| Destination transfer | redirect/replay/ambiguous timeout | one-time capability + reconciliation | protocol conformance | never false success | safe fingerprints |
| Bootstrap | crash, concurrency, identity drift | idempotent compare-and-set | restart/config-drift | setup required | generation/fingerprint |

No retained path may be unrescued, untested, silent, and unlogged.

## Not in Milestone 1

- nested Groups and Group-derived membership;
- custom Roles/Grants and temporal membership;
- Assignment inheritance, slots, overrides, masks, and mandatory baselines;
- personal overlays and per-capability assignment;
- Project-specific runtime credentials;
- Artifact sync/follow/fork/export/reshare;
- destination federation and purge acknowledgements;
- Artifact-backed Loadouts and transfer dependency graphs;
- persistent explanation evidence;
- authorization caching;
- CLI/API/web parity, public anonymous policy, and service-account administration UI.

These remain documented dependent milestones rather than discarded scope.

## Completion summary

```text
Architecture issues: 7  |  Simplicity: 9  |  Security: 8  |  Performance: 6
Critical gaps before update: 3  |  Recommendations applied: 19
```
