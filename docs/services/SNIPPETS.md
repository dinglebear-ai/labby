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

## Tool Declaration Scope

Markdown frontmatter may include `tools` as a JSON string array or an indented
list of exact `<upstream>::<tool>` identifiers. Storage and Code Mode discovery
preserve omission separately from an explicit empty array. Declarations are
bounded to 128 unique identifiers of at most 1,024 bytes each; reserved local
capabilities, malformed identifiers, and duplicate declaration keys are rejected.

For native saved-snippet execution (`snippets.exec` / `snippets.test`), Labby
intersects a declaration with the caller's existing Code Mode policy before
building the catalog. The declaration can narrow authority but never grant it:
omission keeps the legacy caller scope, `[]` denies all upstream tools, and a
nonempty list exposes only those exact dependencies. This also keeps one-shot
snippet runs from cold-probing unrelated gateway upstreams.

Nested `codemode.run()` inherits the already-established execution scope. Trusted
local saved snippets may compose inside that declared scope; route-scoped callers
still cannot use nested snippet resolution to widen their authority.

## Skill Resolution Policy

Markdown snippets may declare a bounded `skills` policy. The policy is preserved in
`SnippetInfo`, resolved snippet metadata, and Code Mode discovery so resolver-aware
clients can select expertise without hard-coding one Skill into every workflow.

Dynamic discovery uses search queries and ordered sources:

```yaml
skills:
  resolution: dynamic
  queries: ["{{task}}", "rust security review"]
  sources: ["team", "public", "mounted"]
  max_candidates: 20
  max_selected: 5
  max_bytes_per_skill: 16384
```

Pinned workflows use exact `skill://` identifiers and must not also declare dynamic
queries:

```yaml
skills:
  resolution: pinned
  pinned: ["skill://team-depot/skill/unmarket/core-qa-evidence/SKILL.md"]
  sources: ["team"]
```

Trust is source-defined rather than author-defined: `team` is authoritative,
`public` and `mounted` are advisory, and `skills_sh` is an untrusted candidate
source. Snippet authors cannot promote an external source to authoritative trust.

Skill policy never grants tools. The snippet's `tools` declaration remains the
authority ceiling. In particular, using `skills_sh` requires the snippet to already
declare an exact `*::depot.skills.search_skills_sh` tool id; the policy cannot add
that capability itself. Limits are validated before publication, pinned values must
be bounded `skill://` URIs, and malformed or repeated policy blocks are rejected.

The declaration is intentionally separate from execution. Current saved-snippet
execution still runs the snippet body unchanged; a resolver may consume the policy
and load Skill instructions, but those instructions cannot widen the execution
scope. This keeps the policy usable for adaptive workflows without silently changing
legacy snippets.

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
