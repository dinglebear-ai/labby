---
title: "RMCP Integration"
updated: "2026-08-18"
---

# RMCP Integration

Labby uses the official Rust Model Context Protocol SDK, `rmcp`, as its MCP protocol and transport implementation. The workspace pins `rmcp = 3.1.0`; the version/conformance adoption contract is documented in [MCP_CONFORMANCE.md](./MCP_CONFORMANCE.md).

This document describes the **current** integration boundary. The product behavior itself remains owned by shared Labby dispatch/runtime layers rather than by RMCP adapters.

## Dependency Posture

The workspace enables these RMCP features:

- `server`
- `transport-io`
- `transport-child-process`
- `transport-streamable-http-server`
- `client`
- `auth`
- `transport-streamable-http-client-reqwest`
- `elicitation`
- `schemars`

That is intentional: Labby is both an MCP server for downstream clients and an MCP client/gateway for upstream servers.

## Server Shape

Labby has one product MCP server shape backed by the same service registry and action dispatch used by the other product surfaces.

Current entrypoints are:

- `labby mcp` for local stdio
- `labby serve` with `/mcp` for hosted Streamable HTTP
- the same hosted router over a Unix-domain socket when `transport = "unix_socket"`
- gateway-managed protected MCP routes for selected upstreams

Transport choice must not create a second service catalog or business-logic layer. See [TRANSPORT.md](./TRANSPORT.md).

## Advertised Capabilities

`LabMcpServer::get_info()` currently advertises:

- tools + tool list-changed notifications
- resources + resource list-changed notifications
- prompts + prompt list-changed notifications
- completions
- Labby MCP extensions

When a gateway manager is configured, Labby additionally advertises:

- modern resource subscriptions
- tasks

Legacy `initialize` is an edge-compatibility adapter. For legacy sessions Labby withholds resource subscription capability because the deprecated `resources/subscribe` handler is not implemented; modern sessions use the 2026-07-28 subscription model. The full lifecycle and regression matrix lives in [MCP_CONFORMANCE.md](./MCP_CONFORMANCE.md).

Labby does **not** advertise the removed legacy logging capability or accept `logging/setLevel`; local structured tracing is the observability contract.

## Product Tools Versus Upstream Tools

Built-in Labby services use one MCP tool per registered service with the shared `action` + `params` shape. The generated [service catalog](../generated/service-catalog.md) and [MCP help](../generated/mcp-help.md) are authoritative.

Upstream MCP tools are different: the gateway preserves the upstream tool's name, schema, metadata, and normal MCP argument payload. Do not wrap arbitrary upstream tools in Labby's built-in service `action` shape.

Code Mode provides an additional bounded execution projection over the live upstream catalog; it does not replace normal MCP tool discovery or change the underlying upstream schemas.

## Handler Ownership

RMCP handlers own protocol adaptation only:

- MCP request/notification parsing
- MCP response/result construction
- capability negotiation
- protocol metadata and request context
- MCP resources/prompts/completions/tasks/subscriptions
- cancellation/progress/MRTR relay behavior

Shared operation semantics, authorization/destructive metadata, gateway connection behavior, retries, persistence, and product error contracts belong below the RMCP adapter in product dispatch or the extracted runtime crate that owns them.

Do not create stdio-, HTTP-, or Unix-specific copies of MCP operation logic.

## Schemas And Output

Agent-facing schemas are contract surfaces. Keep tool parameters, descriptions, optionality, output schemas, structured content, and annotations aligned with the shared catalog and the actual implementation.

Labby's normative output/error behavior is documented in:

- [MCP.md](./MCP.md)
- [../contracts/mcp-tool-output.md](../contracts/mcp-tool-output.md)
- [../contracts/agent-error-contract.md](../contracts/agent-error-contract.md)
- [../dev/ERRORS.md](../dev/ERRORS.md)

Do not stringify structured errors or discard upstream structured content merely because the RMCP boundary can represent plain text.

## Auth Boundary

Inbound MCP authentication is enforced by Labby's surrounding HTTP/Axum and `labby-auth` layers before protected requests reach the RMCP service. RMCP request context may carry the authenticated caller, but RMCP handlers do not own the authorization server or browser session policy.

Outbound MCP OAuth/token lifecycle belongs to the gateway/auth runtime. Credentials must remain target-scoped and follow the same redaction/secret-handling rules as the rest of Labby.

The scope strings `lab:read`, `lab`, and `lab:admin` are intentional public authorization contracts. They are not remnants of the historical product-name rename.

## Lifecycle, Relay, And Subscriptions

The modern hosted contract is stateless MCP 2026-07-28 discovery. Labby uses RMCP's stateless Streamable HTTP lifecycle and does not require `Mcp-Session-Id` for the hosted endpoint.

Gateway relay code preserves MCP request metadata, MRTR intermediate results, task handles/status notifications, progress, cancellation, provenance, and upstream error fidelity. Resource/tool/prompt list changes and modern resource subscriptions propagate through the dedicated gateway notification/subscription paths rather than being synthesized by ordinary request traffic.

Never replace the bounded Labby relay/subscription machinery with RMCP convenience helpers that enumerate unbounded pages. Workspace Clippy policy intentionally bans `Peer::list_all_*` calls.

## Elicitation

Destructive built-in actions derive confirmation from shared `ActionSpec.destructive` metadata. Labby represents required interactive input through MCP's MRTR/input-response model; RMCP elicitation must not become an independent destructive-policy table.

## Review Checklist

For RMCP-facing changes verify that:

- all product transports still use the same MCP server semantics
- service/action schemas remain aligned with generated catalogs
- upstream tools preserve their own MCP contract
- auth and destructive policy stay in their owning shared layers
- modern lifecycle/tasks/subscriptions/cancellation/progress behavior still matches the conformance matrix
- no unbounded upstream listing helper is introduced
- focused protocol tests plus the MCP conformance gates cover the changed behavior
