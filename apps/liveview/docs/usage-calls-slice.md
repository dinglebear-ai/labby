# Retained call activity slice

Route: `/activity`. Authenticated read-only activity, separate from log viewers.

## Contract

- `crates/labby-gateway/src/gateway/params.rs`: `GatewayUsageCallsParams` supports time bounds, exact outcome and opaque cursor; this UI sends `since_unix`, `until_unix`, optional `outcome`/`cursor`, and `limit: 25`.
- `crates/labby-gateway/src/gateway/types.rs`: `GatewayUsageCallView` and `GatewayUsageCallsView`. Calls, optional next cursor, and optional aggregate total are separate fields. This slice does not request or infer a total.
- `crates/labby-gateway/src/gateway/dispatch.rs`: `gateway.usage.calls` forwards transport enrichment scope.
- `crates/labby-gateway/src/gateway/manager/usage.rs`: `usage_calls_scoped` applies route-visible upstream restrictions, rejects offset pagination, clamps limits, and passes a parsed cursor to the retained usage store. The client treats returned cursors as opaque.
- Read transport revalidates the current caller, supplies the server-held gateway session cookie and fresh CSRF token, and removes local authorization after 401/403.

## Behavior and limits

- One-hour, 24-hour, and seven-day windows; all outcomes, succeeded, timeout, or upstream-error exact matching. These selected outcomes are shortcuts, not the full possible outcome vocabulary; all outcomes still displays other API outcomes.
- Next-page uses only the API-returned cursor, retains fixed window bounds while paging, and replaces the page. Filter changes reset pagination. Refresh reloads the first page of the selected window.
- Clear retained rows and cursors while loading and after failures. Empty, unavailable, and expired-session states remain distinct.
- Project only safe call identity/timing fields; omit attribution payloads, response sizes, raw response details, and extra fields. Bound rows, strings, cursor lengths, time values, and elapsed values. Reject invalid local filters before HTTP dispatch.
- Display counts describe the current retained page, never aggregate usage. No live feed, mutations, or log-stream claims.

## TDD and regression receipts

1. Added client contract test first. `/tmp/usage-calls-red.log`: undefined `LabbyConsole.UsageCalls.list/2`, 0/1 passed.
2. Implemented the client and HTTP fixture, then added LiveView pagination, filters, stale-data clearing/recovery, empty state, and access regressions.
3. Added out-of-range timestamp regression before its guard. `/tmp/usage-time-red.log`: 1/2 passed, unsafe timestamp accepted. Bounded timestamps before rendering.
4. Final focused run: `mix test test/labby_console/usage_calls_test.exs test/labby_console_web/usage_calls_live_test.exs` — **4 passed**, seed 497058. `/tmp/usage-calls-green.log`.

Playwright journey: `e2e/usage-calls.spec.mjs` signs in through the actual browser form, tests cursor pagination, filter reset, outcome filtering, stranger isolation, 320px overflow, malformed/empty/recovery, and revoked-session redirect. Coordinator owns its serial browser run and the full suite; this agent did not run Playwright or assets builds. All API evidence uses loopback fixtures; no live gateway was probed or mutated.
