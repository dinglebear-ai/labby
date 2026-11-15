# Usage analytics vertical slice

Authenticated `/usage` reads `gateway.usage.metrics` through the caller's server-side gateway session. Gateway administrator permission remains enforced by Rust. This is exact aggregate telemetry, not the retained calls page and not token/cost reporting.

## Contract evidence

- `crates/labby-gateway/src/gateway/catalog.rs`: `gateway.usage.metrics` is non-destructive, requires administrator permission, and rejects queries above 250,000 matching rows.
- `crates/labby-gateway/src/gateway/params.rs`: `GatewayUsageMetricsParams` accepts Unix-second bounds, timezone and bucket count.
- `crates/labby-gateway/src/gateway/types.rs`: `GatewayUsageMetricsView` supplies counts, elapsed-time percentiles and time buckets.
- `crates/labby-gateway/src/gateway/manager/usage.rs`: preserves scoped upstream restrictions and fails when telemetry storage is unavailable.
- `crates/labby-gateway/src/usage/query.rs`: maximum bucket count is 168.

The console requests 24 buckets for 1 hour, 24 hours or 7 days in UTC, with facets and upstream series disabled. It projects only non-negative aggregate counts, mean/percentile elapsed time, distinct identities, peak calls/minute and bounded timestamp/calls/failure buckets. Invalid count relationships, out-of-window timestamps, malformed and oversized responses fail closed. Private and unknown response fields are omitted. Zero calls is distinct from unavailable telemetry.

## TDD and verification receipts

`/tmp/usage-red.log`: the three HTTP-contract tests first failed because `UsageMetrics.fetch` did not exist (0/3 passed). After implementation, `/tmp/usage-green.log` recorded 3 passed. The LiveView regressions exercise public mounting/events, asynchronous loading, selector request bounds, stale removal/retry, empty results, anonymous access and expiry during an in-flight read.

Final focused command:

```sh
mix test test/labby_console/usage_metrics_test.exs test/labby_console_web/usage_metrics_live_test.exs
```

`/tmp/usage-final.log`: 7 passed, seed 886616. Shared compilation emitted a warning in the concurrently authored usage calls slice; these owned files emitted no warnings in that receipt.

`e2e/usage-metrics.spec.mjs` authors the browser sign-in, visible aggregates, both selector changes, isolated caller context, 320px overflow assertion, malformed/oversized/unavailable recovery, zero calls and forbidden-session redirect. The coordinator owns the serial combined Playwright execution; this slice did not launch browsers or build assets independently.

## Scope

No live gateway requests, repairs, credential changes, writes, or fabricated successful API operations. The shared authenticated transport, route, fixture dispatcher, navigation and common styling are coordinator-owned. Broader filter controls, leader rankings, upstream series and visual charts remain subsequent scope; this slice implements bounded aggregate/window behavior only.
