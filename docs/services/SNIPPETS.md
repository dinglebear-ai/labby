---
title: "Snippets Service"
created: "2026-08-18"
updated: "2026-09-29"
---

# Snippets Service

The `snippets` service is Labby's surface for reusable Code Mode workflows. The execution engine lives in `labby-codemode`; this service owns product registration, storage/discovery adapters, validation, execution, testing, promotion, and removal semantics.

The generated [action catalog](../generated/action-catalog.md) is authoritative for exact parameters, scopes, and destructive classification.

## Read-Only Discovery

`snippets.list`, `help`, and `schema` are discovery operations. Built-in snippets are loaded from the checked-in snippet directory and user snippets are resolved from the Labby home.

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
data; there is no automatic fixture recorder in this implementation.

### Execution Boundary And Budgets

Mock tests use the production snippet parser, input merger, and isolated
QuickJS subprocess. They have no gateway host, live tool credentials, local
providers, or resource access. The supported mock surface is `callTool()` plus
`codemode.batch()`; discovery, generated tool helpers, nested snippets, and
other helpers are not emulated. Mock fixtures are not a substitute for checking
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
