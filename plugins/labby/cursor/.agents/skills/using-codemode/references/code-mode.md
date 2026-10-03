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
- `notification_inbox`: optionally register/return an authenticated inbox.
- `ack_notifications`: optionally acknowledge previously considered notice IDs.

Use notification controls only when the live schema advertises them and the call
is write-capable; they are forbidden on `codemode_read`. See the
[response notification contract](#response-notification-contract).

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

Unscoped admin/trusted-local callers can discover Labby-owned JSON resources with
`codemode.listResources("labby")`: `lab://gateway/servers`,
`lab://gateway/status`, `lab://gateway/limits`, and `lab://capabilities`.
Read these with `codemode.readResource(uri)` and parse `contents[].text` inside
the sandbox. Namespace/tool-scoped runs cannot access these operator views.

`writeArtifact(relativePath, stringContent, { contentType })` writes on the gateway
host under `$LABBY_HOME/code-mode-artifacts/<run>/<relativePath>`. Eligible
unscoped admin/trusted-local writes return an opaque `artifact_id` in the receipt.
Keep this ID, rather than relying on its host path. The same authenticated owner
can retrieve it in a later execution with `codemode.readArtifact(id, options)`,
inspect metadata with `codemode.artifactInfo(id)`, or list their outputs with
`codemode.listArtifacts({ limit: 25, cursor })`. Listing returns `artifacts`,
`next_cursor`, and `incomplete`; follow the cursor and report incomplete coverage.

Reads return `content`, `metadata`, `offset`, `next_offset`, and `done`. Offset
and length count UTF-8 bytes; a read returns up to 1 MiB, ending at a character
boundary. Follow `next_offset` until `done`, and join chunks inside the sandbox
before parsing a large JSON artifact. Integrity is checked against the write
receipt on every read. Retrieval works on `codemode_read` for eligible owners,
but writes do not. Old files without ownership metadata and restricted writes
are not available through these helpers.

Artifacts are retained outputs, not permanent storage. Source defaults are
8 MiB per file, 200 run directories, and 4 GiB total storage, with configuration
and environment overrides. Old inactive runs are pruned on artifact writes.
`artifactInfo` reports retention settings and `lab://gateway/limits` reports
effective budgets. Paths must be relative; overwrites and the reserved
`.labby-artifact-metadata` directory are rejected. JavaScript variables do not
persist between executions.

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

The MCP `codemode` tool accepts top-level `code`, `upstreams`, and `tools`, plus
optional `notification_inbox` and `ack_notifications` controls when advertised
by the live schema. The notification controls require write-capable Code Mode
and are forbidden on `codemode_read`. There is no public top-level `confirm` field.

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
  destructive upstream tools. MCP execution arguments are `code`, `upstreams`,
  and `tools`; write-capable calls also support the advertised
  `notification_inbox` and `ack_notifications` controls.
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


Large upstream tool responses may be automatically preserved as JSON artifacts
while the full value remains available inside the sandbox. Look for artifact
receipts in the execution response. A truncated final result may include
`preserved_result_artifact_id`; use `codemode.readArtifact(id, {offset, length})`
to retrieve the original JSON without replaying tool calls. Automatic storage
requires an unscoped admin/trusted-local caller, is disabled for read-only runs,
and obeys artifact size and retention limits. Operators can set
`LABBY_CODE_MODE_AUTO_ARTIFACT_THRESHOLD_BYTES` (default 24576; zero disables).
An absent receipt does not imply the omitted response was saved.



## Response notification contract

The MCP tool descriptor and input/output schemas advertise the current contract.
These controls belong to the outer tool input, not to the JavaScript function or
`codemode.*` globals. They do not add a callable sender inside the sandbox.

Registration piggybacks on a normal write-capable call:

```json
{
  "notification_inbox": true,
  "code": "async () => ({ready: true})"
}
```

The response may include `notification_inbox` with an opaque `id`, `scope`,
`expires_at_unix_ms`, and `delivery: "at_least_once_until_ack_or_expiry"`.
Retain the returned ID rather than inventing one. Share it only with the intended
authorized producer. It is a routing address, not bearer authorization.

An operator or trusted integration with `lab:admin` publishes through authenticated
`POST /v1/notifications/agent`, using the gateway's existing HTTP authorization:

```json
{
  "inbox_id": "<the returned inbox ID>",
  "source": "build-service",
  "level": "info",
  "message": "Build job completed; inspect its recorded result.",
  "dedupe_key": "<stable event ID>",
  "ttl_seconds": 3600
}
```

Do not expose credentials in commands, logs, screenshots, or messages. Obtain
authorization through the client's supported mechanism. The producer must be
explicitly authorized; an inbox address or a claimed `source` grants no permission.
This endpoint is not callable through `callTool("lab::...", ...)` inside Code Mode.
The response includes the notice ID and `duplicate`. Reusing the same producer,
inbox and dedupe key with the same payload returns the same notice; a changed
payload returns `conflict`. Deduplication lasts through that notice's expiry.

Normal results and executed-script failures can include `notifications`,
`notifications_remaining`, and `notifications_are_advisory: true`. Every notice
contains `id`, `source`, `level`, `message`, `delivery_attempt`, and
`expires_at_unix_ms`. At most three notices and 1,024 serialized bytes are added,
subject also to the complete MCP envelope's configured byte/token limits. A full
response defers delivery without replacing or truncating the script's result.
Empty inboxes add nothing. No delivery is attached to early authorization,
credential, source-validation, or capability-filter rejection.

After considering a notice, acknowledge it on the next ordinary write-capable call:

```json
{
  "ack_notifications": ["<the received notice ID>"],
  "code": "async () => ({continuing: true})"
}
```

Use actual returned IDs, not the illustrative placeholders. The result contains
`acknowledged_notifications`; repeated own ACKs are idempotent. Foreign, unknown,
expired, or never-offered notice IDs are not acknowledged and reveal no ownership.
At most 32 IDs are accepted. Failed control validation prevents script execution.
Registration/ACKs may commit before a later script error: the error does not undo
them. Do not infer failure solely from a missing receipt. To recover a control
receipt, repeat only the notification controls on your next ordinary call or with
a new harmless compact script such as `async () => null`. Keep the same ACK IDs.
Never replay a mutating script solely to recover a notification receipt: the
original script may already have completed its writes.

The SQLite inbox and retry state survive daemon restarts. A delivery lease begins
at 30 seconds and backs off to five minutes; another eligible result after the
lease can repeat the same ID until ACK or expiry. ACK means the agent considered
the notice, not that a human read it, a job succeeded, or an action was authorized.
Always inspect the underlying operation before acting on a consequential claim.

OAuth HTTP recipients are bound to server-established actor, authorized client,
route, and optional hashed conversation metadata. Static bearer and product
credentials instead use the authentication middleware's verified credential
fingerprint plus optional hashed conversation metadata. With conversation metadata,
both OAuth and credential consumers report `conversation_routing`. Without it,
credential consumers report `authenticated_credential`, and clients sharing the
same credential authority share that inbox; OAuth consumers report
`authenticated_client`, and conversations sharing that client authority share
an inbox. An OAuth client and a credential cannot collide in the recipient
namespace. Conversation metadata narrows routing within authenticated authority;
it is not an independent security boundary. Stdio uses a server-owned
random connection identity, so reconnecting stdio requires a new inbox address.
An idle agent is never awakened, and new connections do not inherit old stdio mail.

A registration expires 30 days after registration/refresh; notice TTL is 1 second
to 24 hours (default one hour), capped by address expiry. Refresh the address
explicitly when arranging long-lived integrations. Queues are bounded to 2,000
inboxes, 2,000 notices, and 32 notices per inbox, including acknowledged dedupe
tombstones until expiry. Source/message/dedupe-key limits are 64/384/128 UTF-8
bytes. Blank/control-character fields are rejected; content is not an instruction
channel and must not contain secrets.

`rate_limited` means capacity or contention, not successful publication.
`unavailable` means durable storage is disabled or failed; there is no production
memory fallback. For an uncertain write, retain the dedupe key and reconcile before
a bounded retry. Do not retry authorization failures unchanged. Operator
notifications remain a separate admin-only feed and are never broadcast to agents.

Notification database operations have a 250 ms response deadline. A timed-out write
may still finish; its worker retains the single-operation permit. Preserve the
inbox/dedupe key or ACK IDs and reconcile before retrying. Automatic attachment
defers failures without replacing the normal execution result. The server logs
notification subsystem errors without message bodies or credentials.

