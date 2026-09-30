---
name: using-codemode
description: Use when discovering and calling upstream MCP tools, prompts, resources, or skills through Labby Code Mode; batching gateway calls; or diagnosing Code Mode scope, schema, and execution errors. For saved reusable workflows, use using-snippets.
---

# Using Labby Code Mode

Use this skill for a live gateway call. For saved workflows, use `$using-snippets`; for general Labby operation, use `$using-labby`.

## Discover before calling

Use the active Labby connection. Search the live catalog with `codemode.search()` and inspect an exact match with `codemode.describe()` before calling it. Copy the returned tool ID, helper path, and input schema. Do not infer them from a skill example or an older session.

```js
async () => {
  const hits = await codemode.search({ query: "github issues", limit: 5 });
  return hits.results.map(t => ({ id: t.id, path: t.path, signature: t.signature }));
}
```

Call a confirmed target through `callTool("<upstream>::<tool>", params)` or its confirmed generated helper. Keep return values bounded and decision relevant. Use `codemode.batch()` for independent calls when partial success is useful; run dependent calls in order.

## Authority and recovery

`codemode` executes with the caller's available authority. `codemode_read` admits only tools whose live descriptors are explicitly read only and non-destructive; an absent or contradictory annotation fails closed. The optional `codemode_ui` has the same authority as `codemode`. Narrow a call with the top-level `upstreams` or `tools` allowlist when appropriate.

When a call reports `confirmation_required`, follow its structured recovery guidance. Supply a confirmation field only if the live upstream schema declares one and the user has authorized that action. Before retrying a timed-out or failed mutation, inspect whether it took effect.

Use `codemode.listSkills()`, `codemode.getSkill(uri)`, and `codemode.readSkill(uri)` to inspect skills visible through this gateway. Those skills are separate from the client's installed skill catalog.

Read [references/code-mode.md](references/code-mode.md) for payload examples, action-dispatched upstreams, catalog fields, limits, result shaping, and error recovery.
