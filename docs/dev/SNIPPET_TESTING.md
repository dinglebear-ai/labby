---
title: Snippet development and testing
created: 2026-09-27
updated: 2026-09-29
---

# Snippet development and testing

Saved snippets have three separate checks: source validation, deterministic
fixture execution, and explicitly requested live execution. CLI, MCP and HTTP
share the dispatch implementation; the fixture engine lives in
`labby-codemode::snippet::harness`.

## Commands

Use the native singular `snippet` command group:

~~~sh
# Parse the frontmatter and JavaScript without executing the snippet.
labby snippet validate unraid-linear-pr-triage
labby snippet validate example --file ./example.md

# Offline fixture execution. No configured gateway is required.
labby --json snippet test unraid-linear-pr-triage
labby --json snippet test unraid-linear-pr-triage \
  --fixture docs/snippets/unraid-linear-pr-triage.deep.test.json \
  --param deep=true

# Every listed snippet must have a usable adjacent fixture.
labby --json snippet test --all

# Explicit opt-in to real upstream operations.
labby --json snippet test unraid-linear-pr-triage --live
~~~

A named test uses an adjacent `<name>.test.json` unless `--fixture` supplies
another JSON file. Resolution follows the selected built-in or user snippet,
so a user override uses its own adjacent fixture. `--all` tests up to 100 unique
listed names sequentially; an empty set or any failing member fails the run.
A missing or malformed fixture is a failure, not permission
to fall back to live execution. Existing automation that intentionally exercised
live tools through `snippet test` must add `--live`.

With `--json`, tests emit structured reports; validation or runner failures can
instead return a structured action error. The CLI exits nonzero on failure. The
shared `snippets.test` action accepts `name` or `all`, `params`, `live`, and an
inline `fixture` object. File paths are resolved only by the local CLI; remote
callers send fixture contents rather than arbitrary local paths.

Live tests execute the snippet's real operations, which may include writes for
other snippets. They preserve the caller and declared tool scope. Review a
snippet before opting into live execution. The Unraid triage example is read-only.

## Fixture contract

~~~json
{
  "calls": [
    {
      "tool": "synthetic::read",
      "match": {"limit": 2},
      "result": {"items": ["one", "two"]},
      "times": 1
    }
  ],
  "expect": {"/ok": true, "/count": 2},
  "snapshot": {"ok": true, "count": 2, "generatedAt": null},
  "ignore_paths": ["/generatedAt"],
  "budgets": {
    "wall_clock_ms": 20000,
    "tool_calls": 40,
    "output_bytes": 16000
  }
}
~~~

Fixtures contain synthetic data, not automatically recorded live responses.
Do not store credentials, private signed URLs, personal messages, or raw
production payloads in fixtures.

Rules match an exact qualified tool identifier and, optionally, a subset of its
top-level parameters. Nested values compare exactly and object key order does
not matter. Rules are considered in declaration order. Every rule must be fully
consumed, and `times` defaults to one. Omit `match` to accept any parameters for
that exact tool. This is a response-matching contract, not automatic validation
against a live upstream's input schema.

A rule can provide an `error` object with `kind` and `message` instead of a
non-null result. Expected failures can be asserted, including `"/ok": false`.
Unexpected or over-budget calls fail the test even when the snippet catches
their JavaScript errors. Unused rules also fail, preventing accidentally skipped
work from appearing green.

`expect` keys are RFC 6901 JSON Pointers; malformed `~` escapes are rejected. Equality distinguishes missing properties from
explicit null values. `snapshot` compares complete output, including an explicitly supplied JSON null; `ignore_paths`
normalizes selected volatile values on both sides to null. Use narrowly scoped
paths, such as one timestamp, rather than suppressing meaningful output.

Fixture JSON is bounded to 512 KiB. Rules and total expected calls are bounded to
512. A fixture may set at most 64 equality assertions, 64 absence assertions, and 64 normalization paths. The
wall-clock budget is 1 to 30,000 milliseconds, call budget 0 to 512, and output
budget 1 to 16,000 UTF-8 bytes. The snippet plus serialized input must fit the
configured production
source limit (at most 1 MiB). The harness then embeds that admitted invocation
and bounded fixture data in a separate wrapper with a 10 MiB source ceiling;
that larger allowance is private to fixture execution.

## Execution and isolation

The harness reuses the production snippet parser, input/default merging,
Code Mode broker, QuickJS runner subprocess, and native `codemode.batch`.
It replaces lexical `callTool` with fixture matching. Its host has no live
gateway and runs with an explicit deny-all, read-only scope. Escaping to the
real global tool bridge is denied and reported. Local providers, artifact
writes, resources, and nested snippet execution cannot reach live state.

This first harness surface supports snippets using `callTool` and
`codemode.batch`. Synthetic discovery, generated helper methods,
`codemode.run`, resources, artifact fixtures, automatic recording, and live
schema-contract testing are not implemented. Unsupported fixture behavior
fails instead of silently reaching a real service.

The report includes elapsed time, attempted call count, raw output bytes,
a bytes/4 token estimate, and peak overlapping synthetic calls. Its synthetic
trace is capped at 32 entries and contains tool IDs, status, rule index and
elapsed time, not parameters or upstream payloads. An over-budget result is
omitted rather than echoed in full. Bulk reports omit individual result bodies
and traces. Fixture latency is not a live upstream performance measurement.

Live reports include elapsed time, calls by tool, failed calls, raw output size,
and shaping status. A live report does not turn incomplete upstream pagination
or a snippet's intentional compacting into complete data. Inspect the snippet's
own completeness and omission fields as well as the test status.

## Unraid triage regression examples

### Original triage snippet

The example discovers assigned U8 started issues and searches the whole Unraid
organization, including `unraid/cloudflare`. It uses up to four identifiers per
GitHub search. The first batch loads issues and GitHub identity; the second
contains related PR searches, the current user's open PR search, and optional
handoffs. Fan-out is bounded. Default execution requests open PRs only and omits
history and handoff properties entirely.

`deep=true` enables historical PR matching and compact handoffs. History uses
one all-state search per identifier chunk rather than separate open and closed
requests. For the 26-issue fixture, the fast path attempts 10 calls, and deep
mode 36. The fixtures exercise organization-wide coverage, identifier boundary
matching, release-noise exclusion, an unmatched issue, and compact handoffs.

Output is deliberately bounded below 16 KB. The snippet reports failed queries,
rate-limit retry guidance, upstream page limits, and output omissions. A
rate-limited query is not evidence that an issue has no PR. Repeated live
benchmarks share the GitHub search quota with other clients; respect reset
information rather than immediately repeating a failing batch.

### Version 2

[unraid-linear-pr-triage-v2](../snippets/unraid-linear-pr-triage-v2.md) has a
separate result schema and fixture family. It stores PRs once and returns keys
from issue and personal-PR lists. Unlike the original snippet, deep mode uses
separate open and closed searches, and GitHub searches paginate (three pages
by default). Linear issue discovery remains one page with a continuation cursor.
Search and handoff batches each allow four concurrent jobs, with an 80-call
ceiling. The adjacent v2 fixture uses four calls; the harness acceptance
fixtures use four for fast mode and seven for deep mode. The original
26-issue fixtures and their 10/36-call counts do not apply to v2.

The acceptance matrix in `crates/labby-codemode/tests/snippet_harness_acceptance.py`
and product `snippet_harness` tests cover fast/deep output, pagination and page
budgets, quota and worker failures, absent fields, snapshots, forbidden host
access, malformed source, and timeouts. Expected error cases are part of the
matrix, not fixtures that should all report `passed: true`. Fixtures control
tool responses, not the clock: elapsed times and date-based attention signals
remain variable. Normalize volatile fields for snapshots.

The v2 snippet accepts `maxOutputBytes` up to 20,000, but the fixture harness
allows at most 16,000 raw result bytes. A larger snippet setting does not raise
that harness ceiling. Inspect `complete` and coverage gaps separately from
`passed`; deliberately incomplete results can satisfy a fixture.

## Focused verification

~~~sh
cargo test -p labby-codemode --lib snippet::
cargo test -p labby --test snippet_fixture_runtime --test snippet_harness \
  --test snippet_harness_regressions --test snippet_triage_v2_regressions
cargo test -p labby --lib dispatch::snippets
cargo fmt --all -- --check
just docs-generate
just docs-check
~~~

Run from the intended worktree so its Cargo configuration applies. Keep local
fixture results, live benchmark results, source publication, and deployed
runtime verification separate in release evidence.

## Acceptance suite

The offline example and command-contract cases exercise the same harness used by product dispatch:

```sh
cargo build -p labby-codemode --example snippet_harness
python3 crates/labby-codemode/tests/snippet_harness_acceptance.py target/debug/examples/snippet_harness
```

The product integration suite runs the v2 triage fixture matrix in CI as well.

Fixtures also support `absent`, a list of JSON Pointers that must not exist. A present field containing `null` fails this assertion.
