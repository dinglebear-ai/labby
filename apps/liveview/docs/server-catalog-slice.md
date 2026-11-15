# Server catalog vertical slice

The authenticated `/gateway/servers/:id/catalog` route reads a retained capability snapshot. The route parameter is the upstream **name** (inventory links URL-encode the `name` field), because these API operations accept `{name: string}`. It does not reconnect, discover, execute tools, fetch resources, or get prompts.

## Source contracts

- `crates/labby-gateway/src/gateway/dispatch.rs`: `gateway.discovered_tools`, `gateway.discovered_resources`, `gateway.discovered_prompts` parse `GatewayNameParams` and enforce caller-visible scope.
- `crates/labby-gateway/src/gateway/manager/views.rs`: tools return exposure rows; resources return cached URI strings; prompts return cached name strings.
- `crates/labby-gateway/src/gateway/types.rs`: `GatewayToolExposureRowView` has `name`, optional `description`, `exposed`, optional `matched_by`.

There is no `gateway.discovered_skills` operation in this contract. The screen says so; it does not invent skill inventory. Resource URI strings are escaped text, never navigation links. Only tool name, optional description and exposed state are retained in the page model; extra response fields are discarded.

## Integration

`LabbyConsole.ServerCatalog.load/2` uses the shared `Auth.read_action/4` read-only allowlist. It validates at most 1,000 entries per family and 4,096-byte individual strings. Tool descriptions may be absent, null or empty. Transport/session/CSRF/response-byte limits remain in the common auth client. Any failed family invalidates the entire displayed snapshot; the page never presents partial or stale results as current.

The LiveView gates both disconnected and connected mounts through `Auth.current/1`, loads asynchronously, clears prior results before explicit reload, redirects on expired/revoked sessions, and shows distinct loading, error and empty states.

## TDD and verification

Observed red: first HTTP test failed because `ServerCatalog.load/2` did not exist. Implemented the read-only projection, then observed green. First route test failed with 404 until coordinator integrated the route. Later contract regression deliberately failed on an empty optional tool description, then passed after accepting the Rust-compatible value.

Focused command (2026-10-27):

```sh
mix test test/labby_console/server_catalog_test.exs test/labby_console_web/server_catalog_live_test.exs
```

Result: **7 tests passed**. Covers valid projection, discarded fields, malformed/invalid/oversized results, empty results, unknown server, caller isolation, revocation, optional empty descriptions, loading, stale-result removal and retry.

`e2e/server-catalog.spec.mjs` adds one browser journey for sign-in, all three families, isolated anonymous context, 320px overflow, malformed result, empty result, recovery and expiry. The coordinator runs this serially with the other slice journeys; this worker did not launch competing browser suites.

The fixture uses the common authenticated HTTP server, validates cookie/CSRF before dispatch, and switches string modes through the test-only `POST /fixture/server_catalog/:mode` endpoint. These are isolated test responses, not production gateway observations.
