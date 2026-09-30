---
title: "Issue 771 Lane C: Server Lifecycle Seam"
created: "2026-09-30"
updated: "2026-09-30"
---

# Issue 771 Lane C: Server Lifecycle Seam

Status: **source audit plus executable patch/seam proposal, not production
reacquisition, ADR acceptance, or Gate 1/2 completion.** Only Lane C test and
handoff files change. No production pool, TaskRoute, SDK pin, shared ADR, schema,
or other worker's branch is edited.

## Verified checkout and coordination

Host: MACPOO (`macpoo.local`, Darwin arm64, user `jmagar`). Repository origin:
`git@github.com:dinglebear-ai/labby.git`. Worktree:
`/Users/jmagar/workspace/labby/.worktrees/issue-771-server-lifecycle`.
Branch: `codex/issue-771-server-lifecycle`.
Audited/tested base: `138ef902dc1842daf883a6e7710b140fbfaf53c0`.

[Issue #771](https://github.com/dinglebear-ai/labby/issues/771) and
[PR #848](https://github.com/dinglebear-ai/labby/pull/848) were read live. #848
was already merged at the verified base, not still draft. Its retained-peer
boundary remains a B/C coordination seam; the merge is not permission to edit
B's worktree or `pool/tasks.rs`.

Gate 0 was inspected read-only, then pinned to local commit
`a5e3f78ca55e52c64b2013a8fae3d26a9bf5819e`, ADR path
`docs/adr/0007-durable-mcp-task-routing.md`, blob
`3ad75a27b9c46a4ff211cef10365f46057fdaba2`. That ADR is still proposed;
its acceptance, B's revision/migration contract, and shared guard handoff remain
unresolved. It was not cherry-picked, amended, pushed, or changed here.

Read the root, gateway, upstream, and docs AGENTS instructions, Architecture,
Upstream service contract, Testing, and Documentation Maintenance. Protocol
references are [MCP 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28)
and the [Tasks extension](https://tasks.extensions.modelcontextprotocol.io/specification/draft/tasks).
The checked-out RMCP fork is `b19cfc03025047153fa283c0064c651b6e2f623e`;
Lane A retains SDK upgrade ownership.

## Source findings

| Finding | Source at the audited base |
| --- | --- |
| UpstreamConnection retains service/peer/process lifetime. Runtime metadata copies server name/version and effective protocol, not a complete server observation. | [pool.rs](../../../crates/labby-gateway/src/upstream/pool.rs), UpstreamConnection::new_with_client_service |
| RMCP ServerPeerInfo::from_discover_result drops supported_versions, ttl_ms and cache_scope. Reconstructing a complete ServerSnapshot from peer_info is therefore lossy. | Pinned rust-sdk crates/rmcp/src/model.rs:1127-1167, 1213-1328; executable red evidence below |
| Existing opaque incarnation and catalog binding coordination already reject ABA and stale publication. | [incarnation.rs](../../../crates/labby-gateway/src/upstream/pool/incarnation.rs), observe/apply helpers and tests |
| OAuth cache provides scoped epochs and the shared publication/invalidation barrier. These are process-local and do not prove durable non-revocation after restart. | [cache.rs](../../../crates/labby-auth/src/upstream/cache.rs):58-68, 165-234 |
| Invalidation detaches publication-capable entries under the writer; transport shutdown is scheduled afterward. Return is not proof an already dispatched remote mutation stopped. | [oauth_invalidation.rs](../../../crates/labby-gateway/src/upstream/pool/oauth_invalidation.rs):59-140 |
| Main authorizes the durable record but still requires live.connection.peer; closed/missing creator connections cannot be reacquired. | [tasks.rs](../../../crates/labby-gateway/src/upstream/pool/tasks.rs):131-188 |
| TaskRouteRecord has immutable binding, timestamps and retention hints, but no durable row revision or revoked-at field yet. | [task_route_record.rs](../../../crates/labby-gateway/src/upstream/pool/task_route_record.rs):8-21 |
| Config reconciliation already advances/removes fingerprints before late publication; changed-config task-operation tests enforce non-dispatch. | [lifecycle.rs](../../../crates/labby-gateway/src/upstream/pool/lifecycle.rs), [tasks_review_tests.rs](../../../crates/labby-gateway/src/upstream/pool/tasks_review_tests.rs) |

## C1 executable proposal

The [test-only ServerSnapshot](../../../crates/labby-gateway/tests/issue_771_server_lifecycle/snapshot.rs)
retains one SDK-native discovery or explicit legacy response plus trusted
observation context. Capabilities own extensions; there is no second mutable
extension map. Optional server_info remains display data. Instructions, unknown
extension payloads and SDK metadata are preserved, not promoted to authority.
There is no peer, session, bearer, authorization method, or task store in this DTO.

Definition identity, safe config reference, opaque observation scope, an optional
existing catalog fingerprint, observation time and publication generation are
supplied by the owning adapter. They are not computed from server-reported names.
The generation is process-local only. Public cache hints do not erase owner/route
scope. Freshness uses checked arithmetic and an exclusive deadline; zero TTL is
non-cacheable. Observation expiry does not expire a task. The adapter must retain
existing transport/metadata bounds and pass the correct observation age.

Two explicit contract clarifications need owner agreement: legacy initialize
reports one effective version, not a complete supported-version inventory, so
unknown discovery fields stay absent; WebSocket is a configured transport and
requires its own durability qualification. Unknown effective lifecycle versions
are not guessed to be legacy. No SDK discovery replay is introduced to recover
lost metadata; see the [capture and reacquisition handoff](reacquisition-seam.md).

## C3 executable boundaries

New tests reuse the real OauthClientCache and UpstreamPool APIs for subject and
upstream invalidation, same-owner epoch refresh, deterministic reader/writer
ordering, late-after-await publication, replacement pools, and the absence of
durable revocation knowledge in a new cache. A real duplex in-process fixture
proves retaining the proposed snapshot does not keep a drained peer alive.

Existing tests supply the stronger production checks for incarnation ABA,
interrupted binding publication, selective reload/config fencing, owner/route
isolation, and rejection of invalidated OAuth discovery. Neither the new seam
nor an in-process drain proves a task survives creator loss or process restart.

## C4 transport durability matrix

All rows require a currently authorized owner/route, current credentials/config,
durable non-revocation, and retention that the backend can actually honor.
An advertised Tasks extension, matching display name, cache hint, or stored
mapping is not proof of backend persistence.

| Transport/lifecycle | Creator peer loss | Labby restart | Upstream restart |
| --- | --- | --- | --- |
| HTTP to independent service | Conditional on connection-independent upstream task state and authenticated reacquisition | Additionally reopen the same supported route DB | Additionally prove persistent backend task state |
| Unix Streamable HTTP | Same conditional guarantee as HTTP; socket identity alone is insufficient | Same route DB plus surviving daemon task state | Prove daemon task persistence |
| WebSocket | Reconnect alone is insufficient; qualify backend task identity/store and native task RPC behavior | Same route DB plus surviving independent backend | Prove persistent backend state; do not infer per-RPC HTTP header behavior |
| Local stdio | No default guarantee; a new child may own a different store | Prove reexecution preserves task identity and reopen route DB | Prove persistent task state across child restart |
| SSH-spawned stdio | Same child-reexecution boundary; SSH reconnection is not task continuity | Prove remote backend continuity plus route DB | Prove remote task persistence |
| In-process connector | Task owner must survive lease replacement | Requires a durable hosted task implementation plus route DB | Requires durable hosted task implementation |
| Explicit legacy lifecycle | Separately qualify actual legacy Tasks compatibility for every transport | Never infer modern durability from a recovered session | Same backend-persistence requirement |

[Executable matrix](../../../crates/labby-gateway/tests/issue_771_server_lifecycle/transport_matrix.rs)
exhaustively matches the actual four-variant UpstreamTransport enum, with separate
hosted and legacy overlays. It encodes proof obligations, **not a runtime permit
or a claim that a backend has passed them**. Local SQLite restart support is not
arbitrary multi-node failover or permission to use independent route databases.

## Verification and reproduction

The 21-test focused production baseline passed before the prototype change.
The initial characterization failed because peer_info omitted supportedVersions;
raw red sources and its nonzero receipt are preserved. The subsequent 22-test
snapshot/fencing run passed. The expanded all-feature nextest suite passed
**1,540 tests with five skipped**, including all **29 Lane C tests**. The
dedicated default-feature Lane C target also passed **28 tests**; its in-process
fixture is explicitly gated by testkit. Strict all-target Clippy, workspace
formatting, all five module-reachability tests, and the full documentation gate
also passed (19 generated artifacts fresh). Exact commands and verdicts are in
[the evidence directory](evidence/).

The first default-feature Cargo/libtest run at two test threads failed five
unchanged enrichment-provider process tests: all returned provider_timeout
instead of their expected success/error classification (1,478 passed, five
failed, five ignored). The exact same default-feature unit-test binary passed
the isolated failure and the entire 12-test provider group serially. The same
five timeouts recur in the complete one-thread suite, so reducing libtest
concurrency alone does not fix them. This is evidence of whole-suite-sensitive
behavior, not a proven root cause or a claim of default-feature correctness.
No timeout was weakened and no enrichment code was changed. Both failed runs
and the successful isolated diagnostic are retained under distinct receipt
names. This leaves the default full-suite gate unresolved outside Lane C; the
all-feature nextest success must not be presented as an entirely clean baseline.
The large macOS unit-test binary also emitted a linker unwind-table size
warning; strict Clippy itself passed without warnings.

Run checks from this worktree with the repository-pinned Rust 1.97.1. The
[runner](run_checks.py) records host, base, command, environment, exit status,
log hash and source hashes; a repeated check name is rejected. Build jobs and
test threads are bounded to two. It uses Cargo's locked shared compilation cache
only; it never cleans another worker's artifacts or changes their source tree.

~~~bash
cargo +1.97.1 test --locked -p labby-gateway --all-features --test issue_771_server_lifecycle -- --test-threads=2
cargo +1.97.1 nextest run --locked -p labby-gateway --all-features --test-threads 2 --no-fail-fast
cargo +1.97.1 clippy --locked -p labby-gateway --all-features --all-targets -- -D warnings
cargo +1.97.1 fmt --all -- --check
just module-reachability
just docs-check
~~~

These are local test receipts, not signed provenance or live Gate 1/2 evidence.
No release, deployment, push, merge, issue mutation, or other worktree cleanup
was performed. The [blocked-seam proposal](reacquisition-seam.md) is the handoff
for B, Gate 0, A, and subsequent C integration.
