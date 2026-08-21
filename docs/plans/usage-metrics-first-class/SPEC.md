# First-Class Usage Metrics Specification

## Goal

Make Labby usage analytics window-complete, scalable, and drillable. No dashboard metric may silently derive from only the newest page of raw calls.

## Contracts

1. Window-complete analytics: `gateway.usage.metrics` computes all displayed durable usage analytics in SQLite over the requested time/routing scope.
2. Raw calls are pagination, not sampling: `gateway.usage.calls` remains bounded per page but filters and counts against the complete retained window.
3. Consistent dimensions: target identity is upstream + tool + capability + operation + subject-scoped. Stable upstream/tool identity remains available for drill-downs.
4. Route scope is invariant: every aggregate, filter, count, facet, and row honors the same allowed-upstream scope.
5. Drillable UI: dashboard metrics, chart buckets, rankings, failures, upstreams, actors, and latency affordances link to or open detail views with window/filter context preserved.
6. Truthful labels: rolling-window totals are labeled as such; recent rows are labeled recent and never presented as full-window analytics.
7. Performance: aggregate responses stay bounded; raw rows use keyset pagination; SQL uses time/scope predicates and grouped aggregates rather than transferring whole windows to the browser.

## Window analytics returned by gateway.usage.metrics

- total / failed / average latency
- p50 / p95 / p99 latency
- exact bucketed call/failure series
- top and least-used dimensional targets, including failures
- distinct dimensional target count
- top actors
- upstream call/failure counts
- failures by outcome kind
- slowest dimensional targets by average latency
- peak calls per minute

## gateway.usage.calls filters

- time bounds
- upstream
- tool
- capability
- operation
- subject-scoped
- actor
- outcome (`ok`, `failed`, or exact failure kind)
- text search over target/operation/actor/outcome
- keyset cursor and exact filtered total

## Non-goals

Token, source-IP, surface, and Code Mode fan-out analytics are not fabricated when the durable usage store does not record those dimensions.
