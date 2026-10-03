---
title: Snippet development and testing
created: 2026-09-27
updated: 2026-10-02
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

## Generating schema-backed fixtures

`snippet fixture` creates a deterministic, editable draft without executing the
snippet or invoking its tools. By default it reads caller-visible tool schemas
over the selected gateway's authenticated MCP connection for the snippet's exact tool declarations. The snippet body and fixture generation remain local. Supply
`--schemas` to use saved contracts entirely offline. Legacy snippets without
tool declarations require `--tool upstream::tool` (repeat as needed) or an explicit saved schema map; dynamic JavaScript calls are not inferred. MCP operator schema resources require unscoped admin access. Remote discovery failures never fall back to local schemas.

~~~sh
# Discover schemas on the selected gateway and save a new fixture.
labby --json snippet fixture example --param query=synthetic \
  --output ./example.test.json

# Generate entirely offline from a saved schema map.
labby --json snippet fixture example --schemas ./schemas.json \
  --variant minimal --output ./example.minimal.test.json

# Explicit synthetic responses for missing or difficult output contracts.
labby --json snippet fixture example --schemas ./schemas.json \
  --results ./results.json --output ./example.test.json

# Compare saved contracts with current gateway metadata; execute no tools.
labby --json snippet fixture example --check ./example.test.json

labby --json snippet test example --fixture ./example.test.json
~~~

The saved schema map is keyed by exact tool ID:

~~~json
{
  "synthetic::lookup": {
    "input_schema": {
      "type": "object", "required": ["id"],
      "properties": {"id": {"type": "integer"}}
    },
    "output_schema": {
      "type": "object", "required": ["items"],
      "properties": {"items": {"type": "array", "items": {"type": "string"}}}
    }
  }
}
~~~

`results.json` is another map, for example
`{"synthetic::lookup":{"items":["synthetic-item"]}}`. Explicit null responses
are preserved. Missing output schemas require an explicit result; missing tool
metadata requires a saved contract. Missing input schemas produce a warning.
Missing or un-generatable responses return `ready: false`, diagnostic failures
and a partial draft. Invalid saved contracts, explicit response overrides and
fixture admission failures return an error without a draft. Both cases exit
nonzero and do not write the requested output file. Existing output files are
never overwritten. Complete fixture files are published atomically, so a failed
write does not leave a partial file at the requested path.

The default `populated` variant includes optional object fields and one array
item when allowed. `minimal` includes required fields and minimum array lengths.
Values are synthetic and deterministic; schema defaults and examples are not
copied. Local references, enums, constants, objects, arrays, primitive types and
simple bounds are supported. Constraint combinations the generator cannot
satisfy, including `allOf` generation and some patterns/unions, require explicit
response overrides. Unsupported validation keywords such as `format` and
`multipleOf`, remote references and excessive depth/work fail explicitly.

Generated contracts include a SHA-256 `fingerprint` over canonical input and output schemas. Offline tests reject a fingerprint that no longer matches its saved contract. `--check` compares saved and current contracts, lists changed or missing tool IDs in `changed_tools`, exits nonzero on drift, and leaves the saved fixture unchanged. A check never executes upstream tools and requires no execution inputs, including required snippet parameters. Saved fixtures are validated before discovery; current contracts must also pass structural validation. Older fixtures without fingerprints can still be checked using their saved schemas.

When Code Mode truncates a schema response, discovery reads the same authenticated gateway metadata resource directly. Permission failures remain errors, and selected contracts retain the 128 KiB limit.

Older gateways may omit output schemas from their schema resources even when native MCP tools expose them. The CLI can recover a native output contract when the namespaced identity matches exactly, or when a read-only bare name resolves uniquely to the requested tool and its input contract matches. Ambiguous names and Labby service identities are never used as upstream aliases. Otherwise an explicit synthetic result is required. On older gateways, ordinary tools hidden by the Code Mode projection have no native JSON output contract to recover; supply `--results` or a saved `--schemas` map for those tools. Visible MCP App tools can still provide native output contracts. JSON schemas are never inferred from TypeScript descriptions.

The draft contains one response rule per selected tool and no invented assertions.
Edit matching, repeated calls, conditional paths, pagination and expected output
to reflect the workflow. `ready` means the fixture is structurally usable, not
that it covers every branch or will pass before these edits.

Fixtures may include a `schemas` map with the same shape. Successful mocked
responses are validated before execution; synthetic error rules bypass output
validation. Actual tool arguments are checked after execution, independently of
subset `match` rules, using the production tool-argument validator's supported
subset. A violated input contract fails the test even if the snippet returns
success. Argument snapshots used internally for validation are bounded to
512 KiB per run and are discarded from the public report and trace. Schema and
value validation also enforce bounded aggregate work across the fixture; each
response-validation and argument-validation phase allows at most 16,384 schema
node visits across its rules or calls. A fixture that exceeds the work budget
fails explicitly. The measured wall-clock budget includes schema validation
before and after snippet execution. These saved
contracts never trigger live catalog discovery and do not establish that a
deployed upstream still follows the same schema.

The shared `snippets.fixture` action takes `name`, optional `tools` (exact tool
IDs), `check` (a saved fixture object), `params`, `schemas`, `results` and
`variant`. It returns `fixture`, `ready`, `coverage`, `warnings` and `failures`,
plus `changed_tools` when a contract check detects drift; it does not save files.
For a shared-action check, pass the saved fixture as data rather than a filename:

~~~json
{
  "action": "snippets.fixture",
  "params": {
    "name": "example",
    "tools": ["synthetic::lookup"],
    "check": {
      "schemas": {
        "synthetic::lookup": {
          "input_schema": {"type": "object"},
          "output_schema": {"type": "boolean"}
        }
      },
      "calls": [{"tool": "synthetic::lookup", "result": true}]
    }
  }
}
~~~

CLI, HTTP and MCP retain normal admin and caller/tool-scope checks for metadata
discovery.

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
that exact tool. Optional saved `schemas` separately check actual arguments;
tests never fetch a live upstream's input schema.

`params` supplies fixture input defaults; explicit CLI/API caller parameters
override matching keys before production input validation and default merging.
This lets required-input examples remain runnable with an adjacent fixture.

`snippets` uses rules with `name`, optional `match`, `result` or `error`, and
`times` for synthetic child invocations. `artifacts` uses rules with exact
relative `path`, optional `content_type`, required literal content fragments in
`contains`, and `times`. For example:

~~~json
{
  "params": {"alias": "fixture-host"},
  "snippets": [{"name": "docker-host-inventory", "match": {"alias": "fixture-host"}, "result": {"ok": true}}],
  "artifacts": [{"path": "report/inventory.json", "content_type": "application/json", "contains": ["fixture-host"]}]
}
~~~

All three rule families share consumption checks and the invocation budget.
Nested calls appear in traces as `snippet::<name>` and artifact writes as
`artifact::write`. Unexpected writes or content mismatches fail even when the
workflow catches the error. Artifact content is bounded to 512 KiB; a fixture
may provide at most 64 fragments per artifact and 64 input keys.

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

The harness supports `callTool`, native `codemode.batch`, synthetic nested
`codemode.run` results, and checked synthetic `writeArtifact` writes. Nested
fixtures supply responses rather than executing the child snippet; test child
workflows separately. Artifact content stays in memory and the returned receipt
contains `mode: "mock"`; no artifact file is created. Synthetic discovery,
generated helper methods, resources, automatic recording, and live schema
contract testing remain unsupported. Unsupported behavior fails instead of
reaching a real service.

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

All ten shipped executable built-ins are covered by adjacent offline fixtures.
Run `cargo test -p labby --all-features --test builtin_snippet_fixtures` to enforce
dependency declarations, fixture presence, production runner execution, input
override precedence, and the synthetic artifact filesystem boundary.

### Failure-path evidence

The built-in fixture integration suite also runs mixed parallel successes and a
synthetic timeout rejection through native `codemode.batch`, preserving the
success indices and the rejection's `kind` and `message`. Synthetic fixture
errors use the same JSON-message decoder as live tool errors. An undeclared
response rule remains an unexpected mock call even when other batch jobs
succeed or the workflow catches the rejection.

The suite separately exercises a genuinely unresolved JavaScript promise under
a short production-runner deadline. Artifact scenarios test exactly 512 KiB of
UTF-8 content, the first value above that limit, and an unexpected escaping path.
These artifact checks validate the synthetic harness; real artifact storage and
filesystem containment are tested by the artifact runtime and service suites.

`cargo test -p labby-codemode --lib runner_drive::cancellation_tests` exercises
the actual runner protocol and broker: aborting an execution after its host call
starts must drop the pending host future and reap its owned subprocess. This
establishes local cancellation cleanup, not a remote upstream cancellation
acknowledgement. Fixture response rules alone cannot establish cancellation or
real upstream timeout behavior.
