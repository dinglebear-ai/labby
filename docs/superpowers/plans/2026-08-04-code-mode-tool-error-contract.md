# Code Mode Tool Error Contract Implementation Plan

Date: 2026-08-04
Status: implemented and under verification

## Goal

Replace the lossy `{kind,message}` Code Mode error path with a versioned, evidence-preserving contract aligned with MCP tool-error semantics.

## Work packages

1. Define host-neutral Rust contract types for origin, recovery, exact-retry policy, side-effect risk, safety hints, and sanitized evidence.
2. Extend the runner protocol so caught and uncaught JavaScript errors carry the complete object.
3. Convert completed MCP `isError` results in the gateway adapter while preserving all blocks and structured content.
4. Keep real transport failures distinct and retry-oriented.
5. Add redaction, binary omission, and evidence caps.
6. Emit the rich object through Labby's outer MCP envelope and execution trace.
7. Update Code Mode's model-facing instructions.
8. Publish the normative contract and JSON Schema.
9. Add regression coverage for multi-block partial failures, structured errors, binary data, secrets, annotations, transport separation, and health isolation.
10. Run formatting, checks, tests, strict clippy, commit, push, and open the protected-branch PR.

## Verification gates

- Rust types serialize against the schema examples.
- A completed MCP tool error remains `tool_error` and does not alter upstream health.
- A transport closure remains `upstream_error` with `origin: upstream_transport`.
- `JSON.parse(e.message)` works for caught failures.
- An uncaught failure returns the same object in the outer envelope and trace.
- All text blocks survive in order.
- Structured content survives after redaction/capping.
- Binary data is not embedded.
- Secret-shaped strings are redacted.
- Read-only/idempotent hints only soften guidance; they never become trusted guarantees.
