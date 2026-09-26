# Snippet testing

Snippet tests reuse Labby's production Markdown parser, input merger and
QuickJS subprocess. Deterministic fixtures do not connect to a gateway or call
real upstream tools. Static validation, fixture execution and live execution
are different evidence levels.

## Validate before running

    labby snippet validate example --file ./example.md

This checks the saved-snippet format and JavaScript syntax without executing
JavaScript or contacting upstreams. Input metadata accepts camelCase keys while
snippet filenames retain their lowercase naming rules. Executed input objects
are still checked against declared names, required fields, defaults and types.

## Run deterministic fixtures

    labby snippet test example --fixture ./example.test.json --json
    labby snippet test example --json
    labby snippet test --all --json

Without an explicit file, a fixture is loaded beside the resolved snippet:
example.md uses example.test.json. Missing fixtures fail clearly; they never
trigger a live fallback. The all mode checks at most 100 unique snippet names,
reports errors per snippet, and omits full output and call traces from the bulk
report. A failed assertion or test produces a nonzero CLI exit code.

The checked-in Unraid triage example has a synthetic 26-issue default fixture:

    labby snippet test unraid-linear-pr-triage --json

It expects seven bounded related-PR searches, one independent author search,
no handoff requests, ten total tool calls and less than 16,000 output bytes.
The names, issues and PR data in this fixture are synthetic, not a recording of
an account or production system.

## Fixture format

For a snippet that returns the result of github::get_me, a minimal JSON fixture
is:

    {
      "calls": [
        {
          "tool": "github::get_me",
          "match": {},
          "result": {"login": "fixture-user"},
          "times": 1
        }
      ],
      "expect": {"/login": "fixture-user"},
      "snapshot": {"login": "fixture-user"},
      "ignore_paths": [],
      "budgets": {
        "wall_clock_ms": 20000,
        "tool_calls": 1,
        "output_bytes": 16000
      }
    }

Rules are consumed in declaration order. Tool IDs match exactly. An omitted
match accepts any parameter object; a supplied match is a top-level subset,
with exact recursive equality for each supplied value. Additional parameters
are allowed unless included in the match. Each rule must be consumed exactly
times times, with a default of one.

A rule can reject instead of resolve:

    {"tool":"github::get_me","error":{"kind":"timeout","message":"Synthetic timeout"}}

An expected rejection counts as consumption. The snippet must still handle it
and satisfy its assertions. Caught unexpected calls, exhausted rules, unused
rules and unhandled exceptions fail the test. Output ok: false also fails unless
the fixture explicitly asserts /ok equals false, which supports negative tests.
Unknown fixture fields fail validation to catch misspelled controls.

Assertions use JSON Pointer. Missing values are different from explicit null.
An optional snapshot compares the complete output after replacing only the
selected ignore_paths with null on both sides. This can normalize timestamps
without masking unrelated fields. Diagnostics identify failed pointers without
echoing entire expected and actual values.

## Budgets and reports

Wall-clock budgets are 1 to 30,000 milliseconds; call budgets are 0 to 512;
output budgets are 1 to 16,000 UTF-8 bytes. A fixture may contain at most 512
rules, 512 total required calls and 512 KiB of serialized fixture data. There
may be at most 64 assertions and 64 normalization pointers.

Reports distinguish mock execution and include elapsed wall time, attempted
call count, peak overlapping synthetic calls, raw output bytes and a bytes/4
token estimate. The estimate is not a tokenizer measurement. Synthetic tool
latency is not a measurement of upstream performance.

The first 32 call records contain tool IDs, rule indices, status and timing,
but no parameters or response payloads. The full attempted count remains in
metrics when the trace is capped. Oversized result bodies are omitted from the
report and fail the output budget.

## Explicit live verification

    labby snippet test unraid-linear-pr-triage --live --json
    labby snippet test unraid-linear-pr-triage --live --param deep=true --json

The native test action accepts live: true for API/MCP callers. Only explicit
live execution initializes upstream connectivity. Supplying both a fixture
and live is rejected. Live execution uses normal caller authority and the
snippet's exact tool declaration; this harness grants no additional access.

Live reports identify their mode, include wall time, call counts by tool,
failed calls, raw output bytes, estimated tokens and result-shaping status.
Failed calls, absent results, an explicit ok: false, or changed/shaped output
make the live test fail. A complete: false result is a separate coverage signal
that consumers must inspect. Not all snippets define that field.

Migration: test previously executed live tools implicitly. Add --live (or
live: true in the native action payload) only for workflows intentionally
performing live execution. Fixture and all modes never silently fall back.

Route-scoped Code Mode does not permit nested codemode.run resolution. Use an
authorized native saved-snippet surface rather than widening a scoped run.

## Isolation and limits of the evidence

The fixture host has an empty tool catalog, no gateway manager and a deny-all,
read-only scope. The wrapper supplies synthetic callTool and the production
codemode.batch helper. Attempts to escape to the real host bridge fail. The
production subprocess deadline terminates infinite loops, and the runner pool
is shut down after each test.

Fixtures currently support callTool and codemode.batch. They do not emulate
search, describe, generated upstream helpers, nested snippets, resources or
local providers. They do not validate against freshly fetched upstream schemas.
Live schema discovery and a bounded live test remain separate compatibility
checks. Recording live responses, automatic redaction, fixture scaffolding and
snapshot-update commands are not part of this implementation.

## Repository verification

    cargo test -p labby-codemode --lib
    cargo test -p labby-codemode --test snippet_fixture_runtime
    cargo test -p labby --lib dispatch::snippets

The runtime integration target re-execs itself as the actual Code Mode runner;
it requires no installed Labby binary, credentials or live upstreams. It tests
successful and failing assertions, synthetic errors, unused/unexpected calls,
output/call budgets, batch overlap, real bridge denial, timeout containment and
26-issue fast/deep triage scenarios. The deep fixture expects 36 calls; the
fast fixture expects ten.

See [Snippets service](../services/SNIPPETS.md) for authority rules and
[snippet authoring](../snippets/README.md) for discovery and composition.
