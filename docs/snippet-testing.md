# Testing Code Mode snippets

The native command is `labby snippet test`. Choose an offline fixture or explicitly
opt into live calls. A failed assertion, unexpected tool failure, missing result,
truncated display, unused fixture rule, or exceeded performance threshold makes the
report fail and the CLI exit nonzero. A successful MCP or HTTP transport is not a
passing test: inspect the report's `passed` field.

## Offline fixture execution

From the repository root, with a binary built from this checkout:

~~~sh
LABBY_HOME="$(mktemp -d)" labby snippet test unraid-linear-pr-triage \
  --fixture crates/labby-codemode/tests/fixtures/snippets/unraid-triage-fast.json --json

LABBY_HOME="$(mktemp -d)" labby snippet test unraid-linear-pr-triage \
  --fixture crates/labby-codemode/tests/fixtures/snippets/unraid-triage-deep.json \
  --param deep=true --json
~~~

For arrays or nested objects, use `--input` with a JSON object rather than
`--param`, which intentionally coerces scalar values only. For example:

~~~sh
labby snippet test unraid-linear-pr-triage \
  --fixture crates/labby-codemode/tests/fixtures/snippets/unraid-triage-repo.json \
  --input '{"repos":["unraid/unraid-e2e"]}' --json
~~~

`--input` is also available on `snippet run`; it conflicts with `--param` and,
for tests, `--all`. Malformed JSON and input larger than 1 MiB are rejected.

Fixture mode runs the production QuickJS runner and broker against an in-memory
fixture host. It does not initialize a gateway, carry upstream credentials, or
fall back to live tools. An explicit read-only scope blocks local state, Git,
OpenAPI, and artifact writes, including when the fixture contains no tools.

The shared `snippets.test` action accepts the fixture as an inline JSON object, not
a server-side path. The CLI alone reads `--fixture`; both paths limit JSON to 1 MiB.
The action remains admin-only. Fixture mode does not grant extra caller authority.

Example fixture for a snippet that calls `fake::read` and returns its response:

~~~json
{
  "calls": [
    {
      "tool": "fake::read",
      "params": { "id": 7 },
      "response": { "returns": { "ok": true, "count": 2 } },
      "times": 1
    }
  ],
  "expect": { "/ok": true, "/count": 2 },
  "budgets": { "wall_clock_ms": 20000, "tool_calls": 1, "output_bytes": 16000 }
}
~~~

Rules match exact tool IDs and recursive object parameter subsets. Arrays and
scalar values match exactly. Each rule must be consumed exactly `times` times;
matching precedence follows fixture order. Extra calls cannot reach real services.
Prefer parameter-specific rules for concurrent calls rather than relying on arrival
order. There may be at most 512 required calls.

Error injection uses `"response": {"error": {"kind": "upstream_timeout",
"message": "synthetic timeout"}}`. Set `expected_failures` to the exact count of
injected error responses. A count without matching error rules is invalid; it
cannot conceal an unmatched call. Error-path cases must assert the intended result
explicitly, for example `"expect": {"/ok": false}`.

## Assertions, snapshots, and measurements

`expect` keys are JSON pointers. At least one assertion or a snapshot is required.
An empty pointer addresses the entire result, including an explicit JSON null.
`snapshot` compares the complete unshaped result. Optional `ignore_paths` replaces
only explicitly named, existing values with null on both sides before comparison.
There is no automatic timestamp scrubbing or snapshot update that could conceal a
regression. Use synthetic fixture data, never captured tokens or personal records.

The report includes elapsed milliseconds, attempted and failed call counts,
per-upstream counts, serialized UTF-8 result bytes, full response-envelope bytes,
truncation status, and a compact per-call timing table. Arguments and upstream
response bodies are excluded from the timing table. Result values that exceed the
output threshold are omitted from the report rather than emitted unbounded.

Budgets are acceptance thresholds, not transactional rollback or universal resource
limits. The offline wall-clock value also configures the runner timeout. The call
threshold is checked after execution; the engine's configured global call ceiling
still applies. Result bytes are measured before display shaping. Fixture input is
bounded before execution, but do not use output assertions as a memory isolation
mechanism. The complete report can be larger than its result-only byte threshold.

## Explicit live tests

~~~sh
labby snippet test unraid-linear-pr-triage --live --json
labby snippet test unraid-linear-pr-triage --live --param deep=true \
  --max-runtime-ms 20000 --max-calls 40 --max-output-bytes 16000 --json
~~~

Live mode invokes real configured upstreams with the caller's existing authority
and the snippet's declared tool scope. Only run snippets whose effects you intend.
`--all --live` explicitly runs every available snippet with default parameters; it
is not a safe offline CI command. Live thresholds are post-execution assertions and
do not override the gateway's configured runtime limits. No test can undo upstream
writes that completed before failure or timeout.

Both `fixture` and `live: true`, or neither, are rejected. Existing automation that
used implicit live `snippet test` must add `--live` or supply a fixture. The native
report now exposes `result`, `metrics`, `calls`, and `violations` rather than the
legacy nested `response` field.

## Regression suite

~~~sh
cargo build -p labby --all-features --bin labby
LABBY_CODE_MODE_RUNNER_EXE="$PWD/target/debug/labby" \
  cargo test -p labby-codemode --lib snippet -- --test-threads=2
cargo test -p labby --all-features --lib snippets -- --test-threads=2
~~~

The runner executable must be an absolute, owned, non-group/world-writable Labby
binary. Ordinary Rust test executables are not Code Mode runner entrypoints.
Build from the same checkout; do not silently skip execution tests when a runner is
missing. Use an isolated `LABBY_HOME` for product tests and local CLI verification.

The checked-in triage cases cover 26 issues, the 10-call default, the 36-call deep
path, repository overrides, organization-wide own PRs, release/changelog noise,
whole-issue-ID matching, empty issues, partial failures, pagination, call-budget
preflight, and oversized output. Harness tests cover snapshots, unused/extra calls,
intentional errors, UTF-8 accounting, input preservation, and timeout cleanup.

This first slice does not implement automatic fixture capture, schema-contract
comparison against live upstreams, scaffold generation, nested snippet fixtures,
or an offline `--all` suite. Fixture tool schemas are synthetic, so fixture success
is not evidence that an upstream's current schema is compatible. Discover and
inspect live tool contracts separately, then opt into bounded live verification.

See [the triage snippet](snippets/unraid-linear-pr-triage.md) for its version-2 PR-reference
output contract, or [the snippet overview](snippets/README.md) for authoring guidance.
