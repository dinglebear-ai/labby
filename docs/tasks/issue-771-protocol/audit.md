---
title: "Issue 771 Lane A Protocol Audit"
created: "2026-09-30"
updated: "2026-09-30"
---

# Issue 771: Lane A RMCP/protocol audit

## Decision and boundary

**HOLD the production RMCP 3.3.0 fork pin.** Do not switch to stock 3.4.0 or
replace Labby's connection fallback with SDK Auto. After Gate 0 acceptance,
prefer a separately qualified rebase of the required fork patches onto 3.4,
not a blind dependency replacement. The production manifests/lockfiles and
shared pool/task/server implementation are unchanged by this lane.

The separately prepared Gate 0 ADR was inspected read-only at
`docs/adr/0007-durable-mcp-task-routing.md` in branch
`codex/issue-771-gate0`. Its status is proposed; Gate 0 explicitly remains
NOT CLEARED. This report supplies compatibility evidence and interface needs,
not approval of that ADR or an implementation of its semantic DTOs.

Scope: A1 audit/decision, A2 parity characterization, A3 protocol fixtures and
stale RMCP version documentation. No B/C route/pool edits, D task input or
notification implementation, #208 synchronous MRTR changes, or #709 Code Mode
changes. No push, merge, deployment, service restart or shared-cache cleanup.

## Verified checkout and instructions

| Item | Observation |
| --- | --- |
| Host / user | macpoo.local / jmagar, aarch64-apple-darwin |
| Repository / origin | /Users/jmagar/workspace/labby; git@github.com:dinglebear-ai/labby.git |
| Isolated worktree | /Users/jmagar/workspace/labby/.worktrees/issue-771-protocol |
| Branch | codex/issue-771-protocol |
| Fetched origin/main and base | b42818f256968466c7d85828aa42c1cd8523efad |
| Rust | 1.97.1, explicitly invoked with rustup run; two Cargo jobs |
| Task | [#771](https://github.com/dinglebear-ai/labby/issues/771) |
| Read-only prerequisite | Draft [#848](https://github.com/dinglebear-ai/labby/pull/848), initially observed at 829fbac724897f301fbb2b3a8b8fd51f4ac97c27 |

The main checkout was already on another agent's branch and was not switched.
The prerequisite worktree was not edited, staged, committed, reset or cleaned.
Local gh issue/PR reads returned HTTP 401; connected GitHub reads supplied the
issue and draft PR instead. SSH Git fetch succeeded. A transient tool ENOSPC
failure and later free-space recovery are environment observations, not test
failures or permission to remove anyone else's files.

Reviewed root/global instructions, gateway and upstream AGENTS.md, docs AGENTS.md
and documentation maintenance, and verification-workspace instructions. No
protected history or generated catalog was edited. New tests are auto-discovered
by Cargo and use the existing gateway dev dependencies; no manifest registration
or new production dependency is needed.

## A1: exact fork versus official comparison

| Reference | Immutable identity |
| --- | --- |
| Production fork | 3.3.0, b19cfc03025047153fa283c0064c651b6e2f623e |
| Stock conformance fixture / common ancestor | 3.3.0, 3e636cab26c013eca5131103c03d20237f12c4df |
| Official rmcp-v3.4.0 | fd7811fdaa9fefa1c8034534b4d7a31c97204f89 |

The fork adds four non-merge commits plus a merge relative to the common
ancestor; official 3.4 adds twelve commits. The fork-to-ancestor diff is 21
files, 1553 insertions and 133 deletions. A direct official-to-fork tree diff is
85 files, 2649 insertions and 1931 deletions: do not confuse GitHub merge-base
comparison output with direct tree equivalence.

Evidence: [fork audit receipt](evidence/a1-fork-audit.json),
[raw fork delta](evidence/a1-fork-audit.log),
[config/seam receipt](evidence/a2-config-and-seams.json), and
[config/seam diff](evidence/a2-config-and-seams.log).

| Fork commit / concern | Consumers and migration disposition |
| --- | --- |
| 0665dca and 0e1184b: typed custom responses on the 3.3 base | Raw response preservation spans service/transport, HTTP, stdio, worker and Unix boundaries. Retain the behavior and its regression fixtures. |
| 68e6f4a: typed response compatibility hardening | Preserve behavior through adapters, not just similarly named APIs. Qualify source and wire parity. |
| b19cfc0: bounded Unix response bodies | LabbyUnixSocketHttpClient calls with_max_response_bytes. Stock 3.4 lacks this fork API; retain a bounded equivalent before upgrading. |
| Fork Origin behavior | Strict serialized-origin parsing, default-port normalization and duplicate-Origin rejection differ from stock 3.4. Rebase with differential tests rather than assuming the upstream Origin commit subsumes the fork. |
| 2faf762: merge | History integration, not another independently required feature. |

Concrete gateway consumers include
`crates/labby-gateway/src/upstream/http_client.rs` (raw response preservation),
`pool/connect.rs` and `pool/stdio_transport.rs` under the same upstream root,
and `transport/unix_socket.rs` (response bound and raw forwarding) and
`transport/websocket.rs` (raw forwarding).

The [official 3.4 release](https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.4.0)
adds ServerConfig/ClientConfig and fixes lifecycle cancellation, first pre-init
request dispatch, auth metadata discovery, malformed JSON handling and HTTP
HeaderMismatch mapping. It also changes Origin validation. A future rebase must
retain these upstream fixes while porting the necessary fork protections; in
particular, preserving raw responses must not reintroduce malformed-200-as-ack
behavior. No production upgrade was attempted here.

## A2: lifecycle/config parity decision

Labby already calls RMCP serve_with_lifecycle with an explicit modern Discover
attempt and a separately constructed legacy Initialize retry. Existing gateway
library tests exercise the unchanged `pool/lifecycle_compat.rs` and connection
paths separately. The new SDK integration target does not copy private pool
modules, add exports, or rewrite the classifier.

SDK Auto demonstrates discovery-first and typed legacy fallback, but is not a
replacement for the product's fresh-transport retry. It can retry on its existing
transport and has its own timeout policy. Labby also distinguishes network
closed-discovery evidence from stdio child EOF, and protects protocol/capability
errors against downgrade. Preserve these policies until C's lease/reconnect
contract and the ADR agree on authority and fencing.

| Area | 3.4 change candidate | Current action |
| --- | --- | --- |
| Local server handler configuration | ServerInfo return values become ServerConfig; product MCP server.rs and shared adapters are affected | Inventory only; no shared edit |
| Local client handler configuration | ClientInfo return values become ClientConfig; legacy_client.rs and client builders are affected | Inventory only; no shared edit |
| Observed remote state | Keep ServerSnapshot independent of local config objects and live peers | C owns the DTO; consume negotiated version/capabilities/cache metadata without inventing identity authority |
| Startup | Reuse SDK Discover and Initialize helpers, preserve fresh transport and typed failure classification | Characterize in fixtures; no helper swap |
| Task RPCs | Reuse GetTaskParams, UpdateTaskParams, CancelTaskParams and SDK transport headers | Exercise unchanged SDK behavior |

Config types configure local handlers; they do not make peer self-reported
identity authoritative and are not a durable routing schema. The official-only
fixture harness mechanically renames ClientInfo/ServerInfo to the 3.4 config
types in disposable test copies. It does not change product source.

## A3: fixture scope and honest proof levels

The canonical target is
`crates/labby-gateway/tests/issue_771_protocol.rs`, with focused fixture,
lifecycle, HTTP-header and explicitly named subscription-gap modules in the adjacent directory. It reuses RMCP
TaskManager, models, extension helpers, server dispatch, lifecycle modes,
Transport and StreamableHttpClientTransport. Wiremock is only the HTTP recorder.
All protocol exchanges have bounded deadlines; fixture tasks and peers are
stopped at test completion. These are local SDK tests plus a separate gateway baseline, not a
live deployed gateway or a durable upstream backend.

| Requirement | Fixture assertion | Not established |
| --- | --- | --- |
| Extension advertisement | enable_tasks serializes under extensions, not legacy top-level tasks | Backend persistence guarantees |
| Per-request capability enforcement | Prior discovery declaration does not authorize later calls; create/get/update/cancel reject absent capability with -32021 | Product pre-dispatch side-effect prevention or caller authorization |
| Result shapes | Flat CreateTaskResult is task; all five GetTaskResult status payloads are complete | Full adversarial schema coverage |
| Acknowledgements | First get resolves without retries; update/cancel are empty complete acknowledgements; cancel may leave working | B's SQLite commit-before-ack, retention and restart proof |
| Failure | A handler creation error cannot become a successful task result | Real storage-fault injection |
| Modern method set | tasks/list and tasks/result are method-not-found; disabled server extension rejects task methods | A custom legacy task API |
| Lifecycle | SDK Auto discovers first, explicitly negotiates 2025-11-25 on legacy fallback, avoids initialize for modern and does not downgrade protocol errors | Full C connection-pool/reconnect implementation |
| HTTP | SDK get/update/cancel put native task ID in Mcp-Name and method in Mcp-Method; modern requests have no Mcp-Session-Id | Authenticated live multihop header capture |

The Tasks wire authority is the
[Tasks extension](https://tasks.extensions.modelcontextprotocol.io/specification/draft/tasks),
with [MCP 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28)
for lifecycle and per-request metadata. Task creation durability is an upstream
and B/C obligation; in-memory TaskManager fixtures do not satisfy Gate 1/2.

The stale 3.1 reference in the canonical RMCP document is corrected. The extension
expected-failure YAML comment now points to the actual stock 3.3 fixture and
removes an unverified blanket pass claim. Its scenario entries are unchanged.

## Confirmed blocker: task subscriptions are not conformant

Both the pinned 3.3 fork and stock 3.4 SubscriptionFilter omit taskIds. The SDK
deserializer silently drops the field. This contradicts the current Tasks
extension, which requires preserving requested task IDs and returning -32021
when a non-declaring client requests task notifications. A stock 3.4 upgrade
therefore does not resolve this gap.

Two tests are explicitly named known_sdk_gap: one records model-field loss;
one sends raw JSON after discovery to bypass the client model and records the
server response. These are characterization checks, **not passing conformance
or accepted exceptions**. Set LABBY_LANE_A_REQUIRE_TASK_SUBSCRIPTIONS=1 when
running the raw test to enforce the strict specification oracle. That strict
result remains separately visible in the evidence. No tests are ignored.

A coordinated SDK patch must preserve taskIds through model parsing, filter
intersection, capability checks and acknowledgements; D then owns authorized
route rewriting, notifications and subscription reconnection. This lane stops
before editing the dependency or shared subscription/lifecycle contracts.

## Interface needs for the other lanes

1. **B:** Commit and authorize the route before any public-ID acknowledgement or
   notification. Supply immutable native ID/upstream/auth binding and revision;
   do not expose peers or bearer material. Preflight current client capabilities
   before side effects: RMCP's defensive task-result guard runs after a handler.
2. **C:** Pass the authorized native ID through SDK task request constructors.
   RMCP owns Mcp-Name/Mcp-Method. Supply a separately scoped ServerSnapshot and
   current fenced lease; transport death must not delete the route. Coordinate
   config-type adoption and transport recreation before touching shared adapters.
3. **D/F:** An update/cancel ack is not terminal-state proof. Task input stays in
   tasks/update, not #208 continuation storage. Reuse these wire fixtures but add
   durable storage, owner isolation, stale revision, real reconnect/restart and
   notification tests; do not claim this suite closes those requirements.

Unknown/unauthorized/expired/stale-route non-enumeration remains B/C's shared
adapter contract. The proposed Gate 0 ADR specifies the existing uniform
-32602 task-not-found path; Lane A does not change that mapping or reinterpret
transport/infrastructure errors as upstream task states.

## Verification and reproduction

Every executed check has an immutable JSON receipt and raw log in evidence/.
Receipts include argv, source HEAD, environment overrides, timestamps, timeout,
free-space floor, exit/stop reason and log SHA-256. An environment stop is not a
passing test. Initial failures are retained separately from corrected reruns.
Use capture.py with a fresh label to avoid overwriting evidence. The evidence-local
.gitattributes preserves raw log bytes and treats capture whitespace as data;
source files remain subject to normal whitespace checks.

Canonical command (from this worktree, Rust 1.97.1):

~~~bash
python3 docs/tasks/issue-771-protocol/capture.py lane-a-fixtures --   rustup run 1.97.1 cargo test -p labby-gateway --all-features --locked --offline   --test issue_771_protocol -- --test-threads=2
~~~

The disposable official comparison uses prepare_harness.py official34 and
`target/lane-a/harness-official34/Cargo.toml`, with the official SDK checkout at
the exact tag commit above. It is an audit-only workspace, never a replacement
for the product manifest. The source-hash receipt identifies copied fixtures;
the only SDK compatibility edits are the named config-type substitutions.
No private pool modules are copied into this harness.

## Validated results

| Evidence label | Result |
| --- | --- |
| baseline-sdk-pin | Production fork and stock fixture pins verified, exit 0 |
| gateway-existing-suite | Existing all-feature gateway library: 1484 passed, 5 skipped, exit 0 |
| gateway-fixtures-final | 11 passed: nine positive protocol fixtures and two explicitly labeled gap characterizations |
| official34-fixtures-final | Same 11 passed against the isolated official 3.4 SDK with config-type substitutions |
| strict-pin-subscription | Strict raw task-subscription capability oracle FAILS, test exit 101 |
| strict-official34-subscription | Same strict oracle FAILS on official 3.4, test exit 101 |
| gateway-clippy | All-feature/all-target gateway Clippy with -D warnings, exit 0 |
| gateway-doctests | Command passed; zero gateway doctests existed, so no additional coverage |
| full-docs-check | Complete just docs-check, including generated freshness and checker tests, exit 0 |
| docs-product / docs-links / rustfmt / module-reachability | All passed |

The baseline command was cargo nextest run -p labby-gateway --all-features
--locked --offline --lib --test-threads 2. Existing HTTP discovery-first,
legacy fallback, lifecycle error classification, tasks, MRTR-related and
subscription/notification library tests ran unchanged as part of that suite.
Five skipped tests are not counted as coverage. This is not the complete
external MCP conformance package or a deployed end-to-end test.

Strict wire observation on BOTH SDKs:

~~~json
{"jsonrpc":"2.0","method":"notifications/subscriptions/acknowledged","params":{"_meta":{"io.modelcontextprotocol/subscriptionId":71},"notifications":{}}}
~~~

The expected response is a JSON-RPC -32021 missing-capability error. Neither
model-field loss nor this wire failure is accepted as conformance. Exact strict
commands and logs are retained in their receipts. One earlier queued strict
Cargo invocation timed out waiting on the shared audit build directory; the
subsequent direct invocations of the successfully built test executables supply
the actual strict failure evidence, not that timeout.

Initial fixture compilation and setup failures are retained in
`gateway-fixtures-initial` and `gateway-fixtures-r2` through `r4`. They were
corrected in Lane A-owned test code: private-module coupling was removed,
SDK result/request types corrected, fixture shutdown made explicit, TLS provider
initialized, and the raw subscription probe separated from pre-init handling.
The baseline product library was not repaired or changed to make tests pass.

The official audit has a separately resolved dependency graph, retained as
`official34.Cargo.lock` (SHA-256
`d61f4372aa29559df510c898e54a457fb765bceefd12c440ea27ecedc92eb9cb`).
It is not a full Labby-on-official-3.4 build or a controlled comparison with
identical transitive dependency versions. No production lockfile was changed.

## Concurrent main drift and integration boundary

A final fetch found origin/main at
`138ef902dc1842daf883a6e7710b140fbfaf53c0`: #847 and then #848 landed while
this isolated lane was running. #848 is therefore no longer only a draft in
the final observed main history. Lane A did not push or merge either change.
The base and all product verification here remain
`b42818f256968466c7d85828aa42c1cd8523efad`; no automatic rebase was performed.

The intervening changes touch shared pool/task/relay implementation but not
Lane A's test files, RMCP pin, RMCP document or extension-expectation comment.
The exact commit list and file overlap are in
[main-drift evidence](evidence/final-main-drift.log). Integration must rerun
product checks on the combined main/Gate 0/B/C revision; the successful baseline
must not be presented as verification of newly landed #848.

No full product conformance, restart/durability or Gate 0 clearance is inferred
from this audit. A1's compatibility decision is complete; A2 config adoption is
held; A3 has reproducible positive fixtures and an unresolved strict
subscription conformance blocker. The SDK patch and shared D/C adaptation need
coordination before implementation.
