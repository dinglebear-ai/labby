# Implementation Plan

1. Extend usage query/result structs with exact analytics and bounded bucket configuration.
2. Extend SQLite aggregation with grouped series/distributions/percentiles/throughput and tests.
3. Extend usage calls filtering and exact server-side counts; preserve keyset pagination.
4. Thread new fields through gateway manager/types/catalog and generated docs.
5. Replace dashboard row-derived analytics with backend aggregates.
6. Rework Usage Explorer to use server-side filters and cursor pagination.
7. Make dashboard/hero/chart/panels drillable with preserved window and filters.
8. Fix tool/agent detail fetchers to use exact filtered backend data.
9. Add frontend unit/browser coverage for full-window analytics and deep links.
10. Benchmark representative 24h/7d queries against a copy of the live usage DB; verify indexes/query plans.
11. Run full Rust/frontend/docs gates, adversarial review, create PR.

No production deployment is part of this work.
