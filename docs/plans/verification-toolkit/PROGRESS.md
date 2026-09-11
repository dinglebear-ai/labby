# Progress

- [x] Agreed the reusable artifact is a generic toolkit, not Labby-specific
      formal verification.
- [x] Agreed the three-layer split (infrastructure / domain patterns / project
      models) and the rule that L1 must contain no domain vocabulary.
- [x] Agreed the scenario envelope is the most reusable piece and that
      production incidents reduce to the same artifact.
- [x] Agreed a cross-backend spec DSL is out of scope.
- [x] Researched and pinned candidate upstream versions (SPEC §3.1).
- [ ] M0 incubation workspace.
- [ ] M1 core vocabulary and catalog.
- [ ] M2 scenario format and replay engine.
- [ ] M3 first Labby model (`crates/labby-model`, request lifecycle).
- [ ] M4 Stateright backend.
- [ ] M5 reporting.
- [ ] M6 Loom/Shuttle, Kani, TLA+, Alloy backends.
- [ ] M7 incident-to-scenario pipeline.
- [ ] M8 second adopter and extraction decision.

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
