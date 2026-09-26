---
title: "Bounded Skill Discovery - Changelog"
created: 2026-09-26
updated: 2026-09-26
---

# Implemented

- Intersect Code Mode namespaces with the authorized request Skill context before discovery; retain subject and Artifact authorization.
- Apply requested descriptor and proportional candidate budgets during upstream traversal; stop at a full page boundary before requesting another page.
- Reuse fresh full snapshots with bounded exposed cloning. Fetch cold or expired previews under existing transport, lock and invalidation gates without publishing them as complete cache entries.
- Retain full/default discovery, direct get/read, integrity, collision and exposure behavior. Keep hard-cap warnings separate from normal preview budget events.
- Reclaim idle per-subject acquisition locks, including completed/cancelled preview keys, without replacing active locks.
- Add nine gateway regressions (eight discovery cases and lock reclamation), four scope/authorization regressions, and three test-home isolation regressions.
- Replace the test-only process-global home override with a thread-affine RAII override and explicitly propagate it into setup blocking workers. Production path resolution and authentication are unchanged.
- Add durable task/session records and a validation manifest; create Cortex Microsandbox telemetry issue #256.
