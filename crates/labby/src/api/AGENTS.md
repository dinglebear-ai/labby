# api/ — axum HTTP Surface

This directory adapts shared Labby operations to HTTP. It is a peer of CLI and MCP, not a second implementation of product behavior.

## Responsibilities

- route construction and feature gating
- auth/session middleware and caller context
- HTTP request extraction
- shared-dispatch invocation
- stable status/error mapping
- streaming/binary response adaptation where HTTP requires it
- OpenAPI integration when the feature is enabled

## Rules

- operation semantics and validation belong in shared dispatch or the owning reusable crate
- use shared action metadata for destructive/admin policy; do not maintain a second action catalog
- preserve `ToolError`/agent-error structure until HTTP mapping
- do not expose secrets through error bodies, tracing fields, or debug responses
- keep canonical observability surface value `api`
- MCP-only elicitation behavior must not be copied blindly into HTTP; apply the HTTP contract documented in `docs/dev/ERRORS.md` and `docs/surfaces/MCP.md`

`AppState` may hold shared runtime managers/clients; that does not make the API layer the owner of their semantics.
