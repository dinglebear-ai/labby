---
title: "Bounded Skill Discovery - Plan"
created: 2026-09-26
updated: 2026-09-26
---

# Plan and gates

1. Add failing regressions for page/validation work, cache preservation, direct reads and OAuth isolation.
2. Push smaller discovery budgets into upstream traversal and clone only a bounded exposed projection. A cold preview must not overwrite or impersonate the full operator snapshot.
3. Intersect Code Mode namespaces with request-authorized Skill scope before federation; preserve caller subject and artifact authorization.
4. Run focused regressions, full affected gateway suite, all-feature Clippy, product tests and formatting. Review the diff and publish a PR with evidence.
5. Separate real-fleet staging qualification is a prerequisite to production promotion. No production mutation in this source-validation phase.

Not addressed by this slice: persistent Depot repo ingestion/credentials, complete 71k-catalog progressive search, runtime/MCP version qualification, engineering OCI image, fleet installation and final staging promotion.
