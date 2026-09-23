# Code Mode

Use this reference when invoking upstream MCP capabilities through Labby's Code
Mode tools.

## Surfaces And Authority

- `codemode` is the full text executor. It requires `lab` or `lab:admin` and
  may invoke write-capable or destructive upstream tools.
- `codemode_read` accepts the same payload with `lab:read`, `lab`, or
  `lab:admin`, but exposes only tools whose live descriptor explicitly says
  `readOnlyHint: true` and `destructiveHint: false`. Missing or contradictory
  annotations fail closed, and the descriptor is rechecked before dispatch.
- `codemode_ui` is an optional MCP App twin of `codemode`, controlled by
  `mcp_ui_enabled`; it is not a read-only shortcut.

The retired `trusted_read_only_tools` field grants nothing. All entry points
still enforce the caller's route and tool scope.

## Public Tools

`codemode.search()` filters the live upstream MCP catalog inside a sandbox:

```js
async () => {
  const hits = await codemode.search({ query: "github issues", limit: 5 });
  return hits.results.map(t => ({ path: t.path, id: t.id, signature: t.signature }));
}
```

`codemode` runs a JavaScript async function and lets that function call upstream
MCP tools with `callTool()` or generated `codemode.<upstream>.<tool>()` helpers:

```js
async () => {
  const issues = await callTool("github::search_issues", { q: "bug" });
  return issues.items?.length ?? 0;
}
```

Always run `codemode.search()` before calling an upstream, then call
`codemode.describe()` for the exact target when you need parameter details. The
live catalog is the authority for tool IDs, signatures, helper names, and
generated TypeScript parameter docs.

## Complete Working Examples

Search for candidate tools and return only compact catalog fields:

```json
{
  "code": "async () => {\n    const hits = await codemode.search({ query: \"axon\", limit: 5 });\n    return hits.results.map(t => ({ path: t.path, id: t.id, signature: t.signature }));\n  }"
}
```

Call a discovered tool by raw ID. Prefer this when the upstream/tool is selected
from search results:

```json
{
  "code": "async () => {\n    const help = await callTool(\"axon::axon\", { action: \"help\" });\n    return { ok: true, actions: help.actions ?? help };\n  }",
  "tools": ["axon::axon"]
}
```

Call an action-dispatched upstream using the shape from `codemode.search()` and
`codemode.describe()`. Axon uses flat action fields:

```json
{
  "code": "async () => {\n    const result = await callTool(\"axon::axon\", {\n      action: \"search\",\n      query: \"Labby Code Mode examples\",\n      limit: 5\n    });\n    const results = result.data?.data?.results ?? [];\n    return { count: results.length, results };\n  }",
  "upstreams": ["axon"]
}
```

Use a generated helper after `codemode.search()` confirms the exact helper path:

```json
{
  "code": "async () => {\n    const help = await codemode.axon.axon({ action: \"help\" });\n    return { ok: true, help_type: typeof help };\n  }",
  "upstreams": ["axon"]
}
```

Fan out independent reads without throwing away partial successes. Prefer the
first-class fail-soft batch helper:

```json
{
  "code": "async () => codemode.batch([\n    () => callTool(\"axon::axon\", { action: \"help\" }),\n    () => callTool(\"unraid::unraid\", { action: \"help\" }),\n    () => callTool(\"cortex::cortex\", { action: \"help\" })\n  ])",
  "upstreams": ["axon", "unraid", "cortex"]
}
```

Call a Windows helper through the live-confirmed helper path:

```json
{
  "code": "async () => {\n    const result = await codemode.agent_os_windows_mcp.PowerShell({\n      command: \"$PSVersionTable.PSVersion.ToString()\"\n    });\n    return { ok: true, result };\n  }",
  "tools": ["windows_windows-mcp::PowerShell"]
}
```

## Search Catalog Entries

Each `codemode.search()` entry contains:

| Field | Meaning |
| --- | --- |
| `path` | Exact path accepted by `codemode.describe()`. |
| `id` | Canonical capability ID; tool IDs use `<upstream>::<tool>` for `callTool`. |
| `namespace` | Upstream or source namespace. |
| `name` | Capability name. |
| `description` | Sanitized capability description. |
| `signature` | Compact callable signature. |
| `kind` | `tool`, `snippet`, `resource`, `prompt`, `skill`, or reserved `agent`. |
| `tags` | Source-specific tags when present. |
| `tools` | Optional snippet dependency declaration; it narrows native saved-snippet execution and never grants authority. |
| `safety` | Optional compact intrinsic safety facts from the live descriptor. |
| `score` | Search relevance score. |

The catalog searched by `codemode.search()` is complete for capabilities that
were successfully discovered and are visible to the current caller, route, and
tool scope. Only
your filtered return value enters the model context. Use `codemode.describe()`
for exact target docs, including generated TypeScript parameter declarations
for tools.

## Codemode Arguments

Top-level `codemode` arguments:

```json
{
  "code": "async () => { ... }",
  "upstreams": ["optional-upstream-allowlist"],
  "tools": ["optional-tool-or-id-allowlist"]
}
```

Only `code` is required. The rest are Labby `codemode` arguments:

- `upstreams`: allow only named upstreams for this run.
- `tools`: allow only raw tool names or `<upstream>::<tool>` IDs.

Do not place these fields inside upstream tool params.

## Calling Tools

Use `callTool` when selecting dynamically or when helper sanitization is
unclear:

```js
async () => {
  return await callTool("github::search_issues", { q: "fix" });
}
```

Use `codemode.<upstream>.<tool>` only after `codemode.search()` confirms the helper name:

```js
async () => {
  return await codemode.github.search_issues({ q: "fix" });
}
```

The host validates params against the upstream input schema before dispatching.
The enforced subset includes local `$ref`, type/enum/const constraints, object and
array constraints, `anyOf` / `oneOf` / `allOf`, and conditional `if` / `then` /
`else` / `not` branches. Generated TypeScript signatures preserve root object
properties when composition keywords are present instead of replacing the type
with `unknown` intersections.

## Resources, Snippets, And Steps

- `codemode.listResources(upstream)` returns `{ resources: [...] }` for one
  visible upstream. Pass a returned `resources[].uri` unchanged to
  `codemode.readResource(uri)`; resource URIs are not tool IDs.
- `codemode.run(name, input)` executes a discovered saved snippet within the
  enclosing run scope; frontmatter tool declarations are not reapplied on this
  nested path. Snippet discovery and execution require unscoped `lab:admin` or
  trusted-local authority and are unavailable through `codemode_read`,
  route-scoped, or tool-scoped runs.
- `codemode.getPrompt(id, args)` resolves a discovered Prompt using its exact
  `prompt::<upstream>::<name>` ID.
- `codemode.listSkills()` lists caller-visible Agent Skills;
  `codemode.getSkill(uri)` returns authorized metadata and
  `codemode.readSkill(uri)` reads verified manifest-bound content.
- `codemode.step(name, fn)` adds bounded, redacted best-effort journal data.
  It does not provide public resume/replay, and a successful run does not prove
  the detached journal flush completed.
- The `state` and `git` providers, plus static/no-auth OpenAPI operations,
  require unscoped admin/trusted-local execution. An OpenAPI operation with
  `oauth_upstream` may be used by an authenticated, unscoped, execute-capable
  caller with a verified subject. These providers remain unavailable on
  protected/tool-scoped routes and through `codemode_read`.

## Action-Dispatched Upstreams

Many upstreams expose a single action-dispatched tool instead of one tool per
operation — `axon`, and the rmcp family (`unraid`, `unifi`,
`cortex`, ...). They all take an `action`, but the rest of the envelope is
upstream-specific. Do not guess the envelope shape from memory.

- Discover operations with the tool's own `{ "action": "help" }`, or read the
  compact docs returned by `codemode.search()` and `codemode.describe()`.
- Put operation arguments exactly where the upstream schema expects them:

```js
// Axon search uses flat action fields.
async () => callTool("axon::axon", {
  action: "search",
  query: "mcpb",
  limit: 5
});

// Wrong for Axon: guessed nested params rejects with `invalid_param`
//   ("... must match exactly one schema").
async () => callTool("axon::axon", {
  action: "search",
  params: { query: "mcpb", limit: 5 }
});
```

An `invalid_param` that mentions `must match exactly one schema` means the
envelope matched no action variant. Re-read the schema and move arguments to the
expected fields. It is not a bug in the upstream tool.

## Destructive Tools

The MCP `codemode` tool currently accepts top-level `code`, `upstreams`, and
`tools`. It does not accept a public top-level `confirm` field.

Rules:

- `lab` or `lab:admin` scope authorizes execution but does not confirm effects.
- If a call returns `confirmation_required`, follow structured
  `recovery.guidance`. Only if the live upstream schema declares a confirmation
  field should you obtain explicit user confirmation and populate that field.
  Otherwise use the upstream/client's supported elicitation or operator flow.
- `allow_destructive_actions` is internal-only. Do not use it as a public param.

## Return Shape

Successful `codemode` returns a trace envelope:

```json
{
  "result": {},
  "calls": [
    { "id": "name::tool", "ok": true, "elapsed_ms": 12 }
  ],
  "logs": []
}
```

Optional envelope fields include `execution_id`, `artifacts`, and
`result_shaping`; `structuredContent` also carries compact `result_shape`.

Upstream result unwrapping:

- Prefer upstream `structuredContent`.
- Else join all text content and parse JSON when possible.
- Else return text or the full mixed MCP result shape.
- Per-call result payloads are not copied into `calls`.

> **Reading the value back.** `codemode` returns the envelope in the tool's text
> content block and a copy in `structuredContent` carrying both `result` and a
> compact `result_shape`; shaped runs also include `result_shaping` metadata.
> Most MCP clients (Claude Code included) surface `structuredContent` over text.
> If `result` is a truncation marker, execution already happened. Do not replay
> mutations merely to recover omitted output. Inspect `original_size`,
> `original_tokens`, `preview`, and `next_action`. When present, follow the
> exact `resource_read_example` URI and paginate with `next_offset` while
> keeping the resource version stable. Omitted bytes are not automatically
> cached. Reduce inside the sandbox or write an artifact on a future run.

Oversized final responses are replaced with a truncation marker. Reduce data in
the sandbox before returning large values.

## Final Result Shaping

`[code_mode].result_shape_policy` defaults to `"off"`. When set to
`"truncate"`, Labby shapes only the successful completed final `result` after
the sandbox returns and after the `__ui` compatibility unwrap. It then applies
the normal envelope budget and builds MCP text JSON plus `structuredContent`
from that same shaped response.

This does not change values seen inside the sandbox through `callTool()` or
`codemode.<upstream>.<tool>()`, and it does not add raw-result audit retention.
Use `writeArtifact()` for large detailed payloads. Truncation bounds output; it
is not redaction and must not be used to sanitize secrets.

## Error Recovery

Tool-call errors reject only that promise. Catch them locally when you want the
run to continue:

```js
async () => {
  const decodeError = (value) => {
    const message = String(value?.message ?? value);
    try {
      const parsed = JSON.parse(message);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) return parsed;
    } catch (_) {}
    return { message, side_effects: "unknown" };
  };
  const settled = await Promise.allSettled([
    callTool("a::one", {}),
    callTool("b::two", {})
  ]);
  return settled.map(r => r.status === "fulfilled" ? r.value : decodeError(r.reason));
}
```

Inspect `kind`, `origin`, `recovery.guidance`, `recovery.same_arguments`,
`side_effects`, `cause`, and `evidence` before retrying.

Common error kinds:

| Kind | Recovery |
| --- | --- |
| `missing_param` | Read `codemode.search()` / `codemode.describe()` output and include the required field. |
| `invalid_param` | Fix type/shape against the schema. Upstream MCP/JSON-RPC `-32602` errors map here and are not retryable or upstream-health failures. |
| `validation_failed` | Fix nested schema or protocol-capability validation errors. |
| `unknown_tool` | Rerun `codemode.search()`; use `<namespace>::<tool>` IDs only. Upstream `-32601` errors map here. |
| `route_scope_denied` | The protected route scope does not allow that upstream/tool. |
| `forbidden` / `permission_denied` | Caller lacks permission; destructive tools require execute-capable Code Mode callers. |
| `path_traversal` | Fix the workspace/artifact path. |
| `quota_exceeded` / `budget_exceeded` / `call_budget_exceeded` | Reduce fan-out, workspace writes, or split the work. |
| `result_too_large` / `artifact_too_large` | Reduce the upstream payload or write large data to a smaller artifact. |
| `queue_saturated` | Labby's local per-upstream concurrency gate is saturated — not an upstream rate limit. Retry after a short delay or reduce parallel `callTool` fan-out. |
| `response_too_large` | The gateway capped an oversized upstream response; narrow the query or paginate. Distinct from `result_too_large`/`artifact_too_large`, which cap Code Mode's own result/artifact output. |
| `timeout` | Split work into smaller executions. |
| `network_error` / `server_error` / `decode_error` / `upstream_error` | Retry unchanged only when `side_effects` is `none_expected` and `recovery.same_arguments` permits it. Otherwise verify the outcome or idempotency first and follow `recovery.guidance`. Unknown structured upstream-local kinds are returned as `upstream_error` without poisoning upstream health. |
| `oauth_needs_reauth` | Check `labby server auth status <upstream> --json`. |
| `snippet_not_found` | Check the snippet name with `codemode.search()`. |

## Runtime And Limits

Implementation facts that affect operation:

- `codemode.search()` is in-sandbox discovery; it does not execute a discovered
  upstream tool.
- `codemode` uses root `[code_mode]` config for timeout, response, token, log,
  and final-result shaping limits.
- Host-side env knobs also bound runner pool overflow, artifact size/retention,
  per-run `callTool` fan-out, and per-call result size.
- The runner process starts with a cleared environment and temp cwd.
- The parent host brokers all tool calls, validates schemas, enforces
  scope/tool policy, and terminates runaway executions.
- CLI `labby code run` is operator-driven and has its own policy for
  destructive upstream tools; MCP `codemode` exposes only `code`, `upstreams`,
  and `tools` as top-level arguments.
- Code Mode does not add a generic destructive-call confirmation gate. An
  execute-capable caller may call destructive upstream tools directly; other
  callers receive `forbidden`.

Common model-facing config defaults:

```toml
[code_mode]
enabled = true
mcp_ui_enabled = false
trace_params = true
result_shape_policy = "off"
timeout_ms = 30000
max_source_bytes = 1048576
max_response_bytes = 24576
max_response_tokens = 6000
token_estimate_divisor = 4
max_log_entries = 1000
max_log_bytes = 65536
```

Advanced semantic-search, widget-callback, artifact, runner-pool, per-run call,
and per-call result budgets are documented in the runtime configuration and
environment references. `trusted_read_only_tools` is retired compatibility
input and grants nothing.

`gateway.code_mode.set` accepts the public fields in the generated action
catalog, including `result_shape_policy`.

## CLI Code Mode

CLI execution:

```bash
labby code search 'github issues' --limit 5 --json
labby code describe 'github.search_issues' --json
labby code run --code 'async () => ({ ok: true })' --json
labby code run --file ./snippet.js --json
```

CLI `search` and `describe` are convenience wrappers that construct and execute
the same Code Mode discovery calls, so the operator does not need to author the
JavaScript. They inherit Code Mode runtime, authority, and timeout constraints.
Inside an execution, use `codemode.search()` and `codemode.describe()` so
discovery and the call share the same scoped run.

## Safe Execution Pattern

1. Run `codemode.search()` and return only the candidate IDs/signatures needed.
2. Choose a narrow `upstreams` or `tools` allowlist.
3. Prefer `codemode.batch` when independent calls may partially fail; use
   `Promise.allSettled` for custom settlement handling.
4. Return a compact result object rather than raw large payloads.
