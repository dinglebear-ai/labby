# Code Mode Tool Browser Specification

## Product outcome

Add an authenticated, root-administrator-only upstream-tool browser to the Gateway Admin Web UI at `/tools`. An operator can submit a lexical query over the same live tool descriptors used by Code Mode, inspect one visible result, and read its existing TypeScript declaration without running JavaScript or invoking an upstream tool.

## Activation gates

Implementation does not begin until both gates pass:

1. `lab-837l6.2` is closed and its caller-neutral render, safety DTO, render/embedding identities, keyed single-flight, and 4,000-tool structural fixtures are present.
2. A checked-in measurement compares the narrow native path with the least-expensive safe JavaScript-based Web alternative on the deterministic 4,000-tool fixture. The measurement records catalog acquisitions, DTS generations, serialization bytes, process/runtime startup work, and response bytes. Reactivate `lab-837l6.3` only when the native path removes material work or the JavaScript alternative cannot preserve the required API authority boundary.

If either gate fails, keep `lab-837l6.3` deferred and do not ship `/tools` from this plan.

### Recorded activation measurement

The deterministic 4,000-tool fixture passed on 2026-08-17. Both cold paths
acquire one render, generate 4,000 eager DTS values, serialize the existing
3,477,571-byte full catalog, return 50 DTOs in a 12,113-byte response, and call
zero upstream tools. The least-expensive safe JavaScript Web alternative still
starts one Code Mode runner/runtime solely to execute discovery; the direct
borrowed projection starts none. Eliminating that otherwise unnecessary process
and sandbox lifecycle is material, so the native activation gate is **GO**.
This measurement does not claim native discovery avoids the current eager cold
render/DTS/catalog cost; those remain shared and explicitly measured.

## V1 architecture

- `/tools` is the only production consumer.
- Add API-private authenticated endpoints under the existing gateway API module. Do not add shared `ActionSpec` entries, MCP help, CLI commands, or generic dispatch context plumbing.
- The API handler requires `lab:admin` and constructs a fixed root-administrator discovery context internally. Request JSON contains only query/target data.
- `GatewayManager` acquires one existing live `ToolsRender`, filters entries with the canonical `discovery_entry_visible` formula, and delegates to neutral lexical/target-resolution functions in `labby-codemode`.
- Search is lexical-only in v1. It never waits for query embeddings or introduces another semantic hook.
- Describe filters first, resolves only among visible tools, then selects the already-rendered DTS from that entry. This is focused disclosure, not focused generation or fetch.
- No tool execution, exposure editing, snippets, command-palette integration, or public native SDK ships in v1.

## API contracts

### `POST /v1/gateway/codemode/tools/search`

Request:

```json
{ "query": "issue search", "limit": 50 }
```

Response:

```json
{
  "results": [
    {
      "path": "codemode.github.search_issues",
      "id": "github::search_issues",
      "kind": "tool",
      "namespace": "github",
      "name": "search_issues",
      "description": "Search repository issues",
      "signature": "search_issues(input)",
      "tags": ["github", "issues"],
      "score": 22,
      "safety": { "read_only": true }
    }
  ],
  "total": 1,
  "truncated": false,
  "truncated": false
}
```

### `POST /v1/gateway/codemode/tools/describe`

Request:

```json
{ "target": "github::search_issues" }
```

Response adds `helper` and one of:

- `typescript: "..."` when the existing selected DTS fits the response contract;
- `typescript: null, typescript_omitted: "size_limit"` when it does not.

Never syntactically truncate TypeScript.

## Input and output bounds

- Query: non-empty after normalization, maximum 1,024 UTF-8 bytes.
- Target: non-empty after trim, maximum 4,096 UTF-8 bytes.
- Limit: clamp to `1..=50`.
- Description: maximum 4 KiB after existing sanitization.
- Signature: maximum 8 KiB.
- Tags: maximum 32 tags and 256 bytes per tag.
- TypeScript: maximum 64 KiB; omit with `size_limit` above the cap.
- Serialized search response: maximum 256 KiB.
- Serialized describe response: maximum 128 KiB.
- Apply caps at the Rust response projection before JSON serialization. One-byte-over tests are required.

## Search contract

- Normalize lowercase ASCII alphanumerics and spaces exactly as the current lexical Code Mode search does.
- Preserve current lexical field weights, token coverage, deterministic tie-breaking, empty-query hint, and `1..=50` limit behavior.
- Browser v1 does not promise semantic ordering equivalence with sandbox search.
- A machine-readable fixture is executed by both Rust lexical tests and the sandbox JS test harness for the common contract: normalization, lexical weights, limit, exact resolution, ambiguity, and hidden/unknown behavior.
- Candidate scoring uses borrowed entry indices and numeric scores. Clone strings only for final results. Maintain `total` separately and retain at most the best 50 candidates.

## Authority and confidentiality

- Live tool dispatch remains authoritative; discovery safety metadata never grants execution.
- V1 requires authenticated `lab:admin`. Missing or non-admin auth fails before catalog acquisition.
- `(admin)` is only a client navigation/layout group, not a security boundary. Static `/tools` assets contain no tool fixtures or catalog data.
- Filter before score or target resolution. Hidden and random unknown targets have the same HTTP status, stable error kind, message shape, and public fields.
- Ambiguity candidates contain visible paths only.
- Do not accept caller, surface, scope, subject, route, allowlist, or OAuth authority in request bodies.
- Do not log query, target, result IDs, descriptions, DTS, subject, scopes, or allowlists. Log action, elapsed time, result count/response bytes, request ID, and error kind.
- Render description, signature, tags, and DTS only through React text nodes or `<pre><code>`; never HTML/Markdown injection.
- Set `Referrer-Policy: no-referrer` for the authenticated app shell as defense in depth.

## Browser behavior

- Sidebar destination: **Tools**.
- Search submits explicitly; v1 has no per-keystroke debounce.
- Query and selected-tool state remain component-local; catalog searches and internal IDs are not persisted in URL/history.
- Component-local requests use `AbortController`; a newer search/selection or auth transition aborts the old request.
- The auth session store exposes a monotonic, non-sensitive `authEpoch` incremented on login, logout, refresh, and scope/session replacement. Tool browser state clears whenever it changes.
- No SWR/global discovery cache is used in v1.
- States: initial guidance, loading, results, no matches, invalid input, sign-in required, forbidden, backend unavailable with request ID, stale selection, and oversized TypeScript omission.
- Auth/session changes abort in-flight work and clear already-published results and details.

## Performance evidence

Normal CI enforces structural facts on an isolated manager and deterministic 4,000-tool fixture:

- one live render acquisition per request;
- zero upstream tool calls;
- warm search regenerates zero DTS and serializes no full catalog;
- at most 50 owned result DTOs;
- no DTS in search JSON;
- exact search/describe response byte ceilings;
- cancelled requests never publish results.

Dedicated ignored benchmarks record cold render/DTS/catalog cost and warm lexical latency without wall-clock assertions in ordinary CI. Lazy/parallel DTS generation, pre-normalized sidecars, new indexes, and LRU caches remain measurement-driven follow-ups.

## Failure behavior

| Codepath | Failure | Rescue | User sees | Logged |
|---|---|---|---|---|
| Auth gate | Missing/non-admin session | No catalog acquisition | Sign-in or forbidden | action/kind/request ID |
| Render acquisition | Catalog unavailable | No stale visibility fallback | Tools unavailable + retry/request ID | action/kind/elapsed |
| Search input | Blank/oversized/invalid UTF-8 boundary | Local guidance or structured validation | Guidance/validation | kind only |
| Search | Broad 4,000-tool match | Bounded top-50 borrowed selection | Count + 50 results | scanned/matched/returned/bytes |
| Describe | Hidden/random target | Identical generic not-found | Tool not found | same kind only |
| Describe | Selected DTS exceeds 64 KiB | Omit complete DTS | Parameters unavailable: size limit | bytes/omission class |
| Catalog churn | Tool disappears after search | Live re-resolve | Tool no longer available | unknown_tool/request ID |
| Browser race | Older request completes later | Abort and identity-check epoch/query/target | Latest URL state only | server request telemetry only |
| Auth transition | Prior admin response is in flight | Abort, clear local state, increment epoch | Fresh loading/empty state | no identity data |

## Explicitly deferred

- Semantic/TEI ranking in the browser.
- Shared MCP/CLI gateway actions and generic trusted-context plumbing.
- Public `CodeModeBroker::search`/`describe` SDK methods.
- Route-scoped or `lab:read` browser discovery until the API owns exact trusted route/tool metadata.
- Snippets and static snippet safety classification.
- Command-palette integration, result virtualization, streaming, and per-keystroke search.
- Lazy/parallel DTS generation or a separate type endpoint.
- Persistent/multi-entry caches and new indexes.
- Tool execution and exposure editing.
