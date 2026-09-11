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
| `verify-core` | invariant identity, catalog parse/validate, verdicts, `ScenarioTarget`, `StateMachine`, backend-capability vocabulary | serde, thiserror only |
| `verify-scenario` | scenario envelope, step encoding, normalization, shrink-stability, on-disk corpus layout | `verify-core` |
| `verify-runner` | discovery, replay engine, target registry, orchestration, `verify` CLI | `verify-core`, `verify-scenario` |
| `verify-report` | coverage matrix, text/JSON/HTML/Markdown renderers, CI summary | `verify-core` |
| `verify-stateright` | Stateright backend adapter + counterexample extraction | `verify-core`, `verify-scenario`, `stateright` |
| `verify-kani` | Kani harness conventions, catalog binding, result ingestion | `verify-core`, `verify-scenario` |
| `verify-loom` | Loom/Shuttle concurrency harness conventions, interleaving capture | `verify-core`, `verify-scenario` |
| `verify-alloy` | Alloy invocation, instance → scenario projection | `verify-core`, `verify-scenario` |
| `verify-tla` | TLC/Apalache invocation, error-trace → scenario projection | `verify-core`, `verify-scenario` |
| `verify-macros` | `invariant!`, `scenario_test!` and friends | extracted last, never first |

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
2. `kind` constrains which backends may legitimately claim it. A liveness
   property cannot be discharged by Kani (bounded, no fairness); the runner
   rejects such a binding at catalog-validation time rather than silently
   reporting green.
3. `checks` names *handles*, not file paths. Each backend adapter resolves its
   own handles and fails loudly on an unresolved one — a typo must never
   degrade to "uncovered but nobody noticed".
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
2. `origin.kind` is one of `stateright | kani | loom | shuttle | alloy | tla |
   fuzz | incident | manual`. Provenance is retained; it never changes replay
   semantics.
3. `expect` is `invariant_violated` (a regression scenario reproducing a bug) or
   `invariant_holds` (a golden trace pinned against regression). Both replay
   through the same engine.
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
4. **Determinism check** — a normalized scenario must replay to the same verdict
   N times (default 3), or it is quarantined as `unstable` instead of committed.

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
    fn check(&self, id: InvariantId, state: &Self::State) -> InvariantResult;

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

## 8. Replay Engine

```
scenario file
  → resolve target by (project, model)
  → deserialize steps into target::Step
  → init state
  → apply each step, recording per-step invariant evaluations
  → compare final verdict against `expect`
  → emit ReplayReport
```

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

Scenarios ............ 214     (regression: 198, golden: 16, quarantined: 2)
Uncovered ............   4     LABBY-CAT-009 LABBY-SEC-004 …
```

Emitted as text, JSON, Markdown (for PR comments), and a static HTML matrix.
The JSON form is the stable contract; everything else renders from it.

The `Uncovered` line names ids rather than printing a count, because a count is
easy to ignore and a name is not.

## 10. CI Tiers

Cost differs by three orders of magnitude across backends, so a single "run
verification" job is wrong.

| Tier | Runs | Contents | Budget |
| --- | --- | --- | --- |
| T0 | every PR | catalog validation, scenario replay, coverage report | < 60s |
| T1 | every PR | Stateright bounded search, Loom harnesses | < 5 min |
| T2 | merge queue / nightly | Kani proofs, Shuttle long runs | < 45 min |
| T3 | nightly / weekly | Alloy, TLC/Apalache, deep Stateright, fuzz→scenario | unbounded |

T0 is the only tier that is a hard gate at adoption time. T1 becomes a gate once
its runtime is proven stable. T2/T3 report and file, they do not block.

Reusable CI is published as callable workflows in the toolkit repo so an
adopting project's workflow is a `uses:` line plus a catalog path, not a
copy-pasted 200-line YAML.

## 11. Model/Implementation Conformance

The honest limitation: backends verify the *model*, and a verified model with a
divergent implementation buys nothing.

The toolkit does not solve this generically, but it provides the hook: a
`ConformanceTarget` that drives the real implementation through the same `Step`
vocabulary and asserts the model's state predicate after each step. Whether an
adopting project wires it is its own decision, per model. Labby should wire it
for the request-lifecycle model first, where the step vocabulary is smallest.

This is stated as a limitation, not as a feature, because a coverage dashboard
that implies more assurance than exists is worse than none.

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
3. An incident scenario that does not reproduce is still committed, marked
   `unreproduced`. It is evidence that the model is missing a step, which is
   itself a finding.

## 13. Labby As First Adopter

Labby adds, and nothing else:

```
formal/
  invariants.toml
  scenarios/gateway/
  alloy/
  tla/
crates/labby-model/          # new workspace member, dev-facing
```

`labby-model` sits at the same dependency depth as `labby-primitives`: it may
depend on `labby-primitives` for shared vocabulary, and nothing in `labby-model`
may be depended on by product code. It is a model of the product, not part of it.

Candidate first models, smallest step-vocabulary first:

1. **request lifecycle** — dispatch, disconnect, cancel, late response, terminal
   outcome. Highest incident value, smallest alphabet, and the natural first
   conformance target.
2. **upstream catalog generation** — generation/version semantics and
   publication atomicity; reuses L2 patterns directly.
3. **capability/scope authority** — `lab:read` / `lab` / `lab:admin` monotonicity
   and the admin-gated stdio spawn guard.

Explicitly deferred: Code Mode runtime bounds, OAuth token lifecycle. Both are
attractive and both have large step alphabets; they are not where to learn the
toolkit.

## 14. Extraction Path

The toolkit incubates in this repo under `verification/` — *not* as workspace
members of the product workspace, to keep the product's `cargo check --workspace
--all-features` unaffected by a Java/CBMC-adjacent dependency tree. It is its
own workspace, built by its own `just` recipes.

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
