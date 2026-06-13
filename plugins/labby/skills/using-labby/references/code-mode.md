# Code Mode

Use this reference when invoking upstream MCP tools through Labby's public Code
Mode tools: `search` and `execute`.

## Public Tools

`search` filters the live upstream MCP catalog inside a sandbox:

```js
async () => tools
  .filter(t => t.upstream.includes("github"))
  .map(t => ({ id: t.id, signature: t.signature, dts: t.dts }))
```

`execute` runs a JavaScript async function and lets that function call upstream
MCP tools:

```js
async () => {
  const issues = await callTool("github::search_issues", { q: "bug" });
  return issues.items?.length ?? 0;
}
```

Always run `search` before `execute`. The live catalog is the authority for
tool IDs, schemas, output schemas, signatures, and helper names.

## Search Catalog Entries

Each `search` entry contains:

| Field | Meaning |
| --- | --- |
| `id` | Canonical `<upstream>::<tool>` ID for `callTool`. |
| `upstream` | Upstream gateway name. |
| `name` | Upstream tool name. |
| `description` | Sanitized tool description. |
| `schema` | Input JSON schema. |
| `output_schema` | Output JSON schema when provided. |
| `signature` | Compact callable signature. |
| `dts` | TypeScript declaration for the `codemode.*` helper. |

The catalog injected into `search` is complete and in-sandbox; only your
filtered return value enters the model context.

## Execute Arguments

Top-level `execute` arguments:

```json
{
  "code": "async () => { ... }",
  "upstreams": ["optional-upstream-allowlist"],
  "tools": ["optional-tool-or-id-allowlist"],
  "max_tool_calls": 10,
  "confirm": true
}
```

Only `code` is required. The rest are Labby `execute` arguments:

- `upstreams`: allow only named upstreams for this run.
- `tools`: allow only raw tool names or `<upstream>::<tool>` IDs.
- `max_tool_calls`: cap brokered tool calls for this execution; clamped by gateway config.
- `confirm`: permit destructive upstream tools for this execution.

Do not place these fields inside upstream tool params.

## Calling Tools

Use `callTool` when selecting dynamically or when helper sanitization is
unclear:

```js
async () => {
  return await callTool("github::search_issues", { q: "fix" });
}
```

Use `codemode.<upstream>.<tool>` only after `search` confirms the helper name:

```js
async () => {
  return await codemode.github.search_issues({ q: "fix" });
}
```

The host validates params against the upstream input schema before dispatching.

## Action-Dispatched Upstreams

Many upstreams expose a single action-dispatched tool instead of one tool per
operation — `axon`, and the rmcp family (`unraid`, `unifi`, `sonarr`, `radarr`,
`cortex`, ...). These take a single envelope, `{ action, params }`, and some add a
`subaction`. Do not guess the envelope shape from memory.

- Discover operations with the tool's own `{ "action": "help" }`, or read the
  `schema` entry returned by `search`.
- Put operation arguments under `params`, never at the top level:

```js
// Right: action + nested params
async () => callTool("axon::axon", { action: "research", params: { query: "mcpb" } });

// Wrong: guessed top-level fields — rejects with `invalid_param`:
//   "callTool params `params` must match exactly one schema"
async () => callTool("axon::axon", { action: "research", subaction: "help" });
```

An `invalid_param` that says `params must match exactly one schema` means the
envelope matched no action variant. Re-read the schema and nest the arguments
under `params` — it is not a bug in the upstream tool.

## Destructive Tools

Destructive upstream tools require top-level `confirm` on `execute`:

```json
{
  "code": "async () => { return await callTool(\"x::delete\", { id: \"1\" }); }",
  "tools": ["x::delete"],
  "confirm": true
}
```

Rules:

- `lab` or `lab:admin` scope authorizes execution but does not confirm effects.
- `confirm` belongs on the top-level Labby `execute` call.
- `allow_destructive_actions` is internal-only. Do not use it as a public param.
- If the error says `confirmation_required`, retry `execute` with top-level
  `"confirm": true`.

## Return Shape

Successful `execute` returns:

```json
{
  "result": {},
  "calls": [
    { "id": "name::tool", "ok": true, "elapsed_ms": 12 }
  ],
  "logs": []
}
```

Upstream result unwrapping:

- Prefer upstream `structuredContent`.
- Else join all text content and parse JSON when possible.
- Else return text or the full mixed MCP result shape.
- Per-call result payloads are not copied into `calls`.

> **Reading the value back.** `execute` returns the envelope in the tool's text
> content block and a compact, redaction-safe copy in `structuredContent`. Most
> MCP clients (Claude Code included) surface `structuredContent` over text. If a
> client shows you a `code_mode_execute_trace` whose `result` has collapsed to a
> `result_shape` (a description of the value, not the value), the payload was too
> large to inline. Reduce the data inside the sandbox before returning, or write
> large payloads to an artifact and read them back — do not rely on a large
> `result` reaching the model verbatim.

Oversized final responses are replaced with a truncation marker. Reduce data in
the sandbox before returning large values.

## Error Recovery

Tool-call errors reject only that promise. Catch them locally when you want the
run to continue:

```js
async () => {
  const settled = await Promise.allSettled([
    callTool("a::one", {}),
    callTool("b::two", {})
  ]);
  return settled.map(r => r.status === "fulfilled" ? r.value : JSON.parse(String(r.reason.message)));
}
```

Common error kinds:

| Kind | Recovery |
| --- | --- |
| `missing_param` | Read `search` schema and include the required field. |
| `invalid_param` | Fix type/shape against the schema. |
| `validation_failed` | Fix nested schema validation errors. |
| `confirmation_required` | Retry top-level `execute` with `"confirm": true`. |
| `unknown_tool` | Rerun `search`; use `<upstream>::<tool>` IDs only. |
| `tool_call_limit_exceeded` | Reduce fan-out or set top-level `max_tool_calls`. |
| `timeout` | Split work into smaller executions. |
| `oauth_needs_reauth` | Check `labby gateway mcp auth status <upstream> --json`. |

## Runtime And Limits

Implementation facts that affect operation:

- `search` uses a 15s sandbox timeout and cannot call tools.
- `execute` uses root `[code_mode]` config for timeout, tool-call, response,
  token, and log limits.
- The runner process starts with a cleared environment and temp cwd.
- The parent host brokers all tool calls, validates schemas, applies
  confirmations, and terminates runaway executions.
- CLI `labby gateway code exec` is operator-driven and permits destructive
  upstream tools; MCP `execute` requires top-level `confirm`.

Current config defaults:

```toml
[code_mode]
timeout_ms = 30000
max_tool_calls = 1000
max_response_bytes = 24576
max_response_tokens = 6000
token_estimate_divisor = 4
max_log_entries = 1000
max_log_bytes = 65536
```

`gateway.code_mode.set` currently updates only:

- `timeout_ms`
- `max_tool_calls`
- `max_response_bytes`
- `max_response_tokens`

Edit `config.toml` for other Code Mode config fields unless the generated
action catalog has changed.

## CLI Code Mode

CLI execution:

```bash
labby gateway code exec --code 'async () => ({ ok: true })' --json
labby gateway code exec --file ./snippet.js --json
```

The CLI mirrors execution only; there is no CLI `gateway code search`
subcommand. Use MCP `search` for catalog filtering.

## Safe Execution Pattern

1. Run `search` and return only the candidate IDs/signatures needed.
2. Choose a narrow `upstreams` or `tools` allowlist.
3. Set `max_tool_calls` for bounded fan-out.
4. Use `Promise.allSettled` when independent calls may partially fail.
5. Return a compact result object rather than raw large payloads.
