# Real upstream browser tests

`e2e-real/mcp-server.mjs` exports `startMcpServer({port:4300})`. The root owned
launcher starts it before Labby, and awaits `close()` during cleanup. It binds
only IPv4 loopback. Its `/mcp` endpoint implements an actual stateless MCP JSON
transport: initialization, tools/resources/prompts discovery and reads, and two
read-only tools. `echo` succeeds; `fail` returns a completed MCP tool error.
No Labby API response, discovery cache, or usage database is replaced or seeded.

The isolated Labby config needs an enabled `real-e2e-mcp` upstream with URL
`http://127.0.0.1:4300/mcp` and `[code_mode] enabled = false` to expose raw tools.
This test configuration exercises raw MCP routing; it does not establish Code
Mode execution coverage. The support helper resolves the actually advertised
name before execution rather than assuming bare/qualified naming.

`catalog.spec.mjs` verifies populated retained tools, resources, prompts and
320px overflow, then forces this owned upstream offline via `/control`, attempts
a real call, and checks retained catalog remains available before recovery.
`activity.spec.mjs` invokes both real tools through Labby's `/mcp`, verifies
retained UI call records and aggregate usage, changes activity filters, and verifies cursor pagination after 26 bounded real calls.
Every spec establishes its own authenticated session and warms its own catalog.
The shared runner must keep workers=1 and its request/test timeouts enabled.

Protocol regression: `node --test e2e-real/mcp-server.test.mjs`. A missing-module
red was observed before implementation (`/tmp/mcp-red.log`); the protocol test
then passed. Full real Playwright is run by the integrating coordinator, not by
this worker. These tests do not prove OAuth, external upstreams, Code Mode,
browser-driven tool execution (UI is read-only).
