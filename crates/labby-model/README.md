# labby-model

`labby-model` contains deterministic, transport-free development models of
selected Labby behavior. It supplies state machines for replay, bounded
exploration, and comparison with product observations. Production code must
never depend on it; the product uses it only as a development dependency.

The crate belongs to the root Cargo workspace, while its pinned `verify-core`
and `verify-scenario` path dependencies belong to the separately managed
[verification workspace](../../tools/verification/README.md). Root crate checks
can build these pure leaves without building that workspace's runner, host,
or verifier backends. [Cargo.toml](Cargo.toml) declares no features, runtime,
network, or filesystem dependencies. Model transitions do not read ambient
configuration, start services, or execute page callbacks.

## Targets and public interfaces

[src/lib.rs](src/lib.rs) exports both targets and their typed vocabulary:

| Export | Purpose |
| --- | --- |
| `BrowserRequestModel`, `BrowserRequestState` | Browser request lifecycle and retained witnesses for active/terminal state, dispatch, and completion ownership. |
| `Step`, alias `BrowserRequestStep` | Controlled connection, admission, dispatch, completion, cancellation, timeout, and invalidation events. |
| `RequestPhase`, `TerminalOutcome` | Admission/dispatch boundary and the first authoritative terminal observation. |
| `CapabilityVisibilityModel`, `CapabilityVisibilityState` | Startup guard and runtime-degradation state, including Doctor/readiness visibility and operator detail. |
| `CapabilityVisibilityStep`, `CapabilityPhase`, `CapabilityCode`, `RuntimeCapability` | Configuration/startup events, configuring/running/blocked phases, and finite capability classifications. |
| `MODEL`, `CAPABILITY_VISIBILITY_MODEL`, `MODELS` | Registry keys `browser_request` and `capability_visibility`, plus their complete model-key list. |
| `CATALOG_TOML`, `catalog` | Compile-time invariant catalog text and validation against a caller-owned `BackendRegistry`. |

Both targets implement `verify_core::ScenarioTarget`. Import that trait to call
`init`, `apply`, `check`, and `canonicalize`. `apply` returns an applied or
legitimately rejected `StepOutcome`; rejection is not a harness failure.
Malformed initial state and unknown invariant IDs return `ScenarioError`.
`InvariantResult::Holds` describes one observation, not every possible execution.

The catalog is embedded from
[formal/invariants.toml](../../tools/verification/formal/invariants.toml).
`catalog` validates metadata and model-scoped backend handles; it does not run a
backend. The [Labby verification host](../../tools/verification/hosts/labby/src/lib.rs)
registers the targets under project `labby` and drives replay/backend adapters.
Scenario envelopes and fingerprints belong to `verify-scenario`; registry,
replay limits, and normalization orchestration belong to `verify-runner`.

## Modeled behavior

The [browser request model](src/browser_request.rs) separates admission from
successful dispatch. A request belongs to one fresh connection generation;
only that current owner can complete it. Cancellation before dispatch means
page code did not run. Cancellation after dispatch retains possible effects.
Timeout is a controlled event accepted only after dispatch, not a modeled clock.
Replacement/disconnect settles the owned generation's active requests, and
late replies cannot overwrite the first terminal result.

Its invariants cover single terminal authority, cleanup, immutable terminal
results, generation ownership, and the cancellation boundary. Terminal history
is a verification witness, not a prescription to retain production pending
entries forever. Capacity, channels, SQLite cleanup, real timing, and scheduler
interleavings remain implementation/conformance concerns.

The [capability visibility model](src/capability_visibility.rs) accepts
configuration changes only before startup is attempted. Startup either blocks
a fatal feature/route mismatch or enters running state with visible degradation.
Runtime degradation is accepted only while running. Every degraded capability
must appear in Doctor and readiness projections with actionable operator detail.
This is an abstract policy model; it does not inspect an installed server.

Browser canonicalization consistently renames opaque request/generation IDs in
first-appearance order without adding, deleting, or reordering steps. Empty IDs
stay empty so normalization cannot repair an invalid event. Capability
canonicalization validates the initial object and preserves the events.
The runner owns replay checks around normalization and counterexample reduction.

## Example: inspect modeled degradation

This source-backed example applies two deterministic events without starting a
server or reading configuration:

```rust
use labby_model::{
    CapabilityCode, CapabilityVisibilityModel, CapabilityVisibilityState,
    CapabilityVisibilityStep,
};
use verify_core::{ScenarioError, ScenarioTarget, StepOutcome};

fn remote_bind() -> Result<CapabilityVisibilityState, ScenarioError> {
    let model = CapabilityVisibilityModel;
    let mut state = model.init(&serde_json::json!({}))?;
    for step in [CapabilityVisibilityStep::BindRemote, CapabilityVisibilityStep::Start] {
        assert_eq!(model.apply(&mut state, &step)?, StepOutcome::Applied {});
    }
    assert!(state.doctor_visible.contains(&CapabilityCode::RemoteHostNotAllowed));
    Ok(state)
}
```

Use `check` with a catalogued invariant to obtain a typed property observation.
Model tests also corrupt independent state witnesses to verify that invariant
checks detect violations. Product comparison has a separate owner in
[lifecycle conformance tests](../labby/tests/lifecycle_conformance.rs).

## Verification and evidence limits

Run focused checks from the root workspace with its pinned toolchain:

```bash
cargo check -p labby-model --all-targets
cargo nextest run -p labby-model
cargo nextest run -p labby-model --lib browser_request::tests
cargo nextest run -p labby-model --lib capability_visibility::tests
cargo clippy -p labby-model --all-targets -- -D warnings
```

The existing optional adoption gates run in the separate verification workspace:

```bash
just verify-t0
just verify-t1
```

T0 validates catalog/corpus adoption and finite model replay. T1 runs bounded
model checking under the host's declared domain and resource limits. Neither
establishes production conformance or universal proof; unavailable, uncovered,
incomplete, and violated results remain distinct. The
[formal model notes](../../tools/verification/formal/README.md) describe intentional
abstractions, and [Verification and Compliance](../../docs/dev/VERIFICATION.md)
owns evidence levels. Changes here do not authorize changes to toolkit contracts
or generated artifacts. Nextest excludes doctests; this README's Rust fence is
an illustrative example rather than an executable doctest.
