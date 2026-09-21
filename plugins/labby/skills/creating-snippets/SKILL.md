---
name: creating-snippets
description: Use when creating, editing, promoting, validating, testing, running, explaining, or removing Labby Code Mode snippets; when turning a successful Code Mode execution into a reusable workflow; or when building schema-backed snippets from live upstream tool ids, JSON schemas, inputs, defaults, artifacts, and CLI/MCP/API snippet actions.
---

# Creating Snippets

## Overview

Labby snippets are saved Code Mode workflows: pick gateway MCP tools, fill their schema-typed params, call them from one async JavaScript arrow function, and return structured JSON. Keep snippet business logic in the snippet body; use Labby's snippets dispatch/CLI/MCP actions to store, validate, test, and execute it.

## First Checks

Use `$using-labby` before authoring any snippet that calls upstream tools. Search
the live catalog with `codemode.search()` and inspect the selected path with
`codemode.describe()`, then copy the returned ID, path, signature, and generated
parameter docs; use the upstream's help/schema action where applicable. Never
guess tool IDs or parameters.

When a Labby source checkout is available, resolve its Git root and read these
paths relative to it:

- `docs/snippets/README.md`
- `docs/services/SNIPPETS.md`
- `crates/labby/src/dispatch/snippets/`

If those paths are unavailable, treat the live gateway and `labby snippet --help` as the source of
truth. Do not invent snippet actions, flags, tool ids, or schemas from memory.

## Snippet Shape

Use Markdown for reusable snippets. The filename stem is the id, and frontmatter `name` must match it.

````markdown
---
name: my-workflow
description: Brief human-readable purpose
tags: [research, readonly]
inputs:
  topic:
    type: string
    required: true
    description: Topic to search
  limit:
    type: integer
    default: 5
    required: false
tools:
  - axon::axon
---

## Tutorial: How This Snippet Is Built

Explain the selected tools, params, validation, execution order, and output.

```js
async (input) => {
  const topic = input.topic;
  const limit = input.limit ?? 5;
  const axon = (params) => callTool("axon::axon", params);

  const timed = async (label, fn) => {
    const started = Date.now();
    try {
      const result = await fn();
      return { label, ok: true, ms: Date.now() - started, result };
    } catch (error) {
      return { label, ok: false, ms: Date.now() - started, error: String(error) };
    }
  };

  const results = await Promise.all([
    timed("web-search", () => axon({ action: "search", query: topic, limit })),
    timed("rag-query", () => axon({ action: "query", query: topic, limit }))
  ]);

  return { snippet: "my-workflow", input: { topic, limit }, results };
}
```
````

Raw JavaScript is allowed, but Markdown with frontmatter and a tutorial is preferred.

The worked example is conditional on `axon::axon` appearing in the current live
catalog. Substitute only IDs and parameters returned by search/describe. The
`tools` declaration narrows native saved-snippet execution by intersecting with
the caller's existing Code Mode scope; it never grants authority. Omitting it
keeps the caller's scope, `[]` denies all upstream tools, and a nonempty list
permits only those exact dependencies. A nested `codemode.run` keeps the
enclosing run scope and does not reapply the saved snippet's declaration.

## Inputs And Defaults

Use frontmatter `inputs` for user-configurable values. Supported types are `string`, `integer`, `number`, `boolean`, `object`, `array`, and `json`.

Rules:

- Give optional inputs defaults when possible so snippets still run with sparse params.
- Mark genuinely required values with `required: true`.
- Keep unknown input rejection useful: declared inputs cause `snippets.exec` to reject unexpected caller params.
- Mirror upstream schemas in the generated call params. Snippet inputs describe user-facing knobs; upstream schemas validate each MCP tool call.

## Authoring Workflow

1. List existing snippets: `labby snippet list --json`.
2. Inspect an existing body before editing: `labby snippet get my-workflow --json`.
3. Search gateway tools with `codemode.search()` and inspect the selected path
   with `codemode.describe()` for generated parameter docs. If `labby` is not on `PATH`,
   locate the active Labby CLI before continuing instead of guessing command syntax.
4. Pick tools and decide parallel vs chained execution.
5. Draft Markdown with frontmatter, tutorial text, declared inputs, and one `js`/`javascript` fenced block.
6. Validate without saving: `labby snippet validate my-workflow --file draft.md`.
7. Save as a user snippet: `labby snippet add my-workflow --file draft.md --description "..."`.
8. Smoke-test execution: `labby snippet test my-workflow --param topic="mcp-ui rust"`.
9. Run normally: `labby snippet run my-workflow --param topic="mcp-ui rust"`.

Use `--force` only when intentionally replacing a user snippet.

## MCP And Dispatch Actions

Snippets are also available through the shared dispatch layer and MCP/API service:

```json
{ "action": "snippets.list", "params": {} }
{ "action": "snippets.get", "params": { "name": "my-workflow" } }
{ "action": "snippets.validate", "params": { "name": "my-workflow", "body": "..." } }
{ "action": "snippets.create", "params": { "name": "my-workflow", "body": "...", "description": "...", "force": false } }
{ "action": "snippets.promote", "params": { "execution_id": "01JEXAMPLE", "name": "my-workflow", "description": "...", "force": false, "shadow_builtin": false } }
{ "action": "snippets.exec", "params": { "name": "my-workflow", "params": { "topic": "mcp-ui rust" } } }
{ "action": "snippets.test", "params": { "name": "my-workflow", "params": { "topic": "mcp-ui rust" } } }
{ "action": "snippets.test", "params": { "all": true } }
{ "action": "snippets.remove", "params": { "name": "my-workflow" } }
```

On MCP/API, only `snippets.list`, `help`, and `schema` are non-admin. Reading
bodies, executing, validating, testing, creating, promoting, and removing require
`lab:admin`; the local CLI is trusted-local.

`remove` is destructive and only removes user snippets. Built-ins are read-only.

`snippets.promote` is a destructive MCP/API-only action; there is no standalone
promotion CLI. It copies the retained raw source of a successful live Code Mode
execution into a user snippet. The `execution_id` is ephemeral, actor/route
scoped, admin-only, and retained only for successful admin executions. It is
lost on expiry, eviction, restart, or another gateway process.
Promotion persists source verbatim as plaintext, so never promote code containing
literal credentials. MCP may elicit confirmation. The HTTP API dispatches after
admin authorization, so its caller or operator must obtain explicit confirmation
before submitting the request. There is no promotion CLI and no payload-level
`confirm`. Use `force` to replace a user snippet and `shadow_builtin` only when
intentionally shadowing a built-in name.

## Execution Patterns

- Prefer `codemode.batch` for independent fail-soft calls. Use a custom wrapper
  or `Promise.allSettled` when you need bespoke labels, timings, or shaping.
- Chain calls when later params depend on earlier results.
- Wrap each call with timing and error capture.
- Return stable JSON fields: `snippet`, `input`, `summary`, `results`, `evidence`, `gaps`, `followup_calls`, `timings`.
- Keep responses compact; large Markdown, tables, screenshots, or manifests should be written with `writeArtifact("relative/path.md", content, { contentType })`.
- Include enough ids, URLs, labels, and raw evidence handles for follow-up verification.
- Code Mode final-result shaping may be enabled by the operator. It can shape the displayed final `result`, but it does not change values the snippet sees from `callTool()`/`codemode.*` during execution.
- `snippets.test` evaluates pass/fail from the pre-shape result, so return `{ ok: true }` or `{ ok: false }` deliberately. `snippets.exec` and the `response` inside `snippets.test` show the shaped display response when shaping is enabled.
- Truncation is not redaction. Do not return secrets and rely on output shaping to hide them.

## Validation Checklist

Before calling the work done:

- `name` starts with a lowercase ASCII letter or digit, continues with only
  lowercase ASCII letters, digits, hyphens, or underscores, and matches the
  filename/frontmatter.
- Description is non-empty.
- Body contains exactly the intended async arrow function.
- All upstream tool ids came from live gateway `codemode.search()`.
- The `tools` declaration is the intended narrow dependency set and does not
  exceed the caller/route authority.
- Tool params match upstream schemas.
- Optional inputs have defaults or code fallbacks.
- Required inputs fail fast with clear validation.
- Fan-out is bounded by explicit snippet limits and the Code Mode wall-clock/output budgets.
- `labby snippet validate` passes.
- `labby snippet test` passes for one snippet, or `labby snippet test --all` passes when changing shared built-ins.
