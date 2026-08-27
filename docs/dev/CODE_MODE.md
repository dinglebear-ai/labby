---
title: "Code Mode"
created: "2026-07-30"
updated: "2026-08-08"
---

# Code Mode

Code Mode is the JavaScript execution surface behind the MCP `codemode` and
`codemode_read` tools. It lets an agent discover upstream MCP tools, inspect
compact docs, and run one async JavaScript function in a sandbox that can call
the tools allowed by that entry point.

Labby actions are intentionally not exposed through Code Mode. Call Labby built-in
service tools directly when raw tools are visible, or use the native gateway
management/API surfaces for Labby actions.

## Surface

Code Mode has two text MCP entry points with the same input shape:

- `codemode({ code })` is the full execution surface. It requires `lab` or
  `lab:admin` and may invoke write-capable or destructive upstream tools.
- `codemode_read({ code })` is the enforced read-only surface. It accepts
  `lab:read`, `lab`, or `lab:admin`. It exposes only upstream tools whose live
  MCP descriptor explicitly sets `readOnlyHint: true` and
  `destructiveHint: false`. An absent annotation or a descriptor with only
  `destructiveHint: false` is not proof that a tool is read-only.

The catalog checks the standard MCP annotations during discovery. The call path
checks the current live descriptor again immediately before dispatch. A changed
descriptor is rejected and must be rediscovered. The old
`trusted_read_only_tools` configuration field remains accepted for compatibility,
but it no longer controls catalog admission.

The optional `codemode_ui` MCP App entry point has the same full execution
authority as `codemode`; it is not a read-only inspector shortcut. The full
entry points are annotated `readOnlyHint: false`, `destructiveHint: true`,
`idempotentHint: false`, and `openWorldHint: true`. `codemode_read` is annotated
`readOnlyHint: true`, `destructiveHint: false`, `idempotentHint: true`, and
`openWorldHint: true`.

For every entry point, the code runs as one async JavaScript function in the
sandbox. Discovery, focused compact docs, allowed upstream calls, fan-out,
filtering, and final result shaping all happen inside that same execution.

The approval-facing Tool descriptors include the enabled, route-scoped upstream
namespaces and their normalized operator hints. Names and hints come from the
current gateway configuration, are sorted and deduplicated, and therefore
change the descriptor when that configuration changes. Runtime health, tool
counts, and individual tool names stay out of the descriptor; discover those
inside the run with `codemode.search(...)` and `codemode.describe(...)`. The
8192-byte description cap applies after tool-specific suffixes are composed, so
no final Tool description can exceed the host-facing limit.

Inside the sandbox:

- `await codemode.search("GitHub pull requests")` searches the reduced
  in-execution catalog and includes compact intrinsic safety facts when the
  live descriptor supplies an unambiguous fact.
- `await codemode.describe("github.list_pull_requests")` returns compact docs
  for an exact tool or snippet target.
- `await codemode.run("gateway-summary", input)` resolves and runs a snippet
  inside the same sandbox runtime.
- `await codemode.github.list_pull_requests(params)` calls the generated helper.
- `await callTool("github::list_pull_requests", params)` calls the raw bridge.
- `await codemode.readResource("lab://upstream/<name>/<uri>")` reads an
  upstream MCP resource and returns its normal `ReadResourceResult` object.

Resource reads use the same route and caller scoping as Code Mode tool calls.
Native `ui://` widget resources are also supported when the owning upstream is
visible to the current run.

### Local State And Git Providers

Unscoped admin/trusted-local Code Mode also exposes two local sandbox globals:
`state` and `git`. They are not upstream MCP tools and they do not grant host
filesystem or shell access. Route-scoped Code Mode runs do not receive these
globals, and neither does `codemode_read`. Hand-written
`callTool("state::...")` / `callTool("git::...")` calls are denied at dispatch
time on those surfaces. All paths are virtual workspace paths rooted inside
`$LABBY_HOME/code-mode-workspaces/`. Parameters use the documented JavaScript
names; result payloads preserve the existing serialized Rust field names where
those names are already part of the Code Mode contract.

V1 state methods:

- `state.readFile({ path })`
- `state.writeFile({ path, content })`
- `state.list({ path })` / `state.readdir({ path })`; use `"/"` or `"."` for
  the workspace root
- `state.glob({ pattern, limit })`
- `state.searchFiles({ pattern, query, limit })`
- `state.replaceInFiles({ pattern, search, replace, dryRun })`
- `state.planEdits({ edits })`
- `state.applyEditPlan({ planId })`

V1 git methods:

- `git.init({ cwd })`
- `git.status({ cwd })`
- `git.add({ path, cwd })`
- `git.commit({ message, authorName, authorEmail, cwd })`
- `git.log({ limit, cwd })`
- `git.diff({ path, cwd })`

V2 state methods add:

- `state.appendFile({ path, content })`
- `state.exists({ path })`
- `state.stat({ path })`
- `state.mkdir({ path })`
- `state.rm({ path, recursive })`
- `state.cp({ from, to })` for files
- `state.mv({ from, to })`
- `state.walkTree({ path, limit })` / `state.summarizeTree({ path, limit })`
- `state.readJson({ path })`
- `state.writeJson({ path, value, pretty })`
- `state.hashFile({ path, algorithm: "sha256" })`
- `state.detectFile({ path })`
- `state.archiveCreate({ source, destination })` to a `.tar` destination
- `state.archiveList({ path, limit })`

V2 git methods add:

- `git.branch({ name, delete, list, cwd })`; omit `name` or pass `list: true`
  to list branches
- `git.checkout({ ref, create, cwd })`
- `git.remoteList({ cwd })` returns `stdout` and structured `remotes`
- `git.remoteAdd({ name, url, cwd })`
- `git.remoteRemove({ name, cwd })`
- `git.clone({ url, directory, cwd })`

Remote git URLs must be explicit `https://github.com/...` URLs without embedded
credentials. Labby does not inject hidden credentials or host git config into
Code Mode. Use `cwd` to run git commands inside a workspace-relative child repo,
for example after cloning into `directory: "repo"`. Clones are shallow
(`--depth 1`). V2 does not expose `fetch`, `pull`, or `push`; those remote
mutation methods are deferred until Code Mode has an explicit transaction and
credential model for them.

### OpenAPI Provider (`openapi`)

`openapi` is the third local provider. It turns an operator-configured OpenAPI
spec into locally-dispatched, LLM-callable operations. Unlike `state`/`git`, it
performs outbound HTTP — through the isolated `labby-openapi` crate's OWN hardened
client, never through a sidecar MCP server.

**JS API** (flat, non-discoverable in v1 — `codemode.search` does NOT list
`openapi` operations):

```ts
async () => {
  // openapi.call(label, operationId, params)
  const user = await openapi.call("vendor", "getUser", { id: "7" });
  return user;
}
```

`params` supplies path-template values (substituted, PATH_SEGMENT-encoded) plus
either query params (GET/HEAD/DELETE) or a JSON body (POST/PUT/PATCH). The JS
snippet never sees the credential — it is injected server-side after the sandbox
boundary.

**Config.** Non-secret fields in `config.toml`; credentials in `.env`
(`OPENAPI_<LABEL>_TOKEN` → `Authorization: Bearer`, or `OPENAPI_<LABEL>_API_KEY`
→ a header named by `api_key_header`, default `X-API-Key`). `base_url` is
**mandatory** — `rmcp-openapi` never reads the spec's `servers[]`.

```toml
[[openapi.specs]]
label = "vendor"
base_url = "https://api.vendor.example.com"     # MANDATORY, SSRF-validated
spec_url = "https://api.vendor.example.com/openapi.json"  # or spec_path = "..."
api_key_header = "X-API-Key"                      # optional
allowed_operations = ["getUser", "listUsers"]     # deny-by-default allowlist
```

**Gate.** Three layers, all required: the admin+unscoped local-provider gate
(same as `state`/`git`), a mandatory deny-by-default per-operation allowlist
(operations not listed are never dispatched), and SSRF containment.

**SSRF containment.** The base URL is validated at load time via the canonical
`labby_primitives::ssrf` guard (https-only, rejects loopback / link-local /
RFC1918 / CGNAT / private-TLD). At request time the outbound client disables
redirects, forces `https_only`, resolves + validates every IP, pins one validated
address, and re-checks the connected peer IP — closing the redirect-bypass and
DNS-rebinding gaps. Each dispatch emits exactly one structured event on both the
success and failure path — `service`, `action` (operationId), `label`, `host`,
`method`, `status` (`ok`/`error`), `elapsed_ms`, plus `kind` on failure — never a
third-party response body, a query-with-auth, or a credential.

**Refresh.** Specs load once at process start (concurrently, per-spec timeout,
4 MiB body-size cap). A spec that fails to load is omitted with a WARN;
`labby serve` still reaches ready. There is no background refresh in v1.

**Deferred follow-ups (v1):** discovery-catalog integration (which would
re-introduce per-operation `input_schema`, per-op JS proxies, and operationId→JS
sanitization), background `ArcSwap` refresh, per-spec rate/concurrency caps, and
apiKey-in-query / apiKey-in-cookie injection (header-style only in v1).
Connection pooling across dispatches is also deferred: because `resolve_to_addrs`
pins the validated IP at client-build time, v1 builds a fresh pinned client per
request (no keep-alive reuse across calls). Pooling would require replacing the
per-call pin with a custom `reqwest::dns::Resolve` that validates every resolved
IP on a single shared client, keeping the post-connect peer re-check as the
TOCTOU backstop — a change to the SSRF-critical path deferred out of v1.

Example:

```ts
async () => {
  const matches = await codemode.search({ query: "GitHub pull requests", limit: 1 });
  const docs = await codemode.describe(matches.results[0].path);
  const pulls = await codemode.github.list_pull_requests({ state: "open" });
  return {
    docs: docs.path,
    open: pulls.items.map(pr => ({ number: pr.number, title: pr.title }))
  };
}
```

`Promise.all([...])` and `Promise.allSettled([...])` fan out independent upstream
calls. A failed `callTool` rejects only that promise; catch locally when partial
success is useful.

Synthetic Code Mode exposes the fixed Labby-owned entry points instead of raw
upstream tools. Discovery, schema inspection, tool calls, and intermediate
values stay inside one sandbox execution.

## Snippets

Snippet metadata appears in `codemode.search()` and `codemode.describe()` for
trusted-local or `lab:admin` callers. Snippets are listed as `kind: "snippet"`
and are invoked through the single helper:

```ts
async () => {
  const found = await codemode.search("snippet gateway");
  const docs = await codemode.describe(found.results[0].id);
  const summary = await codemode.run("gateway-summary", { includeHealth: true });
  await writeArtifact("gateway-summary.json", JSON.stringify(summary, null, 2), {
    contentType: "application/json"
  });
  return { docs: docs.path, summary };
}
```

`codemode.run()` lazily resolves snippet source through the host, then evaluates
`return await (<snippet-code>)(input)` inside the same Javy/QuickJS runtime as the
caller. A snippet can call `codemode.<upstream>.<tool>()`, `callTool()`,
`writeArtifact()`, and other snippets, bounded by the same Code Mode timeout plus
per-run snippet depth/count/byte budgets.

`writeArtifact()` defaults `contentType` to `text/plain` when omitted or blank.
When provided, it must be a simple ASCII `type/subtype` media type, up to 256
bytes after trimming surrounding ASCII spaces.

Snippet execution is admin/trusted-local only. Route-scoped Code Mode catalogs do
not expose user snippets, and host-side snippet resolution repeats the permission
check because discovery is not a security boundary.

Successful Code Mode executions return an `execution_id`. Admin callers can
promote the live process's retained source into a user snippet through the
`snippets` service:

```json
{
  "action": "snippets.promote",
  "params": {
    "execution_id": "01JEXAMPLE",
    "name": "gateway-summary",
    "description": "Summarize gateway health",
    "confirm": true
  }
}
```

Promotion source is deliberately ephemeral and live-gateway scoped. It is stored
only in memory, is evicted by retention limits, and disappears after restart,
deploy, or a different gateway process handles the promotion request. Promoted
source is written as plaintext executable snippet content under the user snippet
directory and may contain anything the original Code Mode source contained.

> **Persistence caveat.** Promotion writes the source **verbatim and unredacted**
> as a plaintext file on disk (`$LABBY_HOME/snippets/<name>.md`, subject to the
> process umask). If the original Code Mode source embedded a literal secret,
> token, or captured credential, that value is now persisted in cleartext and
> survives restarts until the snippet is removed. Promotion is `destructive: true`
> (MCP elicitation when available; HTTP/CLI use their own confirmation surfaces)
> precisely because it is a persistence action — do not promote sources that
> carry inline secrets; pass them through snippet `input`/params at run time instead.

## Tool IDs and Helpers

Upstream tool IDs use:

```text
<upstream-name>::<tool-name>
```

`codemode` injects a runtime proxy generated from the live readable catalog, so
`codemode.github.search_issues(params)` calls the same bridge as:

```ts
callTool("github::search_issues", params)
```

Legacy `search` entries include both raw JSON Schemas and generated TypeScript:

- `schema` — input JSON Schema.
- `output_schema` — output JSON Schema when the upstream tool declares one.
- `signature` — one-line TypeScript call signature.
- `dts` — focused TypeScript declarations with JSDoc for that tool.

The `codemode.search` helper uses a reduced in-execution catalog (`kind`, `id`,
`path`, `namespace`, `name`, `helper`, `description`, `signature`, `tags`,
snippet `inputs`, and optional tool `safety`)
so normal runs do not inject full schema, output schema, dts payloads, or snippet
source. `safety.read_only` and `safety.destructive` are optional advisory facts:
unknown or contradictory facts are omitted, while explicit `false` hints remain `false`,
and snippets omit `safety` because they are composite programs. These facts do
not grant access, request approval, or replace the live descriptor and policy
checks immediately before dispatch. When a schema is missing or too complex for
the TypeScript emitter, generated signatures fall back to `unknown`.

### Authenticated Web tool browser

The Gateway Admin UI exposes `/tools` for authenticated operators carrying
`lab:admin`. Its private `POST /v1/gateway/codemode/tools/search` and
`POST /v1/gateway/codemode/tools/describe` routes project the same live,
scope-filtered descriptors used by Code Mode without executing JavaScript or
calling an upstream tool. Search responses are capped at 256 KiB and describe
responses at 128 KiB; the browser reads response streams incrementally and
cancels them immediately when either cap is exceeded, including when a server
omits `Content-Length`. The static page contains no catalog fixture or tool
definition, and safety metadata remains advisory rather than dispatch authority.

### Builtin services as in-process peers

Builtin Labby services (`gateway`, `doctor`, `setup`, …) join the Code Mode
catalog as in-process upstream peers under synthetic
`__in_process__<service>` namespaces, so the dispatch-envelope
`outputSchema` and the callable capability arrive together (issue #210
FU-1). Registration is lazy, single-flighted, and idempotent — it happens
on the root-scope catalog build, with a cooldown so a failing peer is not
retried on every call. Protected MCP routes never see them: the
`__in_process__` prefix is **rejected at config load** both as an upstream
name and in a route's `target.upstreams`, so the exclusion is enforced,
not merely conventional.
Authorization matches ordinary upstream tools: builtin peers carry no
annotations, so they fail closed as destructive (Code Mode execute
permission required) and are excluded from read-only Code Mode unless
operator-trusted. Each peer serves exactly its own service, pinned to Raw
mode regardless of the process-wide Code Mode flag.

The peer transport carries no `AuthContext` — there is no HTTP layer to
inject one — so it does **not** inherit the stdio trust model. Actions
marked `requires_admin`, and the stdio-only `setup` actions, are refused
over this transport; only non-admin actions are reachable through Code
Mode. Without that guard a caller holding just `lab` (enough for Code Mode
execute, deliberately not enough for admin) could reach admin builtins.

## Catalog Freshness

Code Mode does not build or read a durable vector, lexical, or RRF index. Each
`codemode` execution projects a transient catalog from the gateway runtime and
refreshes enabled upstream tool metadata through the gateway manager before
building the local discovery helpers and runtime proxy. Legacy `search` uses the
same catalog source, so helper visibility and direct `callTool` routing stay
aligned.

`gateway.reload` swaps in a freshly seeded lazy upstream pool. The next Code Mode
execution or compatibility catalog call reprobes the relevant live upstreams and
should see tool-list changes such as the agent-workstation Windows-MCP `PowerShell`,
`FileSystem`, `Snapshot`, and `Wait` tools without requiring a process restart.

## Catalog Drift Diagnostics

When search results do not match live execution, check the layers in order:

1. Gateway runtime:

   ```bash
   labby gateway list --json
   ```

   Confirm the upstream reports the expected discovered tool count and is not
   carrying a tools-capability error.

2. Code Mode `codemode` proxy:

   ```ts
   async () => Object.keys(codemode.agent_os_windows_mcp).sort()
   ```

   For agent-workstation, the list should include `PowerShell`, `FileSystem`, `Snapshot`,
   and `Wait`.

3. Direct callability:

   ```ts
   async () => callTool("windows_windows-mcp::PowerShell", {
     command: "Write-Output MCP_OK"
   })
   ```

   If this succeeds while search is stale, the upstream is callable and the
   issue is catalog visibility rather than tool execution.

4. MCP legacy `search` injected catalog:

   ```ts
   async () => tools
     .filter(t => t.upstream === "windows_windows-mcp")
     .map(t => t.name)
     .sort()
   ```

   Missing `PowerShell`, `FileSystem`, or `Snapshot` here after layers 1-3 are
   fresh indicates Code Mode catalog freshness drift in the active MCP session.
   Run `gateway.reload` once to swap the runtime pool; if the same MCP session
   still sees stale search results while execute is fresh, reconnect that MCP
   client session so it receives the current gateway manager state.

`codemode` accepts optional `upstreams` and `tools` arrays to narrow the per-run
capability set. When present, each filter must be a JSON array of strings; other
shapes reject with `invalid_param`. Empty strings are ignored. The injected proxy only
includes allowed tools, and direct `callTool` IDs outside the allowlist reject as
`unknown_tool`.

## Result Contract

Successful upstream tool calls resolve to the payload, never the raw MCP
`CallToolResult` envelope. The unwrap precedence is a locked contract
([mcp-tool-output.md §C6](../contracts/mcp-tool-output.md)) — byte-identical
since 2026-05-31 and pinned by an edge-case test matrix. First match wins:

0. `isError: true` is handled before the unwrap and surfaces in-sandbox as a
   thrown `CodeModeCallError`.
1. `structuredContent` when **present** — including falsy JSON values
   (`false`, `0`, `null`, `""`). Presence, not truthiness; content blocks are
   discarded (the mcp-ui link is read from `_meta`, not content).
2. Otherwise, when every content block is text: **all** blocks joined with
   `"\n"`, then a single JSON parse; on parse failure, the joined string.
3. Otherwise, empty content resolves to `null`.
4. Otherwise (mixed/binary content): the entire `CallToolResult` as JSON,
   including the upstream's `_meta` — a deliberate, upstream-controlled
   exposure.

`codemode` returns a capped envelope with:

- `result` — the JavaScript function return value.
- `calls[]` — lightweight per-call metadata: `id`, canonical `namespace`,
  `tool`, `ok`, `elapsed_ms`, redacted/capped `params` when tracing is enabled,
  and `error_kind` on failure. Older UI parsers may still accept `upstream` as a
  compatibility alias, but new producers and tests use `namespace`.
- `logs[]` — sandbox console output when available.

The Code Mode inspector accepts execute/search/history traces from the initial
global, ExtApps bridge, or OpenAI Apps `window.openai.toolOutput`. It drops
malformed rows with a warning, displays at most 50 calls/matches/history rows per
section, and stringifies params/results only after the user opens that details
panel.

Binary-like JavaScript values crossing the runner boundary use a tagged base64
codec. JavaScript return values (`ArrayBuffer` and typed-array views) are encoded
as JSON:

```json
{ "__labBinary": "base64", "type": "Uint8Array", "data": "AQL/" }
```

Tagged binary values received from the parent bridge are decoded back to
`ArrayBuffer` or `Uint8Array` inside the sandbox. Mixed or binary MCP content
blocks that are not unwrapped as `structuredContent` or all-text content remain in
their JSON MCP representation.

Defaults:

- `max_source_bytes = 131072`
- `max_response_bytes = 24576`
- `max_response_tokens = 6000`

### Final Result Shaping

Code Mode can optionally shape the final model-facing `result` of a successful
execution. This is disabled by default.

Ordering:

1. The sandbox finishes and returns the raw final value.
2. Labby applies the existing `__ui` compatibility unwrap.
3. Labby applies the configured final-result shaping policy.
4. Labby applies the envelope budget truncation.
5. MCP text JSON and `structuredContent` are built from the same shaped response.

This does not change values seen by sandbox code through `callTool()` or
`codemode.<upstream>.<tool>()`. It also does not add raw-result audit retention.
Use `writeArtifact()` when a snippet needs to preserve a large detailed payload.

The `truncate` policy bounds model-facing output; it is not a redaction policy
and must not be used to sanitize secrets.

Two distinct truncation markers exist, and structure survives both — nothing
is ever stringified-and-reparsed beyond the marker itself:

- **Envelope budget (always on, default path):** when the response exceeds
  `max_response_bytes`/`max_response_tokens`, the final `result` is replaced
  with an **object** marker carrying `truncated: true`, `original_size`,
  `original_tokens`, a bounded `preview`, `artifacts`, and `next_action`.
  Structured `calls[]` metadata survives verbatim. Logs are trimmed
  oldest-first after result truncation if needed.
- **Shaping policy `truncate` (opt-in, non-`Off` policy only):** the final
  result becomes a single marker **string** prefixed
  `[code mode result truncated]` with a pretty-printed preview.

Truncation happens only at the outer sandbox→MCP boundary. Values seen by
sandbox code through `callTool()` / `codemode.<upstream>.<tool>()` are never
truncated or reshaped.

## MCP Apps (mcp-ui) widgets

An upstream tool can return a native MCP Apps (mcp-ui) widget by carrying
`_meta.ui.resourceUri` (a `ui://<upstream>/...` URI served as
`text/html;profile=mcp-app`). Inside `execute`, the unwrapped `callTool` payload
drops that envelope metadata, so a widget would otherwise collapse to plain JSON.

When a snippet calls a widget-bearing upstream tool, `codemode` surfaces the most
recent captured widget metadata on the final tool result. The caller can also
return an object with a `__ui` key to unwrap a specific payload shape while
rendering the captured widget:

```ts
async () => {
  const dashboard = await codemode.axon.status_dashboard({});
  return { __ui: dashboard };   // optional: render the widget; surface `dashboard` as the result
}
```

Semantics:

- **Last-wins.** The broker records the most recent widget-bearing upstream call
  during the run; that link is the one surfaced. If the final return value uses
  `{ __ui: <result> }`, `<result>` is unwrapped into the execute `result` field
  so the model still sees the payload.
- **Native URIs.** The widget's `ui://<upstream>/...` URI is preserved verbatim.
  The gateway routes a `resources/read` of that URI to the owning upstream peer
  via catalog reverse-lookup (it is **not** rewritten to `lab://upstream/...`).
  `ui://lab/code-mode/*` remains reserved for Labby's own Code Mode app resources.
- **Identical mirroring.** The execute `CallToolResult` carries the upstream's
  `_meta.ui` object verbatim, so the host renders the widget identically to a
  direct connector. The widget itself is driven by the `ui://` resource read, not
  by inline content, so the execute trace content is left intact.
- The `CodeModeExecutionResponse` gains an optional `ui` field when a
  widget-bearing upstream result was captured.

### Widget → host callbacks

While synthetic Code Mode is active, raw upstream tools stay hidden from
`tools/list`, including tools that carry `_meta.ui.resourceUri`. Upstream health
therefore cannot add or remove approval-facing callback actions. The advertised
MCP App actions are the fixed Labby-owned surface (`codemode_ui`, `mcp_app`, and
the configured Labby-owned admin apps), not raw upstream callbacks.

An upstream widget returned by a Code Mode execution can still render through
the native resource URI mirrored in the result. Rendering that resource does
not advertise its server's callback tools.

A rendered MCP App can call back to its server only through host
`callServerTool` / `tools/call`. Labby allows those callback calls through Code
Mode's raw-tool gate only when all of these are true:

- the requested tool is an exposed upstream tool, not a Labby built-in service;
- the upstream is routable and allowed by the current protected route scope;
- the same upstream exposes at least one MCP App UI tool;
- the requested tool is not destructive.

The callback exemption changes callability only. It does not put sibling tools
back into `list_tools`, so the model-facing surface remains collapsed.
Destructive sibling callbacks are refused with `forbidden` for callers who lack
Code Mode execute permission; a caller with execute permission may call them
directly with no separate confirmation step (see "Destructive tool calls"
below).

`LABBY_CODE_MODE_WIDGET_CALLBACKS=1` remains as a broader legacy operator bypass.
With that variable set, any known exposed non-destructive upstream tool may pass
the raw-tool gate while Code Mode is enabled. Leave it off unless a legacy widget
depends on callbacks that cannot be represented by the same-upstream MCP App
sibling rule.

## Error Contract

Tool errors reject with a JSON-encoded string that can be decoded in the sandbox:

```ts
try {
  await callTool("github::search_issues", {});
} catch (e) {
  const env = JSON.parse(String(e.message));
  return env.kind;
}
```

Canonical error kinds:

| Kind | Bucket | Meaning |
| --- | --- | --- |
| `missing_param` | Fix and retry | Required input was absent. |
| `invalid_param` | Fix and retry | Input shape or type is invalid, including non-object upstream params. |
| `invalid_code_mode_id` | Fix and retry | Code Mode tool id parsing failed; valid ids are `<upstream-name>::<tool-name>` only. |
| `validation_failed` | Fix and retry | Nested schema validation failed. |
| `unknown_tool` | Fix and retry | Tool id is unknown or outside this run's route scope. |
| `unknown_action` / `unknown_subaction` | Fix and retry | Action id is not exposed by the upstream dispatcher. |
| `route_scope_denied` | Terminal | Protected-route policy denied the upstream/tool. |
| `forbidden` / `permission_denied` | Terminal | Caller lacks permission, including destructive tool execution permission. |
| `path_traversal` | Terminal | Path-safety checks rejected a workspace or artifact path. |
| `quota_exceeded` / `budget_exceeded` / `call_budget_exceeded` | Retry with smaller work | Workspace, response, or call fan-out budget was exceeded. |
| `result_too_large` / `artifact_too_large` | Retry with smaller output | Returned value or artifact exceeded configured caps. |
| `timeout` | Retry with smaller work | The live QuickJS/Javy runner wall-clock backstop interrupted execution. |
| `rate_limited` | Retry later | Upstream or host-side rate limit was hit. |
| `network_error` / `server_error` / `decode_error` / `upstream_error` | Retry or operate upstream | Upstream transport, protocol, server failure, or unknown structured upstream-local kind. Unknown structured upstream kinds are returned as `upstream_error` without poisoning upstream health. |
| `auth_failed` / `oauth_needs_reauth` | Reauthenticate | Upstream credentials are absent or rejected. |
| `snippet_not_found` | Fix and retry | Requested snippet name does not exist. |
| `internal_error` | Bug or unsupported state | Unexpected host/runner failure. |

`code_mode_fuel_exhausted` is **not** emitted on the live path; it belongs to
the dead Wasmtime reference engine and is normalized away by the host.

## Destructive tool calls

Destructive upstream tools on the full surface are gated by host-side metadata
(`destructive_permitted` in `labby-codemode`'s `types.rs`) before dispatch, and
by nothing else. Full Code Mode execution is itself scope-gated — a caller needs
`lab` or `lab:admin` to reach `codemode` or `codemode_ui` — so there is no
additional per-call confirmation or pause step on top of that. Concretely:

- **MCP:** an execute-capable caller (`lab` or `lab:admin`) may call any
  destructive upstream tool from Code Mode with no separate confirmation. A
  caller without execute permission is refused with `forbidden` before dispatch.
- **Read-only MCP:** `codemode_read` never admits a tool unless its descriptor is
  explicitly read-only. Discovery filters the catalog, and invocation checks the
  live descriptor again immediately before dispatch. `writeArtifact`, local
  `state`/`git` providers, and other write paths are unavailable.
- **CLI:** Code Mode execution is operator-driven and always execute-capable,
  so destructive upstream calls are permitted unconditionally.

Code Mode keeps a best-effort **append-only step journal** and read-only notebook
projection for each `codemode.step(name, fn)` boundary (owner-scoped and
redacted at rest). The callback executes in the current run; current runtime
code never reads an older journal row to resume or replay it. Persistence is
detached after the response, so a successful Code Mode response is not proof
that the journal flush completed.

The `codemode` MCP tool has **no** `resume_token` or `confirm` parameter and no
pause/resume/reject mechanism. The journal is orthogonal to dispatch and never
interrupts, gates, or confirms a running snippet. This preserves the permanent
decision to remove the destructive-call pause gate — the journal is a record,
not a gate. A caller that can invoke a full Code Mode entry point can call
destructive tools immediately. The separate `codemode_read` boundary is an
enforced capability restriction, not a pause/confirm gate.

## Scope

- `lab` or `lab:admin` can use `codemode`.
- `lab` or `lab:admin` can use `codemode_ui` when the app surface is enabled.
- `lab:read`, `lab`, or `lab:admin` can use `codemode_read`.

OAuth callers retain their subject attribution when Code Mode calls upstream tools.
Trusted local callers use the shared gateway subject.

## Runner Architecture

The stdio parent-broker protocol is:

1. Parent starts (or reuses a pooled) `labby internal code-mode-runner` process.
2. Parent sends a `start` line; the child builds a FRESH QuickJS runtime and
   evaluates the normalized async function.
3. Child emits `tool_call` lines for `callTool` requests.
4. Parent dispatches through the gateway broker and replies with `tool_result` or
   `tool_error`.
5. Child settles pending promises and emits `done`.
6. The child then resets and parks for the next `start` (warm-runner pool).

### Warm-runner pool

The runner **process** is pooled and long-lived; the **JS runtime is rebuilt for
every execution**. Pooling amortizes the dominant fixed cost (process fork +
startup) without ever sharing JS state across callers — a brand-new runtime has
no globals, no leftover pending tool calls, and no captured data from a prior
run, so isolation holds by construction.

- **Process reuse, fresh runtime.** A pooled runner loops: read `start` → build a
  fresh `javy::Runtime` → run → emit `done`/`error` → reset and read the next
  `start`. It exits only when the parent closes stdin.
- **Per-execution isolation.** Each run resets the `callTool` sequence counter and
  creates a fresh, empty per-execution QuickJS working-directory jail (removing
  the prior one), so a long-lived process never accumulates JS runtime state
  across callers. Code Mode's `state.*` and `git.*` local providers deliberately
  use a separate persistent workspace under `LABBY_HOME/code-mode-workspaces/`;
  persistence is scoped to that workspace and guarded by virtual path, symlink,
  quota, archive, and git remote restrictions. The 64 MiB heap, 30 s wall-clock
  timeout, and stack limit are enforced per execution.
- **Bounded pool, one execution per runner.** `N` runners serve `N` concurrent
  executions. When all are busy, an extra request is served by a bounded
  ephemeral (overflow) runner rather than queueing unboundedly.
- **Robustness.** If a pooled runner exits before emitting any valid protocol
  event, it is killed and the execution is replayed once on a guaranteed-fresh
  ephemeral runner; at that boundary no host-visible side effect can have run.
  A crash after protocol activity, timeout, or protocol violation is killed and
  surfaces a clean error without replay (`timeout` on wall-clock expiry). A
  pooled runner is also recycled after a fixed number of executions as cheap
  insurance against native-side leaks. External `callTool` operations reserve a
  250 ms result-ack window inside the same per-execution wall-clock budget when
  at least twice that budget remains, so host work cannot consume the runner's
  acknowledgement budget without materially shortening normal calls. The
  separate hung-runner watchdog remains 5 seconds. After the final tool result is
  relayed, the runner gets up to that 5-second grace to emit `done`/`error`, capped by the
  overall execution deadline. Only expiry of the full dedicated grace is reported
  as a runner settlement timeout; an earlier outer deadline remains an ordinary
  Code Mode timeout. Very short executions that cannot fit the reserve keep their
  original tool deadline. Reserve activation is logged once per execution at
  `DEBUG` with `action = "codemode.result_ack.reserve"`, `event = "armed"`, and
  the monotonic `result_ack_reserve_use_count`. A genuine full-grace watchdog
  expiry is logged at `WARN` with `action = "codemode.settlement"`,
  `event = "watchdog_expired"`, and `settlement_watchdog_expiry_count`. These
  process-wide counters remain local observability data and are not returned to
  Code Mode callers.
- **Configuration / kill switch** (environment, read at startup):
  - `LABBY_CODE_MODE_POOL_SIZE` — number of pooled runners (default `2`, clamped to
    `16`). **`LABBY_CODE_MODE_POOL_SIZE=0` disables pooling entirely**, falling back
    to spawn-per-execution with behavior identical to the pre-pool path.
  - `LABBY_CODE_MODE_POOL_RECYCLE_AFTER` — executions before a runner is recycled
    (default `100`).
  - `LABBY_CODE_MODE_POOL_MAX_OVERFLOW` — cap on simultaneous ephemeral overflow
    runners (default `8`).

### Microsandbox runner isolation (opt in)

Linux hosts with KVM may place each pooled runner process inside a Microsandbox
microVM while retaining the same Javy/QuickJS engine and JSON-lines broker
protocol:

```bash
LABBY_CODE_MODE_RUNNER_BACKEND=microsandbox
LABBY_CODE_MODE_MICROSANDBOX_EXE=/absolute/path/to/msb
LABBY_CODE_MODE_MICROSANDBOX_IMAGE=debian@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
LABBY_CODE_MODE_MICROSANDBOX_MAX_RUNNERS=4
```

All three variables are required to opt in. The image must be an immutable
`name@sha256:<64 hex>` OCI digest reference and must already be cached; URLs,
userinfo, queries, and tag-only references are rejected. The runner uses
`--pull never` and never performs an implicit registry fetch.
Labby creates one restricted microVM per runner process with one CPU, 256 MiB
memory and no network. A process-wide admission gate defaults to four concurrent
guests and is capped at 16, independently of the generic process-pool limits.
Its lifecycle is tied to the pooled runner rather than a
short VM idle timer, so a parked warm runner cannot expire between ordinary
requests. A 24-hour hard lifetime bounds orphaned guests after ungraceful host
death; a live pool recycles its runner before reaching that backstop.
The validated Labby runner executable is the only host file mounted,
read-only at `/opt/labby/labby`. The parent attaches with `msb exec --stream`,
which preserves the existing byte-faithful stdio protocol. Dropping or evicting
the runner attempts a bounded force-removal; failure is logged and triggers a
separately bounded best-effort fallback.
Each guest carries the `labby.owner=codemode` label. Startup removes stale
labeled guests before admitting work, and graceful service shutdown awaits pool
drainage. An unconfirmed cleanup opens a fail-closed creation circuit. Before a
later creation is refused, Labby rechecks every failed cleanup for that `msb`
executable against the live labeled guest inventory. Proven-absent guests clear
the ledger and repair active-runner accounting; still-live guests get a bounded
force-removal retry. If inventory or removal cannot be proven, creation remains
fail closed. This prevents transient cleanup failures from poisoning the process
for the rest of its lifetime without weakening the cleanup boundary.

This changes only the execution boundary. Tool discovery, authorization,
exposure filters, OAuth subjects, secrets, upstream dispatch, result caps, and
telemetry remain in the Labby host process. No gateway credential is injected
into the guest. `process` remains the default backend and the immediate rollback
value for `LABBY_CODE_MODE_RUNNER_BACKEND`.

Before enabling the backend, verify `msb doctor`, read/write `/dev/kvm` access,
the pinned cached image, and dynamic-library compatibility between the host
runner binary and the guest image. Host-service install/restart performs an
additional pre-stop image preflight: legacy mutable aliases are resolved from
the service user's existing cache, registered under the canonical immutable OCI
reference, persisted, and re-verified before systemd is allowed to restart the
service. A hardened Incus deployment must explicitly pass `/dev/kvm` into the
guest and install `msb`/`libkrunfw`; Labby does not weaken the container or
install runtime dependencies at request time.

  The conservative default (`size = 2`) keeps idle memory bounded while absorbing
  typical `codemode` bursts. The security invariants (`env_clear`,
  process-group/Job-Object reaping, `kill_on_drop`, `PR_SET_DUMPABLE`) are set
  once at spawn and therefore hold for the pooled process's whole lifetime.

Code Mode always uses Javy/QuickJS for snippet execution — it is the **sole live
engine**, with no Boa fallback and no `code_mode_wasm` feature. `codemode` runs
in the Javy/QuickJS child runner over stdio. The Javy toolchain is pulled in by
the `gateway` feature.

The direct-process runner starts with an empty environment in a temporary
directory. With the Microsandbox backend, `env_clear()` applies to the host
`msb` transport; no Labby host environment or gateway credentials are forwarded,
while the guest may retain image/runtime defaults. The Javy runner additionally
executes inside the no-network guest. It does not
provide Node, Deno, Bun, `fetch`, `connect`, `XMLHttpRequest`, `require`, or host
module `import()` access. `callTool` is the only host bridge exposed to user code.

> **Wasmtime is dead reference code, not a live path.** `wasm_runner.rs` is an
> unused engine skeleton retained only for reference; nothing on the live Code
> Mode path constructs or runs it. Its fuel/epoch-interruption design would
> normalize fuel and timeout traps to `code_mode_fuel_exhausted` and
> `code_mode_timeout`, but because the skeleton never executes, **neither kind is
> emitted today.** The only budget kind a caller observes on the live
> Javy/QuickJS path is `timeout` (the wall-clock backstop). Treat
> `code_mode_fuel_exhausted` / `code_mode_timeout` as reserved-for-the-dead-path
> and do not switch-case on them as live outcomes.

Loose JavaScript snippets are normalized before execution. Already-formed
function expressions pass through, while statement blocks such as
`const x = await callTool(...); x.items` are wrapped as `async () => { ... }` and
the trailing expression is returned.
