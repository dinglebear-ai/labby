# MCP 2026-07-28 Conformance

Labby targets the `2026-07-28` MCP protocol through
`rmcp = "=3.0.0-beta.2"`. The dated protocol suite and experimental extension
suite are intentionally reported separately: extension results must never
inflate or reduce the dated-protocol score.

## Reproducible Pins

| Component | Pin |
|---|---|
| MCP protocol | `2026-07-28` |
| rmcp | `3.0.0-beta.2` |
| rmcp tag commit | `14298b72e0b25473ea79d5465fe186e22eb86397` |
| MCP conformance package | `0.2.0-alpha.9` |

Run the same gate locally with:

```bash
scripts/ci/mcp-conformance.sh
```

The script verifies the exact Cargo dependency, resolves the exact upstream
rmcp tag commit, installs the JavaScript conformance dependency once, and then
runs:

1. every dated `2026-07-28` server scenario
2. every Tasks extension server scenario
3. every dated `2026-07-28` client scenario
4. the complete client extension suite

Reports are written under `target/mcp-conformance/`. Experimental extension
gaps use the strict
`conformance/expected-failures-extensions.yaml` baseline. An unexpected
failure or an expected failure that starts passing both fail CI.

The upstream conformance binaries deliberately exercise the SDK-level
dispatches for auth, scope step-up, schemas, tools, MRTR, task lifecycle,
OAuth client credentials, cache hints, event stores, subscriptions, lifecycle,
disconnect behavior, protocol versions, and SEP-2243 request headers. Labby
then adds product-adapter tests through its actual Axum `/mcp` route and
gateway proxy.

## Labby Product Contract

| Area | Labby posture | Regression evidence |
|---|---|---|
| Protocol lifecycle | `2026-07-28` only; legacy initialize is rejected | `http_mcp_rejects_legacy_initialize_lifecycle` and `http_mcp_discovers_only_the_new_stateless_protocol` |
| Stateless HTTP | No `Mcp-Session-Id`; `NeverSessionManager`; JSON responses | HTTP lifecycle tests and rmcp dated suite |
| SEP-2243 headers | rmcp validates method/name headers before dispatch | `http_mcp_rejects_mismatched_sep_2243_method_header` and `http_mcp_rejects_missing_sep_2243_name_header_for_tool_call` |
| Cache hints | Dynamic Labby lists/reads emit `ttlMs: 0` with private scope | tool, prompt, resource, and server serialization tests |
| MRTR | Input-required results remain first-class; destructive confirmation uses elicitation with no legacy `requestState` | `destructive_builtin_uses_stateless_mrtr_elicitation` and gateway relay tests |
| Tasks | Gateway/bridge preserves upstream task outcomes and get/cancel operations | bridge tests plus the rmcp Tasks extension suite |
| Subscriptions | List-changed notifications work without a legacy session identifier | `stateless_subscription_receives_catalog_notifications` |
| Disconnect | Stateless HTTP owns no resumable server session to delete | rmcp dated lifecycle suite |
| Auth and scope step-up | Labby owns inbound bearer/OAuth policy; rmcp client conformance exercises protocol auth dispatch | dated client suite and auth middleware tests |

### MRTR confirmation

Destructive actions are not converted into local policy flags on the MCP
surface. The tool call returns an MRTR `input_required` result with an
elicitation form. The client resubmits the accepted form response using the
normal MCP elicitation flow. `requestState` is not required or emitted.

### Tasks

Labby's root product server does not create a second, private task scheduler.
When Labby is bridging an upstream MCP server, task-bearing results and
`tasks/get` or `tasks/cancel` calls pass through without being collapsed to a
complete-only response. SDK-owned task examples and lifecycle behavior are
covered by the pinned rmcp conformance binary.

### Cache and subscriptions

Catalog-derived responses are dynamic and private:

```json
{
  "ttlMs": 0,
  "cacheScope": "private"
}
```

Catalog mutations notify connected peers through list-changed subscriptions.
Clients that do not honor those notifications must reconnect to refresh their
cached catalog.

### OAuth client credentials and scope step-up

Labby's inbound authorization server remains authorization-code based for
interactive operators. OAuth client-credentials examples belong to rmcp's
outbound client/auth implementation and are exercised by the extension suite.
Labby does not claim a product-level machine-credential grant that it does not
expose.

### Event stores and disconnect

The hosted Labby endpoint is intentionally fully stateless. It uses
`NeverSessionManager`, so there is no server-side session event store, resume
cursor, or disconnect cleanup contract. Event-store examples are validated in
the pinned rmcp SDK harness; adding a durable event store to Labby would be a
different deployment mode, not a prerequisite for stateless conformance.

## CI and Maintenance

The `MCP 2026-07-28 conformance` CI job is part of `ci-gate`. Its JavaScript
dependency installation is serialized before scenarios start. Bump the four
pins above together and review the scenario list plus extension baseline on
every conformance package update.

`rmcp` and `rmcp-macros` are upstream packages. Labby's release automation
publishes Labby artifacts only; it consumes the exact published rmcp version
and must not attempt to republish upstream crates. This repository also has no
Chinese README to remove.

Primary references:

- <https://modelcontextprotocol.io/specification/2026-07-28>
- <https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.0.0-beta.2>
- <https://github.com/modelcontextprotocol/conformance>
