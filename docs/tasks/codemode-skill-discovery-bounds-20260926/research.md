---
title: "Bounded Skill Discovery - Research"
created: 2026-09-26
updated: 2026-09-26
---

# Findings

- Local b7637da has identical content to merged main 95983b2 (PR #816).
- Baseline named regressions pass; gateway suite: 1421 passed, 5 ignored, 0 failed. All-feature gateway Clippy passes.
- CanonicalCodeModeSkillProvider filters origin only after skills.list has federated route-visible upstreams.
- SepSkillProvider applies request.max_items after full upstream_skills discovery and exposed-vector cloning.
- Existing safety caps are 256 retained skills, 1024 candidates and 16 pages per upstream; these are not the complete Depot inventory. Do not increase them or call a preview exhaustive.

## Review findings

- Scope intersection was proven with a real listening TCP socket: before wiring the scope, the Code Mode list timed out; after wiring, it returned bundled Skills and the excluded upstream received zero connections.
- Keep the new facade helper behind the same gateway+skills feature combination as its production caller; test-only builds can still exercise authorization preservation.
- Cold previews do not initiate background full refreshes. The existing guard map previously relied on later refresh cleanup; acquiring a guard now reclaims completed subject keys while retaining active/waiting guards.
- Preserve warning-level observability for hard safety caps, including exact page-boundary exhaustion. Smaller requested previews are debug-level budget events, not upstream failures.
- Open Labby PR #805 changes OAuth subject selection in crates/labby/src/mcp/skills.rs. This patch does not alter that file and preserves the supplied request subject when narrowing discovery.
- Depot repo-auto-ingest is now committed and pushed at 8436e81 in open dinglebear-ai/depot PR #112; it is not merged. The live catalog-depot service is active. Current credential-file configuration could not be rechecked with the unprivileged labby account (sudo denied); do not infer absence from that permission failure.
- Created the requested Microsandbox telemetry issue: https://github.com/dinglebear-ai/cortex/issues/256 .

A final stale-cache review tightened the preview contract: reuse only fresh snapshots. An expired snapshot triggers a bounded foreground refresh, without replacing the full operator snapshot or spawning a full background walk. The initial prototype reused stale snapshots without scheduling refresh, which could have kept preview-only callers stale indefinitely; the new regression covers this correction.
