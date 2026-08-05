# Code Mode Retained Results — Design Bundle

Design for [issue #274](https://github.com/dinglebear-ai/labby/issues/274), *"Retain and page oversized Code Mode results by
handle"* — Phase 2 of [#217](https://github.com/dinglebear-ai/labby/issues/217).

**Status:** design complete and research-revised; implementation not started.
**Branch:** `feat/codemode-retained-results-274` · **Epic:** `lab-zca58`

---

## The one-paragraph version

A Code Mode final result is capped by a 24 KB / 6000-token envelope. Over-budget
results are replaced with a 1 KB preview and told to re-run with a narrower query
— right for cheap idempotent reads, wrong when the call burned a rate limit, ran
an expensive aggregation, or cannot safely be repeated, because the data already
exists on the host and is being discarded. This feature retains the complete value
in a bounded, TTL-expiring, per-caller in-memory store, returns an opaque handle
alongside the existing preview, and lets a later execution page it with
`codemode.fetch(handle)` and `codemode.slice(handle, path, range)` — no upstream
re-call. It ships **disabled by default** and is a selective fallback: reducing
inside the sandbox before returning stays the primary path.

## Read in this order

| # | Document | Answers |
|---|---|---|
| 1 | **[SPEC.md](./SPEC.md)** | What must be built (FR/NFR, acceptance criteria, risks) |
| 2 | **[CONTRACT.md](./CONTRACT.md)** | What callers may rely on — helpers, handle, five error kinds, stability |
| 3 | **[MODELS.md](./MODELS.md)** | Entities, ownership, state machine, admission, concurrency |
| 4 | **[TYPES.md](./TYPES.md)** | Rust and TypeScript definitions |
| 5 | **[IMPLEMENTATION_PLAN.md](./IMPLEMENTATION_PLAN.md)** | Bead-by-bead sequence with real code and `file:line` |
| 6 | **[PROGRESS.md](./PROGRESS.md)** | Status, decision log, research findings, open items |

Published schemas: `docs/contracts/schemas/code-mode-retained-{result-marker,fetch-response,slice-request,slice-response}.schema.json`

## How it behaves

```
Execution 1                                    Host
───────────                                    ────
result = await expensiveSearch(...)   ──▶  900 KB, over the 24 KB budget
return result                              retention on, marker will be emitted?
                                           ├─ retain the pre-shaping value
                                           └─ mint cmr_9f2c41d8…
                                  ◀──  { truncated, preview, next_action,
                                         result_handle, retained_until }

Execution 2 (same owner)                       Host
────────────────────────                       ────
await codemode.fetch(handle)          ──▶  owner match? → metadata
                                             (value inlined when under 1 MiB —
                                              the common case; no loop needed)
await codemode.slice(handle,          ──▶  JSON Pointer + clamped range
      "/items", {start:0,end:500})          → bounded selection
return names.slice(0, 20)                   ZERO upstream calls
```

Disabled, full, or over cap → none of this happens and the response is
**byte-identical to today's**.

## The decisions that shaped it

| Decision | Why |
|---|---|
| Retention is a **surface-built context**, not a `CodeModeHost` extension | The trait's only implementor is the shared `GatewayManager`, which has no per-request route scope, actor, or surface |
| Ownership keys on the **transport session** (`relay_session_id`) | Auth identity can't isolate: bearer gives every caller `sub = "static-bearer"`, and there is only one static token, so hashing it doesn't help either. Two agents sharing a token still occupy two sessions. Issue #274 sanctions per-session ownership directly. |
| Capability sets compare by **containment** | A narrower later execution must not read a broader earlier one |
| Admission **rejects** when full; never evicts a stranger | Evicting strangers is a cheap cross-tenant DoS, and would make "store full → truncation" unimplementable |
| Unauthorized returns **the same error as unknown** | Otherwise the error kind is a handle-existence oracle |
| Paging has its **own fail-closed budget** — and the existing fail-open shim must be amended | An empty page mid-stream silently corrupts a paged reconstruction |
| Slice **errors** rather than truncating | A silently short page is worse than a failed one |
| The slice ceiling comes from the **64 MiB sandbox heap**, not the 8 MiB calltool cap | Otherwise a *successful* slice can OOM the runner |
| Retention runs **after** the marker decision | Otherwise a declined marker strands an entry holding quota for the whole TTL |
| Handle lives **in the marker**, and `next_action` names the field | `execute.rs` discards `result_shaping` on exactly the path that needs it |

Full rationale in [SPEC.md](./SPEC.md) §7; the 20 research-driven changes and the decision log are in [PROGRESS.md](./PROGRESS.md).

## What this is not

- Not durable storage — in-memory, dies with the process, flushed on reload.
- Not cross-owner memory — handles never cross an ownership boundary.
- Not automatic — only over-budget results, only when explicitly enabled.
- Not per-`callTool` retention.
- Not a replacement for reducing before returning, which stays primary and leads
  every marker's guidance.

## Known limitations (documented, not hidden)

- **Resident memory is 8–13x the configured cap.** Caps are defined over
  serialized bytes; the store holds parsed `serde_json::Value`, measured at
  7.9–12.6x expansion for MCP-shaped data. Defaults were re-derived accordingly,
  and the multiplier is stated rather than assumed.
- **Token revocation** does not flush the store, so a retained value can outlive a
  revoked token by up to the TTL (300 s). Revocation call sites do exist
  (`labby-auth/src/token.rs:588` and two others), so a targeted eviction is a
  realistic follow-up rather than an impossibility.
- **All trusted-local callers share one owner**, since they have no `actor_key`.
  Acceptable — trusted-local already implies full local trust.
- **The published error schema is stale** (6 origins listed, 9 in the Rust enum).
  That is a live bug on `main`, not one this feature introduces, but it must be
  fixed here because retention emits one of the missing three.
- **A caller who opts into `result_shape_policy: truncate`** gets handle-less
  truncation; that marker has no `next_action` field to carry guidance.

## Related

- `docs/dev/CODE_MODE.md` — the Code Mode surface (gains a "Retained results" section)
- `docs/dev/ERRORS.md`, `docs/contracts/agent-error-contract.md`
- `docs/dev/OBSERVABILITY.md`
- `crates/labby-codemode/CLAUDE.md` — runner invariants and `__lab_internal` rules
- RFC 6901 (JSON Pointer), RFC 7233 §2.1/§4.4 (range-clamping precedent)
