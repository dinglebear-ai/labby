---
title: "Issue 771 Lane F: Adversarial Tests and Operator Handoff"
created: "2026-09-30"
updated: "2026-09-30"
---

# Issue 771 Lane F: adversarial tests and operator handoff

## Scope and checkout

This is a bounded, local-only Lane F slice for
[issue #771](https://github.com/dinglebear-ai/labby/issues/771). It changes only
new integration fixtures and operator/evidence documentation. It does not change
production code, shared test helpers, Cargo manifests, Gate 0, or other lanes.

- Host verified: `macpoo.local`, user `jmagar`, accessed through Labby write-capable Code Mode and `claude-macpoo`.
- Worktree: `/Users/jmagar/workspace/labby/.worktrees/issue-771-adversarial-docs`.
- Branch: `codex/issue-771-adversarial-docs`.
- Fetched origin: `git@github.com:dinglebear-ai/labby.git`.
- Base: `138ef902dc1842daf883a6e7710b140fbfaf53c0`, the merged [PR #848](https://github.com/dinglebear-ai/labby/pull/848).
- Pinned Rust: `1.97.1`; own target directory; two build jobs and two test threads; debug info and incremental compilation disabled.

The root, gateway, upstream, and documentation AGENTS.md instructions and the
Testing/Documentation Maintenance references were read before edits. Gate 0's
local ADR 0007 was inspected read-only as a proposed contract: its acceptance and
merge remain coordinator-owned. PR #848's deliberate retained-relay limit is
present in this fetched baseline. No other worktree was edited, reset, cleaned,
or committed by this lane.

## Source audit and focused plan

The public integration seams already support independent testing:

| Source | Observation / fixture use |
| --- | --- |
| [task_registration.rs](../../../crates/labby-gateway/src/upstream/pool/task_registration.rs) | Commit before public-ID translation and notification; reject failed storage without replaying the already-dispatched upstream create. |
| [tasks.rs](../../../crates/labby-gateway/src/upstream/pool/tasks.rs) | Public get/update/cancel authorize the durable row; then require a live companion. This gives a precise current-limit oracle and a red future-reacquisition test. |
| [oauth_invalidation.rs](../../../crates/labby-gateway/src/upstream/pool/oauth_invalidation.rs) | Public subject invalidation detaches matching live task companions; no durable revoke transition exists here. |
| [lifecycle.rs](../../../crates/labby-gateway/src/upstream/pool/lifecycle.rs) | Public config reconciliation and pool draining can be driven without private-field access. |
| [relay.rs](../../../crates/labby-gateway/src/upstream/pool/relay.rs) | Public tool relay, ordered SSE task notifications, and downstream rebinding are usable without changing shared fixtures. |
| [task_route_store.rs](../../../crates/labby-gateway/src/upstream/pool/task_route_store.rs) | Use the real file-backed store; failure injection is limited to the test's private SQLite database. |

Plan: establish the existing task baseline, add a separately discovered Cargo
integration target, prove authorization/non-dispatch and failure behavior through
real HTTP/SQLite/RMCP boundaries, preserve a red dependency assertion for pool
reacquisition, and publish an operator guide with explicit limitations.

No production implementation was requested in this lane. The passing tests are
regressions for existing behavior, so an artificial production mutation was not
introduced to manufacture a red/green cycle. The desired pool-reacquisition test
is a real executable assertion, explicitly ignored until its B/C dependency lands;
its manual failure is recorded separately and is not counted as passing coverage.

## Fixture boundaries

Entry point: [issue_771_adversarial.rs](../../../crates/labby-gateway/tests/issue_771_adversarial.rs).
The adjacent Lane F-only modules contain an independent loopback HTTP upstream,
RMCP duplex downstreams, real public pool calls, and temporary SQLite storage.
They do not include production source by path, access private pool fields, add
production test hooks, or modify existing workers' test fixtures.

The HTTP fake emits typed pinned-RMCP discovery/task results and complete task
notifications; records task method/native-ID dispatch and create count; supports
finite duplicate SSE notifications and temporary backend failure. It is a test
oracle, not a second durable task engine. Missing notification delivery never
implies task failure or permission to duplicate creation.

OAuth coverage supplies a trusted credential-owner context to the real gateway
invalidation API. It does not simulate a successful token exchange or credential
refresh. Expiry uses a fixed past creation time and a private row mutation, not a
claim about an exact virtual-clock TTL boundary. Pool replacement reopens the
route database but is not an OS process restart. Notification recovery is
new-downstream rebinding through the retained upstream, not Lane D's independent
subscription reconnection.

## Validation ledger

All results below were executed in this worktree against the pinned base. The
final origin/main fetch still resolved to the same base. Each log has an adjacent
exit-code receipt; raw compiler/test output is retained rather than inferred from
another lane or PR's CI.

| Check | Result | Evidence |
| --- | --- | --- |
| Pre-change task baseline | 35 passed; 1,481 filtered/ignored | [baseline.log](evidence/baseline.log) |
| Final Lane F target, all features | 14 passed; one explicit dependency test ignored | [focused-final.log](evidence/focused-final.log) |
| Final Lane F target, default features | 14 passed; same dependency ignored; no testkit requirement | [focused-default.log](evidence/focused-default.log) |
| Entire gateway, all features | 1,525 passed; six skipped (five existing plus the Lane F dependency) | [gateway.log](evidence/gateway.log) |
| Strict gateway Clippy, all features/all targets | Exit 0 with `-D warnings` | [clippy.log](evidence/clippy.log) |
| Explicit desired reacquisition contract | One expected failure; exit 100; `upstream task connection unavailable` | [reacquisition-red.log](evidence/reacquisition-red.log) |
| Workspace formatting | Exit 0 | [fmt.exit](evidence/fmt.exit) |
| Module reachability | Five tests passed; exit 0 | [module-reachability.log](evidence/module-reachability.log) |
| Documentation gates | Final: 1,113 local links, 65 fragments, 189 canonical docs; seven link-check and 24 product-doc tests passed | [docs-final.log](evidence/docs-final.log); unit-test results in `docs.log` |

The whole-gateway log includes the existing macOS lib-test linker unwind-table
warning and a redundant qualification warning in this lane's test. The latter was
removed; final focused/default compilation and strict Clippy were rerun cleanly.
The initial authoring build had two missing public gateway-ID arguments; that
fixture-only compile failure is retained in `focused-first.log`, then corrected.
It is not a production failure or part of the acceptance result.

### Exact commands

Run from the worktree root with these environment settings:

~~~bash
export CARGO_TARGET_DIR="$PWD/target"
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0

cargo +1.97.1 nextest run --locked -p labby-gateway --all-features --lib \
  -E 'test(upstream::pool::tasks::) | test(upstream::pool::task_route_store::)' \
  --test-threads 2 --no-fail-fast
cargo +1.97.1 nextest run --locked -p labby-gateway --all-features \
  --test issue_771_adversarial --test-threads 2 --no-fail-fast
cargo +1.97.1 nextest run --locked -p labby-gateway \
  --test issue_771_adversarial --test-threads 2 --no-fail-fast
cargo +1.97.1 nextest run --locked -p labby-gateway --all-features \
  --test-threads 2 --no-fail-fast
cargo +1.97.1 clippy --locked -p labby-gateway --all-features --all-targets -- -D warnings
cargo +1.97.1 fmt --all -- --check
RUSTUP_TOOLCHAIN=1.97.1 just module-reachability
python3 scripts/check-doc-links.py
python3 scripts/check-product-docs.py
python3 -m unittest discover -s scripts/ci -p test_doc_links.py
python3 -m unittest discover -s scripts/ci -p test_product_docs.py
git diff --check
~~~

The dependency probe is deliberately separate from the green acceptance command:

~~~bash
cargo +1.97.1 nextest run --locked -p labby-gateway --all-features \
  --test issue_771_adversarial --run-ignored only \
  -E 'test(blocked_pool_replacement_must_reacquire_and_resolve_the_same_task)' \
  --test-threads 1
~~~

No generated metadata, product API, or shared module was changed. Full
`just docs-check`, workspace Rustdoc, release builds, product MCP/API suites,
live OAuth exchange, real process restart, and Gate 2 were not run. The relevant
non-generated documentation gates and gateway tests were run instead; no skipped
or failing desired-contract test counts as a passed durability guarantee.

### Coordinated activation

When B/C provides authenticated reacquisition, activate the ignored desired
contract and replace the current-limit pool-replacement assertion. Do not weaken
either test to accept both success and failure. OAuth invalidation's current
live-companion expectation must likewise be advanced only with the durable
revocation contract. F owns changes to these new fixtures; production integration
and revision/lease interfaces remain with B/C/D.

## Dependencies and acceptance boundaries

| Dependency | Required next proof | Ownership |
| --- | --- | --- |
| Formal Gate 0 | Accepted/merged ADR and coordinated interfaces | Coordinator / Gate 0 |
| Authorized reacquisition | Create, destroy creator upstream peer, acquire a new authenticated peer, poll the same native task without replay; then real Labby process restart | B/C |
| Durable invalidation | Revocation/config change cannot be undone by restart; guard dispatch and late publication across awaits; distinguish same-owner refresh | B/C |
| TTL and GC | Exact expiry boundary, monotonic retention, admission/quota/corrupt-row and revision concurrency guarantees | B (F can extend public fixtures once seams land) |
| Task input/cancel races | Stale/replayed input keys, partial updates, completion-vs-cancel, safe mutation retry | D |
| Notification lifecycle | Authorized subscriptions/listen reacquisition, old-stream fencing, bounded delivery, no exactly-once guarantee | D |
| Final live qualification | Real process/client/server Gate 2, transport matrix, current protocol pin, Soma comparison | Coordinator with A/C/D/F |

The new tests and guide deliberately do not close F1/F2/F3/F4 in full or close
issue #771. Soma archaeology and the full canonical shared-doc synchronization
are outside this independent slice; no stale/process-local Soma pattern is copied.

## Operator handoff

See [MCP Task Failure Recovery](../../guides/MCP_TASK_FAILURE_RECOVERY.md) for
non-enumeration, retained-relay/restart limitations, duplicate and lost delivery,
failed acknowledgement side-effect uncertainty, cooperative cancellation, and
safe storage diagnostics. Shared MCP/RMCP/transport documents are left to their
owners rather than concurrently rewritten.
