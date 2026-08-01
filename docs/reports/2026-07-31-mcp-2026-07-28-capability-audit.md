# MCP 2026-07-28 Capability Audit

Date: 2026-07-31

## Anchors

- Worktree: `/home/jmagar/workspace/labby/.worktrees/mcp-2026-07-28-capabilities`
- Branch: `audit/mcp-2026-07-28-capabilities`
- Labby commit: `eff39c79d97c907f6c9956f4711ecab5cd8df62f`
- MCP tag: `2026-07-28`
- MCP spec commit: `5f5440bb26a62e2cf3440b92da5a667efa03b267`
- SDK pin: `rmcp = 3.0.0-beta.2`

## Verdict

Labby's downstream MCP server has real 2026-07-28 support. The HTTP endpoint is stateless, implements `server/discover`, enforces modern request metadata and standard headers, adds required cache metadata, and exposes the main tools, prompts, resources, completion, and subscription primitives.

The remaining work is concentrated in Labby's role as an aggregate MCP client and transparent gateway. Request envelopes are not always preserved, MRTR forwarding is narrow, several upstream primitives are not aggregated, and several notification families are dropped. Labby cannot yet claim full end-to-end support for every 2026-07-28 capability and primitive.

## Verified regressions

The compiled Labby test binary was run directly. All four tests passed:

- `http_mcp_adapts_legacy_initialize_lifecycle`
- `http_mcp_discovers_only_the_new_stateless_protocol`
- `http_mcp_rejects_mismatched_sep_2243_method_header`
- `http_mcp_rejects_missing_sep_2243_name_header_for_tool_call`

Result: 4 passed, 0 failed. Stateless support is not an outstanding gap.

## Capability matrix

| Area | Status | Notes |
| --- | --- | --- |
| Stateless lifecycle and discovery | Pass | Modern discovery, no downstream transport sessions, legacy adapter retained |
| Required request metadata | Pass server-side | Proxy fidelity is partial |
| Standard HTTP request headers | Mostly pass | Method/name covered; full parameter-header E2E coverage is missing |
| Tools | Partial | List/call work, but MRTR retries lose fields |
| Prompts | Partial | Aggregated, but pagination and MRTR relay are incomplete |
| Resources | Partial | Aggregated, but pagination, MRTR, and update subscriptions are incomplete |
| Resource templates | Missing aggregate support | Server returns an empty aggregate list |
| Completion | Missing aggregate support | Local completion only |
| Unified subscriptions | Partial | Downstream listen exists; upstream listen is not consumed |
| Sampling, roots, elicitation | Partial | Relayed only on selected tool calls |
| Tasks extension | Incomplete | Bridge methods exist, aggregate lifecycle does not |
| Progress and notifications | Incomplete | Several upstream notifications are no-ops |
| Cache metadata | Pass | Required list/read results use zero TTL and private scope |
| JSON Schema 2020-12 | Pass, SDK-backed | Add Labby proxy conformance tests |
| OAuth July changes | Pass, SDK-backed | Add Labby integration tests |
| Protocol logging | Intentionally omitted | Deprecated capability is not advertised |

## Confirmed support

### Stateless server

- `crates/labby/src/cli/serve.rs` uses `NeverSessionManager`.
- Modern legacy-session mode is disabled.
- `LabMcpServer` advertises only `ProtocolVersion::V_2026_07_28`.
- `LabMcpServer::discover` implements modern discovery.
- Direct endpoint regression tests passed.

### Core server primitives

`LabMcpServer` implements discovery, tools list/call, prompts list/get, resources list/read, resource-template listing, completion, and subscriptions/listen. Some handlers are only locally complete. Resource templates and completion do not include upstream behavior.

### Cacheable results

Required `ttlMs` and `cacheScope` fields are present for tools, prompts, resources, resource templates, and resource reads. The current zero-TTL private policy is conservative and valid for an authenticated dynamic gateway.

### Upstream lifecycle compatibility

The client attempts modern discovery first and can retry using legacy initialize behavior. See:

- `crates/labby-gateway/src/upstream/pool/lifecycle_compat.rs`
- `crates/labby-gateway/src/upstream/pool/connect.rs`
- `crates/labby-gateway/src/upstream/pool/connect_stdio.rs`

### OAuth

The pinned SDK supports RFC 9207 issuer validation, issuer-bound registration persistence, clearing credentials when an issuer changes, and Dynamic Client Registration `application_type` with a native default. Labby's authorization server emits the response issuer and advertises support.

## Prioritized gaps

### P0: Upstream MRTR retries lose required fields

`crates/labby/src/mcp/call_tool.rs` passes only cloned arguments into the upstream tail. `crates/labby/src/mcp/call_tool_upstream.rs` reconstructs `CallToolRequestParams` from the tool name and arguments.

Lost fields:

- `_meta`
- `inputResponses`
- `requestState`

An upstream may return `input_required`, but when the downstream client retries, Labby discards the answer and state before calling the upstream. Pass the complete request object through the proxy and rewrite only gateway-owned identifiers.

Required test: an upstream returns `input_required` with request state; the downstream retries through Labby; the upstream receives input responses and identical request state; the final result reaches the caller.

### P0: Relay decisions use connection history instead of current request capabilities

`downstream_supports_relay` and `RelayClientHandler::get_info` derive capabilities from the peer's stored discovery information. The 2026 protocol carries client capabilities on each request.

This can miss a supported interaction or advertise capabilities not present on the current request. Build a request-scoped forwarding context from incoming metadata. Relay cache identity must include a capability fingerprint, or connections must be recreated when capabilities differ.

### P1: MRTR relay covers tools only

Prompt and resource requests use pooled connections with the default unit client handler.

- `pool/prompts_get.rs` calls `peer.get_prompt`.
- `pool/resources_read.rs` creates a URI-only request and calls `peer.read_resource`.

Prompt interaction is internally declined or consumed. Resource retries also lose metadata, input responses, and request state. Add single-round relay paths for tool call, prompt get, and resource read so incomplete results are returned to the downstream client.

### P1: Tasks are not exposed through the aggregate endpoint

Task methods exist in `crates/labby/src/mcp/bridge.rs`, but the aggregate `LabMcpServer` does not advertise or route task get, update, and cancel operations. Task status notifications are not forwarded.

Add subject-scoped task ownership and route lifecycle requests back to the server that created each task. Advertise the extension only when the full lifecycle works.

### P1: Upstream subscriptions/listen is not consumed

No managed upstream listen stream was found. Normal upstream connections use the unit client handler, whose callbacks are no-ops.

Lost events include tools/prompts/resources list changes, resource updates, task status, and subscription acknowledgements. Open a managed listen subscription per upstream and feed events into catalog invalidation, downstream subscriptions, resource updates, and task routing.

### P1: Unsupported resource subscriptions are acknowledged

`LabMcpServer::accepted_subscription_filter` returns the requested filter unchanged, but no aggregate `ResourceUpdatedNotification` producer was found.

Until delivery exists, remove `resourceSubscriptions` from the accepted filter. Later, acknowledge only URIs whose owner supports updates.

### P1: Resource templates are not aggregated

`LabMcpServer::list_resource_templates` returns an empty list. Collect every page of upstream templates, apply route and subject scope, preserve metadata, and return deterministic ordering.

### P1: Upstream completion is not proxied

`crates/labby/src/mcp/completion.rs` supports only local Labby prompt arguments. Add ownership resolution and proxy completion for upstream prompt and resource-template references.

### P1: Prompt and resource catalogs stop after one page

Tools use `list_all_tools`, while prompt and resource paths commonly call `list_prompts(None)` or `list_resources(None)` once. Large catalogs are silently truncated and ownership resolution can fail for later pages.

Use full traversal helpers or a shared bounded cursor loop for prompts, resources, and templates.

### P1: Progress, cancellation, and notifications are not transparently forwarded

`RelayClientHandler` implements sampling, roots, and elicitation requests but not notification callbacks. `BridgeClientHandler` advertises tasks and elicitation but also lacks notification forwarding.

Progress, cancellation, resource updates, list changes, task status, and protocol log messages can disappear at the gateway boundary. Implement explicit forwarding and preserve originating request and progress-token relationships.

### P1: Bridge capabilities are static

`BridgeClientHandler` always advertises tasks and elicitation and does not mirror the actual caller's sampling or roots capabilities. Replace fixed `ClientInfo` with request-scoped information or multiplex bridge connections by capability fingerprint.

### P2: Custom request metadata is lost

Tool and resource proxy paths reconstruct request objects. This drops trace context, request log-level metadata, and extension metadata.

Define a policy:

1. Regenerate required upstream-facing client context.
2. Preserve safe metadata unchanged.
3. Remove or translate gateway-private metadata.

Test `traceparent`, `tracestate`, `baggage`, request log level, and a custom extension key.

### P2: Upstream error results are rewritten

`crates/labby/src/mcp/upstream.rs` rewrites many upstream tool errors into Labby's envelope. This can discard additional content, structured content, or metadata. For transparent routes, preserve the full result and attach Labby diagnostics in metadata.

### P2: Result-level server identity metadata is absent

No general injection of `io.modelcontextprotocol/serverInfo` was found. This is recommended rather than required, but it would improve provenance for aggregate results.

## Intentional exclusion: deprecated logging

`crates/labby/src/mcp/logging.rs` intentionally does not advertise or emit protocol logging notifications. This is reasonable for a new 2026 implementation because logging is deprecated. Document it as intentional rather than an accidental gap.

## Conformance blind spot

`scripts/ci/mcp-conformance.sh` primarily scores the pinned `rmcp` fixture client and server. Labby's actual endpoint receives only a small legacy-shaped tools/list smoke request. A green fixture run proves the SDK pin, not Labby's aggregate proxy behavior.

Add Labby-native end-to-end scenarios for:

- modern discovery, metadata, headers, and cache fields
- two-round tool, prompt, and resource MRTR
- request-state and input-response preservation
- per-request capability changes
- sampling, roots, and elicitation delegation
- tasks lifecycle and status notifications
- upstream and downstream subscriptions
- resource update acknowledgement and delivery
- multi-page catalogs and resource templates
- upstream completion
- progress, cancellation, and trace forwarding
- OAuth issuer/application-type behavior through Labby configuration
- JSON Schema 2020-12 and arbitrary structured content through the proxy

## Documentation defect

`docs/surfaces/MCP_CONFORMANCE.md` says legacy initialize is rejected and references `http_mcp_rejects_legacy_initialize_lifecycle`. Current code adapts legacy initialize, and the test is named `http_mcp_adapts_legacy_initialize_lifecycle`.

Update the document to distinguish modern discovery, which advertises only 2026-07-28, from compatibility adaptation of legacy initialize traffic.

## Recommended sequence

1. Preserve complete request envelopes and derive capabilities per request.
2. Generalize single-round MRTR relay to tools, prompts, and resources.
3. Add task routing, resource templates, completion, and full pagination.
4. Consume upstream listen streams and forward all supported notifications.
5. Replace the Labby smoke check with modern end-to-end conformance and multi-hop gateway fixtures.

## Definition of done

Labby can claim full support when every advertised capability works, every acknowledged subscription has an event producer, proxy routes preserve request and response envelopes, all MRTR-capable primitives complete multiple rounds, tasks remain manageable through the creating endpoint, catalogs are complete across pages, notification forwarding has an explicit policy, capabilities come from the current request, and the actual Labby endpoint passes modern multi-hop conformance.

## Final assessment

Stateless support is complete enough to remove from the gap list. Labby's core downstream server is substantially aligned with the final 2026-07-28 protocol. The aggregate client and gateway still need focused request-fidelity, interactive relay, primitive aggregation, and live-subscription work before Labby can accurately claim full support for all MCP capabilities and primitives.
