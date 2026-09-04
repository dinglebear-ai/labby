---
title: "MCP 2026-07-28 Conformance"
created: "2026-07-30"
updated: "2026-08-30"
---

# MCP 2026-07-28 Conformance

Labby targets the `2026-07-28` MCP protocol through the immutable rmcp Git
revision recorded in `Cargo.toml` and `Cargo.lock`.
The gate verifies that production dependency exactly, exercises Labby's real
authenticated `/mcp` boundary, runs the matching upstream rmcp fixture, and
covers Labby-native gateway and multi-hop scenarios. Dated protocol, extension,
and native-product results are reported separately so fixture coverage cannot
hide a product regression or inflate the dated-protocol score.

Labby has simultaneous protocol roles, and the evidence keeps them distinct:

- over Streamable HTTP, Labby is an MCP protected resource and runs an inbound
  OAuth authorization server for clients such as ChatGPT;
- as a gateway, Labby is an OAuth client of protected HTTP upstream MCP
  servers;
- over stdio, downstream credentials come from the local environment or the
  already configured gateway runtime. Stdio is not itself an HTTP OAuth
  transport, although `labby mcp` may initiate OAuth for an HTTP upstream and
  receive its callback on a process-local loopback listener.

An assertion about one role is not accepted as evidence for another role.

## Reproducible Pins

| Component | Pin |
|---|---|
| MCP protocol | `2026-07-28` |
| Labby rmcp dependency | pinned Git revision `0665dcac` |
| rmcp conformance fixture | `3.1.0` |
| rmcp fixture tag commit | `1f9358eddca42d3a510c70ae6446dd6548c7c856` |
| MCP conformance package | `0.2.0-alpha.10` |

Run the complete gate locally with:

```bash
scripts/ci/mcp-conformance.sh
```

The script verifies the exact production Cargo dependency and matching upstream
fixture tag commit, installs the JavaScript conformance dependency once, builds
a fresh Labby binary, and then runs:

1. a Labby-native client -> root Labby -> middle Labby -> synthetic leaf chain
2. the authenticated stateless Labby HTTP boundary smoke checks
3. every dated `2026-07-28` server scenario
4. every Tasks extension server scenario
5. every dated `2026-07-28` client scenario
6. the complete client extension suite

Reports are written under `target/mcp-conformance/`. Known dated fixture gaps
use the strict `conformance/expected-failures-dated.yaml` baseline, while
experimental extension gaps use
`conformance/expected-failures-extensions.yaml`. An unexpected failure or an
expected failure that starts passing both fail CI.

## Labby-Native Multi-Hop Matrix

The `mcp_multihop_conformance` driver launches production Labby transports
with isolated configuration:

```text
MCP client -> root Labby (stdio) -> middle Labby (authenticated HTTP) -> leaf MCP server (stdio)
```

Middle and root are warmed through Labby's real `gateway.reload` lifecycle,
not by test-only pool mutation. Each child process also pins its working
directory to its isolated temporary home because Labby intentionally resolves
`./config.toml` before HOME-scoped configuration; a caller's unrelated working
directory must never shadow the fixture. The leaf publishes more than one page
of every catalog family so the chain proves ownership and pagination beyond the
first page.

| Scenario | Multi-hop assertion |
|---|---|
| Discovery | Root negotiates `2026-07-28` and identifies itself as Labby |
| Tools | 75 late-page tools plus an MRTR tool survive both gateways |
| Tool calls | A late-page tool executes through root and middle with its arguments intact |
| MRTR | `input_required` remains first-class through both gateways |
| Tasks | Create, get, update, and cancel route through both gateways; gateway IDs remain stable and task-status notifications are translated and delivered before an overtaking response can tear the request down |
| Progress | Progress tokens and messages survive both gateway hops in upstream wire order; the driver observes order at the raw client transport rather than concurrent callback completion order |
| Cancellation | A downstream cancellation traverses root -> middle -> leaf, where the leaf observes it |
| Prompts | 70 prompts aggregate; a late-page prompt can be fetched |
| Resources | 70 resources aggregate; nested gateway URIs remain routable on read |
| Resource templates | 70 templates aggregate without flattening nested gateway namespaces |
| Completion | Completion against a nested resource template reaches the leaf |
| Subscriptions | Tool, prompt, and resource list changes plus an exact resource update cross both gateways; re-listing exposes the dynamically added catalog entries |
| Provenance | Labby stamps the responding server while preserving leaf identity and custom metadata |

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
| Progress and cancellation | Request IDs and progress tokens are translated per relay connection; cancellation targets the actual upstream request | relay connector/route tests plus the multi-hop driver |
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

Request-scoped relay connections intercept progress and task-status
notifications at the upstream transport's sequential receive boundary and
commit them to one bounded FIFO before any later response can be returned to
rmcp. This is intentionally below `ClientHandler` callbacks: rmcp may cancel and
re-poll `Transport::receive()` and may run notification callbacks concurrently.
Queuing immediately after wire consumption makes delivery cancellation-safe,
preserves progress wire order, and prevents a task response from closing the
downstream request while an earlier `Working` notification is still in flight.

### Cache and subscriptions

Catalog-derived responses are dynamic and private:

```json
{
  "ttlMs": 0,
  "cacheScope": "private"
}
```

Labby establishes upstream `subscriptions/listen` streams, consumes list and
resource updates, and publishes normalized events to downstream subscription
sinks. Downstream acknowledgement is filtered to notification categories and
resource URIs Labby can actually deliver. Protected-route scope remains applied
when notifications fan out. A tool-list change first re-lists the exact named
upstream and atomically replaces that upstream's cached tools before peer-visible
contracts are evaluated. Both long-lived subscription streams and request-scoped
relay connections publish through this refresh path.

### Multi-hop cancellation

Stateless HTTP sessions cannot use transport-local request IDs to correlate a
cancellation reliably across gateway hops. Labby therefore attaches an opaque
relay token to the proxied request and sends an acknowledgement-bearing custom
JSON-RPC request using `MCP_RELAY_CANCELLATION_REQUEST_METHOD`. A negative
acknowledgement is retried within a bounded delivery window after the upstream
request ID is published. Standard MCP cancellation remains the compatibility
fallback for peers that do not understand the relay request.

### OAuth extensions and scope step-up

Labby's inbound authorization server supports authorization-code flow,
refresh-token rotation and revocation, and the optional MCP OAuth client
credentials extension. Machine clients are preregistered out of band and may
authenticate with `client_secret_basic` or RFC 7523 `private_key_jwt`;
assertions are audience-bound to `/token` and replay-protected.

For an upstream URL with a path, protected-resource discovery tries the three
MCP candidates in this order: `/.well-known/oauth-protected-resource/<path>`,
`/<path>/.well-known/oauth-protected-resource`, then the origin-root
`/.well-known/oauth-protected-resource`. Authorization-server discovery keeps
the selected issuer path and tries RFC 8414 metadata, path-scoped OIDC metadata,
then the issuer-path OIDC form. Published `issuer` values are compared exactly;
only the explicitly tested Google issuer/token-origin split is permitted.

Trusted enterprise issuers may be configured for enterprise-managed
authorization. Labby validates `oauth-id-jag+jwt` assertions against pinned
inline or HTTPS JWKS and enforces issuer, audience, client, resource, scope,
expiry, and one-time `jti`.

The expected-failure entries for enterprise-managed authorization, DPoP,
DPoP nonce, and WIF JWT-bearer describe experimental outbound flows not
implemented by the pinned rmcp conformance client. They do not describe Labby's
inbound authorization server. Product-server coverage runs before the SDK
fixture harness in the same CI job.

### Authorization requirement denominator

[`conformance/auth-requirements.json`](../../conformance/auth-requirements.json)
is the concise requirement-family summary for MCP `2026-07-28` authorization
and the OpenAI plugin authentication requirements used by ChatGPT. The fixed,
exhaustive denominators are maintained separately in the MCP and OpenAI
normative matrices described below. Each summary row records an authoritative
URL, paraphrase, applicability, implementation evidence, stable test ID, and
status. `scripts/ci/test_auth_spec_matrix.py` rejects missing, duplicate,
unevidenced, or non-authoritative rows. `gap` and `partial` are explicit
follow-ups, not passing conformance claims.

OpenAI rows use `scripts/ci/openai-auth-conformance.sh OAI-AUTH-NNN` as their
requirement-specific executable evidence. Running the script without an ID
runs the complete OpenAI authentication denominator in CI; `--list` prints the
stable IDs. The focused checks cover discovery and authorization-server
metadata, tool descriptor security declarations and their compatibility
mirror, MCP result challenges, per-request token validation, public-route
401/403 challenges, DCR advertisement parity, and refresh/revocation drills.
`OAI-AUTH-011` derives its denominator from the generated HTTP route inventory
and the live registered MCP services/actions. Every mounted customer-specific
or write HTTP route is probed behind the shared authentication layer;
runtime-conditional entries must remain explicitly classified. Every MCP
service advertises the canonical OAuth scope, and every action's admin
requirement is checked against its `ActionSpec` before dispatch. A newly
inventoried route, registered service, or action therefore fails CI until its
authentication exposure is explicitly classified.

The independently generated
[`conformance/mcp-auth-normative.json`](../../conformance/mcp-auth-normative.json)
preserves every normative-keyword occurrence in the official Authorization,
Authorization Server Discovery, Client Registration, and Authorization
Security Considerations Markdown pages. At the 2026-08-30 refresh this is 132
requirements: 82 `MUST`, 11 `MUST NOT`, 37 `SHOULD`, and 2 `SHOULD NOT`.
`scripts/ci/refresh_mcp_auth_denominator.py` refreshes that snapshot directly
from the four official primary-source URLs; the structural CI test prevents a
summary matrix from silently becoming the standards denominator.

The canonical disposition is 128 applicable passing rows and four explicit
product-boundary exclusions. Client Registration rows 004–007 describe an MCP
OAuth client that elects CIMD and hosts its own HTTPS client metadata; Labby is
the authorization/resource server validating inbound CIMD, not such a client.
`scripts/ci/mcp_auth_normative_conformance.py` resolves every stable row ID to
one or more invoked focused assertions. Direct rows name exact behavioral tests.
Broad summary clauses instead use `subordinate_row_ids`: an explicit, acyclic
aggregate-evidence graph whose leaves are direct tests or justified
not-applicable role branches. Aggregate rows may not also claim direct tests,
and CI rejects unknown, duplicate, self-referential, cyclic, or unresolved
subordinate mappings.
`scripts/ci/publish_mcp_auth_disposition.py` reproducibly publishes the reviewed
row mappings from `conformance/mcp-auth-coverage-manifest.json` without changing
the frozen primary-source denominator. Every manifest entry binds the extracted
source-clause digest, asserted obligation, executable assertions, and evidence;
the publisher contains no numeric-range promotion logic.

`scripts/ci/refresh_mcp_auth_denominator.py --check` re-downloads the four frozen
official pages and verifies their digests and extracted clauses. SDK-level auth
and custom-response tests run against Cargo's exact immutable rmcp Git checkout,
resolved from locked workspace metadata. Labby therefore keeps no copied rmcp
source tree or parallel vendor-provenance policy in this repository.

### Rollout and Inspector verification

Before an auth change is rolled out, run the normative and OpenAI denominators,
the repository auth gates, and the backup/restore drill documented above. In a
staged instance, verify unauthenticated discovery, a `lab:read` challenge and
catalog read, execution step-up, refresh, revocation, and a denied destructive
call from a client without elicitation. Use the Code Mode Inspector only as an
operator observability aid: it may confirm the authenticated subject's visible
catalog and structured failures, but it is not a substitute for the executable
wire assertions and must never display bearer tokens, OAuth codes, or provider
credentials.

### Event stores and disconnect

The hosted Labby endpoint is intentionally stateless. It uses
`NeverSessionManager`, so there is no server-side resume cursor or disconnect
cleanup contract. Event-store examples remain validated by the pinned rmcp SDK
harness; a durable event-store deployment would be a separate mode.

## CI and Maintenance

The `MCP 2026-07-28 conformance` job is part of `ci-gate`. JavaScript
dependency installation is serialized before scenarios start. The production
rmcp dependency and upstream fixture remain explicit pins even when they match;
adopt version changes together only after reviewing the dated fixture baseline,
and review the Labby-native matrix plus dated and extension baselines on every
update.

The separate `MCP upstream drift` workflow compares
`conformance/upstream-baseline.json` with the current specification branch and
latest rmcp release. Detected drift opens or updates one stable issue; advance
the baseline only in the PR that adopts and verifies the upstream change.

Primary references:

- <https://modelcontextprotocol.io/specification/2026-07-28>
- <https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization>
- <https://developers.openai.com/plugins/build/auth>
- <https://developers.openai.com/plugins/reference>
- <https://github.com/modelcontextprotocol/rust-sdk/releases/tag/rmcp-v3.1.0>
- <https://github.com/modelcontextprotocol/conformance>
