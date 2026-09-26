# Snippet development and verification

Executable snippets are parsed and run by `labby-codemode`. The CLI, MCP and HTTP
test surfaces share `dispatch/snippets/testing.rs`. Fixture tests use the same
Javy/QuickJS subprocess as production, without a gateway or live credentials.

## Develop without contacting upstream services

Validate a source file before saving it:

```sh
labby snippet validate unraid-linear-pr-triage --file docs/snippets/unraid-linear-pr-triage.md
```

Save the snippet with the normal `labby snippet add` workflow, then test it with
synthetic data. An existing user snippet shadows the built-in entry of the same
name; the report therefore names the resolved entry, not an assumed source.

```sh
labby --json snippet test unraid-linear-pr-triage
labby --json snippet test unraid-linear-pr-triage --fixture docs/snippets/unraid-linear-pr-triage.test.json
labby --json snippet test unraid-linear-pr-triage --fixture docs/snippets/unraid-linear-pr-triage.deep.test.json --param deep=true
```

Without `--fixture`, the test reads a sibling `<name>.test.json` next to the
resolved snippet. A missing fixture fails explicitly. It never falls back to
live execution. `--all` uses each resolved snippet's sibling fixture, processes
at most 100 names, and reports missing fixtures as failures. It does not silently
skip untested snippets. Bulk reports retain diagnostics and metrics but omit
per-snippet outputs and traces to bound response growth.

## Fixture format

```json
{
  "calls": [
    {
      "tool": "example::read_item",
      "match": { "id": 7 },
      "result": { "ok": true, "count": 2 },
      "times": 1
    }
  ],
  "expect": { "/ok": true, "/count": 2 },
  "budgets": {
    "wall_clock_ms": 20000,
    "tool_calls": 40,
    "output_bytes": 16000
  }
}
```

A rule matches an exact `upstream::tool` identifier and an optional top-level
parameter subset. Nested parameter values compare exactly. Rules are consumed
in declaration order among matching candidates. All required counts must be
consumed. Unexpected calls fail the test even when snippet code catches the
rejection. Unknown fixture keys are rejected to catch misspelled assertions or
budgets. Reserved local provider namespaces cannot be used as mock upstreams.

Use `error: {"kind":"rate_limited","message":"synthetic rejection"}` instead
of a result to exercise failure handling. A returned `ok: false` normally fails
the test; an explicit `expect: {"/ok":false}` permits an intentional negative
test. Other assertion failures, unused rules, unexpected calls and budget
violations still fail. Missing fields are distinct from explicit JSON null.

An optional `snapshot` compares the complete output. `ignore_paths`, expressed
as JSON Pointers, replaces only those selected values with null on both sides
before snapshot comparison. Pointer assertions in `expect` are not normalized.
Snapshots and fixtures should contain synthetic or deliberately sanitized data,
never credentials or copied private payloads by default.

## Isolation and resource limits

Only lexical `callTool` is mocked. `codemode.batch` is the production batch
implementation; generated helper APIs, nested snippet resolution and live
catalog discovery are not emulated in this first implementation. A separate
empty host with an explicit deny-all, read-only scope blocks attempts to reach
the real bridge or local providers. Artifact writes are unavailable. The CLI
does not initialize configured upstreams for fixture tests.

The fixture file is limited to 512 KiB. The combined source, fixture and input
must also fit the existing Code Mode source limit after wrapping. Budgets accept
1 to 30,000 milliseconds, 0 to 512 attempted synthetic tool calls, and 1 to
16,000 raw UTF-8 output bytes. The runner's memory and stack limits still apply.
Timeouts remain structured execution errors. An infinite loop cannot evade the
runner deadline by bypassing the mock call counter.

Reports contain pass/fail, bounded diagnostics, elapsed time, attempted calls,
raw output bytes, a bytes/4 token estimate and peak overlapping mock calls.
The trace contains at most 32 entries with identifiers and synthetic timings,
without parameters or tool response payloads. An oversized output is omitted.
Synthetic call timing is not a live upstream latency benchmark. CLI assertion
failures return a nonzero exit status after printing the JSON report.

## Explicit live verification

```sh
labby --json snippet test unraid-linear-pr-triage --live
labby --json snippet test unraid-linear-pr-triage --live --param deep=true
```

Live mode uses the caller's existing authority and configured upstreams. It
reports real call counts, failed calls, raw output size and execution latency.
Existing Code Mode runtime limits remain authoritative. Fixture assertions and
fixture-specific budgets do not apply to `--live`; compare its measured metrics
with the acceptance criteria for the snippet. `--fixture` and `--live` conflict.
Use only reviewed read-only snippets for unattended live smoke tests.

Live catalog contract validation, automatic recordings, scaffolding and mocks
for generated helpers are not implemented by this fixture runner.

## Unraid triage contract

The response retains the live `schemaVersion: 2` contract. `pullRequests` stores
each PR once, keyed by `owner/repo#number`; `myOpenPRs` and per-issue PR lists
contain those keys. Resolve them through `pullRequests`, not as inline objects.
Independent upstream work runs in batches of at most eight.

The fast path fetches assigned started Linear issues and identity, then searches
open PRs in batches of at most four issue identifiers. The default scope is the
entire `unraid` organization, including `unraid/cloudflare`. History and compact
handoffs require `deep=true` or the individual opt-in flags. PR matching uses
whole identifiers; release-summary body references are filtered by default.

The output is bounded to 16,000 UTF-8 bytes. Incomplete upstream pages, failures
and output omissions are explicit and must not be interpreted as proof of no
matching PR. Historical merge state remains null when the search response does
not provide enough information to distinguish merged from closed.

The supplied fast fixture requires 10 calls; the deep fixture requires 36.
Both use 26 synthetic issues and exercise Cloudflare visibility. The published
triage Markdown validates its input in JavaScript and deliberately omits named
input frontmatter for compatibility with deployed Labby 2.3.1, whose parser
rejects camelCase input metadata. The framework parser fix in this change allows
camelCase JSON input keys without relaxing filesystem-backed snippet names.

## Repository checks

```sh
cargo test -p labby-codemode --lib
cargo test -p labby --test snippet_harness_cli -- --test-threads=2
cargo test -p labby --lib dispatch::snippets
just docs-generate
just docs-check
```

The CLI integration suite launches the built Labby binary with a temporary
installation and a cleared environment. It exercises real runner execution,
snapshots, parameter types, budgets, missing fixtures, denied bridge escapes,
malformed source and both triage modes.

See [Code Mode](CODE_MODE.md), [Testing](TESTING.md), and the
[snippet examples](../snippets/README.md).
