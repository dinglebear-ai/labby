---
title: "Snippets Service"
created: "2026-08-18"
updated: "2026-10-02"
---

# Snippets Service

The `snippets` service is Labby's surface for reusable Code Mode workflows. The execution engine lives in `labby-codemode`; this service owns product registration, storage/discovery adapters, validation, execution, testing, promotion, and removal semantics.

The generated [action catalog](../generated/action-catalog.md) is authoritative for exact parameters, scopes, and destructive classification.

## Read-Only Discovery

`snippets.list`, `help`, and `schema` are discovery operations. Built-in snippets are loaded from the checked-in snippet directory and user snippets are resolved from the Labby home. `snippets.list` returns `snippets` and bounded per-file `diagnostics`; invalid user files continue to block the corresponding built-in because execution never silently falls back. The UI shows only effective entries with override provenance. Listing caches validation and metadata by content digest, and detects edits by reading the current bytes. The LRU cache retains at most 128 entries and 8 MiB of charged metadata; entries above 2 MiB are not cached. Charges include keys, strings, collections, JSON and errors. See [performance measurements](../dev/SNIPPET_PERFORMANCE.md) for the reproducible listing and batch benchmark.

## Tool Declaration Scope

Markdown frontmatter may include `tools` as a JSON string array or an indented
list of exact `<upstream>::<tool>` identifiers. Storage and Code Mode discovery
preserve omission separately from an explicit empty array. Declarations are
bounded to 128 unique identifiers of at most 1,024 bytes each; reserved local
capabilities, malformed identifiers, and duplicate declaration keys are rejected.

For native live saved-snippet execution (`snippets.exec` / `snippets.test` with
`live: true`), Labby intersects a declaration with the caller's existing Code Mode policy before
building the catalog. The declaration can narrow authority but never grant it:
omission keeps the legacy caller scope, `[]` denies all upstream tools, and a
nonempty list exposes only those exact dependencies. This also keeps one-shot
snippet runs from cold-probing unrelated gateway upstreams.

Offline fixture tests have no live authority. When a snippet declares tools,
every fixture rule must also name a tool in that declaration.

Nested `codemode.run()` inherits the already-established execution scope. Trusted
local saved snippets may compose inside that declared scope; route-scoped callers
still cannot use nested snippet resolution to widen their authority.

## Administrative Actions

Reading snippet bodies, executing or testing snippets, creating/removing snippets, and promotion flows require the scopes shown in the generated catalog. Promotion and removal are destructive actions.

Built-in snippets are read-only through the user-snippet mutation surface. Explicit shadowing is required before a promoted user snippet may replace a built-in name.

## Inputs And Concurrent Edits

Declared inputs reject unknown keys and enforce their types. Explicit `null`
requires `nullable: true`, including on required inputs. Omitted optional inputs
without defaults retain the existing null sentinel. Required nullable inputs
still must be supplied or have a default.

Resolved source and list metadata include `content_digest`, the SHA-256 hex digest
of exact source bytes. An edit through `snippets.create` may supply
`expected_digest` alongside `force: true`; the write fails with `conflict` when
that digest no longer matches the current user file. The UI captures the digest
when opening the editor. Cross-process file locks serialize cooperating writers;
non-overwriting creates also use filesystem no-clobber publication. Atomic writes
use unique temporary files and preserve the effective legacy `.js` path.
Lock acquisition waits at most 1.5 seconds. Contention returns `conflict` with
`existing_id: snippet-write-lock`, `side_effects: none_expected`, and conditional
retry guidance. A later attempt still rechecks `expected_digest` under the lock;
stale edits remain a separate conflict requiring reload.

## Execution Receipts

Saved live execution returns an `execution_id` and `receipt_status`:
`persisted`, `disabled`, or `unavailable`. A telemetry failure does not change the
execution result and must never cause a mutation to be replayed.
`snippets.receipt` accepts `execution_id` and requires admin authority plus the
original owner, route and caller-capability fingerprint. There is no admin
cross-owner bypass. Receipt reads use the existing Code Mode journal database;
when journaling is disabled or unavailable, lookup reports `journal_unavailable`.

Receipts retain exact snippet and merged-input digests, the effective tool scope,
Labby runtime version/engine, surface, timestamps, status, wall time, final-result
digest/byte count, and bounded tool-call identities, parameter digests and
artifact references. They contain no raw source, parameters, logs or results.
The first 32 calls are retained with explicit total and omitted counts; artifact
references are bounded to 16. Receipts identify output rather than providing a
full-result replay store. Artifact bytes retain their existing authorization and
retention lifecycle.

The journal retains at most 1,000 receipts globally and 100 per owner/route,
with a seven-day lookup window and 64 KiB per receipt. Expired, evicted and
out-of-scope identities return the same `unknown_execution` error. A `started`
receipt means completion was not recorded (for example cancellation or an
interrupted process); it is not evidence that execution is still running.
Completion persistence and step flushing are independently bounded and
fail-open. Existing step-journal databases gain an additive, validated receipt
table without altering existing step rows.

`snippets.history` lists receipts in exactly the same authority, with an optional
snippet `name` filter, `limit` of 1–50 (default 20), and an opaque `cursor`.
Ordering is descending creation time and execution ID. Cursors are bound to the
owner, route, capability fingerprint and name filter; they cannot be reused in
another scope. Disabled recording returns an explicit empty `disabled` state.
The UI shows persisted history, call failures and timings, supports pagination,
and refreshes after execution. Authority changes abort and discard prior reads.

`snippets.artifact` reads an exact reference using `execution_id` and `path` after
authorizing its receipt. It returns bounded base64 bytes and verified metadata;
downloads are capped at 8 MiB with a five-second read deadline. The kernel rejects
traversal and symlinks, requires a regular file, and checks its size and SHA-256
before returning any bytes. Unix reads walk components through directory handles
with `NOFOLLOW`; other platforms repeat ancestor checks around opening and still
require exact digest equality. Pruned, changed or inaccessible artifacts return
`artifact_unavailable`. Older receipts without the broker storage identity remain
readable as metadata but cannot download files. UI links fetch through this action
and download blobs rather than exposing filesystem paths or rendering HTML.

The builder uses the visible tool's bounded input schema from the authenticated
describe endpoint to render typed fields, required controls, enum choices and
snippet-input selectors. Unsupported or oversized schemas retain the advanced
JSON editor. All mappings still pass backend tool validation at execution. See
[snippet UI](../dev/SNIPPET_UI.md) for interaction and verification details.

## Dependency-Aware Builder

The builder models named steps with separate parameter mappings and optional
explicit dependencies. A mapping can contain JSON constants, a declared input
reference (`$input.query`), or another step's output
(`$steps.lookup.items.0.id`, or `$steps.lookup` for the complete output). Output
references infer dependencies automatically.
Independent steps execute in deterministic waves using deferred `codemode.batch`
jobs. Each step retains its `succeeded`, `failed`, or `skipped` status; a failed
prerequisite skips its descendants while unrelated branches continue. The
generated result includes ordered step evidence plus `ok` and `all_ok` flags.

Schema suggestions use compatible, unambiguous input/output fields. They remain
editable and do not invent output fields when a tool lacks an output schema.
Advanced JSON and JavaScript authoring remain available. Missing references,
unsafe property paths, duplicate step IDs and dependency cycles are rejected
before code generation. Runtime output selectors use own-property checks, and
missing output data fails that step rather than passing an undefined parameter.

## Execution Preview And Explicit Replay

`snippets.preview` accepts a snippet `name` or an owner-scoped historical
`execution_id`, plus fresh caller-supplied `params`. It merges declared input
defaults and reads bounded, caller-visible tool metadata without evaluating the
snippet or invoking its tools. It returns tool availability, safety annotations,
input-key provenance, fingerprints and explicit coverage limits. Arbitrary
JavaScript branches, dynamic calls and runtime parameter values are unresolved;
a metadata preview is not a simulation. The builder can additionally show its
known static mappings, masking sensitive values and marking output references
that resolve only during execution.

The preview fingerprint binds source, merged input, effective scope, runtime
configuration, metadata and caller authority. `snippets.exec` accepts an optional
`expected_preview_fingerprint`; a stale preview fails before execution. The
guard and runner use the same prepared source/input snapshot. Tool invocation
still revalidates current authority and metadata: upstream contracts can change
after preparation, so the preview does not freeze an external server.

`snippets.replay` starts a new execution of the current snippet. It requires the
historical `execution_id`, fresh `params`, the current preview fingerprint and
`acknowledged_drift` matching every changed or unverifiable field. It compares
snippet, input, effective scope, runtime and tool-schema evidence. Older receipts
without schema evidence remain readable and report that comparison as
unverifiable. Receipts retain optional bounded schema digests, never raw input
or tool parameters. Replay neither resumes a journal nor reconstructs historical
input; it never runs automatically to recover an incomplete receipt.

## Execution

Snippet code must evaluate to an async arrow function and executes inside the same bounded Javy/QuickJS Code Mode runtime used by gateway Code Mode. Live execution resolves tool calls through the gateway catalog. Offline testing uses exact fixture rules without gateway discovery.

## Fixture-First Testing

Testing is offline by default. A named test loads the sibling
`<name>.test.json` file, or a fixture supplied explicitly. It never falls back to
live execution when the fixture is missing, invalid, or incomplete. Existing
scripts that intentionally tested real upstreams must add `--live` (or
`live: true` through the shared action).

```bash
labby snippet validate unraid-linear-pr-triage --file docs/snippets/unraid-linear-pr-triage.md
labby snippet test unraid-linear-pr-triage
labby snippet test unraid-linear-pr-triage --fixture docs/snippets/unraid-linear-pr-triage.deep.test.json --param deep=true
labby snippet test --all
labby snippet test unraid-linear-pr-triage --live
```

The CLI reports a failing test with a nonzero exit status. With `--json`, its
stdout remains the structured test report. Bulk tests retain diagnostics and
metrics without multiplying full result payloads. A bulk run containing a snippet
without a fixture fails rather than silently skipping it. Bulk tests run
sequentially and are capped at 100 unique snippet names; an empty run fails.

The equivalent shared `snippets.test` parameters are a `name` (or `all: true`),
an optional input object in `params`, and optionally a `fixture` object or
`live: true`. Omit both to load the selected snippet's adjacent fixture. Inline
fixtures cannot be combined with `all` or `live`.
Authorization still comes from the shared action catalog.

### Fixture Contract

`labby snippet fixture <name>` generates an editable fixture draft from the
selected gateway's visible tool schemas over its authenticated MCP connection without executing tools. The CLI resolves the snippet and generates the fixture locally. `--tool` supplies exact dependencies for legacy snippets; `--check` compares saved contracts with current metadata and exits nonzero on drift. `--schemas`
uses a saved contract map offline; `--results` supplies synthetic response
overrides, and `--variant minimal` omits optional fields and extra array items.
`--output` writes only a complete draft to a new file. The shared
`snippets.fixture` action returns the draft without saving it. Generated rules
cover one call per selected tool; authors must edit repetition, branches and
assertions. See [fixture generation](../dev/SNIPPET_TESTING.md#generating-schema-backed-fixtures).

Generated contracts store SHA-256 fingerprints; offline execution checks saved contract integrity. Optional fixture `schemas` validate successful response rules and actual tool
arguments against saved contracts. Tests remain offline; unsupported schema
assertions fail explicitly instead of reporting partial validation as complete.

Fixtures are JSON documents containing ordered exact-tool response rules,
JSON Pointer equality assertions, optional normalized snapshots, and resource
budgets. Rule `match` objects compare a subset of top-level parameters; nested
values compare exactly. Each rule is consumed once unless `times` specifies
another count. Unknown fixture properties are rejected.

```json
{
  "calls": [
    {
      "tool": "github::get_me",
      "match": {},
      "result": { "login": "fixture-user" }
    }
  ],
  "expect": { "/login": "fixture-user" },
  "budgets": {
    "wall_clock_ms": 20000,
    "tool_calls": 1,
    "output_bytes": 16000
  }
}
```

A rule can provide `error: {"kind":"network_error","message":"synthetic failure"}`
instead of a result. Tests fail on unexpected calls, unconsumed rules,
exceptions, assertion or snapshot mismatches, and budget violations. Catching an
unexpected tool rejection inside the snippet does not make the test pass. To test
an intentional failure result, explicitly assert `"/ok": false`.

Snapshots compare the complete result. `ignore_paths` lists JSON Pointers whose
values are replaced with null on both sides, for example
`["/summary/elapsedMs"]`. Missing properties remain distinguishable from null.
Fixtures must be synthetic or separately scrubbed of credentials and personal
data; there is no automatic live-response recorder in this implementation.

### Execution Boundary And Budgets

Mock tests use the production snippet parser, input merger, and isolated
QuickJS subprocess. They have no gateway host, live tool credentials, local
providers, or resource access. The supported mock surface is `callTool()`, `codemode.batch()`, explicitly
fixture-backed `codemode.run()` results and synthetic `writeArtifact()` receipts.
Fixture `params` supply baseline inputs overridden by caller params. Nested
response rules test composition, not a child's implementation; child workflows
need their own fixtures. Artifact rules check exact paths, content types and
content fragments without writing files. Discovery and generated tool helpers
remain unavailable. Mock fixtures are not a substitute for checking
parameters against the current live catalog.

Defaults are 20,000 milliseconds, 40 calls, and 16,000 raw UTF-8 output bytes.
Maximum supported values are 30,000 milliseconds, 512 calls, and 16,000 output
bytes. Fixture documents are capped at 512 KiB. The report records elapsed time,
call count, raw output bytes, a bytes/4 token estimate, and peak synthetic
concurrency. Synthetic call timings do not measure real upstream performance.
The synthetic trace is bounded and explicitly indicates omitted entries.

Live tests require explicit opt-in and retain normal caller and tool-declaration
scope. Their report records wall time, calls grouped by tool, failed calls,
raw output bytes, the token estimate, and output shaping/truncation. An execution
with failed tool calls or a changed/truncated result is not a passing live test.
Neither mode changes gateway configuration or repairs upstream failures.

## Related Docs

- [Snippet development and testing](../dev/SNIPPET_TESTING.md)
- [Code Mode](../dev/CODE_MODE.md)
- [Snippet authoring](../snippets/README.md)
- [Gateway](./GATEWAY.md)
- [Service model](../dev/SERVICES.md)
