# Resource subscriptions passthrough — issue #211

Planning artifacts for MCP resource subscription passthrough in the Labby
gateway.

- **Issue:** [dinglebear-ai/labby#211](https://github.com/dinglebear-ai/labby/issues/211)
- **Epic bead:** `lab-n27j2` · **Branch:** `feat/resource-subscriptions-211` · **Base:** `132448802`
- **Status:** Planned and reviewed. **Scope changed after review — see the verdict below.** No implementation code written.

---

## Verdict

Research (9 agents) and engineering review (4 agents) found that the original
plan rested on a false transport assumption. **Do not build it as scoped.**

The original framing was *"the handlers are missing — add them."* The accurate
framing is *"Labby advertises a capability it cannot deliver on its primary
network transport."* Those call for different fixes.

**Ship this instead — a ~1 day PR:**

> Clear `capabilities.resources.subscribe` for legacy sessions inside
> `compat_legacy_initialize` (`crates/labby/src/mcp/server.rs:378-384`), so
> Labby stops advertising a method it cannot honor. Modern clients negotiate via
> `discover` and are unaffected. Fix two false code comments while there.

That closes the actual defect on every transport, with no new state, no new
locks, no cross-crate API, and no unresolved lifecycle question. It is also a
strict *prerequisite* for the full build rather than a competitor to it: if a
real legacy consumer is ever identified, the advertisement predicate gains one
clause — nothing is wasted.

**Then stop and check demand.** The full handler build serves clients that are
(a) on a pre-`2026-07-28` protocol, (b) connected over stdio specifically — HTTP
can never work — and (c) using `resources/subscribe` rather than the modern
`subscriptions/listen` path that already works. Nobody has reported such a
client. Building a gated, tested, documented feature for a hypothetical
consumer is the wrong trade, especially in an area that has already required
three corrections to one migration.

## Why — the finding that changed everything

Labby offers **no server→client push channel a legacy client can use.**

This is a *protocol-era* fact, not a property of HTTP. MCP defines push over
HTTP in both eras, but differently:

| Era | Channel | In Labby |
|---|---|---|
| 2025-06-18 | `GET` → standalone SSE stream (optional; `405` is the spec's "not offered") | **405** |
| 2026-07-28 | GET stream removed; POST-only, replies are a JSON object or a *request-scoped* SSE stream. Push rides a held POST — `subscriptions/listen` | **works** |

Labby implements the newer era. Legacy clients expect the older one. So a
`subscribe` handler would answer `Ok(())`, its POST would close, and the peer
just registered would be dead before the client read the `200` — silently, and
forever.

And HTTP is the *likely* path, not an edge case: a POST with no
protocol-version header defaults to `V_2025_03_26` (`tower.rs:2098`) and is
classified legacy.

So the naive implementation would trade a loud `-32601` carrying the agent
recovery contract for a silent `Ok(())` that never fires — **worse than the bug
it fixes, and invisible to any test asserting `subscribe → Ok(())`.**

> **The real question this exposes.** An earlier draft framed the constraint as
> inherent and used that to dismiss building HTTP support. That was wrong.
> Making legacy subscribe deliverable means **serving both transport eras** —
> adding the 2025-era GET/SSE stream and session management alongside the
> current POST-only model. That is bounded transport-layer work, not a rewrite,
> and it is a legitimate product decision rather than a foreclosed one. See
> [FINDINGS §1.2](FINDINGS.md#12-the-real-decision). P0 below is correct either
> way — it makes the *current* state honest without prejudging that choice.

Full evidence, including the spec correction and five agent claims that did
*not* survive verification, is in [FINDINGS.md](FINDINGS.md).

## Recommended bead shape

| Priority | Work | Size |
|---|---|---|
| **P0** | Version-conditional capability advertisement; fix the two false session-lifetime comments (`serve.rs:1693`, `server.rs:38`) | ~1 day |
| **P1** | File the missing auth gate as its own bead — live on `main`, modern path (FINDINGS §5) | file now |
| **P1** | Correct the G-3/G-4 gate ordering in [CONTRACT.md](CONTRACT.md); record F-0 here and in [PROGRESS.md](PROGRESS.md) so this plan cannot re-mislead | done |
| **P2** | Observability: standard fields on the `Lagged` warn; any tracing at all in `notify_resource_update_peers` | ~20 lines |
| **P2** | `prune_closed_peers` second call site not dependent on `listen()` | ~half day |
| **P2** | Two independent perf beads: P-1 short-circuit, P-2 filter-under-lock — both benefit the *modern* path today and are unblocked | ~half day each |
| **Blocked** | Phases A/B (stdio-only handlers), the `labby-gateway` pool API, bridge translation — **gated on identifying a real legacy stdio consumer** | — |

If no consumer surfaces, close #211 as resolved by P0.

## Artifacts

| File | What it is |
|---|---|
| [FINDINGS.md](FINDINGS.md) | **Read this first.** Verified evidence from research + review, with citations |
| [SPEC.md](SPEC.md) | Requirements and scope, revised |
| [CONTRACT.md](CONTRACT.md) | Wire contract and invariants — corrected gate ordering, G-0 and G-5 added, G-2 removed |
| [TYPES.md](TYPES.md) | Rust types, current vs target |
| [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) | P0 plus the deferred build |
| [PROGRESS.md](PROGRESS.md) | Live tracking; open items O-1/O-2/O-3 all resolved |
| [schemas/](schemas/) | JSON Schemas for the two requests, the notification, and the error `data` |

> The review recommended collapsing CONTRACT/TYPES into SPEC and dropping
> `schemas/` as having no named consumer. The full artifact set is retained
> because it was explicitly requested; treat that as a deliberate exception to
> the usual YAGNI rule, not an oversight.

## If the full build is ever approved

Non-obvious constraints established by the review — all verified:

- **G-0 (new, mandatory, precedes everything):** reject `subscribe` on any
  transport that cannot carry a server-initiated notification. Without it the
  epic ships a worse capability violation than the one it fixes.
- **G-2 is deleted** — rmcp already rejects modern sessions before the handler
  (`handler/server.rs:185-201`), and gates `subscriptions/listen` symmetrically
  for legacy ones (`:146-149`).
- **G-5 (new):** a per-session subscription cap, keyed on session identity so
  the HTTP double-registration path cannot bypass it.
- **Bridge:** open one daemon-side `listen()` at connect with a superset filter
  and filter locally. Do *not* re-listen per subscribe — `SubscriptionFilter` is
  immutable after `listen()` and streams are "not resumable", so extending means
  cancel + re-listen, which drops updates for every already-subscribed URI.
  `BridgeClientHandler::on_resource_updated` cannot work at all: notifications
  routed to a `Subscription` are never delivered through `ClientHandler`
  callbacks.
- **Gate G-4 currently cannot fire** — all stdio sessions run at
  `McpRouteScope::Root` (`serve.rs:1651`, `route_scope.rs:73-78`). Build the
  indistinguishability anyway, but do not claim it protects anything today.

## Out of scope

- Building HTTP subscription support (event store + standalone SSE stream) —
  that means reversing the core of the stateless migration; wrong order of
  magnitude.
- Upstream-side legacy subscribe.
- The `ui://` route-scope bypass (`lab-1415y`) — though the review argues it
  should land *alongside* any Phase B reverse-lookup extraction rather than
  being deferred indefinitely, to avoid two acceptance paths with different
  scope semantics.
