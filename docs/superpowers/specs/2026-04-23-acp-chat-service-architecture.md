# ACP Chat Service Architecture

Status: Draft
Scope: ACP-backed chat as a first-class product-local service inside `lab`

## Purpose

This document defines how ACP chat must live inside the repository as a proper `lab` service rather than as ad hoc product code.

It exists to prevent architectural drift in three areas:

1. backend ownership of ACP session/runtime behavior
2. correct use of `lab-apis`, `dispatch`, and thin product adapters
3. avoiding a React- or route-local ACP implementation that bypasses repo conventions

## Current architectural decision

ACP chat is a product-local service.

That means:
- ACP runtime and session ownership live in the Rust backend
- the exported admin app consumes backend endpoints
- product surfaces remain thin adapters over shared service logic

It does not mean:
- React owns ACP process management
- API routes own business logic
- MCP or API become the shared backend for other surfaces

## Service classification

ACP chat is not an upstream SaaS SDK service in the same sense as Radarr or Unraid.

It is a product-local orchestration service that still must follow the same layering contract:

- backend runtime behavior below product adapters
- shared execution path
- thin API, MCP, and CLI surfaces

## Layer model

The required stack for ACP chat is:

1. `lab-apis`
2. `crates/lab/src/dispatch/acp`
3. `crates/lab/src/api/services/acp.rs`
4. `crates/lab/src/mcp/services/acp.rs` when MCP exposure is desired
5. `crates/lab/src/cli/acp.rs` when CLI control is desired
6. `apps/gateway-admin` as the frontend consumer

## Ownership rules

### `lab-apis`

`lab-apis` owns reusable ACP-facing primitives.

That includes:
- ACP transport helpers if they are reusable
- provider launch config types
- ACP wire/domain types that are not tied to one product surface
- reusable session client abstractions if they make sense outside the binary

It does not own:
- axum routing
- SSE endpoint shaping
- chat-specific presentation models
- UI event derivation

Important nuance:

ACP subprocess lifecycle may stay in `lab` if it is tightly coupled to binary runtime concerns and not meaningfully reusable as a standalone SDK.

Do not force complexity into `lab-apis` just to satisfy symmetry.

The rule is reuse-driven, not aesthetic.

### `dispatch/acp`

`dispatch/acp` is the shared semantic layer for ACP chat operations.

It owns:
- operation catalog
- param metadata and validation
- provider selection and provider capability resolution
- session creation, prompting, cancellation, listing, loading, closing, resuming
- session/event normalization into surface-neutral results
- client/runtime resolution
- destructive metadata when applicable
- `Result<Value, ToolError>` behavior for shared operations

It does not own:
- axum extractors
- HTTP status code mapping
- SSE framing details at the route layer
- React-ready UI modeling

### API layer

`crates/lab/src/api/services/acp.rs` must be a thin adapter.

It owns:
- routes
- request extraction
- status mapping
- response shaping
- SSE transport framing

It does not own:
- provider selection semantics
- operation validation rules
- session execution logic
- direct environment lookup

### MCP layer

If ACP is exposed through MCP, the MCP service must also be thin.

It owns:
- tool registration
- MCP envelope shaping
- protocol `help` / `schema` projection

It must not own:
- duplicated ACP operation logic

### CLI layer

If ACP gets a CLI surface, it must be typed and thin.

It owns:
- human command UX
- flags
- output formatting

It must not own:
- duplicated session/runtime semantics

### Frontend

The frontend owns:
- transcript rendering
- reasoning rendering
- action-flow rendering
- user input UX
- session list UX

It must not own:
- ACP subprocesses
- provider auth/env resolution
- protocol lifecycle semantics

## Required file layout

If ACP is treated as a first-class service, the backend shape should converge on:

- `crates/lab/src/dispatch/acp.rs`
- `crates/lab/src/dispatch/acp/catalog.rs`
- `crates/lab/src/dispatch/acp/client.rs`
- `crates/lab/src/dispatch/acp/params.rs`
- `crates/lab/src/dispatch/acp/dispatch.rs`

Optional domain modules:
- `providers.rs`
- `sessions.rs`
- `events.rs`
- `permissions.rs`

Rules:
- `acp.rs` is an entrypoint only
- `catalog.rs` is the source of truth for action metadata
- `client.rs` owns runtime/provider/env resolution
- `params.rs` owns coercion and request-body helpers
- `dispatch.rs` owns top-level action routing and help/schema payloads

## Operation surface

The canonical ACP operation catalog should live in `dispatch/acp/catalog.rs`.

Minimum operations:
- `provider.health`
- `session.list`
- `session.new`
- `session.load`
- `session.prompt`
- `session.cancel`
- `session.close`
- `session.resume` when supported
- `session.fork` when supported
- `session.events` for stream/replay access

Optional:
- `session.permissions.respond`
- `session.mode.set`
- `session.config.set`

The exact API route shapes may differ, but the semantic contract must come from the shared catalog.

## Provider abstraction

Provider support must be implemented below the API layer.

The backend must not be codex-specific at the route layer.

Provider abstraction should own:
- launch command/args
- auth/env injection
- capability reporting
- supported lifecycle methods
- provider-specific event quirks

Provider selection must be explicit in shared ACP service logic, not hidden in frontend defaults.

## Event contract

The shared backend contract must preserve ACP richness before any UI-specific projection.

The backend service should preserve:
- message chunks
- thought chunks
- tool calls
- tool call updates
- plans
- permission requests and outcomes
- available commands
- current mode
- config option updates
- session info updates
- usage updates
- prompt stop reason
- structured `ContentBlock[]`

Do not flatten non-text `ContentBlock` values before the shared service contract unless the flattening is a deliberate consumer-specific projection.

## API contract

The API should expose thin HTTP endpoints over the shared ACP service.

Likely routes:
- `GET /v1/acp/provider`
- `GET /v1/acp/sessions`
- `POST /v1/acp/sessions`
- `POST /v1/acp/sessions/{session_id}/prompt`
- `POST /v1/acp/sessions/{session_id}/cancel`
- `POST /v1/acp/sessions/{session_id}/close`
- `GET /v1/acp/sessions/{session_id}/events`

SSE framing belongs in the API surface, but event ownership belongs below it.

## Frontend contract

The frontend should consume backend-owned ACP event models and derive product presentation from them.

It may maintain UI-local derivations like:
- assistant turns
- reasoning blocks
- action-flow groups
- inline artifacts

It must not become the canonical owner of ACP semantics.

## What is currently missing from the existing specs

The current UI spec correctly covers transcript-first behavior and action flow.

It does not fully define:
- how ACP becomes a proper first-class `lab` service
- where provider/runtime ownership belongs relative to `dispatch`
- when code belongs in `lab-apis` versus `lab`
- the required `dispatch/acp` file layout
- the canonical shared operation catalog for ACP

The older bridge spec is also stale because it still describes:
- a Next-integrated bridge
- a separate activity lane

Those are no longer the target architecture.

## Required follow-up

Before further ACP backend expansion, the implementation should be brought into alignment with this service architecture:

1. move ACP semantics behind `dispatch/acp`
2. keep API handlers thin
3. genericize provider handling
4. preserve `ContentBlock[]`, `usage_update`, and session lifecycle richness in the shared backend contract
5. keep frontend as a consumer of backend ACP semantics, not the owner of them
