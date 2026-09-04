---
title: "Core provider and delegated actor protocol"
created: "2026-09-04"
updated: "2026-09-04"
status: implemented-unbundled
version: 1
---

# Core provider and delegated actor protocol, version 1

This private HTTP-over-UDS protocol lets Labby Code Mode use a bounded Core
provider. It is not MCP, a public HTTP surface, a GraphQL passthrough, or a
second Code Mode implementation. Labby remains the Code Mode and sandbox
authority; Core remains the provider catalog, execution, and actor authority.

The canonical golden fixture is maintained alongside Core as
`priv/contracts/core_provider_protocol/v1.json`. Before either implementation
ships, this repository must carry the same fixture bytes and the conformance
suite must compare both copies before running every listed case.

## Mandatory trust boundary

Labby accepts this protocol only over the integrated Unix socket and verifies
the Core-issued delegated assertion only after `SO_PEERCRED` accepts the local
peer. It rejects TCP, proxy/forwarded identity headers, assertion-controlled
key URLs, non-EdDSA signatures, unknown critical headers, wrong fixed `typ`,
issuer, audience, authority generation, expiry, or replayed `jti`.

The actor assertion is profile `1.0`: issuer `unraid-core`, audience
`labby-trusted-host`, type `unraid+delegated-actor+jwt`, TTL at most 60 seconds,
skew at most five seconds, and only the current plus previous Core signing key.
Its bounded claims preserve the original subject and Core actor, client,
request/correlation ID, authority generation and nonce. It cannot authenticate
an external MCP, browser, public HTTP, or upstream provider request.

`scopes` is an authority-narrowing claim, never an authority grant. It contains
at most 64 identifiers, each at most 256 bytes and free of control characters;
Labby replaces the Unix-peer bootstrap scopes with this exact bounded set.

## Provider contract

Protocol `1.0` has exactly `health`, `schema_version`, `search`, `describe`,
`execute`, and `cancel`. JSON uses strict duplicate-key rejection; unknown
critical fields, ambiguous identifiers, malformed Unicode, oversized headers or
bodies, and slow framing are rejected before dispatch. `search` cursors and
retained result handles are opaque and actor-bound.

`execute` takes a per-tool request ID, an approved stable operation ID, typed
variables, and the expected Core schema version—never GraphQL or a resolver
name. The per-tool ID is distinct from the parent correlation ID in the
delegated assertion, so parallel calls from one Code Mode run remain
independently cancellable and do not collide in admission. Search/describe are
discovery only; Core repeats authorization, safety metadata, and approval
checks for execute. Its closed outcome union is `complete`, `partial`,
`denied`, `approval_required`, `schema_stale`, `cancelled_before_attempt`,
`cancelled_after_attempt`, `outcome_unknown`, and `failed`. Transport loss after
dispatch is `outcome_unknown`, not a retry signal.

Schema versions are bounded `sha256:` identifiers. Every operation carries
explicit boolean `read_only` and `destructive` policy fields; Labby rejects a
query unless it is read-only and non-destructive, and preserves a mutation's
explicit destructive classification instead of inferring it from mutation
kind alone.

The fixture fixes 1 MiB request/search/describe responses, 2 MiB execute
inline/chunks, 32 KiB/100 headers, 64 connections, 16 concurrent executes, 100
ms admission, 4 KiB search, page default 20/max 50, and 64 KiB descriptions.
Current and legacy MCP profiles remain separate Labby ingress concerns.
