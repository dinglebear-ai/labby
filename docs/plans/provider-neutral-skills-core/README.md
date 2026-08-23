# Provider-neutral Skills core

Status: active implementation
Owner: Labby gateway/runtime
Tracking: `lab-27juw`

This folder records the implementation package for separating Labby's canonical
Skill model from any one discovery or delivery mechanism.

Labby already has a hardened draft SEP-2640 implementation and a compatibility
projection for MCP clients that do not understand the extension. This project
does not replace either. It introduces the shared model and policy vocabulary
that both can consume, with SEP-2640 becoming one provider adapter.

Artifacts:

- `SPEC.md`: scope, architecture, and product behavior.
- `CONTRACT.md`: invariants that implementations and adapters must preserve.
- `IMPLEMENTATION_PLAN.md`: dependency-ordered delivery slices and tests.
- `PROGRESS.md`: current ledger, evidence, and open decisions.

Canonical current-behavior docs remain under `docs/contracts/`, `docs/guides/`,
and `docs/services/`. This plan becomes product truth only as its slices land.
