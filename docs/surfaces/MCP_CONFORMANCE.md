---
title: "MCP 2026-07-28 Conformance"
created: "2026-07-30"
updated: "2026-08-02"
---

# MCP 2026-07-28 Conformance

Labby targets the `2026-07-28` MCP protocol through
`rmcp = "=3.1.0"`. Dated protocol results, extension results, and
Labby-native gateway scenarios are reported separately so SDK fixture coverage
cannot hide a product-adapter regression.

## Reproducible Pins

| Component | Pin |
|---|---|
| MCP protocol | `2026-07-28` |
| rmcp | `3.1.0` |
| rmcp tag commit | `1f9358eddca42d3a510c70ae6446dd6548c7c856` |
| MCP conformance package | `0.2.0-alpha.10` |

Run the complete gate locally with:

```bash
scripts/ci/mcp-conformance.sh
```

The script verifies the exact Cargo dependency and rmcp tag commit, installs the
JavaScript conformance package once, builds a fresh Labby binary, and runs:

1. a Labby-native client -> root Labby -> middle Labby -> synthetic leaf chain
2. the authenticated stateless Labby HTTP boundary smoke checks
3. every dated `2026-07-28` server scenario
4. every Tasks extension server scenario
5. every dated `2026-07-28` client scenario
6. the complete client extension suite

Reports are written under `target/mcp-conformance/`. Experimental extension
gaps use the strict
`conformance/expected-failures-extensions.yaml` baseline. An unexpected
failure or an expected failure that starts passing both fail CI.

## Labby-Native Multi-Hop Matrix

The `mcp_multihop_conformance` driver launches production Labby transports
with isolated configuration:

```text
MCP client -> root Labby (stdio) -> middle Labby (authenticated HTTP) -> leaf MCP server (stdio)
```

Middle and root are warmed through Labby's real `gateway.reload` lifecycle,
not by test-only pool mutation. The leaf publishes more than one page of every
catalog family so the chain proves ownership and pagination beyond the first
page.

| Scenario | Multi-hop assertion |
|---|---|
| Discovery | Root negotiates `2026-07-28` and identifies itself as Labby |
| Tools | 75 late-page tools plus an MRTR tool survive both gateways |
| Tool calls | A late-page tool executes through root and middle with its arguments intact |
| MRTR | `input_required` remains first-class through both gateways |
| Prompts | 70 prompts aggregate; a late-page prompt can be fetched |
| Resources | 70 resources aggregate; nested gateway URIs remain routable on read |
| Resource templates | 70 templates aggregate without flattening nested gateway namespaces |
| Completion | Completion against a nested resource template reaches the leaf |
| Tasks | Create/get/update/cancel traverse both gateways; Working, Completed, and Cancelled statuses use the gateway task ID |
| Progress | A request-scoped progress token is preserved and translated through both relay connections |
| Cancellation | Client cancellation propagates through the stdio root and stateless authenticated HTTP middle to the leaf request token |
| Subscriptions | Real tool, prompt, and resource catalog mutations plus an exact resource update reach the root subscriber |
| Provenance | Labby stamps the responding server while preserving leaf identity and custom metadata |

The driver also exercises stateless cancellation using a stateful watch handoff,
an opaque per-request cancellation token, and a hidden Labby-to-Labby control
tool for HTTP hops where rmcp rewrites request IDs. The control request carries
the required `Mcp-Method`, `Mcp-Name`, protocol version, client information, and
client capabilities headers/metadata, and its HTTP response is drained before
completion.

Subscription fanout refreshes the cached upstream tool descriptors before
notifying downstream peers. This is required for nested gateways: a bare
`notifications/tools/list_changed` event without cache refresh leaves the outer
gateway contract unchanged and is correctly suppressed by contract-aware
fanout.

The driver discovered and now guards a critical namespace rule: an upstream
resource URI is opaque. If middle returns
`lab://upstream/leaf/fixture://resource/069`, root exposes
`lab://upstream/middle/lab://upstream/leaf/fixture://resource/069`. Each
hop removes only its own outer prefix. Flattening the inner prefix makes the URI
unroutable.

## Labby Product Contract

| Area | Labby posture | Regression evidence |
|---|---|---|
| Protocol lifecycle | Modern clients use stateless `server/discover`; legacy `initialize` is adapted only at the transport edge | discovery tests, bridge tests, and the multi-hop driver |
| Stateless HTTP | No `Mcp-Session-Id`; `NeverSessionManager`; JSON responses | HTTP lifecycle tests and rmcp dated suite |
| SEP-2243 headers | rmcp validates method/name headers before dispatch | HTTP method/name header tests |
| Request envelopes | Metadata, input responses, request state, cancellation, and progress association survive proxy routes | request-envelope tests and relay module |
| Cache hints | Dynamic Labby lists/reads emit `ttlMs: 0` with private scope | tool, prompt, resource, and server serialization tests |
| MRTR | Tool, prompt, and resource intermediate results remain first-class | relay tests and multi-hop driver |
| Tasks | Gateway-owned handles route get/update/cancel and translate task-status IDs | task routing tests and relay task-status regression |
| Subscriptions | Labby consumes upstream listen streams and forwards subscribed list/resource notifications | upstream subscription and peer fanout tests |
| Resource subscriptions | Labby acknowledges only exact URIs an upstream accepted | subscription filter tests |
| Progress and cancellation | Progress tokens are translated per relay connection; stateful watch receivers and opaque cancellation tokens survive early cancellation, stateless request-ID rewriting, and request teardown | focused relay tests plus the three-process driver |
| Provenance | Ordinary results identify Labby and preserve distinct upstream server identity under `ai.dinglebear.labby/upstreamServerInfo` | provenance unit test and multi-hop driver |
| Error fidelity | Tool error classification is observational; original content, structured content, and metadata pass through | upstream normalization tests |
| Auth and scope step-up | Labby owns inbound OAuth 2.1 policy, CIMD, RFC 9207 issuer binding, revocation, client credentials, ID-JAG exchange, and RFC 9728 challenges | dated client suite plus Labby auth contract tests |

### Lifecycle compatibility

Labby's internal contract is stateless `2026-07-28` discovery. A legacy
`initialize` request is accepted as an edge adapter for existing hosts: Labby
records the peer information and returns the negotiated legacy version without
changing internal request handling or creating a resumable session.

### MRTR and tasks

Destructive actions return an MRTR `input_required` result with an elicitation
form. The client resubmits normal MCP input responses; Labby does not replace
this with a private confirmation protocol.

Labby also does not create a second task scheduler. An upstream task receives a
subject-bound gateway handle. Subsequent `tasks/get`, `tasks/update`, and
`tasks/cancel` calls route over the connection that created it. Incoming
`notifications/tasks` messages translate the native task ID back to the
gateway handle before reaching the downstream client.

### Cache and subscriptions

Catalog-derived responses are dynamic and private:

```json
{
  "ttlMs": 0,
  "cacheScope": "private"
}
```

Labby establishes upstream `subscriptions/listen` streams, consumes list and
resource updates, refreshes cached tool descriptors on tool-list changes, and
publishes normalized events to downstream subscription sinks. Downstream acknowledgement is filtered to notification categories and
resource URIs Labby can actually deliver. Protected-route scope remains applied
when notifications fan out.

### OAuth extensions and scope step-up

Labby's inbound authorization server supports authorization-code flow,
refresh-token rotation and revocation, and the optional MCP OAuth client
credentials extension. Machine clients are preregistered out of band and may
authenticate with `client_secret_basic` or RFC 7523 `private_key_jwt`;
assertions are audience-bound to `/token` and replay-protected.

Trusted enterprise issuers may be configured for enterprise-managed
authorization. Labby validates `oauth-id-jag+jwt` assertions against pinned
inline or HTTPS JWKS and enforces issuer, audience, client, resource, scope,
expiry, and one-time `jti`.

The expected-failure entries for enterprise-managed authorization, DPoP,
DPoP nonce, and WIF JWT-bearer describe experimental outbound flows not
implemented by the pinned rmcp conformance client. They do not describe Labby's
inbound authorization server. Product-server coverage runs before the SDK
fixture harness in the same CI job.

### Event stores and disconnect

The hosted Labby endpoint is intentionally stateless. It uses
`NeverSessionManager`, so there is no server-side resume cursor or disconnect
cleanup contract. Event-store examples remain validated by the pinned rmcp SDK
harness; a durable event-store deployment would be a separate mode.

## CI and Maintenance

The `MCP 2026-07-28 conformance` job is part of `ci-gate`. Bump the four
pins above together and review the Labby-native scenario matrix plus extension
baseline whenever the conformance package or rmcp version changes.

The separate `MCP upstream drift` workflow compares
`conformance/upstream-baseline.json` with the current specification branch and
latest rmcp release. Detected drift opens or updates one stable issue; advance
the baseline only in the PR that adopts and verifies the upstream change.

Primary references:

- <https://modelcontextprotocol.io/specification/2026-07-28>
- <https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.1.0>
- <https://github.com/modelcontextprotocol/conformance>
