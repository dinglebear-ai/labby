---
title: "Snippets Service"
created: "2026-08-18"
updated: "2026-09-26"
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

For native saved-snippet execution (`snippets.exec` and live `snippets.test`), Labby
intersects a declaration with the caller's existing Code Mode policy before
building the catalog. The declaration can narrow authority but never grant it:
omission keeps the legacy caller scope, `[]` denies all upstream tools, and a
nonempty list exposes only those exact dependencies. This also keeps one-shot
snippet runs from cold-probing unrelated gateway upstreams.

Nested `codemode.run()` inherits the already-established execution scope. Trusted
local saved snippets may compose inside that declared scope; route-scoped callers
still cannot use nested snippet resolution to widen their authority.

## Administrative Actions

Reading snippet bodies, executing or testing snippets, creating/removing snippets, and promotion flows require the scopes shown in the generated catalog. Promotion and removal are destructive actions.

Built-in snippets are read-only through the user-snippet mutation surface. Explicit shadowing is required before a promoted user snippet may replace a built-in name.

## Execution

Snippet code must evaluate to an async arrow function and executes inside the same bounded Javy/QuickJS Code Mode runtime used by gateway Code Mode. Live tool calls are resolved through the gateway catalog rather than guessed or hard-coded at the host boundary. Offline tests substitute a finite fixture catalog and in-memory responses.

## Testing

`snippets.test` accepts an inline `fixture` for offline execution, or requires
`live: true` to call configured upstreams. The CLI equivalents are
`labby snippet test NAME --fixture FILE` and `labby snippet test NAME --live`.
Fixture tests run through the same production sandbox, but their authority is
limited to finite fixture tool rules. Snippet declarations can narrow that scope.
There is no fallback from missing mocks to live tools.

Fixtures support exact call arguments, injected failures, JSON-pointer assertions,
normalized snapshots, and execution/result budgets. Failed fixture tests return
`passed: false`; the CLI also exits unsuccessfully. See
[Snippet fixture testing](../dev/snippet-testing.md) for the complete contract and
limitations, including the distinction between offline assertions and live smoke tests.

## Related Docs

- [Code Mode](../dev/CODE_MODE.md)
- [Snippet authoring](../snippets/README.md)
- [Gateway](./GATEWAY.md)
- [Service model](../dev/SERVICES.md)
