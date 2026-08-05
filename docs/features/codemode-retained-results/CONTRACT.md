# CONTRACT — Code Mode Retained Results (v1)

**Contract version:** 1 (additive to the agent error contract)
**Audience:** model-authored sandbox code, and anything parsing Code Mode responses
**Schemas:** `docs/contracts/schemas/code-mode-retained-*.schema.json`
**Related:** `docs/contracts/agent-error-contract.md`, `docs/contracts/code-mode-tool-errors.md`

> **Revised after research (2026-08-05).** Seven error kinds became five, guidance
> text is now specified rather than inherited, and the recovery action for the miss
> family changed to match existing `discovery`-origin precedent.

---

## 1. Surface

```ts
codemode.fetch(handle: string): Promise<RetainedFetchResponse>
codemode.slice(handle: string, path?: string, range?: { start?: number; end?: number }): Promise<RetainedSliceResponse>
```

Present only when the host has retention enabled for this execution. Dispatched
through the reserved internal namespace already used by `describe_types` and
`semantic_rank` (`execute.rs:35`, `:366-372`):

| Helper | Internal tool id |
|---|---|
| `codemode.fetch` | `__lab_internal::codemode_result_fetch` |
| `codemode.slice` | `__lab_internal::codemode_result_slice` |

Calling those ids directly via `callTool` behaves identically — ownership is
enforced in the handler, not the helper. When retention is off the handlers are
not registered at all, so a direct call returns the ordinary `unknown_tool`
(`execute.rs:516-519`) rather than a bespoke "disabled" kind.

## 2. Handle format

```
cmr_[0-9a-f]{32}
```

`cmr_` plus a UUIDv4 with hyphens stripped — 122 bits of `getrandom`-backed
entropy, no embedded timestamp.

**Guaranteed:** opacity. Encodes no ownership, path, secret, or creation time.
**Not guaranteed:** anything about derivation. Do not parse, compare beyond the
literal `cmr_`, or infer ordering.

Handles are **capability tokens** — anything holding one *and* satisfying the
owner check can read the value. Don't log them in full or return them to
untrusted consumers.

## 3. Error kinds (normative)

Every failure is a `CodeModeCallError` (`error_contract.rs:42-67`), delivered to
sandbox code as a rejected promise.

| `kind` | Raised when | `origin` | `recovery.action` | `same_arguments` | `side_effects` |
|---|---|---|---|---|---|
| `retained_result_not_found` | Handle unknown, expired, evicted, **or** owned by someone else | `discovery` | `rediscover` | `never` | `none_expected` |
| `retained_handle_malformed` | Absent, wrong type, or failing the format pattern | `validation` | `do_not_retry` | `never` | `none_expected` |
| `retained_slice_invalid` | Bad pointer syntax, pointer resolves to nothing, or a range on a non-array/non-string | `validation` | `revise_and_retry` | `never` | `none_expected` |
| `retained_value_too_large` | The selection exceeds the slice ceiling | `budget` | `reduce_work` | `conditional` | `none_expected` |

**Four kinds, not five.** A per-run paging ceiling overrun reuses the existing
`call_budget_exceeded` kind rather than minting `retained_calls_exhausted`: the
triple is identical (`budget` / `reduce_work` / `never`), and that family already
tolerates one shared guidance string across two unrelated budgets. The specific
limit belongs in `message`/`evidence`, not in a new `kind`.

### 3.1 Required guidance text

`recovery.guidance` is part of this contract and **must not** be inherited from
`recovery_for_kind`'s generic arms (`agent_error.rs:498-606`). Doing so would tell
an agent facing a miss to *"correct the command or parameters and retry"* — advice
that can never succeed, since no parameter change resurrects a dropped entry.

| Kind | `guidance` |
|---|---|
| `retained_result_not_found` | "This retained result is no longer available. Re-run the original query in a new Code Mode execution and reduce the result inside the sandbox before returning." |
| `retained_handle_malformed` | "The handle is not a valid retained-result handle. Use the `result_handle` value from a truncation marker verbatim." |
| `retained_slice_invalid` | "The JSON Pointer did not resolve, or a range was applied to a value that is not an array or string. Call codemode.fetch(handle) to inspect the shape, then retry with a valid path." |
| `retained_value_too_large` | "The selection is {selection_bytes} bytes, over the {max_bytes} limit. Narrow the path or reduce the range width, then retry." |
| `call_budget_exceeded` (paging) | Existing shared guidance; the `message` names the paging limit and that this execution is done paging. |

### 3.2 Why these choices

**One miss kind, not three.** Unknown, expired, evicted, and foreign-owned all
carry an identical recovery contract, so splitting them was diagnostic decoration
only — and merging the foreign case is required regardless, because a distinct
"exists but not yours" is a handle-existence oracle. Ownership is checked before
state, so nothing leaks either way.

**`rediscover`, not `revise_and_retry`.** Every existing `discovery`-origin kind
in the codebase (`unknown_tool`, `not_found`, `unknown_action`) pairs with
`Rediscover` + `Never` (`agent_error.rs:458-461`, `:516-523`). Pairing `discovery`
with `revise_and_retry` would have been unprecedented and would read to an agent
as "change the arguments and call again" — inviting a loop over other handles.
Note honestly: none of the nine recovery actions expresses "abandon this call and
redo the upstream work," so the guidance text carries that meaning.

**`retained_value_too_large` is `conditional`** — the same handle with a narrower
path or range is exactly the right retry.

`origin: discovery` requires reconciling the published error schema, which lists
six of the nine Rust origins. That drift is a **live bug on `main`** (`origin_for_kind`
already emits `discovery` for `unknown_tool` today), not one this feature creates.

## 4. Response shapes

### 4.1 Truncation marker (with retention)

Additive to `truncate.rs:178-193`. The three retention fields appear together or
not at all. The **string** marker from `shape.rs:99` never carries a handle — see
SPEC FR-2.4.

```jsonc
{
  "truncated": true,
  "original_size": 918273,
  "original_tokens": 229569,
  "preview": "{\"items\":[{\"id\":\"a1\",\"name\":\"…",
  "artifacts": [],
  "next_action": "Reduce inside the sandbox before returning — project fields, filter rows, or slice arrays. This result was retained: pass the `result_handle` value below to codemode.fetch(handle) or codemode.slice(handle, path, range) in a later execution instead of re-running an expensive or rate-limited call.",
  "result_handle": "cmr_9f2c41d8a7b04e6ab1c35d90e7f26a48",
  "retained_until": "2026-08-05T19:41:07Z",
  "retained_bytes": 918273
}
```

`next_action` names **`result_handle`**, the field, not just a `handle` parameter.
The existing marker's `next_action` is fully self-contained and models are trained
by the docs to read it; the retention variant preserves that property.

Without retention the marker is **byte-identical to current `main`**.

### 4.2 `codemode.fetch` success

```jsonc
{
  "handle": "cmr_9f2c41d8a7b04e6ab1c35d90e7f26a48",
  "size_bytes": 918273,
  "token_estimate": 229569,
  "value_type": "object",
  "object_keys": ["items", "total", "cursor"],
  "object_key_count": 3,
  "created_at": "2026-08-05T19:36:07Z",
  "expires_at": "2026-08-05T19:41:07Z",
  "value_omitted": true,
  "guidance": "Value exceeds the inline ceiling; select a bounded path or range with codemode.slice."
}
```

To size a paging loop, issue a **zero-width probe slice** —
`codemode.slice(handle, "/items", {start:0,end:0})` returns `source_length`
(§4.3). That costs one host-local paging call, never an upstream re-call. An
`array_lengths` field was considered and cut: the probe already provides the
number, and building the map would walk an unbounded root key count on every
fetch.

When the value fits the ceiling, `value_omitted` is `false`, `guidance` is absent,
and `value` carries the whole payload. Retention triggers at ~24 KB while the
inline ceiling is 1 MiB, so **this is the common case** — most retained results
need no paging loop at all.

### 4.3 `codemode.slice` success

```jsonc
{
  "handle": "cmr_9f2c41d8a7b04e6ab1c35d90e7f26a48",
  "path": "/items",
  "value": [{ "id": "a1", "name": "first" }],
  "selection_bytes": 42,
  "truncated": false,
  "range_applied": { "start": 0, "end": 1 },
  "source_length": 4821
}
```

`truncated` is always `false` — slice errors rather than truncating.

## 5. Semantics

### 5.1 Paths

RFC 6901 JSON Pointer. `""` (or omitted) selects the root. `~0` = `~`, `~1` = `/`.
Dotted paths are not supported; JSON Pointer was chosen precisely because a key
containing a dot stays unambiguous. Leading-zero array indices and the `-` token
resolve to nothing (`retained_slice_invalid`), per RFC 6901.

### 5.2 Ranges

- Arrays and strings only.
- Indices **clamp**; `start >= end` is empty, not an error.
- Strings index **chars, never bytes** — a slice cannot split a multi-byte character.
- Omitting `range` returns the whole value at `path`, subject to the ceiling.

This mirrors **RFC 7233** more closely than `Array.prototype.slice` alone: HTTP
Range clamps a partially-out-of-range request (§2.1) and errors only when there
is no overlap (§4.4), while having no notion of silently returning less than
asked. The clamp-bounds / error-on-oversize split is the same synthesis.

### 5.3 Lifetime

- TTL is **fixed at creation**; neither helper extends it. Page promptly.
- `expires_at` guarantees unusability after that instant, not availability until
  it — eviction, gateway reload, and **losing the connection** can end an entry early.
- Handles do not survive a host restart and are not issued at all on one-shot CLI
  executions.

### 5.4 Ownership

Usable only from an execution whose owner — **transport session**, actor, route
scope, and capability set — matches the one that created it. The capability set
is compared by **containment**: a narrower later execution cannot read a broader
earlier one.

The session component is what isolates two agents that share a single bearer
token, since auth identity is constant in that mode. **A handle therefore does
not survive a reconnect**: a new connection is a new session, and its handles
from the old one return `retained_result_not_found`. Page within the session that
produced the handle.

## 6. Stability

**Additive-only within contract version 1.** Tolerate unknown fields.

A version bump is required to: remove or rename a §4 field; change a `kind` or its
`origin`/`recovery`/`same_arguments` triple; change the handle pattern; change
pointer or range semantics; make `slice` truncate; or extend TTL on access.

Non-breaking: new optional response fields, new kinds for genuinely new
conditions, changed defaults, changed prose.

## 7. Guidance ordering

Every marker's `next_action` **leads** with reduce-before-return and mentions
paging second, qualified to expensive or rate-limited calls rather than presented
as the general remedy. #217 shipped the reduce-first behavior; this must not erode
it. Note that the BAD/GOOD example pair in the tool description
(`mcp/call_tool_codemode.rs:249-264`) is what actually carries that behavior, so
Bead 4's update to that description matters more than the marker prose.
