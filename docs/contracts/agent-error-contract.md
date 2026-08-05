---
title: Agent error contract
created: 2026-08-04
updated: 2026-08-04
---

# Agent Error Contract

Status: implemented
Contract version: 1

## Purpose

Labby exposes one additive, model-actionable error contract across MCP tools, MCP protocol errors, HTTP/OpenAPI responses, Code Mode JavaScript rejections, and `labby --json` failures. `ToolError` remains the canonical runtime error type; surfaces add identity and preserved evidence when they know more than the dispatcher.

## Required fields

Every agent-facing error object contains:

- `contract_version`
- `kind`
- `message`
- `origin`
- `recovery.action`
- `recovery.same_arguments`
- `recovery.guidance`
- `side_effects`

The `message` must remain useful when a client discards every other field.

## Optional context

Surfaces add relevant identity without leaking request secrets:

- `service`
- `action`
- `tool`
- `upstream`
- `command`
- `prompt`
- `resource`
- `cause`
- `original_kind`
- `evidence`
- `safety`

## Origins

- `validation`: the request was rejected before execution.
- `policy`: authentication, authorization, confirmation, or route policy blocked execution.
- `budget`: a size, rate, quota, or fan-out limit blocked execution.
- `discovery`: the requested action, tool, prompt, resource, or instance was not available.
- `tool_execution`: a completed tool result reported failure.
- `upstream_transport`: no completed upstream result arrived.
- `bridge`: the stdio bridge could not reach the canonical daemon.
- `code_mode`: the Code Mode runtime rejected execution.
- `runtime`: another Labby runtime failure.

## Retry contract

`recovery.same_arguments` describes repeating the exact request, not whether a revised request may be useful.

- `safe`: an exact retry is expected to be safe.
- `conditional`: retry only after the stated condition clears and side effects are checked.
- `discouraged`: inspect or revise before retrying.
- `never`: the request must change or external state must be repaired first.

`side_effects` is conservative. `possible` or `unknown` means the caller must check whether work committed before repeating a mutating operation.

## Surface rules

### MCP tool results

Labby returns the same envelope in both text content and `structuredContent`. Direct upstream `isError: true` results prepend a bounded Labby diagnostic block, preserve every original content block in order, preserve upstream structured content under `upstream_structured_content`, and attach the contract under `_meta["ai.dinglebear.labby/error"]`.

A completed `isError: true` result never poisons upstream connection health.

### MCP protocol errors

Prompt, resource, and bridge errors keep the appropriate JSON-RPC error code and place this contract in `ErrorData.data`.

### HTTP and OpenAPI

Non-2xx API responses serialize the canonical `ToolError` contract. The generated OpenAPI specification documents `AgentErrorResponse` and the meaningful 400, 401, 403, 409, 413, 422, 429, 500, 502, 503, and 504 classes.

### CLI JSON

When `--json` is active, failures are written to stderr as:

```json
{
  "ok": false,
  "command": "gateway",
  "error": {
    "contract_version": 1,
    "kind": "oauth_needs_reauth",
    "message": "...",
    "origin": "policy",
    "recovery": {
      "action": "reauthenticate",
      "same_arguments": "never",
      "guidance": "..."
    },
    "side_effects": "none_expected"
  }
}
```

The process exit code remains nonzero.

### Code Mode

Code Mode extends this contract with sanitized MCP evidence and tool safety hints. Caught and uncaught JavaScript failures expose the same object.

## Compatibility

Version 1 permits additive fields. Consumers must ignore fields they do not understand. A semantic change to a required field requires a new contract version.

## Source of truth

- Shared Rust metadata: `crates/labby-runtime/src/agent_error.rs`
- Canonical runtime error serialization: `crates/labby-runtime/src/error.rs`
- MCP upstream analyzer: `crates/labby-gateway/src/upstream/tool_error.rs`
- MCP protocol constructors: `crates/labby/src/mcp/agent_error.rs`
- JSON Schema: `docs/contracts/schemas/agent-error.schema.json`
