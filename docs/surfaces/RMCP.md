---
title: "RMCP"
created: "2026-07-30"
updated: "2026-07-30"
---

# RMCP

This document defines the integration contract between `lab` and the official Rust Model Context Protocol SDK, `rmcp`.

It is the source of truth for:

- which `rmcp` features `lab` relies on
- how `lab` structures its MCP server around `rmcp`
- which RMCP capabilities `lab` exposes
- where auth, transport, schema, and handler ownership live
- what patterns are allowed or disallowed when adding RMCP-related code

This document is normative for future work even where the codebase is still converging.

The reproducible production rmcp Git revision, pinned upstream rmcp `3.1.0`
conformance fixture, and MCP `2026-07-28` validation matrix are
documented in [MCP_CONFORMANCE.md](./MCP_CONFORMANCE.md).

## Scope

`lab` has one MCP server product surface.

That surface may be exposed through multiple transports and may filter which services are visible at runtime, but it is still one server with one shared catalog, one shared dispatch model, and one shared contract.

Rules:

- do not model `lab` as many unrelated mini-servers
- do not let stdio and hosted Streamable HTTP drift in behavior
- do not let transport choice change catalog, schemas, envelopes, or destructive-op policy
- do not let RMCP-specific code become a second business-logic layer

## Server Shape

`lab` uses `rmcp` as a transport and protocol adapter over the shared dispatch/catalog layer.

The intended shape is:

1. shared dispatch/catalog ownership remains in `lab`
2. a reusable MCP server module adapts that shared layer into `rmcp::ServerHandler`
3. stdio, HTTP/TCP, and HTTP/Unix transports wrap the same server core
4. CLI, MCP, and HTTP API continue to share the same service/action model

Rules:

- business logic stays out of RMCP handlers
- `rmcp` handlers call shared dispatch code; they do not reimplement service operations
- the same runtime registry determines what tools, prompts, and resources are exposed
- HTTP MCP and the existing Axum API must be able to coexist in one process

## Required RMCP Posture

`lab` is a server-first RMCP application.

The baseline posture is:

| RMCP feature | Posture | Notes |
|---|---|---|
| `server` | required | primary MCP server functionality |
| `macros` | required | handler/router glue only |
| `transport-io` | required | stdio transport |
| `transport-streamable-http-server` | required | HTTP MCP transport |
| `elicitation` | required | destructive confirmation and future interactive flows |
| `schemars` | required in practice | all RMCP-facing input types should have schema support |
| `client` | required in practice | `lab` must be able to act as an outbound MCP client and gateway |
| `auth` | required | needed for outbound OAuth MCP clients and protected HTTP MCP deployments |
| `transport-streamable-http-client` | required in practice | required for outbound MCP gateway/client work over HTTP |
| per-request logging | not advertised | 2026-07-28 removes `logging/setLevel`; Labby relies on server-side tracing |

Notes:

- the `server` feature already pulls in `schemars` support upstream, but `lab` should still treat JSON Schema generation as a hard requirement for RMCP-facing types
- `lab` deliberately uses both server and client RMCP surfaces, so outbound features are part of the intended product shape rather than speculative extras
- server-side OAuth for `lab`'s HTTP MCP endpoint is still enforced by Axum middleware and surrounding HTTP infrastructure even when RMCP `auth` support is enabled

## Transport Contract

`lab` supports three MCP transports:

- `stdio`
- streamable HTTP over TCP
- streamable HTTP over a Unix-domain socket

Rules:

- all transports expose the same MCP server behavior
- `labby serve` defaults to HTTP/TCP unless CLI, env, or config selects stdio or `unix_socket`
- stdio remains available for explicit local child-process sessions via `labby mcp`
- hosted Streamable HTTP is mounted inside the Axum application under a dedicated MCP path such as `/mcp`
- the Axum API continues to own non-MCP routes such as `/health`, `/ready`, and `/v1/...` for both TCP and Unix listeners
- transport configuration must not fork the catalog or discovery model
- Unix listeners may use bearer/OAuth or Linux kernel peer credentials; client-supplied identity headers never substitute for peer credentials

Hosted Streamable HTTP-specific rules:

- use RMCP's streamable HTTP server transport, not a custom protocol
- mount it as a Tower service inside Axum
- use the 2026-07-28 stateless lifecycle; do not issue or require
  `Mcp-Session-Id`
- preserve the RC request metadata and HTTP method headers through Axum
- respect RMCP host-header protections unless there is an explicit deployment override

Axum route-boundary rules:

- `/mcp` is protected by bearer/OAuth token auth when server auth is configured; it does not accept browser sessions.
- `/v1/*` is protected by bearer/OAuth token auth and, where allowed by the API router, browser sessions. Static web UI auth-disable settings must not bypass `/v1` auth when server auth exists.
- Static web asset serving is an SPA fallback, not an API auth policy. Browser UI auth-disable mode only affects browser-session endpoints and UI behavior.
- `/dev/*` routes are development preview routes mounted before the SPA fallback. They share the same auth middleware whenever server auth is configured; no-auth `/dev/*` is only for explicit local development with no bearer/OAuth auth configured.

## Capability Contract

`lab` intends to expose these RMCP capability families:

- tools
- prompts
- resources
- elicitation

Other capabilities may be added later, but these are the baseline contract.

Rules:

- tools remain the primary operation surface
- prompts are for guided usage, workflows, summaries, and agent-facing composition — not for leaking secrets or replacing structured tool outputs
- resources are for stable discovery and read-oriented contextual data, not for mirroring every tool call as a fake resource
- elicitation is used when the server must explicitly ask the client for confirmation or input

`lab` does not change its one-tool-per-service design because RMCP can support finer-grained tools.

Rules:

- keep one MCP tool per service plus the top-level `lab` meta-tool
- continue to dispatch service operations through `action` + `params`
- do not explode the tool list into one tool per endpoint or one tool per action
- prompts and resources may be richer than tools, but they must still derive from the same shared catalog and dispatch ownership model

Prompt and resource operations must carry the same observability posture as tool
dispatch:

- structured dispatch logs for list/read/get operations
- stable error-kind handling where applicable
- request correlation preserved through the surrounding caller context

## Handler and Macro Contract

`lab` uses explicit `ServerHandler` implementations.

Rules:

- prefer explicit `impl ServerHandler for ...`
- use `#[tool_handler]`, `#[prompt_handler]`, and `#[task_handler]` as glue on that explicit handler impl when those capabilities are present
- do not rely on `#[tool_router(server_handler)]` for the main `lab` server
- reserve `#[tool_router(server_handler)]` for tools-only examples or very small standalone servers

Rationale:

- `lab` is one mixed-capability server, not a tools-only demo
- `lab` needs control over `get_info()`, capability composition, metadata, and shared ownership boundaries
- explicit handler impls make it easier to keep prompts, resources, and future capabilities aligned

Allowed RMCP macro usage:

- `#[tool]` on individual tool handlers
- `#[tool_router]` to build tool routers
- `#[prompt]` on individual prompt handlers
- `#[prompt_router]` to build prompt routers
- `#[tool_handler]`, `#[prompt_handler]`, `#[task_handler]` on explicit `impl ServerHandler`

Disallowed patterns:

- hiding the main server shape behind tools-only shortcut macros
- duplicating handler logic between stdio, TCP, and Unix adapters
- placing business logic directly inside macro-heavy handler bodies when it should live in shared dispatch code

## Schema Contract

RMCP-facing definitions must be schema-first.

Rules:

- all RMCP-facing parameter structs should derive `JsonSchema`
- prefer typed parameter structs over ad-hoc `serde_json::Value` where practical
- use explicit schema overrides only when the inferred schema is not the contract you want
- keep input names, descriptions, and optionality accurate because they shape agent behavior
- do not let tool schemas drift from the shared action catalog

Guidance:

- typed schemas are mandatory for prompts and tools intended for agent consumption
- resource payloads do not need to pretend to be tool schemas, but resource descriptors should still be stable and predictable

## Auth Contract

Auth ownership is split deliberately.

### Protecting `lab` as an MCP server

This is owned by `lab`'s HTTP surface and middleware stack, with RMCP auth support used where needed for protocol-compatible protected-resource behavior.

Rules:

- HTTP MCP auth happens before requests reach the RMCP Tower service
- bearer token validation, OAuth enforcement, and request scoping belong in Axum middleware and app state
- RMCP `auth` support is part of the server contract when `lab` exposes OAuth-protected HTTP MCP, but it does not replace the surrounding HTTP auth boundary
- RMCP handlers may read authenticated request context from injected HTTP request parts or extensions, but they do not own auth enforcement

### Using `lab` as an MCP client

This is also a required part of `lab`'s target architecture.

Rules:

- outbound RMCP client and auth support are first-class capabilities because `lab` must connect to and proxy other MCP servers
- if outbound MCP clients are added, their credentials and token lifecycle must still follow `lab`'s own config and secret-handling rules

## Logging Contract

Labby does not advertise the removed legacy logging capability and does not
accept `logging/setLevel`. Local structured tracing remains required and uses
the same action/service/upstream context and redaction rules.

Current implementation notes:

- prompt/resource/tool read paths do not recompute catalog snapshots or fan out
  list-changed notifications on every request
- gateway-driven upstream catalog changes are propagated through the dedicated
  peer notifier instead of piggybacking on ordinary request traffic

## Resource and Prompt Ownership

Prompts and resources must stay aligned with the rest of `lab`'s model.

Rules:

- prompt/resource definitions belong to the `labby` product crate, not `labby-apis`
- prompt/resource content may call into shared dispatch and catalog layers, but must not duplicate service behavior definitions by hand
- prompts must not bypass structured tools for mutating operations
- resources should be generated from canonical catalog or service-owned read models where possible
- prompts and resources must obey the same redaction and secret-handling rules as every other surface

## Elicitation Contract

MRTR elicitation is a required part of the RMCP posture.

Rules:

- destructive operations continue to derive confirmation behavior from shared action metadata
- return `resultType: "input_required"` and retry with `inputResponses`; do
  not send an in-flight `elicitation/create` request
- RMCP elicitation must not become a second confirmation policy independent of `ActionSpec.destructive`
- when elicitation is unavailable, fallback behavior must remain explicit and consistent with the owning surface contract
- elicitation prompts must be short, structured, and safe to parse by both humans and agents

## HTTP Mounting Contract

The preferred HTTP MCP integration model is:

1. build the reusable `lab` RMCP server core
2. create RMCP's streamable HTTP Tower service from that core
3. mount it under Axum with `nest_service("/mcp", ...)`
4. place auth, request ID, tracing, timeout, and CORS in the surrounding Axum/Tower stack

Rules:

- do not fork a separate HTTP-only MCP server binary unless there is a compelling operational need
- do not special-case HTTP MCP discovery or envelopes relative to stdio
- HTTP middleware may enrich request context, but RMCP remains the MCP protocol owner

## Relationship to `rmcp-openapi`

[`rmcp-openapi`](https://gitlab.com/lx-industries/rmcp-openapi) is useful as a reference project, not as `lab`'s architectural template.

What is useful:

- OpenAPI-to-MCP schema generation ideas
- output-schema handling patterns
- auth passthrough and proxy-boundary thinking
- response transformation ideas for LLM-facing payload shaping

What does not fit `lab`:

- one-tool-per-endpoint generation
- treating the MCP layer as a generic REST proxy
- expanding the tool surface to mirror every upstream API operation directly

`lab` already has a typed client layer and a one-tool-per-service product model.

Rules:

- borrow ideas from `rmcp-openapi`, not its product shape
- do not replace `lab`'s typed service clients with OpenAPI-generated MCP proxy behavior
- do not let OpenAPI-style tool explosion undo the compact `lab` catalog

## Review Checklist

Any RMCP-related PR should be reviewable against this checklist:

- Are stdio, HTTP/TCP, and HTTP/Unix MCP using the same server core?
- Does the change preserve one-tool-per-service?
- Are prompts/resources/elicitation aligned with shared catalog and action metadata?
- Is auth owned in the correct layer?
- Are schemas typed and intentional?
- Is the main server using explicit handler impls rather than tools-only shortcuts?
- Does the change avoid putting business logic into RMCP adapters?
- Does the change preserve both protected-server and outbound-client RMCP auth requirements?
