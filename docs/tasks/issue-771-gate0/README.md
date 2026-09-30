---
title: "Issue 771 Gate 0 Baseline and Handoff"
created: "2026-09-30"
updated: "2026-09-30"
---

# Issue 771 Gate 0: baseline and local handoff

**Gate status: NOT CLEARED.** Source/command evidence and the
[proposed D1-D6 ADR](../../adr/0007-durable-mcp-task-routing.md) are prepared locally.
S0.1 and S0.2 have recorded successful retries, including the full strict-baseline
conformance script and generated-doc checks. Expected failures/skips remain explicit.
The ADR is not accepted/merged and shared-seam agreements remain pending. This is
not a durability implementation.

## Checkout and authority

| Item | Observed value |
| --- | --- |
| Host / user | macpoo.local / jmagar; aarch64-apple-darwin |
| Repository | /Users/jmagar/workspace/labby |
| Isolated worktree | /Users/jmagar/workspace/labby/.worktrees/issue-771-gate0 |
| Branch | codex/issue-771-gate0 |
| Audited origin/main and worktree base | b42818f256968466c7d85828aa42c1cd8523efad |
| In-worktree Rust / Cargo | 1.97.1 / 1.97.1, selected by rust-toolchain.toml |
| Host-default Rust seen before changing command cwd | 1.98.1; not used for the recorded baseline |
| Nextest / just | 0.9.143 / 1.58.0 |
| Execution boundary | Labby write-capable codemode -> claude-macpoo shell/read/write tools |
| Scope | Gate 0 docs/evidence only; local commit, no push/merge/deployment |

See [identity receipt](evidence/s01-identity.json) and
[identity output](evidence/s01-identity.log). Commands run with cwd at this worktree,
two Cargo jobs, incremental compilation disabled and its own target directory.
The initial repository checkout was on another agent's branch; it was not switched.

At 06:00 UTC, a fresh remote audit observed main at
e756673f450b2b3f354ff891f887a7c0a5d49333 (#847). Its four-file delta touches
notification settings UI/tests, CLI snippets and an Incus contract test, not the
audited Task/RMCP sources, manifests or shared lifecycle seams. See
[evidence/s01-main-advance.log](evidence/s01-main-advance.log). Tests intentionally
remain on the frozen b42818f base; they are not whole-head verification of e756673.

Instructions reviewed: root AGENTS.md; docs/AGENTS.md;
docs/dev/DOCUMENTATION.md; upstream/AGENTS.md under the gateway crate; and
tools/verification/AGENTS.md for separate-workspace evidence boundaries.
Justfile and the conformance script define commands, not historical plans.
Generated catalogs are not hand-edited. Protected historical trees are excluded
from changes and link auditing. Conformance dependency installation uses the
script's isolated temporary directory and npm cache. No host package-manager
changes, host cleanup or service restart is part of this handoff.

## S0.1: source audit and ownership

[Issue snapshots](issue-sources.json) contain #771 and explicitly marked excerpts
from #208/#709, exact connected-tool queries, observed states and timestamps.
#208 is open and owns the synchronous MRTR/request-input overlay. #709 is closed
and retains Code Mode/Microsandbox scope; closure does not transfer ownership to
#771. Gate 0 neither reopens it nor creates another execution subsystem.

| Concern | Source at the audited base | Finding / next owner |
| --- | --- | --- |
| Public IDs and retention | crates/labby-gateway/src/upstream/pool/tasks.rs | Counter IDs, process-local map, 4096-entry bound and 24-hour idle pruning; B owns replacement |
| Creator-peer dependence | Same task module and pool/relay_cache.rs | TaskRoute retains RelayCachedConnection and prunes closed peers; B/C boundary |
| Authorization and adapters | pool/task_route.rs; crates/labby/src/mcp/server.rs:927-1008 | Caller/route checks and uniform task-not-found adapter mapping must survive |
| Server observation | pool.rs:527-583; upstream/types.rs | Peer/service/runtime identity are coupled; C owns normalized ServerSnapshot |
| Fencing | pool/incarnation.rs; pool.rs OAuth lifecycle guards; upstream/AGENTS.md | Reuse existing incarnation, credential epoch and publication guards, not a new authority shortcut |
| Existing end-to-end fixtures | crates/labby/examples/mcp_multihop_conformance.rs; scripts/ci/mcp-conformance.sh | Covers modern discovery, MRTR, Tasks and subscriptions, but not proven by a script-pin check |
| Version docs | docs/surfaces/RMCP.md:21 and expected-failures-extensions.yaml header | Existing 3.1 references disagree with the 3.3 fixture/pin; record for A, not edited here |

### Read-only prerequisite observation

The user-designated worktree is
/Users/jmagar/workspace/labby/.worktrees/issue-771-durable-task-routes on
codex/issue-771-durable-task-routes. Initial inspection saw
829fbac724897f301fbb2b3a8b8fd51f4ac97c27 plus substantial uncommitted changes.
Later read-only inspection saw c5ede54f655be73617e6f0a42064d4b50dd40e79 and a clean
status, reflecting concurrent work outside Gate 0. See the
[observation receipt](evidence/s01-proposal.json) and
[file hashes/status](evidence/s01-proposal.log). No Gate 0 command edited,
committed, reset or cleaned that checkout.

The inspected proposal has UUIDv4 IDs, metadata-only records, a versioned SQLite
store, owner/fingerprint checks, checked upstream timestamps and commit-before-ID
publication. Its live task path still retains the relay; do not call it C2/C3 or
restart-resumption proof. The ADR identifies row-revision, durable invalidation
and terminal-retention additions as B-owned coordination, not existing v1 fields.

### Fork versus official RMCP 3.4

| Reference | Exact identity |
| --- | --- |
| Product fork | 3.3.0 / b19cfc03025047153fa283c0064c651b6e2f623e |
| Independent stock conformance fixture | 3.3.0 / 3e636cab26c013eca5131103c03d20237f12c4df |
| Official rmcp-v3.4.0 | fd7811fdaa9fefa1c8034534b4d7a31c97204f89 |
| Merge base | 3e636cab26c013eca5131103c03d20237f12c4df |
| Graph divergence | Five fork-side commits, twelve official-side commits |
| Direct tree diff | 85 files, 2649 insertions, 1931 deletions |

Fork-side history includes 0665dcac (typed custom responses), 0e1184b4
(3.3 typed responses/Origin validation), 68e6f4a1 (typed compatibility),
2faf7626 (merge), and b19cfc03 (Unix response bounds). This is five commits,
not five independent product features. Official-side history includes
ServerConfig/ClientConfig and lifecycle/auth/cancellation-related work.

The [direct Git comparison](evidence/s01-sdk-direct-diff.log) and its
[command receipt](evidence/s01-sdk-direct-diff.json) are the tree-diff authority.
The [API comparison](evidence/s01-sdk34-compare.log) records graph divergence;
GitHub compare file lists use merge-base semantics and are not the direct tree diff.
[Fixture-to-fork metadata](evidence/s01-fork-patches.log) and
[official tag evidence](evidence/s01-official34.log) preserve the other identities.
A blind switch would also remove fork-side typed-response fixtures; equivalence
must be qualified, not inferred from the version number or patch titles.

**Gate 0 decision: hold the existing pin.** A1 owns the final upgrade/rebase/hold
qualification, and A2/A3 own parity/fixture changes. Neither manifests, lockfiles
nor runtime types were changed here. Isolated SDK fetch attempts first hit the
storage floor; a later small fetch and direct comparison succeeded. Those source
checks are not compilation or SDK behavioral conformance.

## S0.1/S0.2: exact initial-attempt command ledger

Each label has a JSON receipt and raw .log in evidence/. Receipts include command,
cwd, source HEAD, selected environment overrides, UTC times, exit status, stop
reason, byte count and SHA-256 of the exact log. An empty log with a capture stop
is not a passing test. Original receipts are preserved rather than overwritten.
Evidence-only .gitattributes keeps raw .log files byte-exact, with no newline
normalization or textual diff formatting. Final validation initially caught
trailing blank lines emitted by Cargo; the transcripts were not trimmed or
rehashed. Their raw bytes remain checked against the original receipts, while
normal source/Markdown whitespace checks remain enabled.

| Label | Exact command | Observed result |
| --- | --- | --- |
| s01-fmt | cargo fmt --all -- --check | Exit 0 |
| s01-toolchain | bash scripts/check-rust-toolchain-sync.sh | Exit 0; 1.97.1 contracts synchronized |
| s01-product-docs | python3 scripts/check-product-docs.py | Exit 0; 188 canonical and 17 maintained planning/feature docs |
| s01-links | python3 scripts/check-doc-links.py | Exit 0; 1088 local links and 65 fragments |
| s02-sdk-pin | bash scripts/ci/mcp-conformance.sh --check-sdk-pin | Exit 0; pin validation only |
| s02-harness-tests | python3 -m unittest scripts.ci.test_mcp_conformance_script scripts.ci.test_conformance_workflow | Exit 0; 22 tests |
| s01-check | cargo check --workspace --all-features --locked | Environment-blocked; exit -15, no compiler result |
| s02-gateway | cargo nextest run -p labby-gateway --all-features --locked --test-threads 2 | Environment-blocked; exit -15, no tests executed to a result |
| s02-mcp-unit | cargo test -p labby --all-features --locked --lib mcp:: -- --test-threads=2 | Environment-blocked; exit -15 |
| s02-protocol | cargo test -p labby --all-features --locked --test lifecycle_conformance --test mcp_spec_http_contracts --test mcp_spec_wire_compliance -- --test-threads=2 | Environment-blocked; exit -15 |
| s02-conformance | Command below | Environment-blocked; exit -15 |
| s01-docs-check | just docs-check | Environment-blocked; exit -15; generated freshness not verified |

Exact s02-conformance command:

~~~bash
MCP_CONFORMANCE_PORT=28172 MCP_CONFORMANCE_LABBY_PORT=28173 MCP_CONFORMANCE_DIRECT_PROXY_PORT=28174 MCP_CONFORMANCE_OUTPUT_DIR=target/mcp-conformance-gate0 bash scripts/ci/mcp-conformance.sh
~~~

The locked workspace check is the lockfile-preserving equivalent of just check.
The complete conformance script is intended to exercise the real product,
multi-hop MRTR/Tasks/subscriptions, dated server/client scenarios and extension
scenarios separately. The stock fixture remains distinct from the product fork.
The initial attempt produced no scenario report. The later successful retry and
its retained reports are recorded below; harness tests alone would not substitute.

### Existing declarations, observed failures and infrastructure blockers

The six build-dependent commands were terminated by capture.py's 3 GiB free-space
floor within 0.004-0.006 seconds, with zero-byte logs. This is a deliberate
resource stop, not an observed product regression. Space dropped from about
12 GiB at worktree creation to 4.9 GiB during identity capture, below the floor,
and later about 166 MiB. A subsequent shell attempt reported ENOSPC opening its
own output file. Space fluctuated as concurrent work continued. No other agent's
cache, worktree, process or changes were removed to obtain a green result.

The existing dated expected-failure file has empty server/client lists. The
extension file has no server exceptions and four outbound-client declarations:
auth/enterprise-managed-authorization, auth/dpop, auth/dpop-nonce and
auth/wif-jwt-bearer. These were initially source-only declarations; the successful retry subsequently
observed exactly these four expected failing scenarios (11 failed checks).
Their stale 3.1 header is a separate documentation finding. Results previously
reported from another worktree are not imported into this baseline.

The s01-owner-issues shell metadata attempt failed with HTTP 401 from GitHub's
GraphQL endpoint (exit 1). Issue evidence was recovered through the connected
github.issue_read tools and saved separately. This is not a Labby auth test.
SDK fetch/diff attempts stopped by the 0.25 GiB floor were superseded by the
successful direct comparison; their receipts remain in the evidence set.
The missing-binary failures described below were resolved by an explicit build
prerequisite, without source edits. No full-workspace nextest, Clippy, Rustdoc,
or Gate 1/2 live restart proof is claimed.

## Successful retries and final S0.2 baseline

After free space recovered to approximately 31 GiB, the same frozen source was
retested with new receipt labels. The initial stops and failed attempt remain
preserved; they are not overwritten or counted as passes.

| Receipt label | Exact command | Final outcome |
| --- | --- | --- |
| s01-check-r2 | cargo check --workspace --all-features --locked | Exit 0 |
| s02-gateway-r2 | cargo nextest run -p labby-gateway --all-features --locked --test-threads 2 | Exit 0; 1484 passed, 5 skipped |
| s02-mcp-binary-prerequisite | cargo build -p labby --all-features --locked --bin labby | Exit 0; supplies the standalone runner required by seven MCP tests |
| s02-mcp-unit-r3 | cargo test -p labby --all-features --locked --lib mcp:: -- --test-threads=2 | Exit 0; 571 passed, 0 failed, 2294 filtered out |
| s02-protocol-r2 | cargo test -p labby --all-features --locked --test lifecycle_conformance --test mcp_spec_http_contracts --test mcp_spec_wire_compliance -- --test-threads=2 | Exit 0; final per-binary summaries below |
| s02-conformance-r2 | The exact port-isolated s02-conformance command above | Exit 0; strict expected-failure baseline passed |
| s01-docs-check-r2 | just docs-check | Exit 0, including generated freshness and downstream documentation gates |

Protocol final summaries: lifecycle_conformance **73 passed / 8 ignored**;
mcp_spec_http_contracts **71 passed / 8 ignored**; mcp_spec_wire_compliance
**69 passed / 8 ignored**. Embedded supervisor child-test summaries are not added
again to these final per-binary totals. No ignored fixture is represented as passed.

The first actual product MCP run, s02-mcp-unit-r2, had 564 passes and seven
failures. All seven were the explicit missing target/debug/labby assertion in
crates/labby/src/mcp/handlers_tools/tests.rs:348. Building that binary and rerunning
the identical test command produced 571/0. This was a fresh-worktree prerequisite
omission, not a runtime source repair. Both failure and recovery logs are retained.

### Conformance reports and preserved gaps

[Manifest and per-check exceptions](evidence/conformance/manifest.json) and the
[exact report archive](evidence/conformance/reports.tar.gz) preserve every file
from target/mcp-conformance-gate0: 1,238,390 uncompressed bytes in an 83,536-byte
archive. The manifest lists each path, byte count and SHA-256 plus the archive
hash. It ties the reports to s02-conformance-r2 and the frozen source HEAD.
The capture log remains [separate raw command evidence](evidence/s02-conformance-r2.log).

| Suite | Scenario reports | Successful checks | Failed checks | Skipped checks |
| --- | ---: | ---: | ---: | ---: |
| Dated server | 40 | 115 | 0 | 0 |
| Tasks server extension | 10 | 35 | 0 | 1 |
| Dated client | 32 | 377 | 0 | 10 |
| Optional client extensions | 6 | 17 | 11 expected | 0 |

Counts are recorded check executions, not unique specification requirements.
INFO traffic records are not counted as successful checks. The 11 failed optional
client checks occur in exactly the four predeclared outbound-auth scenarios
listed above; the strict runner accepted that unchanged baseline. They are not
Labby's inbound OAuth-server results and are not unexpected regressions.

The stock tasks-status-notifications scenario explicitly skips because its
prior tools/call POST-SSE observer needs a subscriptions/listen rewrite. Preserve
this as an **A3/F conformance harness gap**, not proof of that scenario. Dated
client skips comprise eight standard-header checks and the optional roots and
sampling capability checks; IDs and source report paths are in the manifest.

The separate real-product multi-hop driver reports **Labby multi-hop conformance
passed**, including its existing MRTR/Tasks/subscription forwarding matrix.
Authenticated HTTP smoke gates and the isolated direct-proxy flow passed;
direct-proxy.json records successful result and child cleanup. This product
coverage complements, but does not erase, the stock notification-harness skip.
The conformance script's final SIGTERM line is its owned fixture shutdown after
success, not a failed scenario.

The source checkout has no built apps/web/out, producing a warning about empty
embedded web assets; this is backend/protocol evidence, not a UI build claim.
The macOS linker also emitted a compact-unwind size warning; the recorded commands
still exited zero. No compiler/test baseline exception was added or changed.

## Reproduction and validation boundary

Run capture.py with a fresh label from any cwd; it selects this worktree as cwd.
For example, after sufficient free build space is available:

~~~bash
python3 docs/tasks/issue-771-gate0/capture.py rerun-gateway 'cargo nextest run -p labby-gateway --all-features --locked --test-threads 2' --timeout 900
~~~

Do not reuse a receipt label. The default budget is two Cargo jobs, 600 seconds,
an 8 MiB log guard and a 3 GiB storage floor; the captured large-suite attempts
used 900 seconds. A zero storage floor was used only for small metadata/docs
operations, not the blocked builds. Early receipts precede the addition of the
configurable floor/helper hash; their fixed floor was 3 GiB, recorded in stop reasons.
An isolated SDK comparison cache can be recreated from the explicit fetch command
and product fork revision; cache deletion does not delete the recorded evidence.

Docs-only validation after authoring is recorded under validation-* labels.
It must include local links, product docs, evidence integrity and git diff --check.
Passing those establishes the documentation handoff, not conformance or an accepted ADR.
No generated metadata source changed. The subsequent full just docs-check retry
passed, including generated-doc freshness; final docs-only checks are recorded
again after this ledger update.

## Handoff and holds

Follow ADR 0007 for the schema/API, ServerSnapshot DTO, authenticated reacquisition,
fencing, transport guarantees, uniform authorization/errors and input ownership.
B owns the schema and AuthorizedRoute; C owns ConnectionLease and publication
guards; D owns task-scoped input/notifications, not #208's continuation store.

Before shared implementation: B/C must agree the schema revision/invalidation
migration and registration/observation guard handoff; B4 must choose numerical
admission/terminal-retention policy; A must qualify the fork delta before changing
SDK pins. pool.rs and task registration signatures are shared B/C seams;
mcp/server.rs needs A/D/E coordination. No other lane file was edited here.

The remaining formal gate conditions are explicit interface acceptance and ADR
merge by the authorized integrator. The successful baseline does not close the
retained fixture gaps or prove Gate 1/2 restart durability.
The user requested local-only work, so this run does not push, merge, mark the
issue complete or claim Gate 1/2. The dedicated Gate 0 worktree remains available
for other lanes to read this local handoff.
