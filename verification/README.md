# Verification Workspace

This is the isolated incubation workspace for the
[verification toolkit](../docs/plans/verification-toolkit/README.md).
The workspace scaffold and M1-M7 toolkit slices are implemented. The core parses
and validates JSON/TOML invariant catalogs, registers backend metadata, defines
target/result interfaces, and generates the invariant schema. M2 adds validated
scenario envelopes, content-addressed corpus insertion, target registration,
finite replay and normalization. M3 adopts these interfaces with the pure
[Labby browser request model](../crates/labby-model/) and the separate
`hosts/labby` binary. The isolated `verify-stateright` adapter performs bounded
BFS, and `verify-report` provides evidence-separated JSON/text/Markdown/HTML.
The remaining bounded adapters cover Loom/Shuttle, Kani, TLC, and Alloy with
exact release identities and finite CI tiers. A required test-only real-process lifecycle-conformance lane is described below;
broader Q1–Q6 product qualification remains separate.

Run from the repository root (Rust uses the root `rust-toolchain.toml`):

```sh
just verify-check
just verify-test
just verify-lint
just verify-deny
just verify-t0 # Labby catalog, golden coverage and model replay
just verify-t1 # bounded Stateright exploration, not product conformance
just verify-schema # regenerate after an intentional catalog contract change
```

Python 3.11+ is required for the workspace contract tests. `verify-deny` requires
cargo-deny and access to its advisory database. Missing tools are failures,
not silently skipped checks. These commands do not install external verifiers.

`Cargo.toml` and `Cargo.lock` belong to this workspace. The root product
workspace explicitly excludes it. Direct dependency versions are exact pins;
only dependencies actually consumed by a member enter the lockfile. Stateright,
Loom, Shuttle, and the Kani/TLC/Alloy adapter crates are consumed only in this
isolated graph. Core runtime dependencies are pure
serialization, TOML parsing, schema generation, and typed-error libraries.
JSON Schema validation defaults to no HTTP/file resolution or TLS stack.

`toolchain.toml` records exact external verifier identities and qualification
status. Kani, Alloy, and TLC are enabled only after actual-tool positive and
negative controls; their adapters authenticate the configured artifacts before
execution. Apalache remains disabled with an immutable image pin because no
container runtime was available for qualification. A recorded version or pin
is not evidence that a tool ran; T2/T3 retain separate execution artifacts.

## Core Catalog Contract

`Catalog::from_json` and `Catalog::from_toml` accept caller-supplied text and a
`BackendRegistry`, returning an immutable `ValidatedCatalog`. They reject
unknown fields, invalid IDs, namespace mismatches, duplicate IDs/handles,
unsupported kinds, unknown backends, and unresolved model-scoped handles.
Liveness requires declared fairness support. Validation never probes tool
availability or executes a backend; unavailable tools can still register their
metadata. Missing checks remain explicitly available through `uncovered()`.

Call `validate_evolution(previous)` when historical ID continuity matters.
The core does not load history implicitly: it can enforce retirement tombstones,
no reactivation, and stable ID/model/kind only against the supplied baseline.
Model registration belongs to `verify-runner`, with property kinds bound from
the validated catalog rather than selected by an untrusted scenario.

`ScenarioTarget::check` returns a `Result` so unknown properties and harness
errors cannot become passing observations. `InvariantResult::Incomplete` keeps
pending obligations distinct from `Holds`; `Verdict::Incomplete` retains the
explored bounds of a deadline-limited backend run. Empty struct variants retain
strict rejection of extra serialized fields, unlike Serde's tagged unit variants.

The generated [invariant schema](schemas/invariants.schema.json) is mirrored into
the original design folder by `just verify-schema`. Tests compare both artifacts
byte-for-byte with freshly generated output and validate positive/negative wire
examples. Namespace equality, registry resolution, nonblank strings, uniqueness,
and historical continuity are additional semantic checks, not schema promises.

Product Cargo commands do not build this workspace's runner, host or backend
members. The dev-facing `labby-model` member consumes only its pure core/scenario
leaves; product runtime crates must not depend on the model. Cargo configuration still
inherits from ancestor `.cargo/config.toml` and host configuration, as Cargo
normally does; this is workspace/dependency isolation, not a sandbox. The
contract tests check both workspace roots, membership separation, product
dependency boundaries, MSRV alignment, exact dependency pins, and truthful
qualified-or-disabled external-tool declarations. `.github/workflows/verification.yml` runs these checks and core
tests and the isolated cargo-deny audit in a separate advisory lane, including schema drift assertions without
regeneration. M3 corpus adoption adds an unconditional required T0 call through
`.github/workflows/verification-t0.yml`; it checks catalog and corpus with a
60-second replay limit separate from compilation. The host emits model-only
coverage/replay evidence, including backend-uncovered invariant IDs. Actual
product conformance remains C1.

## Bounded checking and reporting

`labby-verify t1 formal` resolves the five registered Stateright handles, then
checks the finite two-request/two-generation domain through depth 12, with at
most 20,000 states and 32 actions per state. Each run has a 50-second deadline
within a 280-second total; CI adds a 290-second process timeout and kill grace.
The callable T1 workflow runs every PR but remains outside `ci-gate` until its
runtime stability is established. `Bounded` means only this declared domain,
not universal proof; resource/deadline exhaustion is `Incomplete`. Unsupported
seeds and malformed bounds fail explicitly. Projected violations must also
replay against the registered model before they remain active counterexamples.

`verify-report` accepts caller-supplied evidence rather than reading ambient
repository state. Required skips, incomplete runs, failed cleanup and unavailable
provenance cannot qualify. Model checking, model replay, counterexample
reproduction, conformance, real-process tests, browser emulation and actual-host
checks remain distinct lanes. Absent lanes say **no evidence**. Replay gaps,
missing backend configuration, and absent backend execution have separate named
ID lists. A passing model-replay report does not qualify absent product lanes.

The host converts retained artifacts using
`labby-verify report-t0` or `report-t1`, followed by the input JSON, source-revision
file, source-dirty file, binary-SHA256 file, format (`json`, `text`, `markdown`,
`html`), and source name. These commands render evidence; their exit code reports
rendering success, not qualification. They validate structure/consistency, not
cryptographic authenticity. Preserve the original execution exit status.

T0/T1 workflows retain all four formats and attach Markdown to the job summary.
The callable `verification-report.yml` can publish that retained Markdown as a
PR comment only when a caller explicitly opts in with write permission; no
caller enables it by default. It does not execute repository code, cannot publish
for fork PRs, and bounds the comment input. Snapshot tests pin all four renderers.

## Scenario Replay (M2)

`verify-scenario` rejects duplicate JSON keys recursively before parsing a strict envelope with opaque object initial state and
opaque steps. Missing initial state means `{}`; `null` fails. Inputs are limited
to one MiB including the sealed representation, 10,000 steps, 256-byte project
and model identities and 128-byte invariant IDs. Rust-generated scenario schemas
are mirrored and drift-tested with the invariant schema.

Fingerprints are `b3:` plus 64 lowercase BLAKE3 hex digits over recursively
key-sorted compact JSON containing schema, project, model, invariant, initial,
steps and expectation. Array order and the expectation matter; discovery bounds,
provenance and reproduction status do not. Supplied hashes must match. This is
content identity, not an authenticity signature. Opaque identifiers normalize
only through an explicit project-owned `ScenarioTarget::canonicalize` hook.

`TargetRegistry::register` binds `(project, model)` to an implementation and its
catalogued invariant kinds. `replay` checks the initial state and every completed
step. Safety/security violations are sticky; harness failures and malformed
steps are errors. Undecidable observations remain incomplete. Liveness and
refinement return incomplete until explicit monitor adapters exist; finite replay
is never reported as an unbounded proof. Deadlines are **cooperative** around
trusted synchronous target calls, not interruption guarantees. Potentially
hanging or untrusted targets require an externally time-bounded process.

Only active expectation mismatches gate replay. Quarantined and unreproduced
evidence is always reported but does not fail the scenario lane. Matching
unreproduced traces suggest promotion without modifying files. Invalid/unreadable
envelopes cannot establish a trusted status and fail CLI loading.

`normalize` checks determinism at least three times before and after changes,
guards target-owned identifier renaming by replay, and delta-debugs only actually
reproduced counterexamples. Golden and unmatched traces never shrink. Changing
verdicts quarantine the raw trace; invalid/verdict-changing transformations are
discarded. Renaming cannot add or remove steps; only delta debugging may do that.
Active expectation mismatches remain active: normalization never classifies
existing regressions as non-gating evidence. Work is bounded by a default 256
replays (maximum 4096), with exhaustion
reported rather than claiming minimality. Commutative reordering is intentionally
deferred until a real target opts in, as specified in M2. All returned envelopes
are sealed; normalization does not write the corpus.

`insert_scenario` publishes a complete temporary file without overwriting an
existing content-addressed artifact. Existing provenance/status is retained on
dedup. Project/model path components are encoded and length-bounded; symlinks
below the caller-owned root are rejected. Root/ancestors must be trusted and not
concurrently replaced: this API is not a hostile-filesystem sandbox or a
power-loss durability guarantee.

An adopting binary registers its targets then calls the shared `run_cli` adapter:

```sh
verify replay path/to/scenario.json
# Runnable harness self-test host, NOT a real Labby model:
cargo run --manifest-path verification/Cargo.toml -p verify-runner --example replay-fixture --locked -- replay path/to/fixture.json
```

The generic `verify` shell intentionally has no product models and reports
unknown-target errors. `replay-fixture` registers only the test `example/counter`
target with `EX-COUNT-001` and steps shaped as `{"actor":"a","value":0}`.
CLI output is one JSON replay report per loaded file. Exit 0 means no active
mismatches, 1 an active mismatch/error, and 2 invalid usage/loading/output.
The loader rejects FIFOs/devices/directories before opening; caller-owned input
paths must not be concurrently replaced. Library target-resolution failures
retain distinct `UnknownTarget` and `UnknownInvariant` variants through
normalization rather than requiring callers to parse diagnostic strings.
Replay reports live in the runner for M2; M5 owns the later cross-backend report
crate and run manifests. Harness counterexample reproduction is not fixed-product
qualification; those require separate targets/runs in C1.

## Incident reduction and prefix exploration

The Labby host accepts a bounded structured lifecycle extract, not arbitrary
daemon text logs. An extract can supply modeled `step` records; unrelated outer
log fields are discarded. Source identifiers are replaced with canonical
request and generation labels before anything is emitted:

```json
{"schema":1,"events":[{"step":{"action":"connect","generation":"source-generation"}},{"step":{"action":"admit","request":"source-request"}},{"step":{"action":"cancel","request":"source-request"}}]}
```

The structured-log form supplies `generation` and `events` containing tracing
`fields` objects. The reducer selects `action = "browser.call.lifecycle"`,
starting at the selected `connected` generation and following explicit
`previous_generation_id` replacement links. It maps observed admission,
dispatch, completion, cancellation, timeout, invalidation and disconnect phases
without inventing missing dispatches. A missing connection start or inconsistent
replacement lineage fails closed. Independent connections and unrelated log
fields are excluded. Every structured-log transition must apply to the model;
out-of-order or duplicate terminal events fail closed. Infrastructure failures
before dispatch have distinct phases and normalize to `fail_before_dispatch`,
which cannot give credit for page execution. This does not infer identity across
process restarts or across an unlinked reconnect after disconnection.

The C1 fixture captures bounded, allowlisted events from the real daemon in each
case artifact. `scripts/ci/validate_incident_evidence.py` drives the independently
built host over all nine captures and checks the exact action prefix, all-applied
replay, canonical identifiers and bounded exploration. The required conformance
workflow retains both product and verifier binary digests. These are controlled
integration traces, not claimed reproductions of real production incidents.

```sh
cargo run --manifest-path verification/Cargo.toml -p labby-verify -- incident lifecycle.json LABBY-REQ-005
cargo run --manifest-path verification/Cargo.toml -p labby-verify -- incident-explore lifecycle.json LABBY-REQ-005
```

Inputs are limited to one MiB and 256 events. Invalid step payloads produce
static diagnostics without echoing their contents. Output contains a sealed
scenario, a source-content digest (not raw logs), and independent model replay.
A trace that does not reproduce remains `unreproduced`, with no T0 gate failure;
this is not a passing product regression. The commands do not write a corpus or
claim a source deployment revision. Retention and linkage to a fixed-product
regression require separately captured product evidence.

`incident-explore` forces every prefix step (explicit modeled-step inputs may
include rejected operations; structured authoritative logs may not),
before opening the existing finite two-request/two-generation action domain.
The full prefix remains in any projected counterexample. Exploration is bounded
by prefix length plus 12 transitions, 20,000 states, 32 actions per state and a
10-second backend deadline; projection replay shares a 15-second total budget.
Identities beyond the two canonical domain labels can occur in the prefix but
do not expand that action alphabet. Only `Bounded` exits successfully; a
projected violation must reproduce against the registered model before it is
reported as qualified. Neither command proves the production implementation.

## Real-process lifecycle conformance (C1)

The test-only adapter runs the compiled Labby daemon through authenticated HTTP
and browser WebSocket boundaries, with structured admission diagnostics and
read-only audit inspection inside its owned sandbox:

```sh
cargo test -p labby --all-features --test lifecycle_conformance conformance -- --test-threads=1
```

`verify-core::ConformanceTarget` compares independently collected observations
against a project-declared model relation. The caller owns implementation state
outside the cancellable comparison future and performs explicit cleanup after
success, error, or timeout, before assertions. The negative real adapter
deliberately diverges after a successful completion and retains that failing
boundary observation.

The `browser-public-v1` relation compares authenticated fixture generation
labels, admitted/dispatched request phases, received dispatch events, and
terminal outcomes. A held audit-write transaction and a correlated structured
admission event separate Admit from Dispatch. Exact terminal audit snapshots
prove cancellation terminalization and immutability after late completion;
the generic completion acknowledgement alone is not treated as rejection proof.
Success, error, timeout, document invalidation, disconnect and replacement
outcomes also require matching durable terminal evidence.

Nine required artifacts cover success, tool error, timeout, document
invalidation, disconnect, replacement ownership, pre/post-dispatch cancellation
with late completion, and the negative adapter. `LABBY_CONFORMANCE_EVIDENCE_DIR`
retains each controlled trace, fingerprint, boundary observations, source/binary
identity, and cleanup result. Positive traces must compare every position;
the negative self-test must expose its exact expected divergence, not fabricate
passing observations.

The reusable `verification-conformance.yml` workflow is an unconditional
required CI gate. Its five-minute execution limit excludes compilation, and
its validator rejects missing cases, altered action sequences, provenance/hash
mismatches, incomplete observations and cleanup failures. Setup and individual
scenarios have their own bounds. This qualifies the declared controlled
observation relation, not arbitrary concurrency, unbounded liveness, or the
broader Q1–Q6 product matrix.
