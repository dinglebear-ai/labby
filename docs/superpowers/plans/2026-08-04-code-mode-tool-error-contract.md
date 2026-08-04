---
title: Agent error contract implementation plan
created: 2026-08-04
updated: 2026-08-04
---

# Agent Error Contract Implementation Plan

Date: 2026-08-04
Status: implemented and under final verification

## Goal

Replace thin or inconsistent agent-facing failures with one versioned recovery contract across Code Mode, direct MCP proxying, built-in MCP tools, MCP protocol errors, HTTP/OpenAPI, JSON CLI output, and the Code Mode inspector.

## Work packages

1. Define surface-neutral origin, recovery, exact-retry, and side-effect types in labby-runtime.
2. Make ToolError serialize the additive contract without changing existing constructors.
3. Keep Code Mode evidence and safety hints while reusing the shared vocabulary.
4. Analyze completed MCP isError results across all content blocks and structuredContent.
5. Enrich direct MCP errors while preserving every original upstream payload channel.
6. Ensure completed MCP tool errors never poison upstream connection health.
7. Distinguish transport and OAuth failures with tool identity, sanitized cause, and explicit gateway.oauth.start recovery.
8. Emit the same MCP envelope in text and structuredContent for all built-in and early-gate failures.
9. Document AgentErrorResponse and complete non-2xx status classes in OpenAPI.
10. Emit stable machine-readable stderr failures for labby --json while preserving nonzero exit codes.
11. Place contract data in MCP JSON-RPC errors for bridge, prompt, and resource failures.
12. Render recovery, side effects, cause, safety hints, and evidence in the Code Mode inspector.
13. Publish the shared contract and JSON Schemas.
14. Run formatting, generated-doc drift, feature slices, full tests, strict clippy, repository contract, commit, push, and protected-branch PR checks.

## Verification gates

- ToolError is byte-compatible in kind/message/existing extras and additive in recovery metadata.
- Direct MCP completed tool errors retain every original content block and upstream structured content.
- Completed isError results never increment the upstream breaker.
- Transport loss and OAuth reauthorization are distinguishable and course-correcting.
- MCP text and structuredContent contain equivalent error envelopes.
- MCP protocol ErrorData.data contains the versioned contract.
- HTTP runtime and OpenAPI expose AgentErrorResponse consistently.
- labby --json writes valid structured errors to stderr on failure.
- Caught and uncaught Code Mode failures expose the same object.
- The inspector visibly renders recovery, side-effect risk, cause, safety, and evidence.
- Secret-shaped strings and binary evidence remain redacted or omitted.
- All feature slices, tests, generated docs, and repository contracts pass.
