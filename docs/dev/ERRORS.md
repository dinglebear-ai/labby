---
title: "Error Contract"
created: "2026-07-30"
updated: "2026-07-30"
---

# Error Contract

`labby_runtime::error::ToolError` is the canonical surface-neutral error type.
CLI, MCP, and HTTP must preserve its stable `kind` vocabulary and human-readable
`message` rather than inventing surface-specific error shapes.

## Core Dispatcher Kinds

- `unknown_action`: action is not registered; includes valid choices and an
  optional hint.
- `missing_param`: required parameter is absent.
- `invalid_param`: parameter type or value is invalid.
- `unknown_instance`: named instance is not configured.
- `ambiguous_tool`: an upstream tool name requires qualification.
- `confirmation_required`: a destructive action lacks explicit confirmation.
- `conflict`: the requested identifier already exists.
- `forbidden`: caller lacks required scopes; includes `required_scopes`.

SDK and subsystem errors use `ToolError::Sdk` to promote their stable kind to
the same top-level envelope:

```json
{ "kind": "auth_failed", "message": "..." }
```

## Common Subsystem Kinds

Supported code may emit additional stable kinds, including:

- auth/OAuth: `auth_failed`, `oauth_needs_reauth`,
  `oauth_state_invalid`, `oauth_resource_mismatch`,
  `oauth_issuer_mismatch`, `oauth_unsupported_method`;
- routing/upstreams: `not_found`, `unknown_upstream`,
  `upstream_error`, `bad_gateway`, `network_error`,
  `service_unavailable`, `timeout`;
- validation/security: `validation_failed`, `ssrf_blocked`,
  `path_traversal`, `symlink_rejected`, `content_too_large`;
- Code Mode: `code_mode_timeout`, `code_mode_fuel_exhausted`;
- concurrency/state: `rate_limited`, `queue_saturated`,
  `restart_required`, `stale_suggestion`, `merge_write_conflict`,
  `workspace_not_configured`;
- internal failures: `internal_error`, `server_error`, `decode_error`.

The emitting subsystem owns the precise remediation text. New stable kinds require
API mapping tests and documentation.

## HTTP Mapping

`ApiError` is the local axum wrapper around `ToolError`. Broad mapping rules:

- authentication failure: 401;
- forbidden scope/action: 403;
- unknown resource: 404;
- conflict/restart/stale state: 409;
- invalid input, confirmation, SSRF, or path validation: 422;
- payload limits: 413;
- rate/queue limits: 429;
- upstream gateway failure: 502;
- service unavailable: 503;
- timeouts: 504;
- unknown/internal kind: 500.

MCP and CLI retain the same serialized error envelope even when HTTP assigns a
status code.

## Logging And Redaction

Caller-fixable errors log at WARN. Internal failures requiring operator action log
at ERROR. Error messages must not include bearer tokens, OAuth codes, provider
credentials, full secret environment values, or unredacted sensitive paths.

## Removed Error Vocabularies

Error kinds used only by ACP, Registry installers, Marketplace artifacts,
Fleet/node transport, Deploy-product, or Stash must be removed with those
features, including stale status-mapping tests. Their historical definitions are
available in [../references/retired-labby/current-docs/dev/ERRORS.md](../references/retired-labby/current-docs/dev/ERRORS.md).
