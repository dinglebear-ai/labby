# Implementation Plan

Ordered so that value lands before any formal-methods toolchain is installed.
Each milestone is independently useful and independently abandonable.

## M0 — Incubation workspace

1. Create `verification/` as its own Cargo workspace, excluded from the product
   workspace so `cargo check --workspace --all-features` is unaffected.
2. Add `just verify-check` / `just verify-test` recipes delegating into it.
3. Pin the versions in SPEC §3.1; add `deny.toml` coverage for the new tree.

## M1 — Core vocabulary and catalog

4. `verify-core`: `InvariantId`, `Kind`, `Severity`, `Verdict`, `Capabilities`,
   `ScenarioTarget`, `StepOutcome`, `InvariantResult`.
5. Catalog parse + validate, including the capability/kind rejection rule and
   unresolved-handle failure.
6. Generate `schemas/invariants.schema.json` from the Rust types via `schemars`;
   assert in CI that the committed schema matches the generated one.

## M2 — Scenario format and replay

7. `verify-scenario`: envelope, opaque step encoding, corpus layout, fingerprint.
8. `verify-runner`: target registry, replay engine, `verify replay` CLI.
9. Normalization: canonical renaming, delta-debug minimization, determinism
   check, quarantine path. Commutativity reordering lands only after a target
   actually implements `commutes`.
10. Generate `schemas/scenario.schema.json` the same way as M1.

At this point a project gets scenario replay with zero external toolchain, which
is the adoption floor.

## M3 — First real model

11. Add `crates/labby-model` to the product workspace as a dev-facing member;
    implement the request-lifecycle state machine and `ScenarioTarget`.
12. Write `formal/invariants.toml` covering request lifecycle only.
13. Hand-author 5–10 scenarios, including the known cancel/late-response shapes.
14. Wire CI tier T0.

M3 is the design's first falsification point: if the interface feels wrong here,
fix L1 before adding backends.

## M4 — First backend

15. `verify-stateright`: adapter, capability declaration, counterexample
    projection into the scenario envelope.
16. Bind the request-lifecycle invariants; confirm a deliberately-broken model
    produces a scenario that replays to `invariant_violated`.
17. Wire CI tier T1.

## M5 — Reporting

18. `verify-report`: JSON contract first, then text/Markdown/HTML renderers.
19. PR-comment summary via the reusable workflow; snapshot the report with
    `insta` so format drift is visible in review.

## M6 — Remaining backends, by cost order

20. `verify-loom` (plus Shuttle fallback) — concurrency tier, T1/T2.
21. `verify-kani` — bounded proof tier, T2. Report `Bounded`, never `Verified`.
22. `verify-tla` and `verify-alloy` — container-pinned, T3, with honest
    `Skipped` when the image is unavailable.

## M7 — Incident pipeline

23. Labby-side incident reducer from structured lifecycle logs to scenarios,
    with redaction asserted by test (SPEC §12 contract 2).
24. Seeded exploration: feed an incident prefix to Stateright as initial state.

## M8 — Second adopter and extraction

25. Adopt in Unraid Drive / the mount core with no L1 changes.
26. Evaluate the four extraction gates in SPEC §14. Extract only if all hold.

## Ordering Rationale

- Replay before backends, because replay is the only piece every origin shares.
- One model before many, because the interface is the risky artifact.
- Reporting before the expensive backends, because an unreported `Bounded`
  result is indistinguishable from a lie.
- Macros never, until §15 clears them.

No production deployment is part of this work.
