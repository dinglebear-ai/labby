# Snippet fixture testing

Labby runs offline snippet tests with its production Code Mode sandbox. A finite
in-memory tool source replaces configured upstreams. Snippets remain Markdown
or JavaScript artifacts; the fixture is a separate JSON document.

## Commands

Validate source without executing it:

~~~sh
labby snippet validate unraid-linear-pr-triage
labby snippet validate example --file /absolute/path/example.md
~~~

Run one deterministic fixture:

~~~sh
labby snippet test unraid-linear-pr-triage \
  --fixture tests/snippets/triage-fast.json --json
~~~

An assertion, unexpected call, missing expected call, execution error, or budget
violation sets `passed: false` and a nonzero CLI exit status. Mock tests do not
initialize the gateway manager and never fall back to real upstream tools.

Real upstream execution is explicit:

~~~sh
labby snippet test unraid-linear-pr-triage --live --json
labby snippet test unraid-linear-pr-triage --live --param deep=true --json
labby snippet test --all --live --json
~~~

A live test is actual execution, not a dry run. Only run snippets whose effects
you have reviewed and authorized. The current live test path retains the prior
`result.ok` pass/fail behavior and normal Code Mode traces. Fixture assertions and
fixture budgets apply to offline tests only; they are not silently applied to
live tests. Bulk offline discovery, recording, and live catalog contract checking
are not implemented by this slice.

The API/MCP shared action is `snippets.test`. Send either
`{name, fixture: {...}}` or `{name, live: true, params: {...}}`. The existing web
client explicitly opts its execution tests into live mode. An absent fixture and
absent live opt-in are rejected before execution. Fixture mode cannot be combined
with `all`, `live`, or nonempty `params`; put input in `fixture.input`.

## Fixture format

~~~json
{
  "input": {"team": "U8"},
  "calls": [
    {
      "tool": "example::read",
      "params": {"id": 1},
      "response": {"ok": true, "count": 26},
      "times": 1
    }
  ],
  "expect": {"/summary/issueCount": 26},
  "absent": ["/historicalPRs"],
  "budgets": {
    "wall_clock_ms": 20000,
    "tool_calls": 40,
    "output_bytes": 16000
  }
}
~~~

Each call uses an exact `upstream::tool` identifier. When `params` is present,
arguments must match the complete JSON object. Omit `params` for a wildcard
argument rule. The first remaining matching rule is consumed. Independent rules
may execute in any order, so concurrent batches do not depend on completion order.
All rules must be consumed exactly `times` times; the default is one.

To inject an upstream error, replace the response with an error object:

~~~json
{"tool":"example::read","error":{"kind":"timeout","message":"synthetic timeout"}}
~~~

Expected injected errors are distinguished from unexpected calls. Catching an
unexpected error inside the snippet does not turn the test green. A snippet that
returns `ok: false` must have an explicit `expect: {"/ok": false}` assertion to
qualify as an intentional negative test.

Use synthetic data in committed fixtures. The harness does not record or redact
live upstream responses for you. Assertion diagnostics omit actual and expected
values, but a within-budget final snippet result is included in the report.

## Assertions and snapshots

Assertion keys and absence checks use RFC 6901 JSON pointers. Explicit JSON null
is different from a missing property. Pointers support `~0` and `~1` escaping.

The optional `snapshot` contains the entire expected result. The optional
`normalize` list replaces selected pointer values with null on both sides before
snapshot comparison. Ordinary assertions still inspect the original result.

~~~json
{
  "snapshot": {"count":26,"generatedAt":"ignored"},
  "normalize": ["/generatedAt"],
  "expect": {"/count":26}
}
~~~

## Budgets and isolation

Reports include wall time, attempted and failed calls, counts by exact tool ID,
raw result bytes, a rough byte-based token estimate, and whether display shaping
changed the result. Budget checks use the unshaped result, not a truncated preview.
An oversized result is omitted from the report. Diagnostics are capped at 32.
The token estimate is not tokenizer output or a billing measurement.

Wall-clock budgets range from 1 to 30,000 ms. Call budgets range from 0 to 512;
output budgets range from 1 to 24,576 bytes. Defaults are 20 seconds, 40 calls,
and 16,000 bytes. The CLI limits fixture files to 1 MiB. Rule counts and assertion
counts are bounded. Unknown fixture fields are rejected.

Only fixture-declared tools are exposed. The snippet's own tool declarations can
further narrow the scope. Empty fixtures grant no upstream access. Reserved local
providers, artifacts, resources, and nested saved snippets are unavailable. The
production sandbox and its normal timeout/containment rules remain in force.

## Triage regression coverage

The checked-in `tests/snippets/triage-*.json` fixtures use 26 synthetic issues and
cover fast mode, deep mode, a partial tool failure, and incomplete search pages.
Fast mode requires ten calls with no handoffs or historical result fields. Deep
mode requires 36 calls, including 26 compact handoffs. Tests also distinguish
whole issue identifiers, reject release-body-only matches, preserve owned PRs
outside the issue set, and search organization-wide rather than a fixed repo pair.

Run the kernel and native CLI regression suites:

~~~sh
cargo test -p labby-codemode --lib snippet::testing
cargo test -p labby --test snippet_fixtures
~~~

A fixture pass proves behavior for its inputs and mock responses. It does not
prove current upstream availability, current search completeness, or live schema
compatibility. Live acceptance results must be reported separately.
