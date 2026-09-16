---
title: "Verification and Compliance"
created: "2026-09-13"
updated: "2026-09-13"
---

# Verification and Compliance

This is the canonical contributor and operator guide to Labby's verification
toolkit and specification-compliance system. It explains what each artifact
proves, how evidence receives credit, how to run and interpret the gates, and
how to extend the system without overstating coverage.

The two systems described here are related but intentionally separate:

- the MCP compliance system connects pinned external requirements to reviewed
  applicability decisions and exact product tests;
- the reusable Rust verification workspace models invariants and replays or
  explores bounded scenarios.

Neither model evidence nor a catalog entry proves production behavior. Product
qualification requires evidence from the boundary named by the claim.

## Vocabulary

| Term | Meaning |
| --- | --- |
| Normative source | Immutable upstream specification bytes identified by Git revision, path, and SHA-256. |
| Requirement | One mechanically extracted prose obligation or structural JSON Schema constraint. It is an inventory row, not a verdict. |
| Denominator | All extracted requirement rows that must be reviewed. Rows are deliberately granular and are not a count of unique features. |
| Disposition | Human-reviewed decision that a requirement is `applicable`, `conditional`, or `not_applicable` to Labby, with rationale and review reference. |
| Evidence cell | Exact `(role, transport, level)` evidence required by an applicable disposition. |
| Behavior contract | A stable expected outcome at a named boundary, normally encoded in a focused test and documented at the owning product surface. |
| Oracle | An independently reviewable mapping from one exact executable test to requirements, expected behavior, scope, level, and timeout. |
| Receipt | Machine-readable record of the exact oracle commands and their outcomes, bound to source, catalogs, dependencies, and toolchain. |
| Coverage report | Derived per-requirement outcomes and counts. It is reproducible from catalogs plus a valid receipt. |
| Invariant | A property that must hold for every modeled state within a declared model and bound. |
| Scenario | A versioned sequence of inputs and expected observations that can be normalized and replayed. |
| Qualification | A claim supported by the required evidence lane. Passing a weaker lane never substitutes for the required lane. |

Use these names precisely. A Rust assertion can implement a behavior contract;
it becomes a compliance oracle only after `mcp-spec-oracles.json` maps it to a
pinned requirement with the correct scope. An oracle that has not run has no
execution evidence.

“PLA+” is informal shorthand for this broader assurance effort, not another
verdict or evidence level. Use the concrete lane in durable records: T0 replay,
T1 bounded checking, C1 real-process lifecycle conformance, an MCP oracle's
role/transport/level, a provider fixture, or actual-host evidence. This avoids
turning the umbrella name into an unsupported claim that all lanes passed.

## Architecture and ownership

The MCP pipeline is an exact seven-stage chain:

```text
1. pinned specification bytes
  -> 2. deterministic prose and schema inventories
  -> 3. reviewed applicability dispositions
  -> 4. required role/transport/evidence cells
  -> 5. exact executable oracle mappings
  -> 6. source-bound execution receipt
  -> 7. derived coverage report and gate result
```

Committed intent and generated facts live under `conformance/`:

| Artifact | Ownership and mutation rule |
| --- | --- |
| `mcp-spec-sources.json` | Pins the MCP revision and every source page hash. Update only as part of a reviewed spec-version migration. |
| `mcp-spec-requirements.json` | Deterministically generated prose inventory. Never hand-edit wording or inferred metadata. |
| `mcp-spec-schema.json` | Deterministically generated structural schema inventory. Never hand-edit extracted constraints. |
| `mcp-spec-dispositions.json` | Reviewed Labby applicability, rationale, review reference, and required evidence cells. Hand-maintained. |
| `mcp-spec-oracles.json` | Reviewed exact test mappings, expected outcomes, scopes, evidence level, and timeout. Hand-maintained. |

The executable coordinator is `scripts/ci/mcp_spec_compliance.py`. Exact Python
test selection is fail-closed in `scripts/ci/mcp_oracle_runner.py`; Cargo
oracles use exact nextest package, target, and test selectors. Extractor and
reporter unit tests live beside the CI scripts.

The separate [`verification/` Cargo workspace](../../verification/README.md)
owns generic catalog, scenario,
runner, report, and backend contracts. `verification/hosts/labby` owns Labby's
model and adapters. Product wire, browser, provider, and actual-host tests stay
in their product test suites; they must not be relabeled as model evidence.

## The normative denominator and the 2,223 count

For MCP 2026-07-28, the current extracted denominator has 2,223 rows: 927
prose requirements plus 1,296 structural schema constraints. This means 2,223
granular source obligations are present for review. It does **not** mean:

- Labby has 2,223 bugs;
- Labby has promised to implement every row;
- 2,223 unique user-visible features exist; or
- an unmapped row has failed a test.

The current catalog has nine reviewed dispositions and nine registered oracles,
leaving 2,214 rows unreviewed before considering execution state. Initially
crediting five requirements while reporting 2,218 unresolved meant
that five rows had the full chain of reviewed applicability, sufficient oracle
scope, and passing bound execution evidence. The other rows still needed one
or more of: applicability review, a justified not-applicable disposition, an
oracle, the required evidence scope, or a fresh passing run. Always inspect the
per-outcome counts instead of subtracting a single headline number.

The denominator can contain multiple obligations about the same behavior and
schema constraints that do not apply to every Labby role or transport. Those
rows still require explicit review. Bulk-marking them applicable or
not-applicable without source-by-source reasoning defeats the audit trail.

## Applicability and evidence scope

A disposition is valid only when it is bound to the current requirement digest
and contains a non-empty rationale and review reference. Source changes make
the disposition stale instead of silently transferring the old decision.

- `applicable` requires at least one evidence cell.
- `not_applicable` requires no evidence cells and a product-specific reason.
- `conditional` remains unresolved until the condition and evidence policy are
  reconciled; it does not receive compliance credit.
- absent dispositions remain `unreviewed`.

Evidence cells use exact values rather than a strength hierarchy. A
`product_wire` server/HTTP oracle does not satisfy client/stdio, proxy/HTTP,
component, model, browser, pinned-provider, or actual-host evidence. One oracle
may map to multiple requirements only when the same execution genuinely tests
each obligation. One requirement may need multiple oracles to fill all cells.

This prevents common false claims: mocked OAuth is not Google or Authelia host
evidence; a socket bridge simulator is not an installed WebMCP browser bridge;
an MCP Apps emulator is not OpenAI or Anthropic host evidence; and model replay
is not real-process conformance.

## Behavior contracts and executable oracles

A good behavior contract names observable inputs, outputs, side effects,
negative cases, and cleanup. The associated oracle should:

1. test the real boundary required by its evidence cell;
2. use a fully qualified selector resolving to exactly one test;
3. establish a meaningful success baseline before the negative probe when that
   distinguishes rejection from a dead fixture;
4. assert status, typed error or wire shape, and side-effect cardinality;
5. cover adversarial variants where parsing or security boundaries matter;
6. own deterministic fixture cleanup; and
7. complete inside a reviewed timeout.

Oracle registration records the expected outcome in prose so reviewers can
detect a test whose name exists but whose assertions no longer prove the
requirement. Python selectors must resolve to one successful, non-skipped test.
Cargo selectors that execute zero tests fail. Expected failures, unexpected
successes, timeouts, and skipped tests never count as passes.

The runner starts each oracle in its own POSIX process group. On timeout it
terminates descendants as well as the Cargo or Python parent. That containment
is a safety net, not a replacement for fixture-owned shutdown and assertions.

## Evidence lifecycle

`run` deletes the declared old receipt and report before validation or
execution. This ensures a failed attempt cannot leave earlier green artifacts
available for upload. It writes JSON atomically and does not copy test output
into retained evidence, reducing the risk of persisting secrets from failures.

Evidence is bound to:

- the source, requirement, schema, disposition, and oracle catalogs;
- all tracked and untracked non-ignored worktree bytes, including deletions;
- the resolved locked Cargo graph;
- dirty external path-dependency worktrees; and
- the complete `rustc -vV` toolchain identity.

The dependency graph is prepared online, then fingerprinted through locked,
offline Cargo metadata. A source or dependency change during execution rejects
the receipt. `report` accepts an existing receipt only when its binding and
every recorded command still match. Receipts are local execution evidence, not
signed attestations, exhaustive proofs, or proof of deployment.

By default the retained files are:

```text
target/mcp-spec-compliance/receipt.json
target/mcp-spec-compliance/report.json
```

Custom `--receipt` and `--output` paths must be distinct. CI uploads the entire
directory as the `mcp-spec-compliance` artifact.

## Running the MCP gates

Use the repository's MCP specification Just recipes with a checkout whose HEAD
exactly matches `conformance/mcp-spec-sources.json`. They default to
`target/mcp-spec-source`, or accept a checkout path as their final argument.
The recipe list in the Justfile is the command source of truth:

```bash
# Materialize and authenticate the default immutable source checkout.
just mcp-spec-source

# Intentionally rewrite the generated prose and schema inventories.
just mcp-spec-inventory /path/to/modelcontextprotocol

# Validate source extraction, catalogs, mappings, and coordinator tests.
just mcp-spec-check /path/to/modelcontextprotocol

# Check intent, then execute all registered oracles.
just mcp-spec-verify oracles /path/to/modelcontextprotocol

# Equivalent CI-safe aggregate entry point.
just mcp-spec-gate /path/to/modelcontextprotocol

# Rebuild a report from an existing matching receipt without rerunning tests.
just mcp-spec-report /path/to/modelcontextprotocol

# Refresh and print the same report counts without failing merely because the
# denominator is still incomplete. Invalid or stale evidence still fails.
just mcp-spec-summary /path/to/modelcontextprotocol

# Strict denominator-wide gate; expected red while coverage is incomplete.
just mcp-spec-compliance /path/to/modelcontextprotocol
```

`just mcp-spec-verify check`, `oracles`, and `full` provide the three named
tiers. `mcp-spec-inventory` is a mutation and belongs only in an intentional
spec migration; ordinary checks never regenerate committed catalogs.
`mcp-spec-report` preserves the strict compliance exit status and therefore
normally exits 1 while coverage remains incomplete. `mcp-spec-summary` is the
explicitly non-gating operator view: it accepts only exit 0 (complete) or exit
1 (valid report, incomplete compliance), while source, catalog, binding, and
receipt errors still exit 2 and fail the recipe. Neither command runs oracles.

The supported workflow has three stages:

1. **Check intent.** Re-extract in memory and byte-verify the pinned source,
   validate all
   catalogs, and run coordinator tests. This prints `Valid intent`; it is not a
   compliance result.
2. **Run registered oracles.** Execute every registered oracle and write bound
   evidence. The oracle gate succeeds when every mapped oracle passes, while
   uncovered requirements remain visible as gaps.
3. **Run the strict compliance gate.** Require every denominator row to be
   either passed with sufficient scoped evidence or reviewed not applicable.
   Until mapping is complete, this gate is expected to remain red.

For narrow development, the coordinator supports repeatable `--oracle ID`
selectors. A selected run is diagnostic evidence; the oracle-only gate requires
the complete registered set, and the strict report still evaluates the entire
denominator. Run the full registered set before making a suite-level claim.

Calling the Python coordinator's `check` mode without `--spec-checkout`
validates schemas and internal catalog relationships only. Its output explicitly
says source completeness was **not** checked. The supported Just check always
supplies a checkout and therefore performs the stronger immutable-source check.

## Reading the coverage report

The report's `compliant` field is true only when every requirement outcome is
`passed` or `not_applicable`. Requirement outcomes mean:

| Outcome | Interpretation | Next action |
| --- | --- | --- |
| `passed` | All required cells have mapped oracles and every linked oracle in the receipt passed. | Preserve evidence; review again if the source or boundary changes. |
| `not_applicable` | Reviewed disposition says Labby does not own this obligation. | Revisit if product roles, transports, or features change. |
| `applicability_unresolved` | Disposition is absent, unreviewed, or conditional. | Review source and product behavior; add a disposition. |
| `missing_oracle` | Applicable requirement has no mapped executable oracle. | Add or identify the correct boundary test, then register it. |
| `insufficient_evidence_scope` | Oracles exist but do not fill every exact evidence cell. | Add the missing role/transport/level lane. |
| `not_run` | Suitable mappings exist, but the current valid receipt lacks passing results for all of them. | Execute the complete registered suite. |
| `failed` | At least one linked oracle failed or timed out. | Triage the product, fixture, or contract; never rewrite the disposition to hide it. |

`registered_oracles_passed: true` means only that the currently registered
tests passed. It may coexist with thousands of unresolved rows. Only
`compliant: true` supports a full-compliance claim, and even that claim is
limited to the pinned version, reviewed Labby roles/transports, and retained
evidence boundaries.

## Verification workspace and tiers

The reusable verification workspace supplies complementary model evidence:

| Tier | Cadence | Contents | Claim boundary |
| --- | --- | --- | --- |
| T0 | every PR, hard gate | Catalog validation, deterministic scenario replay, coverage/report validation | Bounded model and replay evidence only. |
| T1 | every PR, callable/advisory until stable | Bounded Stateright search and Loom harnesses | Finite state/concurrency exploration, not production execution. |
| T2 | nightly | Kani bounded proofs and longer Shuttle runs | Bounded proof/exploration with retained tool identity. |
| T3 | nightly or weekly | Pinned Alloy and TLC/Apalache, deeper Stateright, fuzz-to-scenario | Formal-tool/model evidence at declared versions and bounds. |

The top-level verification recipes check, test, lint, audit, generate schemas,
and invoke the available tiers. CI stores source revision, dirty state, binary
hash, exact backend releases, bounds/deadlines, results, and rendered reports.
Missing evidence remains explicit. A backend registered in a catalog but not
executed does not receive credit.

The local recipe-to-evidence mapping is exact:

| Recipe | Executes | Claim and prerequisites |
| --- | --- | --- |
| `just verify-t0` | `labby-verify t0 formal` | Model catalog and deterministic replay only. |
| `just verify-t1` | `labby-verify t1 formal` | Bounded Stateright exploration only. |
| `just verify-c1` | Serial `lifecycle_conformance` product test | Real-process controlled lifecycle relation; use the workflow for retained CI provenance. |
| `just verify-t2-shuttle` | `verify-loom` Shuttle lifecycle controls | Local bounded schedules; no external installation. |
| `just verify-t2-kani` | Ignored `actual_kani` controls | Requires executable Kani 0.67.0 in `LABBY_KANI_DRIVER`; the recipe does not install it. |
| `just verify-t3-formal` | Ignored TLC and Alloy actual-tool controls | Requires authenticated TLA+ 1.7.4 and Alloy 6.2.0 jars in the named environment variables; the recipe does not download them. |

The canonical CI workflows remain `.github/workflows/verification-t0.yml`,
`verification-t1.yml`, `verification-conformance.yml`,
`verification-t2.yml`, and `verification-t3.yml`. T2/T3 workflows acquire and
hash their exact tool releases and retain evidence. A local recipe pass alone
does not reproduce those workflow provenance artifacts. Apalache remains
workflow-reported as unavailable because no qualified container execution is
configured; no local recipe silently substitutes for it.

Scenario lifecycle and gate status are separate. An `active` scenario whose
observed result contradicts its expectation fails T0. `quarantined` and
`unreproduced` scenarios remain visible but do not become passing evidence or
silently change expectations. Promotion is reviewed, never automatic.

## CI integration

CI has distinct jobs because their claims differ:

- the MCP specification job validates the immutable checkout and runs the
  complete registered oracle gate, then uploads its receipt/report;
- verification T0, T1, conformance, T2, and T3 workflows retain their own
  model, real-process, or tool-specific evidence;
- the MCP authorization lane currently has a separate 132-row normative matrix,
  coverage manifest, and runner;
- the OpenAI authorization lane has a separate 21-clause captured matrix and
  execution script; these older auth systems store reviewed outcomes in their
  committed matrices, unlike the newer receipt-derived full-spec pipeline; and
- OpenAI tools/connectors/MCP host behavior needs its own pinned or dated
  denominator and evidence mappings. The authorization matrix does not cover
  those host-specific tool, approval, result, or MCP Apps obligations.

Keep these lanes separate in reports. A required lane that is unavailable must
say `no evidence` or remain incomplete; another lane's green result cannot
waive it. Credential-gated actual-provider or host tests must never print or
retain credentials.

## Failure triage

Classify the failure before editing behavior or evidence:

1. **Source mismatch:** checkout revision, page set, blob, line span, or
   extraction differs. Use the pinned revision and regenerate only through the
   extractors. Do not weaken hashes.
2. **Stale intent:** disposition digest, oracle requirement ID, selector, or
   command no longer matches. Re-review the changed source/contract and update
   the hand-maintained mapping with a review reference.
3. **Stale receipt:** worktree, catalog, dependency, toolchain, or command
   binding changed. Rerun; do not copy or edit the receipt.
4. **Zero/ambiguous selection:** the exact selector resolved to none or multiple
   tests. Fix registration or test naming.
5. **Product regression:** the asserted wire or side-effect behavior changed.
   Reproduce the exact test, fix the lowest owning layer, then rerun its crate
   and registered suite.
6. **Fixture/containment failure:** readiness, cleanup, port, process, or timeout
   behavior failed independently of the assertion. Preserve diagnostics, fix
   deterministic ownership, and rerun. Do not call it a product pass.
7. **Coverage gap:** registered tests pass but applicable or unresolved rows
   remain. Continue review and mapping; do not change the full gate.

Never obtain green by broadening selectors, accepting skipped/zero tests,
lowering evidence scope, changing an applicable requirement to not applicable
without product reasoning, or describing the oracle-only gate as compliance.

## Extending Labby coverage

For each new behavior or existing uncovered requirement:

1. locate the exact pinned requirement and read its surrounding specification;
2. identify Labby's role, transport, feature condition, and owning code layer;
3. add a digest-bound disposition with rationale and review reference;
4. define the exact evidence cells before choosing a test;
5. write or strengthen the focused behavior test at that boundary;
6. register the smallest exact oracle and explicit expected outcome;
7. run catalog/coordinator tests, the oracle, its crate suite, and then the full
   registered-oracle gate;
8. inspect the new report counts and retain the receipt; and
9. update the owning product documentation when public behavior changed.

When advancing to a new MCP revision, update the pinned source manifest and
machine schema, regenerate both inventories, and treat every changed hash,
removed row, new row, disposition, and mapping as review work. Do not transfer
old credit solely because an ID or sentence looks similar. Preserve the old
revision's released evidence outside the current default report when historical
compatibility is still claimed.

## Extending `labby-auth`

OAuth coverage should use the same discipline with the existing auth-specific
normative inventories and runners:

- separate public authorization-server, protected-resource, upstream OAuth,
  and identity-provider roles;
- map RFC/MCP/OpenAI obligations to focused deterministic tests;
- require negative tests for issuer, audience/resource, state, PKCE, redirect,
  scope, expiry/not-before, refresh rotation/replay, revocation, and sanitized
  challenges;
- use the pinned Authelia lane for real provider integration while preserving
  deterministic HTTP doubles for precise failures;
- keep live Google or other provider evidence credential-gated, dated,
  non-secret, and distinct from deterministic conformance; and
- preserve `labby-auth`'s standalone dependency contract when tests are run
outside the Labby workspace.

The authorization runners have deliberately separate identifiers and command
semantics:

| Command | Identifier namespace | Meaning |
| --- | --- | --- |
| `just mcp-auth-list` | `MCP-2026-AUTH-INDEX-*` and section-specific `MCP-2026-AUTH-*` matrix row IDs | List all 132 normative denominator rows; does not execute tests. |
| `just mcp-auth-validate` | entire MCP authorization matrix | Validate provenance, actors, dispositions, aggregate links, coverage projection, evidence paths, and exact test resolution; does not execute tests. |
| `just mcp-auth-resolve MCP-2026-AUTH-INDEX-001` | one exact matrix row ID | Print the deduplicated exact tests behind a direct or aggregate row; does not execute them. |
| `just mcp-auth-oracles` | complete MCP authorization matrix | Validate and execute every mapped repository and pinned-rmcp test. |
| `just mcp-auth-oracles MCP-2026-AUTH-INDEX-001` | one exact matrix row ID | Validate the complete matrix, resolve the selected row, and execute its exact tests. |
| `just openai-auth-list` | `OAI-AUTH-001` through `OAI-AUTH-011` | List executable OpenAI authorization verification groups; these are not the `OAI-CLAUSE-*` source rows. |
| `just openai-auth-oracles` | all `OAI-AUTH-*` groups | Execute the optimized exact-test aggregate plus the backup/restore drill. |
| `just openai-auth-oracles OAI-AUTH-NNN` | one exact verification group | Execute that group's exact tests; unknown IDs fail. |

The 21 `OAI-CLAUSE-*` entries in
`conformance/openai-auth-normative.json` are source obligations. They map to
the 11 executable `OAI-AUTH-*` groups, so a clause ID is not accepted by the
OpenAI execution recipe. The OpenAI shell runner supports listing and exact or
complete execution, but has no separate validation-only mode; its matrix
relationships are checked by repository tests and the execution selectors fail
closed on zero matches.

An external provider's availability or branding never proves Labby's token
validation. Conversely, deterministic validation tests do not prove that a live
provider's current metadata and registration still interoperate.

## Extending OpenAI host coverage

`conformance/openai-auth-normative.json` is the reviewed OpenAI MCP connector
**authorization** denominator. It is not a denominator for the complete OpenAI
tools/connectors/MCP guide. Keep source excerpts short, pinned or dated, and map
each applicable auth clause to exact verification IDs. Build a separate
tools/connectors denominator for host-specific requirements instead of silently
folding them into the 21 auth clauses. OpenAI host journeys must add a separate
actual-host evidence lane recording host/version, detected capabilities,
authorization lifecycle, tool/resource behavior, approval and error behavior,
MCP Apps render and callback outcomes where applicable, and teardown—without
retaining tokens or customer data.

Do not use OpenAI success to claim Anthropic compatibility. Give Anthropic and
future hosts separate capability detection, expectations, fixtures, and
actual-host receipts. Likewise, MCP protocol compliance does not automatically
satisfy host-specific policy, OAuth registration, UI, or metadata rules.

## Review checklist

Before accepting a new disposition or oracle, confirm:

- source revision and requirement digest are current;
- applicability rationale describes Labby, not merely the spec text;
- every claimed role, transport, and evidence level is explicit;
- expected behavior includes negative outcomes and side effects where relevant;
- the selector executes exactly one non-skipped test;
- fixture readiness and cleanup are asserted;
- no secret-bearing output is retained;
- a fresh bound receipt exists for execution claims;
- report wording distinguishes registered-oracle success from full compliance;
  and
- public behavior and operator commands are documented at their canonical
  owner.

## Related documentation

- [Testing](./TESTING.md) defines repository-wide test-layer ownership and TDD.
- [MCP conformance](../surfaces/MCP_CONFORMANCE.md) defines Labby's public MCP
  version and protocol behavior.
- [Transport](../surfaces/TRANSPORT.md) defines stdio and Streamable HTTP
  behavior.
- [OAuth](../runtime/OAUTH.md) defines runtime authorization behavior.
- [Verification workspace](../../verification/README.md) documents concrete
  model/replay commands and formats.
- [Verification toolkit design](../plans/verification-toolkit/README.md)
  preserves architecture decisions and qualification planning.
