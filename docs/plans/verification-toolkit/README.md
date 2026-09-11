# Verification Toolkit

Status: proposed design. Nothing in this folder is implemented.

## Purpose

This folder holds the accepted-target design for a **reusable Rust correctness
engineering toolkit** that Labby adopts first, and that later consumers
(Unraid Drive, the shared mount core, future Rust systems work) adopt on the
same interfaces.

The reusable artifact is deliberately *not* "Labby formal verification". It is
orchestration, schemas, replay, reporting, and backend adapters. Domain models
and formal specifications stay with the project that owns the domain.

## Artifacts

- [Specification](SPEC.md): layering, crate boundaries, schemas, traits, backend
  adapter contract, replay semantics, reporting, and CI tiers.
- [Implementation plan](IMPLEMENTATION.md): ordered milestones, from in-repo
  incubation to standalone extraction.
- [Progress](PROGRESS.md): current state and explicit open decisions.
- [schemas/](schemas/): draft JSON Schema for the invariant catalog and the
  scenario envelope. These are design drafts, not yet enforced by any tool.

## Shape In One Screen

```
verification-toolkit/            # eventual standalone repo
  crates/
    verify-core                  # vocabulary: invariants, verdicts, targets
    verify-scenario              # scenario envelope, normalization, storage
    verify-runner                # discovery, replay, orchestration, CLI
    verify-report                # coverage matrix, renderers
    verify-stateright            # backend adapters
    verify-kani
    verify-loom
    verify-alloy
    verify-tla
    verify-macros                # extracted only after duplication is proven
  schemas/
  ci/

labby/                           # first adopter
  formal/
    invariants.toml
    scenarios/
    alloy/
    tla/
  crates/labby-model/            # domain state machines + ScenarioTarget impls
```

## Non-Goals

1. No generic DSL that compiles one source of truth into Alloy, TLA+, and Rust.
   That is a research project, not a tooling investment.
2. No attempt to verify production Labby code paths directly. Backends verify
   *models*; conformance between model and implementation is a separate,
   explicitly-scoped concern (see SPEC §11).
3. No new mandatory CI gate until the toolkit has a stable scenario corpus.
