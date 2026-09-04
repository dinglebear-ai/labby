---
title: "HTTP API Surface"
created: "2026-08-27"
updated: "2026-08-31"
---

# HTTP API

The generated route inventory is in [`../generated/api-routes.md`](../generated/api-routes.md). This document records hand-authored transport contracts that are not evident from the route list.

## Palette capability routes

Authenticated callers browse compact entries through `GET /v1/palette/catalog` or explicit `GET /v1/palette/search?q=...&limit=...`. Search rows contain a `contractHash` but omit schemas.

Search queries in the full `labby:<service>::<action>` or `mcp:<upstream>::<tool>`
form select that exact caller-visible entry from the published snapshot. They do
not connect upstreams or inspect unrelated tool catalogs, and missing or hidden
entries return an empty result. Full IDs are case-sensitive, including the
`labby:` / `mcp:` prefix: differently cased full IDs return an empty result, not
a fuzzy match. Other queries retain case-insensitive fuzzy search behavior.

`GET /v1/palette/descriptor?id=mcp:<upstream>::<tool>` returns the caller-visible live MCP capability contract: identity, bounded description, input/output schemas, the four typed MCP annotation hints, authoritative destructive classification, catalog revision, and SHA-256 contract hash. It accepts MCP IDs only. Schemas are limited to 64 KiB and depth 64; the full descriptor is limited to 160 KiB. Unsupported descriptors fail with `descriptor_unsupported` rather than silently omitting a schema.

`POST /v1/palette/execute` requires `expectedContractHash`. Labby re-resolves and checks the exact subject-bound peer before validation and dispatch. Contract drift returns HTTP 409 with `contract_changed` and no expected side effects. Successful responses include a redacted receipt containing only request ID, tool ID, authoritative hash, catalog revision, and truncation state.

OAuth callers require `mcp:read` to browse. Execution requires `mcp:write` and an exact `gateway:<upstream>` grant. `gateway:*` is not accepted. `lab:admin` remains an operator shortcut. Non-admin callers cannot execute destructive tools or Labby administrative actions.
