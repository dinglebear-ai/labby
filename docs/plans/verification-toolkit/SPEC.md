# Verification Toolkit Specification

## 1. Goal

Make correctness properties of Rust systems projects **named, catalogued,
checked by multiple backends, and reproducible as replayable scenarios** — with
the orchestration shared across projects and only the domain models private to
each project.

A counterexample from a model checker, a fuzzer, and a production incident must
all reduce to the same artifact: a scenario file that a runner can replay
against a project-supplied target.

## 2. Layering Contract

Three layers, strictly ordered. Each layer may depend only on layers above it.

| Layer | Owns | Lives in |
| --- | --- | --- |
| L1 — Infrastructure | catalog schema, scenario format, normalization, replay, reporting, CI helpers, backend adapters | toolkit repo |
| L2 — Domain patterns | reusable state-machine traits: generation/version semantics, single-flight, publication/snapshot, request lifecycle, cancellation, retry/idempotency, ownership/lease | toolkit repo, opt-in crates |
| L3 — Project models | concrete state machines, invariant catalogs, Alloy/TLA+ specs | adopting project |

Rules:

1. L1 must not know any domain vocabulary. No `upstream`, `mount`, `session`.
2. L2 is optional. A project that implements only L1 traits is a full citizen.
3. L3 never lives in the toolkit repo. Labby's gateway model stays in Labby.
4. A change that requires touching L1 to support one project's domain is a
   design bug in L1. Fix the abstraction or keep the behavior in L3.

## 3. Crate Boundaries

| Crate | Responsibility | May depend on |
| --- | --- | --- |
| `verify-core` | invariant identity, catalog parse/validate, verdicts, `ScenarioTarget`, backend-capability vocabulary | serde, serde_json, thiserror, toml; schema generation via schemars |
| `verify-scenario` | scenario envelope, step encoding, normalization, shrink-stability, on-disk corpus layout | `verify-core` |
| `verify-runner` | discovery, replay engine, target registry, backend registry, orchestration, `verify` CLI | `verify-core`, `verify-scenario`, `verify-report` |
| `verify-report` | coverage matrix, text/JSON/HTML/Markdown renderers, CI summary | `verify-core`, `verify-scenario` |
| `verify-stateright` | Stateright backend adapter + counterexample extraction | `verify-core`, `verify-scenario`, `stateright` |
| `verify-kani` | Kani harness conventions, catalog binding, result ingestion | `verify-core`, `verify-scenario` |
| `verify-loom` | Loom/Shuttle concurrency harness conventions, interleaving capture | `verify-core`, `verify-scenario` |
| `verify-alloy` | Alloy invocation, instance → scenario projection | `verify-core`, `verify-scenario` |
| `verify-tla` | TLC/Apalache invocation, error-trace → scenario projection | `verify-core`, `verify-scenario` |
| `verify-macros` | `invariant!`, `scenario_test!` and friends | extracted last, never first |

`verify-report` needs `verify-scenario` because the coverage report counts
scenarios by `expect` and `status`, which are scenario-envelope vocabulary.

`verify-runner` depends on **no backend crate**. The `Backend` trait (§7) lives
in `verify-core`, and the adopting project's own binary registers the backend
implementations it wants into the runner's registry. The dependency is inverted
on purpose: it is what lets a project take Stateright without taking a Java
toolchain, and it is the same reason the target registry is project-populated
rather than path-convention-resolved (§6).

`verify-core` is the dependency leaf and stays transport-free, filesystem-free,
and env-free — the same discipline `labby-primitives` and `labby-apis` already
carry in this workspace.

Backend crates are separate crates specifically so that a consumer pays for
neither a Java toolchain nor a CBMC install unless it opts in.

### 3.1 Pinned upstream versions

Researched 2026-09-11. The toolkit pins these in its own workspace and in a
`toolchain.toml` manifest for the non-Cargo tools:

| Tool | Version | Notes |
| --- | --- | --- |
| `stateright` | 0.31 | explicit-state/BFS model checking of Rust state machines |
| `kani-verifier` | 0.67 | bit-precise bounded proof, CBMC backend |
| `loom` | 0.7 | exhaustive interleavings under the C11 memory model |
| `shuttle` | 0.9 | randomized interleavings where `loom` state-space-explodes |
| `proptest` | 1.11 | structured generation + shrinking for scenario synthesis |
| `arbitrary` | 1.4 | shared derive for step/state generation |
| `schemars` | 1.2 | generate the published JSON Schemas from the Rust types |
| `jsonschema` | 0.56 | validate third-party catalogs/scenarios in CI |
| `insta` | 1.48 | snapshot the coverage report |
| Alloy | 6.2.0 | jar, pinned by checksum |
| TLC / Apalache | pinned by container digest | no host-toolchain assumption |

`loom` and `shuttle` are complements, not alternatives: `loom` is exhaustive and
cheap only on small harnesses; `shuttle` is the fallback tier for harnesses that
blow up. The backend crate `verify-loom` exposes both behind one adapter.

## 4. Invariant Catalog

A project declares its properties in `formal/invariants.toml`. The format is
project-agnostic; identity is namespaced by the project, and the tooling never
interprets the namespace.

```toml
schema = 1
project = "labby"
namespace = "LABBY"

[[invariant]]
id = "LABBY-REQ-001"
title = "A request has at most one authoritative terminal outcome."
kind = "safety"            # safety | liveness | security | refinement
severity = "critical"      # critical | high | medium
model = "gateway"
owner = "gateway"
status = "active"          # active | draft | retired

[invariant.checks]
stateright = ["request_lifecycle"]
kani       = ["request_transition"]
loom       = ["request_completion"]
tla        = ["RequestLifecycle"]
# alloy intentionally absent: uncovered by that backend, and that is reported
```

Contracts:

1. `id` is globally unique within a catalog and stable forever. Retiring an
   invariant sets `status = "retired"`; it never frees the id.
   Every `id` must begin with the catalog's declared `namespace` followed by
   `-`. JSON Schema cannot express that dependency, so it is a hard
   catalog-validation rule in `verify-core`, not merely a convention: without
   it, `DRIVE-PERM-003` validates cleanly inside Labby's catalog and two
   projects can collide on one id while both pass CI.
2. `kind` constrains which backends may legitimately claim it. A liveness
   property cannot be discharged by Kani (bounded, no fairness); the runner
   rejects such a binding at catalog-validation time rather than silently
   reporting green.
3. `checks` names *handles*, not file paths. Each backend adapter resolves its
   own handles and fails loudly on an unresolved one — a typo must never
   degrade to "uncovered but nobody noticed". The key set is open: the schema
   does not enumerate backend ids, because enumerating them means every new
   backend is an L1 schema bump and extraction gate 3 (§14) can never be met.
   An unknown key is rejected by `verify-core` against the *registered* backend
   set, where the error can name what is actually available.
4. Coverage is a first-class output. An invariant with zero resolvable checks is
   reported `uncovered`, which is a legitimate state to ship with, as long as it
   is visible.

### 4.1 Verdicts

```
Verified   backend discharged the property under stated bounds
Falsified  backend produced a counterexample (scenario attached)
Bounded    verified only up to a bound the backend reports
Skipped    backend unavailable in this environment (tool not installed)
Error      backend failed for reasons unrelated to the property
Uncovered  no backend claims this invariant
Incomplete backend stopped before completing its declared search; explored bounds retained
```

`Bounded` is distinct from `Verified` on purpose. A Kani proof at `k = 5` and a
TLC run over a 3-node configuration are not universal claims, and the report
must not print them as if they were.

## 5. Scenario Envelope

The most reusable piece. A scenario is a portable, replayable, project-agnostic
envelope around project-specific steps.

```json
{
  "schema": 1,
  "project": "labby",
  "model": "gateway",
  "invariant": "LABBY-REQ-001",
  "origin": {
    "kind": "stateright",
    "tool_version": "0.31.0",
    "discovered_at": "2026-09-11T00:00:00Z",
    "seed": 12345
  },
  "bounds": { "depth": 14, "actors": 3 },
  "initial": {},
  "steps": [],
  "expect": "invariant_violated",
  "fingerprint": "b3:9f2c…"
}
```

Contracts:

1. `initial` and `steps[]` contents are **opaque to the toolkit**. Only the
   project's `ScenarioTarget` interprets them.
   `initial` is optional and an absent `initial` is normalized to `{}` before
   `ScenarioTarget::init` is called, so `init` always receives a JSON object and
   never has to distinguish absent from `null`. A literal `null` is rejected at
   scenario-validation time rather than silently coerced.
2. `origin.kind` is one of `stateright | kani | loom | shuttle | alloy | tla |
   fuzz | incident | manual`. Provenance is retained; it never changes replay
   semantics.
3. `expect` is exactly two values: `invariant_violated` (a counterexample
   reproduction, not proof of a fixed product) or `invariant_holds` (a golden trace pinned against
   regression). Both replay through the same engine.
   Reproduction status is a **separate** axis, carried by `status`:
   `active` (replay matches `expect`; gated in T0), `quarantined` (failed the
   determinism check), or `unreproduced` (committed as evidence, replay does not
   match `expect`). Only `active` scenarios gate CI; the other two are reported
   and never fail T0. Conflating the two axes is what would otherwise make every
   incident scenario (§12) an instant T0 failure.
4. `fingerprint` is a content hash over the *normalized* scenario, used for
   dedup. Two counterexamples that differ only in irrelevant interleaving order
   must normalize to one fingerprint, or the corpus rots into thousands of
   near-duplicates.
5. Scenarios are checked into the adopting project, not the toolkit.

### 5.1 Normalization

Normalization is what makes cross-backend counterexamples comparable. It runs
before fingerprinting and before corpus insertion:

1. **Canonical identifier renaming** — actor/resource ids are renumbered in
   order of first appearance, so `{upstream_7, upstream_2}` and
   `{upstream_1, upstream_0}` collapse.
2. **Independent-step reordering** — adjacent steps the target declares
   commutative are sorted into a canonical order. This requires an opt-in
   `fn commutes(a, b) -> bool` on the target; the default is "nothing commutes",
   which is correct but weaker at dedup.
3. **Prefix minimization** — replay-driven delta debugging removes steps that do
   not affect the verdict. This is the toolkit's own shrinker and runs even for
   backends (Alloy, TLC) that have none.
   It applies **only to `expect: invariant_violated`**. Verdict-preserving
   minimization of a golden trace shrinks it to the empty trace — which
   trivially satisfies the equality guard below while destroying the entire
   value of the scenario. Golden traces are normalized (steps 1, 2, 4) and never
   minimized.
4. **Determinism check** — a normalized scenario must replay to the same verdict
   N times (default 3), or it is committed with `status = "quarantined"` instead
   of `active`.

Normalization is best-effort and must never change a scenario's verdict. The
runner asserts that: pre-normalization verdict == post-normalization verdict, or
the normalization is discarded and the raw trace is stored.

## 6. Target Interface

The single interface a project implements to join.

```rust
pub trait ScenarioTarget {
    type State: Clone + fmt::Debug;
    type Step: Serialize + DeserializeOwned + Clone + fmt::Debug;

    /// Build the starting state from the scenario's opaque `initial` value.
    fn init(&self, initial: &serde_json::Value) -> Result<Self::State, ScenarioError>;

    /// Apply one step. Rejecting a step is a legitimate modeled outcome,
    /// not a harness failure.
    fn apply(&self, state: &mut Self::State, step: &Self::Step)
        -> Result<StepOutcome, ScenarioError>;

    /// Evaluate one catalogued invariant against the current state.
    fn check(&self, id: &InvariantId, state: &Self::State)
        -> Result<InvariantResult, ScenarioError>;

    /// Opaque ID renaming is project-owned; default preserves initial and steps.
    fn canonicalize(&self, initial: &serde_json::Value, steps: &[Self::Step])
        -> Result<(serde_json::Value, Vec<Self::Step>), ScenarioError>
    { Ok((initial.clone(), steps.to_vec())) }

    /// Optional: declare commutativity to improve normalization (§5.1).
    fn commutes(&self, _a: &Self::Step, _b: &Self::Step) -> bool { false }
}
```

Labby:

```rust
impl ScenarioTarget for LabbyGatewayModel {
    type State = GatewayState;
    type Step = GatewayStep;
    // …
}
```

Unraid Drive:

```rust
impl ScenarioTarget for DriveModel {
    type State = DriveState;
    type Step = DriveStep;
    // …
}
```

Same command against either:

```bash
verify replay formal/scenarios/gateway/LABBY-REQ-001-a91f.json
```

Targets are registered by `(project, model)` in a runner-side registry the
adopting project populates once, so scenario files need no path conventions to
resolve their target.

## 7. Backend Adapter Contract

Every backend implements:

```rust
pub trait Backend {
    fn id(&self) -> BackendId;
    fn capabilities(&self) -> Capabilities;      // safety? liveness? concurrency? bounded?
    fn has_handle(&self, model: &str, handle: &str) -> bool;
    fn availability(&self) -> Availability;      // Ready | Missing(reason)
    fn run(&self, plan: &CheckPlan) -> BackendReport;
}
```

Contracts:

1. **Capability honesty.** `capabilities()` is what gates catalog validation
   (§4, contract 2). A backend that cannot express fairness declares no liveness
   capability, full stop.
2. **Graceful absence.** A missing Java/CBMC/Alloy install yields `Skipped` with
   a reason, never a build failure — except in the CI tier that explicitly
   requires that backend (§10).
3. **Counterexample projection.** A backend that produces a trace must project it
   into the scenario envelope. Backends that cannot (a Kani property with no
   concrete trace extraction) report `Falsified` with a diagnostic payload and
   no scenario; that is reported honestly rather than faked.
4. Backends never write to the corpus directly. They return scenarios; the
   runner normalizes, dedups, and decides what lands.

M1 validates catalog bindings using metadata only: `has_handle` must not run
or install a tool, and validation never calls `availability` or `run`.
`CheckPlan` includes a nonzero deadline, bounds, and optional seed. Until M2
defines the scenario crate, `BackendReport.scenarios` carries opaque JSON for
the runner to decode and validate, preserving the core's dependency direction.
`InvariantResult::Incomplete` represents unresolved observations; unknown
invariant IDs and harness failures use `ScenarioError`, never a Holds result.

## 8. Replay Engine

```
scenario file
  → resolve target by (project, model)
  → deserialize steps into target::Step
  → init state
  → evaluate the initial state
  → apply each step, recording per-step invariant evaluations
  → aggregate the trace verdict and compare against `expect`
  → emit ReplayReport
```

Verdict handling is driven by `status`, not by `expect` alone:

For safety/security predicates, any violation in the initial state or after any
step is sticky: a later valid state cannot erase it. Report the first failing
step and retain subsequent observations. Test a trace that violates then
recovers, an initially invalid state, and a valid empty trace. Invalid input,
failed initialization, and harness errors are errors, not counterexamples.
Finite replay cannot establish unbounded liveness: temporal checks require an
explicit monitor, bounds, and fairness assumptions, and unresolved obligations
remain incomplete rather than passing as proved. Refinement checks use the
project's declared observation relation (§11).

Counterexample reproduction and fixed-product regression are separate runs.
Retain the original reproduction target/revision and expectation. A linked
fixed-product regression reuses the input trace but expects `invariant_holds`;
record the source fingerprint and both target/revision identities in the run
manifest. Deliberately broken fixtures are harness self-tests only. Never make
a surviving product bug green by expecting `invariant_violated` against it.

| `status` | replay matches `expect` | replay does not match |
| --- | --- | --- |
| `active` | pass | **T0 failure** |
| `quarantined` | pass, reported | reported, never fails T0 |
| `unreproduced` | report suggested promotion to `active`; do not mutate corpus | reported, not reproduced |

An `unreproduced` scenario that starts matching `expect` is the interesting
case: the model has grown the step it was missing, and the runner surfaces the
promotion rather than silently flipping the file.

Replay is the common denominator of the whole design: it is the only component
every origin kind flows through, and it is pure, deterministic, and requires no
external toolchain. A project with zero formal specs still gets value from
scenario replay alone.

## 9. Reporting

```
Project: labby                                  catalog: formal/invariants.toml

128 invariants
  safety ............. 94
  liveness ........... 18
  security ........... 16

Verified by
  stateright ......... 63      (bounded: 11)
  kani ............... 47      (bounded: 47)
  loom ............... 22
  alloy .............. 31      (bounded: 31)
  tla ................ 11

Scenarios ............ 214
  by expect            regression 198   golden 16
  by status            active 209   quarantined 2   unreproduced 3
Uncovered ............   4     LABBY-CAT-009 LABBY-SEC-004 …
```

Emitted as text, JSON, Markdown (for PR comments), and a static HTML matrix.
The JSON form is the stable contract; everything else renders from it.

Report model checking, counterexample reproduction, implementation conformance,
real-process E2E, browser emulation, and actual-host qualification separately.
Each run records source revisions, binary/fixture identities, invariant and
scenario IDs, seeds, bounds, deadlines, observed results, and cleanup evidence.
A missing host or backend is `Skipped`; a deadline-limited search is incomplete
with its explored bounds, never `Verified`. Required qualification skips block
that qualification. See [product qualification](QUALIFICATION.md).

The `Uncovered` line names ids rather than printing a count, because a count is
easy to ignore and a name is not.

## 10. CI Tiers

Cost differs by three orders of magnitude across backends, so a single "run
verification" job is wrong.

| Tier | Runs | Contents | Budget |
| --- | --- | --- | --- |
| T0 | every PR | catalog validation, scenario replay, coverage report | < 60s |
| T1 | every PR | Stateright bounded search, Loom harnesses | < 5 min |
| T2 | nightly | Kani proofs, Shuttle long runs | < 45 min |
| T3 | nightly / weekly | Alloy, TLC/Apalache, deep Stateright, fuzz→scenario | 60 min/backend, 120 min/job |

T0 is the only tier that is a hard gate at adoption time. T1 becomes a gate once
its runtime is proven stable. T2/T3 report and file, they do not block.

Reusable CI is published as callable workflows in the toolkit repo so an
adopting project's workflow is a `uses:` line plus a catalog path, not a
copy-pasted 200-line YAML.

## 11. Model/Implementation Conformance

The honest limitation: backends verify the *model*, and a verified model with a
divergent implementation buys nothing.

The toolkit provides a `ConformanceTarget` contract; the project supplies the
adapter and observation relation. **Labby's lifecycle conformance is required
in v1 (C1), after M3/M4, not an optional future hook.** Drive the real compiled
product through public boundaries using the model's step vocabulary. Compare
observable outcomes after every controlled step, allowing explicitly documented
internal/stuttering transitions rather than assuming identical internal state.

Use fixture barriers and event acknowledgements, not sleeps, for dispatch,
disconnect, cancellation, and late responses. Assert at most one authoritative
terminal outcome, no late response overwriting a cancelled outcome, and eventual
cleanup within a deadline. Distinguish cancellation before dispatch from
cancellation after an upstream may already have performed a side effect; do not
claim cancellation undoes that effect. Include a deliberately divergent adapter
self-test proving the comparison fails, plus a real-product passing trace.

The adapter belongs in Labby's test support; product code must not depend on
`labby-model`. C1 completion requires trace-pinned real-process evidence, not
merely another model implementation. Broader E2E qualification remains a
separate evidence lane and need not invent formal models for every UI journey.

## 12. Production Incidents As Scenarios

The payoff that makes this more than formal-methods theater.

```
structured lifecycle logs
  → incident reducer (project-supplied, L3)
  → scenario { origin.kind = "incident" }
  → replay  → reproduces?  → commit as regression scenario
  → hand the prefix to Stateright as an initial state
  → explore the surrounding interleaving space
```

Contracts:

1. The reducer is project-specific and lives in L3. The toolkit only defines the
   envelope it must emit.
2. Incident-derived scenarios are **redacted by construction**: the reducer emits
   modeled steps, never raw log payloads. Labby's existing redaction rules in
   [docs/dev/OBSERVABILITY.md](../../dev/OBSERVABILITY.md) apply — no secrets,
   authorization values, OAuth material, or raw sensitive parameters may reach a
   committed scenario file.
3. An incident scenario that does not reproduce is still committed with
   `status = "unreproduced"` (§5 contract 3). It does not fail T0. It is
   evidence that the model is missing a step, which is itself a finding.

## 13. Labby As First Adopter

Labby's initial model artifacts are:

```
formal/
  invariants.toml
  scenarios/browser_request/
  alloy/
  tla/
crates/labby-model/          # 13th workspace member, dev-facing
```

Integration also updates Cargo membership, Justfile recipes, CI, and the root
`CLAUDE.md` workspace-member table together (§14), and adds the C1 test adapter
and qualification fixtures. M3 implements the browser bridge request-lifecycle
model and T0 replay; C1 conformance and broader qualification remain separate.

`labby-model` sits at the same dependency depth as `labby-primitives`: it may
depend on `labby-primitives` for shared vocabulary and on `verify-core` /
`verify-scenario`, and nothing in `labby-model` may be depended on by product
code. It is a model of the product, not part of it.

Candidate first models, smallest step-vocabulary first:

1. **request lifecycle** — dispatch, disconnect, cancel, late response, terminal
   outcome. Highest incident value, smallest alphabet, and the natural first
   conformance target.
2. **upstream catalog generation** — generation/version semantics and
   publication atomicity; reuses L2 patterns directly.
3. **capability/scope authority** — `lab:read` / `lab` / `lab:admin` monotonicity
   and the admin-gated stdio spawn guard.

Explicitly deferred as formal models, not as E2E coverage: Code Mode runtime
bounds and OAuth token lifecycle. Both are
attractive and both have large step alphabets; they are not where to learn the
toolkit.

## 14. Extraction Path

The toolkit incubates in this repo under `verification/` as its **own** Cargo
workspace, built by its own `just` recipes.

The precise claim, because the loose version is wrong: the *toolkit* crates are
not product-workspace members, which keeps the Java/CBMC-adjacent backend
dependency trees out of `cargo check --workspace --all-features`. But
`crates/labby-model` (§13) **is** a product-workspace member and does depend on
`verify-core` and `verify-scenario` — the two pure, leaf-shaped crates, and only
those. So the product workspace does change:

- workspace members go from 12 to 13 (including `labby-browser`), and the table in the root `CLAUDE.md`
  must be updated in the same change (M3), not left stale;
- `labby-model` depends on the toolkit by path during incubation and by version
  after extraction;
- backend harness binaries live in the separate `verification/` workspace;
  the product model depends only on pure core/scenario crates. Do not place
  backend dev-dependencies on the product model: workspace all-target checks
  can compile those too. Ordinary product gates do not execute external tools.

If that dependency direction ever needs to reverse — product code depending on
`labby-model` — the model has stopped being a model.

Extraction to a standalone repo is gated on evidence, not on schedule:

1. two adopting projects,
2. no L1 change required by the second adopter,
3. schema version 1 unchanged across both,
4. CI workflows consumed as `uses:` rather than copied.

Until all four hold, extraction is premature and the design has not been proven
project-agnostic.

## 15. Deliberate Omissions

1. **No cross-backend spec DSL.** Alloy relational-first, TLA+ temporal-first,
   and Rust operational models are genuinely different formalisms. A generic
   compiler would produce bad specs in all three.
2. **No macros in v1.** `invariant!` / `scenario_test!` / `kani_invariant!` are
   attractive and premature. Extract them once the same boilerplate has appeared
   in two projects, per §14.
3. **No verification of the product's async runtime.** `loom` cannot model Tokio;
   `shuttle` covers the concurrency tier instead, and that boundary is stated
   rather than blurred.
