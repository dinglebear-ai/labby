# Progress

- [x] Agreed the reusable artifact is a generic toolkit, not Labby-specific
      formal verification.
- [x] Agreed the three-layer split (infrastructure / domain patterns / project
      models) and the rule that L1 must contain no domain vocabulary.
- [x] Agreed the scenario envelope is the most reusable piece and that
      production incidents reduce to the same artifact.
- [x] Agreed a cross-backend spec DSL is out of scope.
- [x] Researched and pinned candidate upstream versions (SPEC §3.1).
- [x] M0 incubation workspace, CI routing key, and `just verify-*` recipes.
- [x] M1 `verify-core`: invariant identity, catalog and its four validation rules, verdicts, backend capability vocabulary, `ScenarioTarget`.
- [x] M2 scenario envelope, fingerprint, syntactic and replay-driven normalization, target registry, replay engine, `verify` CLI, and a worked fixture example.
- [ ] M3 first Labby model (`crates/labby-model`, request lifecycle).
- [ ] M4 Stateright backend.
- [ ] M5 reporting.
- [ ] M6 Loom/Shuttle, Kani, TLA+, Alloy backends.
- [ ] M7 incident-to-scenario pipeline.
- [ ] M8 second adopter and extraction decision.

## Decisions Made During M0

1. **No `exclude` in the root manifest.** Verified empirically: with an explicit
   `members` list, the root workspace ignores a nested directory that declares
   its own `[workspace]`. The product `Cargo.toml` is untouched.
2. **`verification` is its own CI routing key**, not an extension of
   `rust_sources`. A product change should not build the toolkit and a toolkit
   change should not run the full product Rust matrix. Without the key a
   verification-only change routed to nothing and CI reported green having built
   nothing; `crates/labby/tests/ci_changed_paths.rs` now guards both directions.
3. **`cargo deny` runs against `verification/Cargo.lock` in that job.** The root
   `deny` recipe reads only the root lockfile.
4. **SHA-256 (`s256:`) fingerprints, not blake3.** `sha2` is already pinned in
   the root workspace; a dedup hash does not justify a new dependency family.
5. **`verify-report` exists from M2, not M5.** SPEC §3 has `verify-runner`
   depending on it and `ReplayReport` has to live somewhere; starting it minimal
   avoids moving a public type later.
6. **`DynTarget` bridges object safety.** `ScenarioTarget` has associated types,
   so the registry holds an object-safe erased trait with a blanket impl.
   Projects never see it.
7. **Normalization is split across two crates.** The replay-driven passes cannot
   live in `verify-scenario` without a dependency cycle.

## Decisions Made During M1

8. **`verify-core` depends on `serde_json` and `toml`, not just `serde`.**
   `serde_json::Value` is in the published `ScenarioTarget::init` signature and
   `toml` parses the catalog. The earlier "serde, thiserror only" wording named
   a crate count rather than the property that matters — transport-free,
   filesystem-free, env-free — and has been corrected in SPEC §3 and
   `verification/CLAUDE.md`.
9. **`Catalog::validate` returns every violation, not the first.** A catalog
   author fixing one id per CI run is a slow loop.
10. **Kind decides trace-wide judgement.** `Kind::violated_at_any_step()` is
    true for safety and security, so replay judges those over the whole trace
    rather than the final state (SPEC §8).
11. **`Capabilities::default()` claims nothing.** Defaulting to "claims
    everything" would silently disable the capability gate for any backend
    whose author left the field unfilled.

## Decisions Made During M2

12. **`verify-report` ships from M2, minimal.** SPEC §3 has `verify-runner`
    depending on it and `ReplayReport` has to live somewhere; starting it now
    avoids moving a public type out of the runner at M5.
13. **The invariant is evaluated before the first step.** A scenario whose
    initial state already violates is a real finding, not a step-zero blind
    spot; `first_violation == 0` means "already false on arrival".
14. **Minimization refuses to shrink a scenario that never reproduced.**
    Otherwise every candidate "still fails to reproduce" and the trace shrinks
    to nothing — the same trap as minimizing a golden trace, from the other side.
15. **`commutes_erased` returns false for a step it cannot deserialize.** A step
    that cannot be read cannot be known to commute, and guessing would silently
    merge distinct counterexamples.
16. **The fixture is an example, not a test-only type.** `tests/replay.rs`
    includes it by path, so one definition both proves the interface and serves
    as the worked example M3 copies.

## Open Decisions

1. **Step encoding.** Scenario steps are currently specified as opaque JSON
   deserialized into the target's `Step`. A tagged binary encoding would be
   cheaper for large corpora but hurts the "readable in a PR diff" property.
   Decision deferred to M2; JSON is the working default.
2. **Commutativity opt-in.** `commutes` defaulting to `false` is sound but weak
   at dedup. Whether normalization should instead *infer* commutativity by
   replay experiment is deferred until corpus size justifies the cost.
3. **Where `labby-model` lives.** Proposed as a product-workspace member with a
   one-way dependency rule. The alternative — placing it in the incubation
   workspace — keeps the product workspace untouched but makes it awkward to
   share `labby-primitives` vocabulary. Proposal stands; not yet ratified.
4. **T1 as a hard gate.** Stateright runtime variance is unmeasured. T1 blocks
   only after two weeks of stable nightly timings.
5. **Conformance scope.** Whether `ConformanceTarget` (SPEC §11) is part of v1
   at all, or deferred until one model has proven value in isolation.

No production deployment is part of this planning milestone.
