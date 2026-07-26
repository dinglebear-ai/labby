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
| Auth and scope step-up | Labby owns inbound OAuth 2.1 policy, CIMD, RFC 9207 issuer binding, revocation, client credentials, ID-JAG exchange, and RFC 9728 challenges | dated client suite plus the Labby auth contract step in conformance CI |

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

### OAuth extensions and scope step-up

Labby's inbound authorization server supports the interactive authorization-code
flow, refresh-token rotation and revocation, and the optional MCP OAuth client
credentials extension. Machine clients
are preregistered out of band and may authenticate with `client_secret_basic`
or RFC 7523 `private_key_jwt`; assertions are audience-bound to `/token` and
replay-protected.

Trusted enterprise issuers may also be configured for
enterprise-managed authorization extension. Labby validates
`oauth-id-jag+jwt` assertions against pinned inline or HTTPS JWKS, enforces
issuer, audience, client, resource, scope, expiry, and one-time `jti`, and mints
the same audience-restricted Labby access token used by the interactive flow.

The remaining `auth/enterprise-managed-authorization` expected-failure entry is
specifically the pinned rmcp beta.2 **outbound conformance client**, not Labby's
authorization server. Product-server coverage runs immediately before the
upstream SDK conformance harness in the same CI job.

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

The separate `MCP upstream drift` workflow compares
`conformance/upstream-baseline.json` with the current MCP specification branch
and latest rmcp release. Its report lists upstream files, the Labby modules
that must be inspected, and the validation commands that must run. Detected
drift opens or updates one stable issue; advance the baseline only in the PR
that adopts and verifies the upstream change.

`rmcp` and `rmcp-macros` are upstream packages. Labby's release automation
publishes Labby artifacts only; it consumes the exact published rmcp version
and must not attempt to republish upstream crates. This repository also has no
Chinese README to remove.

Primary references:

- <https://modelcontextprotocol.io/specification/2026-07-28>
- <https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.0.0-beta.2>
- <https://github.com/modelcontextprotocol/conformance>
