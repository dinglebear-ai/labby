# ACP Registry Coverage

**Last updated:** 2026-05-01
**Source spec:** [../upstream-api/acp-registry.md](../upstream-api/acp-registry.md)
**Format:** SDK-only capability note

## Status

`acp_registry` is a feature-gated `lab-apis` module with plugin metadata and
environment configuration, but it is not currently exposed as a first-class
CLI, MCP, or HTTP service.

## Current Surface

| Layer | Status |
| --- | --- |
| `lab-apis` feature | Present: `acp_registry` |
| Metadata | Present: `ACP_REGISTRY_URL` |
| Dispatch module | Not present |
| CLI command | Not present |
| MCP service | Not present |
| HTTP API route | Not present |
| Generated catalogs | Not present |

## Configuration

| Env Var | Required | Description |
| --- | --- | --- |
| `ACP_REGISTRY_URL` | no | Base URL for an ACP registry-compatible upstream. |

## Follow-Up Options

- Keep this SDK-only and leave it out of runtime catalogs.
- Or onboard it as a full service by adding dispatch, CLI, MCP registration,
  API routing, generated help, and tests.

