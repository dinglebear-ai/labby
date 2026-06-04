# Code Mode — Implementation Specification

This specification describes the current Lab implementation. Older design notes that mention a single advertised `code` tool are historical and do not match the shipped surface.

## Current Surface

Code Mode is exposed through the gateway `search` and `execute` MCP tools when `[code_mode].enabled = true`.

| Tool | Input | Behavior |
|------|-------|----------|
| `search` | `{ "code": string }` | Runs a JavaScript async arrow function against an injected `tools` catalog. |
| `execute` | `{ "code": string, "upstreams"?: string[], "tools"?: string[] }` | Runs a JavaScript async arrow function in the execution sandbox and brokers upstream calls. |

The gateway does not advertise a `code` MCP tool. The old `code_search` / `code_execute` split is removed. The old `code_mode` / `tool_execute` names are legacy aliases for the canonical `search` / `execute` tools.

## Catalog Discovery

`search` injects `const tools = [...]`, where each entry contains:

- `id`
- `upstream`
- `name`
- `description`
- `schema`
- `output_schema`
- `signature`
- `dts`

The expected agent flow is:

1. Call `search` with a filtering/projection function.
2. Use the returned `id`, `signature`, or `dts`.
3. Call `execute` with `callTool(id, params)` or a generated `codemode.<upstream>.<tool>()` helper.

Example:

```js
async () => tools
  .filter(t => t.upstream === "github" && /issue/i.test(t.description))
  .map(t => ({ id: t.id, signature: t.signature, dts: t.dts }))
```

## Execution

`execute` receives JavaScript and wraps it in the Code Mode runner. The runner exposes:

```ts
declare function callTool<T = unknown>(
  id: `${string}::${string}`,
  params: Record<string, unknown>
): Promise<T>;
```

It also injects generated `codemode.<upstream>.<tool>()` helpers for visible upstream tools when the live catalog can be built within limits.

Example:

```js
async () => {
  const result = await callTool("github::search_issues", {
    q: "repo:jmagar/lab gateway"
  });
  return result;
}
```

## IDs

Valid Code Mode IDs are:

```text
<upstream>::<tool>
```

`upstream::<server>::<tool>` is invalid. `lab::<service>` is reserved and rejected because Lab built-in service actions are not available inside the Code Mode sandbox.

## Config Ownership

`[code_mode]` controls MCP visibility and execution limits:

```toml
[code_mode]
enabled = true
timeout_ms = 30000
max_tool_calls = 1000
max_response_bytes = 24576
max_response_tokens = 6000
token_estimate_divisor = 4
max_log_entries = 1000
max_log_bytes = 65536
```

There is no search-ranking or top-k config. `search` is JavaScript over the live
catalog, so callers control filtering in their own code.

## Enforcement

- `search` is read-only and accepts `lab:read`, `lab`, or `lab:admin`.
- `execute` requires `lab` or `lab:admin`.
- `execute` has no filesystem, environment, host network, Node, or Deno APIs.
- Host calls are brokered by the parent gateway and retain upstream visibility, auth, destructive-action, schema-validation, and response-budget checks.
- `timeout_ms` kills runaway executions.
- `max_tool_calls` is enforced in the parent before each brokered upstream call.
- response and console output are truncated according to `[code_mode]` limits.

## Error Kinds

Code Mode specific failures use stable `kind` values including:

- `invalid_code_mode_id`
- `schema_unavailable`
- `validation_failed`
- `code_mode_timeout`
- `code_mode_fuel_exhausted`
- `timeout`

General gateway and upstream failures continue to use the shared error envelope described in [docs/dev/ERRORS.md](../dev/ERRORS.md).
