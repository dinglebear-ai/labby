# Usage metrics dimensional benchmark

Captured 2026-08-21 with the opt-in
`benchmark_production_shaped_dimensional_queries` harness in `UsageStore`.
The fixture contains 240,000 rows spread across seven days, 12 upstreams, 80
targets, 30 actors, two capability/operation families, shared and OAuth-subject
scope, mixed outcomes, and varied latency.

Debug-build measurements on the development host:

| Query | Matching rows | Elapsed |
| --- | ---: | ---: |
| 24h + capability `resources` | 17,281 | 174 ms |
| 7d + operation `tool.call` + subject scoped | 27,428 | 403 ms |

`EXPLAIN QUERY PLAN` selected the covering composite indexes
`idx_upstream_calls_capability_ts`, `idx_upstream_calls_operation_ts`, and
`idx_upstream_calls_subject_scoped_ts` for their respective dimensional time
predicates. Re-run the ignored harness to compare another host; these timings
are evidence for the query shape, not a universal service-level objective.

The benchmark does not resolve the product policy for exact queries above
250,000 matching rows. The existing explicit rejection remains unchanged and
the fixed/configurable/alternative-bounded-strategy choice stays deferred.
