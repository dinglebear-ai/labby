---
title: "Error Contract"
created: "2026-07-30"
updated: "2026-08-05"
---

# Error Contract

`labby_runtime::error::ToolError` is the canonical surface-neutral error type.
CLI, MCP, and HTTP must preserve its stable `kind` vocabulary and human-readable
`message` rather than inventing surface-specific error shapes.

## Agent Error Envelope

Every agent-facing error object carries the versioned recovery contract in
addition to `kind` and `message`. The required fields are:

- `contract_version` — currently `1`; additive changes keep the version.
- `kind` — stable machine-readable tag (this document's vocabulary).
- `message` — human/model-readable diagnosis, useful on its own.
- `origin` — where the failure arose: `validation`, `policy`, `budget`,
  `discovery`, `tool_execution`, `upstream_transport`, `bridge`, `code_mode`,
  or `runtime`.
- `recovery` — advice object with required `action` (`revise_and_retry`,
  `retry_later`, `reauthenticate`, `confirm`, `rediscover`, `reduce_work`,
  `start_dependency`, `inspect_and_escalate`, `do_not_retry`),
  `same_arguments` (`safe`, `conditional`, `discouraged`, `never`), and
  `guidance` (free text), plus optional `retry_after_ms`.
- `side_effects` — `none_expected`, `possible`, or `unknown`.

Optional context fields (`service`, `action`, `tool`, `upstream`, `command`,
`prompt`, `resource`, `cause`, `original_kind`, `safety`, `evidence`) are
additive.

The full normative contract, surface rules, and published JSON Schemas are
owned by [../contracts/agent-error-contract.md](../contracts/agent-error-contract.md)
and [../contracts/code-mode-tool-errors.md](../contracts/code-mode-tool-errors.md)
(schemas: `docs/contracts/schemas/agent-error.schema.json`,
`docs/contracts/schemas/code-mode-call-error.schema.json`). The shared Rust
implementation is `crates/labby-runtime/src/agent_error.rs`; drift between the
emitted values and the published schemas is locked by
`crates/labby-runtime/tests/agent_error_schema.rs` and
`crates/labby-codemode/tests/code_mode_error_schema.rs`.

### Unknown-kind catch-all

A `kind` outside the classification tables is still a valid envelope: it
classifies as `origin: runtime`, `side_effects: unknown`, and
`recovery.action: inspect_and_escalate` with `same_arguments: discouraged`.
Consumers must treat unknown kinds as opaque and fall back on `recovery` and
`message` rather than failing.

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

- auth/OAuth: `auth_failed`, `auth_required`, `permission_denied`,
  `oauth_needs_reauth`, `oauth_state_invalid`, `oauth_resource_mismatch`,
  `oauth_issuer_mismatch`, `oauth_unsupported_method`,
  `oauth_scope_upgrade_required`, `oauth_account_ambiguous`,
  `oauth_client_mismatch`, `oauth_shared_credential_protected`;
- routing/upstreams: `not_found`, `unknown_upstream`, `unknown_tool`,
  `upstream_error`, `bad_gateway`, `network_error`,
  `service_unavailable`, `not_connected`, `connection_error`, `dns_error`,
  `connection_refused` (the latter three come from `classify_upstream_error`'s
  upstream-health classification; see the classifier note below), `timeout`,
  `cancelled` (the upstream reported the proxied call was cancelled; not
  automatically retryable), `unexpected_response`;
- relay/bridge: `bridge_transport_error` (the stdio bridge could not reach the
  canonical daemon), `relay_invalid_target`, `relay_forwarder_init_failed`;
- validation/security: `validation_failed`, `invalid_hint`, `ssrf_blocked`,
  `path_traversal`, `symlink_rejected`, `content_too_large`,
  `invalid_encoding`;
- payload limits: `response_too_large` — gateway cap on upstream MCP response
  bytes (distinct from `content_too_large`'s request/content limits);
- Code Mode: `timeout` (wall-clock expiry — the historical
  `code_mode_timeout`/`code_mode_fuel_exhausted` kinds are retired and must not
  be reintroduced), `invalid_code_mode_id`, `call_budget_exceeded`,
  `snippet_budget_exceeded`, `snippet_resolve_limit`, `snippet_not_found`,
  `artifact_too_large`, `result_too_large`;
- providers: `provider_unavailable`, `provider_timeout`,
  `invalid_provider_output`;
- concurrency/state: `rate_limited`, `queue_saturated`, `budget_exceeded`,
  `quota_exceeded`, `restart_required`, `stale_suggestion`,
  `merge_write_conflict`, `workspace_not_configured`;
- internal failures: `internal_error`, `server_error`, `decode_error`.

The emitting subsystem owns the precise remediation text. New stable kinds require
API mapping tests and documentation.

### `oauth_needs_reauth` vs `auth_failed` — two classifier vocabularies

Two classifiers look at raw upstream transport failures, on purpose:

- `classify_upstream_error`
  (`crates/labby-gateway/src/upstream/pool/helpers.rs`) feeds the circuit
  breaker, backoff, and operator logs with `auth_failed` / `auth_required` /
  `timeout` / `dns_error` / `connection_refused` / `connection_error`.
- `upstream_failure_kind` (`crates/labby/src/mcp/call_tool_upstream.rs`) runs
  on the live MCP call path and emits the model-facing kind.
  `oauth_needs_reauth` is a **deliberate refinement of `auth_failed`** there:
  an authorization-shaped transport failure (`401` as a standalone token,
  `unauthorized`, `invalid_token`, OAuth wording, …) becomes an envelope that
  carries `recovery.action: reauthenticate` and points the agent at
  `gateway.oauth.start` for that upstream instead of a generic auth failure.

Keep the two auth heuristics aligned when either changes; both sites carry
cross-referencing comments.

## Circuit Breaker And Completed Tool Errors

A completed MCP result with `isError: true` proves the upstream protocol
connection worked. Such results are enriched for the model
(`tool_execution` origin) but **never** count toward the upstream circuit
breaker or health state. The same holds for a valid JSON-RPC/MCP `ErrorData`
rejection (`CapabilityCallError::Mcp`): a well-formed protocol error proves the
peer is reachable, so the pool records a breaker **success** for it. Only a
transport-class failure — no completed result and no valid MCP error — records
a breaker failure.

**The upstream pool owns health accounting.** `timed_capability_call`
(`crates/labby-gateway/src/upstream/pool/capability_call.rs`) and
`call_tool_relayed` (`.../pool/relay.rs`) record success/failure for every call
that reaches an upstream. Surfaces layered above them — notably the MCP
upstream proxy in `crates/labby/src/mcp/call_tool_upstream.rs` — must **not**
call `record_failure`/`record_success` for those outcomes: recording again
double-counts transport failures (halving the effective
`CIRCUIT_BREAKER_THRESHOLD`) and flaps a healthy upstream toward `Unhealthy` on
a caller mistake. The one exception is the pooled not-connected (`None`) arm,
where `acquire_peer` only logs and records nothing.

`CapabilityCallError` is also the kind-fidelity carrier: Code Mode and the MCP
proxy both classify a `Mcp` rejection through
`labby_gateway::upstream::tool_error::mcp_error_data_kind`, so an upstream
`invalid_params` surfaces as `invalid_param` on both surfaces rather than a
generic `upstream_error`. Transport-shaped classes keep the string classifier
so the `oauth_needs_reauth` refinement below is preserved.

## HTTP Mapping

`ApiError` is the local axum wrapper around `ToolError`. Broad mapping rules:

- authentication failure, including `oauth_needs_reauth`: 401;
- forbidden scope/action, including `oauth_scope_upgrade_required`: 403;
- unknown resource: 404;
- conflict/restart/stale state, including `oauth_account_ambiguous`,
  `oauth_client_mismatch`, and `oauth_shared_credential_protected`: 409;
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

Upstream-controlled error text is sanitized (control/bidi characters stripped,
prompt-injection markers removed, secret-like segments redacted, length
bounded) before it enters any envelope, log, or the Code Mode sandbox; capped
text ends with a ` …[truncated]` marker. See
`labby_runtime::agent_error::sanitize_error_text`.

## Removed Error Vocabularies

Error kinds used only by ACP, Registry installers, Marketplace artifacts,
Fleet/node transport, Deploy-product, or Stash must be removed with those
features, including stale status-mapping tests. Their historical definitions are
available in [../references/retired-labby/current-docs/dev/ERRORS.md](../references/retired-labby/current-docs/dev/ERRORS.md).
