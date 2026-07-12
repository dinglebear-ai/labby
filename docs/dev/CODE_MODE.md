# Code Mode

Code Mode is the JavaScript execution surface behind the MCP `codemode` tool. It
lets an agent discover upstream MCP tools, inspect compact docs, and run one async
JavaScript function in a sandbox that can call those upstream tools.

Lab actions are intentionally not exposed through Code Mode. Call Lab built-in
service tools directly when raw tools are visible, or use the native gateway
management/API surfaces for Lab actions.

## Surface

Code Mode's primary MCP surface is `codemode({ code })`. The code runs as one
async JavaScript function in the sandbox. Discovery, focused compact docs,
upstream calls, fan-out, filtering, and final result shaping all happen inside
that same execution.

Inside the sandbox:

- `await codemode.search("GitHub pull requests")` searches the reduced
  in-execution catalog.
- `await codemode.describe("github.list_pull_requests")` returns compact docs
  for an exact tool or snippet target.
- `await codemode.run("gateway-summary", input)` resolves and runs a snippet
  inside the same sandbox runtime.
- `await codemode.github.list_pull_requests(params)` calls the generated helper.
- `await callTool("github::list_pull_requests", params)` calls the raw bridge.

### Local State And Git Providers

Unscoped admin/trusted-local Code Mode also exposes two local sandbox globals:
`state` and `git`. They are not upstream MCP tools and they do not grant host
filesystem or shell access. Route-scoped Code Mode runs do not receive these
globals, and hand-written `callTool("state::...")` / `callTool("git::...")`
calls are denied at dispatch time. All paths are virtual workspace paths rooted
inside `$LABBY_HOME/code-mode-workspaces/`. Parameters use the documented
JavaScript names; result payloads preserve the existing serialized Rust field
names where those names are already part of the Code Mode contract.

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

The gateway exposes only `codemode`. Discovery, schema inspection, tool calls,
and intermediate values stay inside one sandbox execution.

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
> (elicitation / `confirm:true` gated) precisely because it is a persistence
> action — do not promote sources that carry inline secrets; pass them through
> snippet `input`/params at run time instead.

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
`path`, `upstream`, `name`, `description`, and `signature`) so normal runs do not
inject full schema, output schema, dts payloads, or snippet source. When a schema
is missing or too complex for the TypeScript emitter, generated signatures fall
back to `unknown`.

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
`CallToolResult` envelope:

1. `structuredContent` when present.
2. Otherwise the first text content block, parsed as JSON when possible.
3. Otherwise raw text, `null`, or non-text content blocks as JSON.

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

When the envelope is too large, the final `result` is replaced with a truncation
marker containing `truncated`, `original_size`, `original_tokens`, `preview`, and
`next_action`. Logs are trimmed after result truncation if needed.

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
  `ui://lab/code-mode/*` remains reserved for Lab's own Code Mode app resources.
- **Identical mirroring.** The execute `CallToolResult` carries the upstream's
  `_meta.ui` object verbatim, so the host renders the widget identically to a
  direct connector. The widget itself is driven by the `ui://` resource read, not
  by inline content, so the execute trace content is left intact.
- The `CodeModeExecutionResponse` gains an optional `ui` field when a
  widget-bearing upstream result was captured.

### Widget → host callbacks

While the synthetic `codemode` surface is active, raw upstream tools stay hidden
from `list_tools`; the public Code Mode MCP surface is the single `codemode`
tool. MCP App tools that carry `_meta.ui.resourceUri` may still be advertised so
the host can render the widget.

A rendered MCP App can call back to its server only through host
`callServerTool` / `tools/call`. Lab allows those callback calls through Code
Mode's raw-tool gate only when all of these are true:

- the requested tool is an exposed upstream tool, not a Lab built-in service;
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

Destructive upstream tools are gated by host-side metadata (`destructive_permitted`
in `labby-codemode`'s `types.rs`) before dispatch, and by nothing else. Code
Mode execution is itself scope-gated — a caller needs `lab` or `lab:admin` to
reach the `codemode` tool at all — so there is no additional per-call
confirmation or pause step on top of that. Concretely:

- **MCP:** an execute-capable caller (`lab` or `lab:admin`) may call any
  destructive upstream tool from Code Mode with no separate confirmation. A
  caller without execute permission is refused with `forbidden` before dispatch.
- **CLI:** Code Mode execution is operator-driven and always execute-capable,
  so destructive upstream calls are permitted unconditionally.

Code Mode persists a **durable, read/replay-only step journal** of every
`codemode.step(name, fn)` boundary (append-only, owner-scoped, redacted at
rest). It has **no** `resume_token` and **no** `confirm` parameter on the
`codemode` MCP tool, and **no** pause/resume/reject mechanism: the journal is
orthogonal to dispatch and never interrupts, gates, or confirms a running
snippet. This preserves the permanent decision to remove the destructive-call
pause gate — the journal is a record, not a gate. A caller that can invoke
`codemode` at all can call destructive tools immediately. Do not reintroduce a
pause/confirm gate on top of Code Mode dispatch.

## Scope

- `lab` or `lab:admin` can use `codemode`.

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
- **Robustness.** A runner that crashes, times out, or violates the protocol is
  killed and replaced (the failing run surfaces a clean error — `timeout` on
  wall-clock expiry — never a hang). A pooled runner is also recycled
  (killed + respawned) after a fixed number of executions as cheap insurance
  against native-side leaks.
- **Configuration / kill switch** (environment, read at startup):
  - `LABBY_CODE_MODE_POOL_SIZE` — number of pooled runners (default `2`, clamped to
    `16`). **`LABBY_CODE_MODE_POOL_SIZE=0` disables pooling entirely**, falling back
    to spawn-per-execution with behavior identical to the pre-pool path.
  - `LABBY_CODE_MODE_POOL_RECYCLE_AFTER` — executions before a runner is recycled
    (default `100`).
  - `LABBY_CODE_MODE_POOL_MAX_OVERFLOW` — cap on simultaneous ephemeral overflow
    runners (default `8`).

  The conservative default (`size = 2`) keeps idle memory bounded while absorbing
  typical `codemode` bursts. The security invariants (`env_clear`,
  process-group/Job-Object reaping, `kill_on_drop`, `PR_SET_DUMPABLE`) are set
  once at spawn and therefore hold for the pooled process's whole lifetime.

Code Mode always uses Javy/QuickJS for snippet execution — it is the **sole live
engine**, with no Boa fallback and no `code_mode_wasm` feature. `codemode` runs
in the Javy/QuickJS child runner over stdio. The Javy toolchain is pulled in by
the `gateway` feature.

The runner starts with an empty environment in a temporary directory. It does not
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
