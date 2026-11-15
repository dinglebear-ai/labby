# Passive upstream runtime slice

`/gateway/upstreams/:name/runtime` uses the configured upstream name, not the server configuration ID. It requires an authenticated Console session and the Gateway enforces administrator access.

## Contract evidence

- `crates/labby-gateway/src/gateway/catalog.rs:1874`: `gateway.mcp.list`, non-destructive, administrator-required, optional `name` filter.
- `crates/labby-gateway/src/gateway/dispatch.rs:1306`: dispatch to `mcp_runtime_list` with caller enrichment scope.
- `crates/labby-gateway/src/gateway/runtime.rs:523`: named lookup checks visibility, uses retained process context and cached subject capabilities, does not connect upstreams.
- `crates/labby-gateway/src/gateway/types.rs:669`: `GatewayMcpRuntimeView[]`, optional process IDs/age and defaulted count/status fields.

The Console accepts one matching row or an empty list, validates numeric/string/status types, and projects only status, process IDs, age, likely stale process count, transport/version metadata, and capability counts. Target commands, filesystem paths, owners, incident messages, and arbitrary extra fields are omitted. Missing process data is “Not reported,” never zero. Likely stale processes remain an observation, not an instruction to kill processes.

OAuth status was deliberately excluded: `gateway.oauth.status` can proactively refresh credentials and reprobe discovery (`oauth_lifecycle.rs:870` onward). It cannot truthfully be called passive status.

## Tests

Approved seams: actual loopback HTTP contract through Auth and rendered LiveView behavior. Client test first failed with `UndefinedFunctionError` (0/3 passed, `/tmp/server-runtime-red.log`), then passed all three after implementation (`/tmp/server-runtime-green.log`). LiveView tests first failed because the module was absent (`/tmp/server-runtime-live-red.log`), then passed after implementation (`/tmp/server-runtime-live-green.log`). Additional empty/disconnected regressions distinguish missing process metadata from failures.

Focused verification: `mix test test/labby_console/server_runtime_test.exs test/labby_console_web/server_runtime_live_test.exs`: 8 passed, seed 535031.

`e2e/server-runtime.spec.mjs` authors browser sign-in, credential-free snapshot rendering, isolated anonymous context, 320px overflow check, malformed/wrong-name/unavailable recovery with stale removal, empty/disconnected states, and authorization loss redirect. Coordinator owns the serial combined Playwright run; this agent has not run it. No live Gateway calls, process control, OAuth operations, or repairs were performed.
