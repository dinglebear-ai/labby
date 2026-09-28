---
title: "Bounded Skill Discovery - Reflection"
created: 2026-09-26
updated: 2026-09-26
---

# Review findings and repairs

A result-length limit is not a work limit. Request budgets must stop traversal and validation before unnecessary work, while authorization and direct-read authority remain separate from a partial preview.

Scope filtering after federation is both slow and observable to an excluded upstream. The regression therefore checks actual TCP connection absence, not only the shape of the returned list. Scope narrowing preserves the immutable request generation, OAuth subject and private Artifact access rather than constructing a broader replacement context.

Review corrected a stale-preview bug in the first implementation: returning an expired snapshot without scheduling any refresh would make preview-only callers stale indefinitely. Expired reads now perform a bounded foreground fetch and do not falsely refresh the full operator snapshot. Idle subject locks are reclaimed during acquisition, including after cancellation, while active lock identity remains stable.

The broader suite was essential: isolated Skill tests did not expose the process-global setup fixture race. The repair is test-only and preserves parallel testing instead of serializing the whole suite or weakening security assertions. Root causes and before/after failures are retained in the evidence directory.

Open OAuth Skill PR #805 changes subject selection in mcp/skills.rs; this patch does not modify that file and preserves the subject supplied by the request context. Full fleet staging remains a separate gate, not an inference from local tests.
