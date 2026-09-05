---
title: "Snippets Service"
created: "2026-08-18"
updated: "2026-08-18"
---

# Snippets Service

The `snippets` service is Labby's surface for reusable Code Mode workflows. The execution engine lives in `labby-codemode`; this service owns product registration, storage/discovery adapters, validation, execution, testing, promotion, and removal semantics.

The generated [action catalog](../generated/action-catalog.md) is authoritative for exact parameters, scopes, and destructive classification.

## Read-Only Discovery

`snippets.list`, `help`, and `schema` are discovery operations. Built-in snippets are loaded from the checked-in snippet directory and user snippets are resolved from the Labby home.

## Tool Declaration Metadata

Markdown frontmatter may include `tools` as a JSON string array or an indented
list of exact `<upstream>::<tool>` identifiers. Storage and Code Mode discovery
preserve omission separately from an explicit empty array. Declarations are
bounded to 128 unique identifiers of at most 1,024 bytes each; reserved local
capabilities, malformed identifiers, and duplicate declaration keys are rejected.

**This is descriptive metadata, not an execution restriction.** Omission records
no declaration; `[]` expresses an intended deny-all declaration; a nonempty list
records intended dependencies. Current execution still uses the existing caller
policy regardless of this metadata. Do not rely on `tools` to limit a snippet's
authority until host-side enforcement is implemented and qualified.

## Administrative Actions

Reading snippet bodies, executing or testing snippets, creating/removing snippets, and promotion flows require the scopes shown in the generated catalog. Promotion and removal are destructive actions.

Built-in snippets are read-only through the user-snippet mutation surface. Explicit shadowing is required before a promoted user snippet may replace a built-in name.

## Execution

Snippet code must evaluate to an async arrow function and executes inside the same bounded Javy/QuickJS Code Mode runtime used by gateway Code Mode. Tool calls are resolved through the live gateway catalog rather than guessed or hard-coded at the host boundary.

## Related Docs

- [Code Mode](../dev/CODE_MODE.md)
- [Snippet authoring](../snippets/README.md)
- [Gateway](./GATEWAY.md)
- [Service model](../dev/SERVICES.md)
