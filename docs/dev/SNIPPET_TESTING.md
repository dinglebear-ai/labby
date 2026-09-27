---
title: "Snippet Testing"
created: "2026-09-26"
updated: "2026-09-26"
---

# Snippet testing

Saved snippets use the existing Markdown/frontmatter parser and the production
Javy/QuickJS runner. Fixture mode does not initialize a gateway. It executes with
no host, a deny-all upstream scope, and read-only local capability policy.

## Offline workflow

Validate source without saving or executing it:

~~~sh
labby snippet validate unraid-linear-pr-triage --file docs/snippets/unraid-linear-pr-triage.md --json
~~~

Run a checked-in synthetic fixture:

~~~sh
labby snippet test unraid-linear-pr-triage --fixture docs/snippets/tests/unraid-linear-pr-triage/fast.json --json
~~~

The CLI exits nonzero for invalid source, execution failure, assertion failure,
unused expected calls, unexpected calls, or exceeded budgets. Catching a mock
call error inside a snippet does not hide that unexpected invocation from the
harness. A snippet returning an expected error report can pass a negative test.

Fixture execution reuses the production batch helper. Literal callTool IDs and
codemode.batch are supported. Generated upstream helper functions and catalog,
resource, skill, nested snippet, and step APIs are not mocked by this first
fixture adapter. Unsupported operations fail rather than reaching live services.

## Fixture format

Fixtures are JSON with strict unknown-field rejection. Use synthetic data only.

~~~json
{
  "input": {},
  "calls": [
    {
      "tool": "github::get_me",
      "match": {},
      "result": {"login": "fixture-user"},
      "times": 1
    }
  ],
  "expect": {"/login": "fixture-user"},
  "absent": ["/secret"],
  "budgets": {
    "wall_clock_ms": 20000,
    "tool_calls": 40,
    "output_bytes": 16000
  }
}
~~~

Rules are matched by exact tool ID and an optional top-level parameter subset.
Values within that subset must be deeply equal. The first unconsumed matching
rule wins, so give repeated calls distinct parameters where ordering matters.
An omitted match accepts any parameters. An omitted result means JSON null.
Use an error object with kind and message instead of a non-null result to model
failure. Each rule must be consumed exactly times times, default one.

Explicit invocation parameters override fixture input, which in turn overrides
snippet defaults. The production input merger enforces declared input types.
Tool declarations restrict which IDs fixtures may name; reserved local
capabilities are not allowed as mock upstreams.

## Assertions and snapshots

The expect map uses JSON Pointers. An empty pointer compares the entire result,
providing a structured snapshot without a second snapshot format. The absent
array asserts that fields are missing, not merely null.

For nondeterministic fields, declare exact normalization pointers:

~~~json
{
  "normalize": {"/summary/elapsedMs": 0},
  "expect": {"/summary/elapsedMs": 0}
}
~~~

A missing normalization pointer fails the test. Normalization cannot replace the
entire result. Snapshot assertions compare normalized values; performance
budgets measure the original result. Do not normalize meaningful domain data
just to make a failing snapshot pass.

min_parallel_calls asserts how many mock calls were simultaneously pending.
This detects accidental sequentialization of independent calls, but is not a
network throughput benchmark. Mock durations are local runtime measurements,
not forecasts of live upstream latency.

## Budgets and reports

Default limits are 20 seconds, 40 mock attempts, and 16,000 result bytes. Hard
limits are 30 seconds, 512 attempts, and 24,000 result bytes. The full compiled
fixture and snippet must also fit the Code Mode source-size limit. All attempts,
including caught failures, count toward the tool budget.

Reports contain pass/fail, assertions, result, mock call trace, unused rules,
elapsed milliseconds, maximum mock concurrency, external capability attempts,
and UTF-8 result bytes. estimated_tokens is only ceil(bytes / 4), not a tokenizer
measurement. Trace rows exclude raw call parameters and responses. Fixtures and
returned results are not automatically scrubbed of personal data or secrets.
There is no live fixture-recording mode in this implementation.

## Explicit live execution

~~~sh
labby snippet test unraid-linear-pr-triage --live --json
labby snippet test unraid-linear-pr-triage --live --param deep=true --json
~~~

Live mode contacts actual services and may mutate state for other snippets.
The Unraid triage snippet is read-only. Live execution retains caller scope,
exact dependency declarations, and production traces. Fixture budgets and
assertions apply only to fixture mode; live output must be reviewed separately.
The web UI labels this action Test live and sends an explicit live flag.

Compatibility change: snippets.test now requires fixture or live: true. Previous
clients that relied on silent live execution must opt in. --all is currently
only the explicit live sweep, not automatic fixture discovery. Fixture mode
requires one named snippet and one fixture file.

## Regression coverage

The CLI integration suite iterates all checked-in Unraid triage fixtures:

~~~sh
cargo test -p labby-codemode --lib
cargo test -p labby --all-features --test snippet_fixture
~~~

Coverage includes the 26-issue fast/deep paths, repository overrides, identity
failures, rate limits, partial search coverage, release-body noise, identifier
prefix collisions, UTF-8 output bounds, and the CLI's failure exit status.

Catalog contract validation without execution, fixture recording/redaction,
automatic fixture discovery, scaffolding, and reusable snippet helper APIs are
separate follow-on capabilities. The current adapter does not claim them.

## Related documentation

- [Snippets service](../services/SNIPPETS.md)
- [Code Mode](CODE_MODE.md)
- [Snippet authoring](../snippets/README.md)
