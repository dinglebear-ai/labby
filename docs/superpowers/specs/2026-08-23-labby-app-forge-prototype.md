# Labby App Forge Prototype Specification

## Objective

Prove arbitrary MCP-to-Livebook application generation with one selected upstream tool. A user opens `app_forge.livemd`, connects to an authenticated Labby gateway, explicitly searches the caller-visible capability catalog, selects one MCP tool, previews and executes a schema-generated form, chooses a bounded result renderer, enters an app title, and downloads a portable `.livemd`. Reopening that notebook must verify and execute the same live tool contract without handwritten tool-specific code.

## Product boundary

- Labby remains the capability, identity, authorization, schema-validation, destructive-policy, and execution authority.
- `kino_labby` owns the fixed-route client, schema projection, bounded rendering, AppSpec, notebook rendering, and Forge UI.
- Credentials never appear in an AppSpec, generated notebook, browser asset, log, error, or download.
- V1 downloads only; it does not register, deploy, commit, publish, or persist applications.
- V1 excludes destructive tools. Confirmation and elicitation are a later slice.

## Existing contract reused

- `GET /v1/palette/catalog`
- `GET /v1/palette/search?q=<query>&limit=<1..100>`
- `POST /v1/palette/execute`

Add `GET /v1/palette/descriptor?id=mcp:<upstream>::<tool>`.

Forge search is explicit-submit, requires two non-whitespace characters, and never fetches descriptors for search rows. Selection performs exactly one descriptor request. The descriptor is authoritative over compact search metadata.

## Capability descriptor v1

```json
{
  "contractVersion": 1,
  "catalogRevision": "opaque-revision",
  "id": "mcp:github::search_issues",
  "upstream": "github",
  "tool": "search_issues",
  "description": "Search issues",
  "inputSchema": {"type": "object"},
  "outputSchema": {"type": "array"},
  "annotations": {
    "readOnlyHint": true,
    "destructiveHint": false,
    "idempotentHint": true,
    "openWorldHint": true
  },
  "destructive": false,
  "contractHash": "sha256-lowercase-hex"
}
```

The descriptor comes from the caller-visible live `UpstreamTool`. Annotations contain only the four typed MCP hint booleans. Arbitrary `_meta`, raw annotation JSON, credentials, and UI metadata are not exposed in V1.

The contract hash is SHA-256 over canonical JSON containing exactly `contractVersion`, `id`, `inputSchema`, `outputSchema`, `annotations`, and authoritative `destructive`. Object keys are recursively sorted; arrays retain order; missing values serialize as JSON `null`. Description and catalog revision are excluded.

Limits are 64 KiB per schema, 160 KiB per descriptor, 2,048 description characters, and 64 schema levels. Over-limit or malformed descriptors return `descriptor_unsupported`; they are never silently represented as missing schemas or hashed as `null`.

## Caller-consistent atomic execution

Palette execution accepts:

```json
{
  "id": "mcp:github::search_issues",
  "params": {"query": "bug"},
  "expectedContractHash": "..."
}
```

Labby must bind visibility, scope, OAuth subject, descriptor, contract hash, schema validation, destructive classification, connection, and invocation to one caller-aware published config/pool revision. Final dispatch must not substitute `SHARED_GATEWAY_OAUTH_SUBJECT`. Hash mismatch returns `contract_changed` with `side_effects: none_expected` and performs zero upstream calls.

Successful execution returns the result plus a safe receipt:

```json
{
  "requestId": "request-uuid",
  "toolId": "mcp:github::search_issues",
  "contractHash": "current-contract-hash",
  "catalogRevision": "opaque-revision",
  "truncated": false
}
```

The receipt contains no params, result content, subject, token, or OAuth data. Generated apps show it in a collapsed plain-text “Execution details” panel and use `requestId` for operator correlation.

Static bearer admin remains supported. OAuth callers with `mcp:read mcp:write` plus `gateway:<upstream>` may browse and execute only that upstream. `mcp:read` alone is browse-only. Forge cannot invoke Labby administrative actions.

Requests carry `x-request-id`. Logs include request ID, upstream, tool, safe subject fingerprint, catalog revision, contract hash, elapsed time, and outcome kind. Logs exclude tokens, OAuth material, params, schemas, bodies, and raw subjects.

## Supported schema subset

V1 compiles a top-level object with at most 100 properties:

| JSON Schema | Kino field |
| --- | --- |
| `string` | text |
| `string` plus at most 100 string enum values | select |
| `string`, `format: date` | date encoded ISO-8601 |
| `integer` | integer |
| `number` | finite number |
| `boolean` | checkbox |

Field order is lexical. Labels/descriptions cap at 512 characters. Minimum/maximum are enforced locally. Optional blank string/number/date values are omitted; booleans are submitted. Unknown browser keys are rejected. Unsupported shapes use an explicit whole-object JSON textarea capped at 64 KiB and requiring an object. Labby remains authoritative.

## Kino runtime and results

`KinoLabby.ToolForm` uses `Kino.JS.Live` only for bounded field collection and feedback. Credentials, headers, raw errors, and full results never enter initialization data.

Execution runs in a monitored task with one active operation, a monotonic ID, and 30-second deadline. Duplicate Run is rejected; late results are ignored; selection changes and termination cancel owned work; controls re-enable on all outcomes.

Elixir renders through `Kino.Frame`:

- `auto`: list of maps becomes a table preview; other values become JSON/plain text
- `table`: list of maps only, otherwise JSON with warning
- `json`: escaped plain JSON/text

No untrusted Markdown or raw HTML. Preview caps: 1,000 rows, 100 fields per row, depth 16, 4,096 characters per string, and 1 MiB rendered JSON. Larger successful results show a bounded preview plus JSON download. HTTP bodies cap at 10 MiB.

## AppSpec v1

```elixir
%KinoLabby.AppSpec{
  version: 1,
  title: "GitHub Issue Search",
  tool_id: "mcp:github::search_issues",
  contract_hash: "...",
  compatibility: :exact,
  renderer: :auto
}
```

`App.render/1` returns visible setup guidance for missing configuration. Exact, non-destructive contracts render normally. Missing, destructive, or changed contracts fail closed. Every Run still sends `expectedContractHash`, closing the render-to-execute race.

Compatibility modes are explicit:

- `:exact` requires byte-identical contract hashes.
- `:additive_input` accepts only newly added optional top-level input properties. Existing input properties, `required`, output schema, annotations, destructive classification, tool identity, and contract version must remain identical. Once accepted, execution sends the current live hash and the receipt records it.

No heuristic or silent compatibility exists. Compatibility comparison uses the stored normalized contract snapshot embedded in the AppSpec alongside its hash; the snapshot is limited to the same 160 KiB descriptor bounds and contains no credentials.

`KinoLabby.AppSpec.decode/1` dispatches by integer `version`. Version 1 has an identity decoder. Unsupported versions fail with `unsupported_app_spec_version`. Migration functions live in `KinoLabby.AppSpec.Migrations`, are pure map-to-map transforms, advance exactly one version, and must have golden before/after fixtures before registration. V1 ships the migration framework and unknown-version behavior; it does not invent a fake v2 format.

Prototype notebooks use a local path or approved pinned Git SHA. They do not claim Hex portability or use `~> 0.1` before publication is authorized and verified.

## Acceptance

1. Two OAuth subjects see and execute only their own session.
2. Browse-only cannot execute; upstream-scoped write cannot cross boundaries or invoke administration.
3. Forge fetches one descriptor and renders supported schema without tool-specific code.
4. Safe execution is caller-consistent and renders a bounded result.
5. Downloaded deterministic credential-free `.livemd` reopens independently.
6. Contract/destructive drift returns `contract_changed` with zero upstream calls.
7. Missing env, auth failure, timeout, malformed/oversized data, duplicate Run, stale selection, and disconnect have bounded visible outcomes.
8. Rust, Elixir, and fresh-browser gates pass at exact final heads.
9. Rust and Elixir verify the same cross-language canonical contract/hash golden vectors.
10. A hermetic stdio MCP fixture proves safe/destructive, schema drift, subject isolation, errors, delay, large results, and invocation counts without external services.
11. Successful calls expose a redacted execution receipt and exact request correlation.
12. `:additive_input` accepts only the specified safe change; all other compatibility changes fail closed.
13. AppSpec decoding rejects unknown versions and the migration registry cannot skip versions.

## Deferred

Destructive confirmation, multi-tool graphs, conditionals, repeaters, Tasks, elicitation, upstream MCP Apps, AI AppSpecs, persistent indexing, Hex publication, Git/deployment integration, custom widgets, Markdown, and arbitrary nested schemas are outside V1.


