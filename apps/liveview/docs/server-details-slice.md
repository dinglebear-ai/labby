# Authenticated server details slice

The read-only `/gateway/servers/:id` page calls `gateway.server.get` through the caller's existing server-held session. It shows the retained identity, status and exposed/discovered capability counts. Refresh clears the previous snapshot immediately. Invalid JSON shapes, mismatched identities and unavailable reads clear details; revoked access redirects to sign-in. Configuration, endpoint URLs, warnings and raw failure bodies are never rendered.

Contract references:

- `crates/labby-gateway/src/gateway/dispatch.rs`: `gateway.server.get` parses `VirtualServerNameParams`, enforces visibility and invokes the scoped getter.
- `crates/labby-gateway/src/gateway/manager/views.rs`: `get_server_scoped` is a cache-only projection.
- `crates/labby-gateway/src/gateway/view_models.rs`: `ServerView` identity, status and capability counts. Optional counts and flags default as in Serde.
- `crates/labby-gateway/src/gateway/dispatch_tests.rs`: `gateway_server_get_returns_custom_gateway_row` independently checks the custom-server ID/source projection.

TDD receipts: the unknown-caller client test failed with missing `ServerDetails.fetch/2` then passed; the success test initially failed before the shared authenticated transport existed, then passed after transport/fixture integration. The page seam initially failed with a missing route, then passed after route integration. Focused regression suite: 6 passing tests, including safe projection, malformed/mismatched response rejection, unavailable/revoked access, refresh recovery, caller isolation, visible loading and expiry during an outstanding request.

Run from `apps/liveview`:

```sh
mix test test/labby_console/server_details_test.exs test/labby_console_web/server_details_live_test.exs
npm run test:e2e -- e2e/server-details.spec.mjs
```

The Playwright journey uses the isolated HTTP fixture, browser sign-in, refresh recovery for three failure cases, independent caller context, 320px overflow check, forbidden-access redirect and protected re-entry. It has no fixed sleeps. The coordinator runs the integrated browser suite after parallel slices finish. No hosted CI or live production gateway acceptance is claimed here.

Integration: route `ServerDetailsLive` at `/gateway/servers/:id`, inventory links use encoded `id`. Shared `AuthFixture` validates session cookie and CSRF before delegating `gateway.server.get` to `ServerDetailsFixture.respond/1`; test-only `/fixture/server_details/:mode` stores a string mode. Do not add mutation or reconnect actions to this read-only view.
