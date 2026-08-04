# Code Mode Tool Error Contract

Status: implemented
Contract version: 1

## Purpose

Code Mode converts an MCP `CallToolResult` with `isError: true` into a rejected JavaScript promise. The rejection must give model-authored code enough trustworthy context to diagnose the failure, choose a safe recovery action, and avoid duplicate side effects.

This contract does not change MCP semantics. A completed `isError: true` result remains a tool execution failure and must not count as a Labby-to-upstream transport failure or poison upstream health.

## Normative behavior

1. Labby MUST preserve the canonical `kind` and a standalone model-readable `message`.
2. Labby MUST identify the fully-qualified tool when known.
3. Labby MUST distinguish `tool_execution` from `upstream_transport`.
4. Completed MCP tool errors MUST NOT open or increment the upstream circuit breaker.
5. The JavaScript `Error.message` MUST be valid JSON conforming to the versioned schema.
6. Caught and uncaught Code Mode failures MUST expose the same contract object.
7. Labby MUST preserve sanitized evidence from every MCP content block, not only `content[0]`.
8. Labby SHOULD preserve sanitized `structuredContent` and a parsed structured error object when available.
9. Binary payloads MUST be omitted and replaced with safe descriptors.
10. Evidence MUST be redacted and capped before entering the sandbox or model context.
11. Recovery advice MUST distinguish exact retries from revised retries.
12. Side-effect guidance MUST be conservative. Unknown or mutating calls may have partially completed.
13. MCP tool annotations MAY influence guidance, but MUST be treated only as untrusted hints.
14. Consumers MUST ignore unknown additive fields within the same contract version.

## Wire surfaces

### JavaScript rejection

`callTool()` and generated `codemode.<namespace>.<tool>()` helpers reject with:

`new Error(JSON.stringify(CodeModeCallError))`

Caller code recovers the object with:

`const error = JSON.parse(String(e.message));`

### Outer MCP response

An uncaught error is returned in Labby's standard MCP error envelope. The complete contract is nested under `error`, and the Code Mode execution trace also includes the same object under `error`.

## Classification

- `origin: tool_execution`: the upstream MCP server returned a completed `isError: true` result.
- `origin: upstream_transport`: no completed MCP result arrived because the Labby-to-upstream connection failed.
- `origin: validation`: input or output validation rejected the call.
- `origin: policy`: authorization, scope, or confirmation blocked the call.
- `origin: budget`: a Code Mode size, fan-out, or quota limit blocked the call.
- `origin: code_mode`: another sandbox or broker failure.

Infrastructure-looking kinds nested inside a completed MCP tool result, including `upstream_error`, `network_error`, and `server_error`, canonicalize to `tool_error`. Their original value remains in `original_kind`.

## Recovery

`recovery.action` describes the next move. `recovery.same_arguments` describes whether repeating the exact call is appropriate. The model-readable `recovery.guidance` remains authoritative when fields appear ambiguous.

A generic `tool_error` is not terminal by definition. It normally means inspect evidence, revise the command or parameters, then retry when safe.

## Evidence policy

Labby preserves up to 16 content blocks. Text is sanitized and capped. Images and audio retain only type, MIME type, encoded size, and an omission marker. Resources are redacted and capped. Structured values use the shared Code Mode JSON redactor.

The current caps are implementation limits, not wire guarantees, and may become configurable without changing contract version 1.

## Compatibility

Version 1 changes are additive unless a field's documented semantics change. A breaking semantic change requires a new `contract_version`. Clients must branch on versions they understand and ignore unknown additive fields.

## Source of truth

- Rust types: `crates/labby-codemode/src/error_contract.rs`
- MCP adapter: `crates/labby-gateway/src/gateway/code_mode/tool_error.rs`
- JSON Schema: `docs/contracts/schemas/code-mode-call-error.schema.json`
