# Implementation Plan

PR #459 completed the core milestone independently while this plan was in
progress. The remaining implementation sequence is now:

1. Add capability, operation, and subject-scoped filters to both usage query
   structs and the shared SQLite predicate builder.
2. Thread those filters through gateway params, manager mapping, catalog, CLI,
   generated action/OpenAPI/help contracts, and the web client.
3. Expand facets with capability and operation values and expose the subject
   scope as a bounded enumerated filter.
4. Extend slowest-target result shapes to include capability, operation, and
   subject-scoped identity, matching top/least target identity.
5. Add store, manager/dispatch, client, adapter, and browser tests covering the
   new filters individually and in combination with route scope and cursors.
6. Benchmark representative 24h/7d queries against a copy of a
   production-shaped usage database and inspect query plans for each dimension.
7. Decide and document the long-window safety policy for exact queries above
   the current 250,000-row ceiling.
8. Run full Rust/frontend/docs gates and adversarial review before the follow-up
   implementation PR.

No production deployment is part of this work.
