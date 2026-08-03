# MCP 2026-07-28 Capability Audit

Original audit: 2026-07-31

Updated closeout: 2026-08-02

## Anchors

- Worktree: `/home/jmagar/workspace/labby/.worktrees/mcp-2026-07-28-capabilities`
- Branch: `audit/mcp-2026-07-28-capabilities`
- Pull request: `dinglebear-ai/labby#311`
- MCP protocol: `2026-07-28`
- SDK pin: `rmcp = 3.1.0`
- rmcp tag commit: `1f9358eddca42d3a510c70ae6446dd6548c7c856`

## Verdict

The aggregate MCP gateway gaps identified in the original audit are implemented.
Labby now preserves modern request envelopes, derives relay capabilities from the
current request, proxies MRTR across tools, prompts, and resources, aggregates
complete catalogs and completion, routes tasks, consumes upstream subscription
streams, translates request/progress/task identifiers, preserves upstream errors,
and stamps result provenance.

A Labby-native process driver now passes through this real chain:

```text
client -> root Labby -> middle Labby -> synthetic leaf
```

That run also discovered and fixed a multi-hop resource namespace defect. Nested
`lab://upstream/...` resource and template URIs are now opaque and remain
routable through every gateway hop.

## Capability Matrix

| Area | Status | Evidence |
|---|---|---|
| Stateless lifecycle and discovery | Pass | HTTP lifecycle tests; multi-hop discovery |
| Required request metadata | Pass | complete request-envelope tests |
| Standard HTTP request headers | Pass at rmcp boundary | dated suite and HTTP header tests |
| Tools | Pass | pagination, relay, MRTR, multi-hop late-page execution |
| Prompts | Pass | full pagination, relay, late-page multi-hop fetch |
| Resources | Pass | full pagination, relay, nested multi-hop read |
| Resource templates | Pass | full aggregation and nested namespace regression |
| Completion | Pass | upstream ownership and multi-hop template completion |
| Unified subscriptions | Pass | managed upstream listen streams and downstream fanout |
| Resource subscriptions | Pass | acknowledgement limited to deliverable exact URIs |
| Sampling, roots, elicitation | Pass by current-request declaration | request-scoped relay tests |
| Tasks extension | Pass | subject-bound handles, lifecycle routing, status ID translation |
| Progress and cancellation | Pass | relay request/progress mapping and live cancellation tests |
| Bridge capability declaration | Pass | fixed declaration removed; per-request metadata forwarded |
| Cache metadata | Pass | zero TTL/private result tests |
| Error fidelity | Pass | original content, structured content, and metadata preserved |
| Result provenance | Pass | Labby identity plus preserved upstream identity |
| Deprecated protocol logging | Intentional compatibility forwarding only | capability remains unadvertised |

## Resolved Findings

### Complete request envelopes and MRTR

Proxy routes retain `_meta`, `inputResponses`, `requestState`, trace fields, log
level, and custom metadata. Only gateway-owned names and URIs are rewritten.
Relay connection identity includes a capability fingerprint from the current
request. Tool, prompt, and resource requests use single-round forwarding so
`input_required` reaches the downstream client unchanged.

### Complete aggregate primitives

Prompt, resource, and resource-template catalogs traverse every cursor page.
Completion resolves upstream prompt and resource-template ownership. Upstream
tasks receive subject-bound gateway handles tied to the creating connection.

### Subscriptions and notifications

Labby opens and retries upstream `subscriptions/listen` streams. It forwards
tool, prompt, and resource list changes plus exact resource updates. Relay
connections translate and forward progress, cancellation, task status, custom
notifications, resource updates, and list changes.

### Error and result fidelity

Upstream error classification is observational and no longer reconstructs the
result. All content blocks, structured content, and metadata survive. Ordinary
results stamp `io.modelcontextprotocol/serverInfo` for Labby and preserve a
distinct upstream identity under `ai.dinglebear.labby/upstreamServerInfo`.

### Multi-hop resource namespaces

Each gateway adds one outer prefix and removes only that prefix when routing a
read or completion request:

```text
leaf:   fixture://resource/069
middle: lab://upstream/leaf/fixture://resource/069
root:   lab://upstream/middle/lab://upstream/leaf/fixture://resource/069
```

Flattening the middle URI caused `unknown resource` at the middle gateway.
Unit regressions and the process driver now guard this rule for resources and
resource templates.

## Verification Evidence

Completed during implementation:

- full `labby --all-features` compile
- relay module: 15 passed, including live progress, cancellation, and task status
- provenance regression: passed
- upstream error-fidelity regressions: passed
- current-request capability regression: passed
- nested resource URI regression: passed
- nested resource-template URI regression: passed
- Labby-native three-hop process driver: passed
- complete conformance script: passed
- dated server suite: 115 passed, 0 failed
- dated client suite: 377 passed, 0 failed, 0 warnings
- every Tasks server extension scenario: strict baseline passed with no server entries
- client extensions: 17 passed; 11 failures matched four explicit SDK-owned scenarios

The process driver covers modern discovery, late-page tools, tool execution,
MRTR, task create/get/update/cancel with translated status notifications,
progress, root-to-middle-to-leaf cancellation, prompts, resources, templates,
completion, authentication, provenance, and tool/prompt/resource list changes
plus an exact resource update. After the subscription signals, it re-lists all
three catalog families and requires the leaf's dynamically added entries to be
visible through both gateways. It runs in `scripts/ci/mcp-conformance.sh` before
the pinned rmcp fixture suites.

## Remaining Closeout Work

Remaining work is additional validation depth, not missing core proxy
implementation:

1. Add proxy-level JSON Schema 2020-12 and arbitrary structured-content cases.
2. Add configured OAuth issuer/application-type behavior to a Labby-native
   gateway scenario; SDK and authorization-server coverage already exists.

## Definition of Done

Core capability implementation is complete when every advertised capability has
a working producer or route and proxy envelopes remain transparent. That
threshold is met by the implemented code and focused regressions. Consolidated
PR closeout still requires successful hosted CI and merge.
