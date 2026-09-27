# Snippet authoring and offline fixture tests

## Entry points and safety

The offline fixture harness is a reusable labby-codemode module plus the
snippet_harness developer example. It runs the production QuickJS parser and
isolated runner, replaces callTool with synthetic responses, and uses the native
codemode.batch implementation. It does not initialize a gateway or contact
upstream services. The host denies tool, resource, nested-snippet, local-provider,
and artifact access. An attempted escape to the real host bridge fails the test.

This change does NOT replace the existing labby snippet test command. That
command still runs live snippets. Do not use labby snippet test --all as a linter:
snippets may perform writes. Fixture execution is available through the commands
below, not through an invented --fixture or --live flag on the deployed CLI.

## Build and run

From the repository root:

```sh
cargo build -p labby-codemode --example snippet_harness
"${CARGO_TARGET_DIR:-target}/debug/examples/snippet_harness" \
  docs/snippets/unraid-linear-pr-triage-v2.md \
  crates/labby-codemode/tests/fixtures/snippet-harness/fast.json

"${CARGO_TARGET_DIR:-target}/debug/examples/snippet_harness" \
  docs/snippets/unraid-linear-pr-triage-v2.md \
  crates/labby-codemode/tests/fixtures/snippet-harness/deep.json \
  '{"deep":true}'
```

The example is self-hosting: its internal code-mode-runner invocation launches
the same compiled binary. Keep the example and library from the same checkout.
Exit status 0 means assertions passed, 1 means a completed test failed, and 2
means invalid input, parser failure, or sandbox execution failure.

Print the machine-generated fixture JSON Schema:

```sh
"${CARGO_TARGET_DIR:-target}/debug/examples/snippet_harness" --schema
```

Static validation does not execute the snippet body:

```sh
labby snippet validate unraid-linear-pr-triage-v2 \
  --file docs/snippets/unraid-linear-pr-triage-v2.md --json
```

## Fixture contract

A fixture contains synthetic calls, assertions, optional snapshots, and budgets.
For example, an identity-only snippet using the verified github::get_me tool can
be tested without making any GitHub request:

```json
{
  "calls": [
    {"tool": "github::get_me", "result": {"login": "fixture-user"}}
  ],
  "expect": {"/login": "fixture-user"},
  "absent": ["/token"],
  "budgets": {"tool_calls": 1, "output_bytes": 1000, "wall_clock_ms": 2000}
}
```

Rules match the exact upstream::tool ID and an optional top-level parameter
subset named match. Nested values compare exactly, with object-key order ignored.
Rules are consumed in declaration order among matching rules. The times field
defaults to one. Every required occurrence must be consumed; unused rules fail.
Responses are JSON-cloned so a snippet cannot mutate the fixture response shared
by a later call. An error object with kind and message produces a synthetic
rejection. Unexpected calls fail even when the snippet catches their exceptions.

Expect keys are JSON Pointers and compare exact JSON values. A missing field is
not equal to null. The absent list asserts actual omission, useful for proving
that history and handoffs are absent in the default triage mode. Snapshot compares
the whole result. Ignore_paths replaces selected values with null on both sides;
use it for timestamps, not substantive fields. Fixtures that deliberately test a
negative result must explicitly expect /ok to be false.

Defaults are 20 seconds, 40 calls, and 16,000 raw output bytes. Hard limits are
30 seconds, 512 calls, and 16,000 output bytes. The fixture itself is bounded to
512 KiB. Oversized results fail before display shaping can conceal them. Reports
include wall time, attempted synthetic calls, raw UTF-8 bytes, a bytes/4 token
estimate, peak synthetic concurrency, and at most 32 trace entries. Synthetic
latency is not a live service benchmark. Inputs and response payloads are not
included in the trace metadata.

## Authoring workflow

Discover the exact upstream tool through Labby, describe it, and verify its
namespace. Declare the exact tool IDs in the snippet frontmatter. Start from a
small synthetic response, assert useful output fields, then add failure and
partial-data cases. Use the generated schema to catch misspelled fixture keys.

The existing frontmatter parser only accepts lower-case declared input names.
The triage snippet keeps its pre-existing camel-case JavaScript interface by
validating those inputs before tool dispatch instead of publishing invalid input
metadata. Unknown declared inputs remain rejected by the production input merger.

The offline facade currently supports callTool and codemode.batch. Generated
codemode namespace helpers, catalog search/describe, nested snippets, recording
live responses, and automatic fixture scaffolding are not implemented here.
Fixtures must be synthetic or deliberately sanitized. Do not paste credentials
or raw production responses into committed fixtures.

## Regression checks

```sh
cargo test -p labby-codemode --all-targets -- --test-threads=2
cargo build -p labby-codemode --example snippet_harness
python3 crates/labby-codemode/tests/snippet_harness_acceptance.py \
  "${CARGO_TARGET_DIR:-target}/debug/examples/snippet_harness"
```

The acceptance script verifies the actual executable, including default/deep
triage, whole-identifier matching, release noise, cross-repository personal PRs,
pagination and page limits, rate-limit diagnostics, upstream failures, incorrect
assertions, absent runtime APIs, swallowed unexpected calls, real-host bridge
denial, malformed JavaScript, runaway deadlines, and normalized snapshots.
Expected negative cases must fail with the correct class of exit status.
The Python driver is not yet wired into the repository CI workflow.

## Live verification and output compatibility

Run live checks separately with an authenticated Labby route and the snippet's
four-tool allowlist. Respect GitHub search reset intervals; do not run repeated
search-heavy probes back to back while the shared quota is exhausted. Record
both ok and complete, output bytes, call counts, missing evidence, and the source
revision. No failure is evidence that a PR does not exist.

Triage output schema version 2 normalizes PR records into pullRequests. The
myOpenPRs list and issue openPRs/historicalPRs contain owner/repo#number references.
Consumers of the previous nested-object shape must use the referenced record.
Historical fields and handoffs are absent unless requested. A closed PR with
merged: null has unknown merge state, not proof it was unmerged. Summary counts
refer to retrieved data before any explicit output-budget omissions.

A broad organization inventory was tested but rejected as the default because
it required more than three 100-PR pages in this environment. The chosen default
uses bounded four-identifier searches and separate personal-PR pagination.
