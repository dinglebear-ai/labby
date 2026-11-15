# Observed MCP clients slice

The authenticated `/clients` page reads retained inbound MCP client observations. It never treats observations as connection liveness or unique people and performs no mutations.

## Contract

- `crates/labby-gateway/src/gateway/catalog.rs`: `gateway.clients.list` has no parameters, requires admin authority and is non-destructive.
- `crates/labby-gateway/src/gateway/dispatch.rs`: dispatch returns `manager.clients()`.
- `crates/labby-gateway/src/gateway/manager/views.rs`: projects registry `subject_tag` into `subject`, plus authorized client ID, name/version, transport, first/last timestamps and observation count.
- `crates/labby-gateway/src/gateway/types.rs`: `GatewayClientView[]`; optional identity/name/version and last-seen fields, required transport and connected-at; observation count defaults to zero and is an unsigned 64-bit integer.
- `crates/labby-gateway/src/gateway/dispatch_tests.rs`: preserves redacted observed-client projection and empty-array behavior.

The console sends `POST /v1/gateway` with `{"action":"gateway.clients.list","params":{}}` through its strict shared authenticated read transport. Gateway authority remains authoritative; rejection removes the console session. The client validates bounded rows/text/counts and projects only documented fields, excluding unknown response data. Names and timestamps are escaped by HEEx. Dates retain their gateway-reported representation, without inferring live state.

The view follows the mock's Observed MCP clients pattern: name/version, transport, observations and reported timestamps; optional identity detail is collapsed. Shared shell, refresh action and status/error patterns match the existing real app.

## TDD and verification

Approved seams are the actual HTTP client against a loopback contract fixture and visible LiveView behavior.

1. Client contract test failed with undefined `LabbyConsole.Clients.list/1` (`/tmp/clients-red.log`) before implementation.
2. Page behavior test failed at the missing `/clients` route (`/tmp/clients-live-red.log`) before page implementation/shared routing.
3. After implementation and root-owned shared routing/allowlist/fixture integration, focused tests passed: **5 tests**, seed `516982` (`/tmp/clients-green.log`).

Command from `apps/liveview`:

```sh
mix test test/labby_console/clients_test.exs test/labby_console_web/clients_live_test.exs
```

Regression coverage includes authenticated HTTP shape/projection, unknown-caller isolation, malformed/oversized/unavailable/invalid responses, optional-field defaults, empty results, forbidden access, loading/disabled refresh, failed refresh removing old rows, recovery, anonymous mount and session expiry while a response is pending.

`e2e/clients.spec.mjs` authors a real browser sign-in journey against isolated test fixtures, stranger-context isolation, narrow viewport overflow, malformed/empty/recovery transitions and revoked-session removal. The coordinating agent owns its serial execution together with the full suite; no browser suite was run by this worker.

No live gateway, service configuration, registry mutation or credentials were used. The page does not add disconnect/revoke controls, polling, inferred user counts or a live-connected badge.
