# Progress

- [x] Agreed the reusable artifact is a generic toolkit, not Labby-specific
      formal verification.
- [x] Agreed the three-layer split (infrastructure / domain patterns / project
      models) and the rule that L1 must contain no domain vocabulary.
- [x] Agreed the scenario envelope is the most reusable piece and that
      production incidents reduce to the same artifact.
- [x] Agreed a cross-backend spec DSL is out of scope.
- [x] Researched and pinned candidate upstream versions (SPEC §3.1).
- [x] M0 incubation workspace, isolated recipes/lockfile and boundary tests.
- [x] M1 core vocabulary/catalog, generated schema and advisory CI checks.
- [x] M2 scenario format and replay engine; reviewed and locally verified.
- [x] M3 first Labby model (`crates/labby-model`, request lifecycle).
- [x] M4 Stateright backend.
- [x] M5 reporting.
- [x] M6 Loom/Shuttle, Kani, TLA+, Alloy backends.
- [x] M7 incident-to-scenario pipeline.
- [x] M8 second-adopter evaluation completed: no qualified second adopter exists,
      so extraction is explicitly premature. See
      [EXTRACTION_DECISION.md](EXTRACTION_DECISION.md).
- [x] Required v1 conformance, trace-wide replay, and E2E qualification planned.
- [x] C1 real-product lifecycle conformance after M3/M4.
- [ ] Q0–Q6 product qualification (see [acceptance contract](QUALIFICATION.md)).

Execution is tracked in Beads epic `lab-jfu6q`; amendment task `lab-jfu6q.1`.
The checkboxes describe design milestones, not evidence that code has shipped.

## Execution Index

Beads is authoritative for status, ownership, acceptance, and dependencies.

| Milestone | Bead |
| --- | --- |
| M0 workspace | `lab-jfu6q.2` |
| M1 core/catalog | `lab-jfu6q.3` |
| M2 replay | `lab-jfu6q.4` |
| M3 lifecycle model | `lab-jfu6q.5` |
| M4 Stateright | `lab-jfu6q.6` |
| M5 reporting | `lab-jfu6q.7` |
| C1 implementation conformance | `lab-jfu6q.8` |
| M6 remaining backends | `lab-jfu6q.9` |
| M7 incidents | `lab-jfu6q.10` |
| M8 second adopter/extraction | `lab-jfu6q.11` |
| Q0 inventory/contracts | `lab-jfu6q.12` |
| Q1 MCP/lifecycle | `lab-jfu6q.13` |
| Q2 auth | `lab-jfu6q.14` |
| Q3 Code Mode | `lab-jfu6q.15` |
| Q4 browser/apps/hosts | `lab-jfu6q.16` |
| Q5 proxy | `lab-jfu6q.17` |
| Q6 delivery/Depot | `lab-jfu6q.18` |

## Open Decisions

1. **Step encoding (resolved in M2).** Opaque JSON is implemented and deserialized
   into the target's `Step`; recursive duplicate-key rejection preserves evidence
   integrity. Binary encoding remains outside the current scope.
2. **Commutativity opt-in.** `commutes` defaulting to `false` is sound but weak
   at dedup. Whether normalization should instead *infer* commutativity by
   replay experiment is deferred until corpus size justifies the cost.
3. **Where `labby-model` lives (resolved in M3).** It is a dev-facing product
   workspace member with enforced one-way dependency boundaries. Production
   crates cannot depend on it, and it can depend only on the two pure toolkit
   leaves plus product vocabulary needed by the model.
4. **T1 as a hard gate.** Stateright runtime variance is unmeasured. T1 blocks
   only after two weeks of stable nightly timings.

## Resolved Conformance Decision

`ConformanceTarget` and Labby's real-process lifecycle adapter are required in
v1 at C1, following M3/M4. Model-only results cannot satisfy C1 or the separate
Q0–Q6 product qualification lanes.

No production deployment is part of this planning milestone.
