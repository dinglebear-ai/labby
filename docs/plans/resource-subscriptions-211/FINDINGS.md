# Findings — research and engineering review

Evidence gathered by a 9-agent research pass and a 4-agent engineering review
(2026-08-05), against `origin/main` @ `132448802` and `rmcp` 3.1.0.

**Every finding below was re-verified directly against source before being
recorded here.** Agent claims that did not survive that check are listed in
§7. Citations are exact; where a claim is inference rather than observation it
says so.

---

## 1. The decisive finding — F-0

**Labby offers no server→client push channel that a legacy client can use, and
HTTP is the *likely* path into the legacy handler, not an edge case.**

> ### Correction (2026-08-05, after spec review)
>
> An earlier draft of this section stated the constraint as though it were
> inherent to HTTP — "each request gets its own connection that dies the instant
> you reply." **That was wrong**, and the error mattered: it was used to dismiss
> the option of building HTTP support.
>
> MCP does define server→client push over HTTP. What it defines *changed between
> protocol eras*:
>
> | Era | Server→client channel | Status in Labby |
> |---|---|---|
> | 2025-06-18 | `GET` → standalone SSE stream. Optional; the spec explicitly permits `405 Method Not Allowed` to mean "not offered" | **405** — not implemented |
> | 2026-07-28 | Standalone GET stream **removed**. Transport is POST-only; replies are "a JSON object or a *request-scoped* SSE stream". Push rides a held POST — i.e. `subscriptions/listen`. Server-initiated *requests* removed entirely | **implemented and working** |
>
> So the accurate framing is: **legacy clients expect a channel the newer spec
> deleted, and Labby implements the newer spec.** Returning 405 is not a
> conformance failure — it is Labby declining to serve a transport era it does
> not claim to support.
>
> **What this changes.** Building HTTP support is *not* "reversing the stateless
> migration with fleet-wide blast radius", as an earlier draft claimed. It is
> "run both transport eras concurrently" — add the 2025-era GET/SSE stream plus
> session management alongside the current POST-only model. Bounded, deliberate
> transport-layer work. It is the only route by which legacy
> `resources/subscribe` ever delivers over HTTP, and it deserves to be weighed
> on its merits rather than dismissed. See [§1.2](#12-the-real-decision).
>
> **What this does not change.** Every verified fact below about Labby's current
> behaviour still holds, and P0 remains the cheapest way to stop advertising a
> method legacy clients cannot use.

```
cli/serve.rs:1720,1738   NeverSessionManager + .with_legacy_session_mode(false) + .with_json_response(true)
rmcp tower.rs:1512-1521  (legacy_session_mode=false, event_store=None) → allowed_methods = "POST"   ⇒ GET returns 405
rmcp tower.rs:1947       let service = self.get_service()   ⇒ a FRESH LabMcpServer per POST
rmcp tower.rs:1968       OneshotTransport::new(...)         ⇒ transport dies with the POST
rmcp tower.rs:2098       header absent ⇒ .unwrap_or(ProtocolVersion::V_2025_03_26)
```

Consequences, in order of importance:

1. **There is no server→client channel outside an open POST.** `listen()`
   survives only because its first emitted message is a notification, so rmcp
   falls back to `stateless_sse_response` and holds the request open. A
   `subscribe` handler returning `Ok(())` emits a *terminal* response first —
   the POST closes, the transport drops, and the `Peer` just registered is dead
   before the client reads the `200`.
2. **Every headerless POST is classified legacy** (`V_2025_03_26`) and routes
   straight into the legacy handler. That is the default for exactly the older
   clients this work targets.
3. **`Transport::UnixSocket` inherits this identically** — `serve.rs:70-72`
   defines it as Streamable HTTP over a UDS, built by the same
   `StreamableHttpService` with the same config. It is not a second viable
   surface.

So a naive implementation would replace *"advertised capability returns
`-32601`"* — a loud, recoverable error carrying the agent recovery contract —
with *"advertised capability returns `Ok(())` and silently never delivers."*
In a codebase whose entire error philosophy is that contract, silent-accept is
the worst available outcome, and **no test asserting `subscribe → Ok(())` would
catch it.**

### 1.1 Where the leak actually bites

The original plan worried about stdio. That was wrong:

| Deployment | Path | Peers | Leak? |
|---|---|---|---|
| Standalone stdio | `serve.rs:266-290` → `run_stdio()` | n = 1, process lifetime | Harmless — a leaked entry in a dying process |
| Stdio bridge | `serve.rs:215-222` → `run_stdio_bridge()` | n = 1 downstream | Harmless, same reason |
| **Daemon HTTP / UDS** | `serve.rs:1698-1816` | fresh server per POST, **shared registry** (`serve.rs:1643,1753`) | **Request-driven, unbounded** |

On the daemon, one retrying legacy client adds a permanently-undeliverable
registry entry **per POST, forever**. That is strictly worse in shape than the
119-dead-sessions incident it reprises (commit `23b2afbb5`), which was at least
bounded by connection rate.

### 1.2 The real decision

Given the correction above, the choice is not "build the handlers or don't." It
is a prior question:

> **Does Labby want to serve the 2025-06-18 transport era at all?**

**If no** — Labby serves only `2026-07-28` transport semantics — then legacy
clients have no push channel by construction, P0 is the complete answer, and
#211 closes. The handler build stays deferred permanently, because a handler
that can never deliver is the silent-accept failure described above.

**If yes** — then the work is a *transport-layer* bead, not a subscription bead:
implement the 2025-era `GET` → SSE stream and the session management it needs
(`Mcp-Session-Id`, and `Last-Event-ID` resumability if wanted; both are `MAY` in
the spec). rmcp already gates the GET route on
`legacy_session_mode || session_manager.event_store().is_some()`
(`tower.rs:1512-1521`), so the hook exists — Labby currently supplies
`NeverSessionManager` and `legacy_session_mode(false)` (`serve.rs:1720,1738`).
Once that channel exists, legacy `resources/subscribe` becomes implementable and
most of the deferred Phase A–C work follows almost mechanically.

Framed that way, the deferred phases are not blocked on "find a legacy stdio
consumer" alone — they are blocked on a **product decision about transport-era
support**, which is a larger and more legitimate question than the original plan
posed. Both P0 and that decision are independent of each other: P0 makes the
*current* state honest either way.

---

## 2. What the SDK already does for us

Three planned mechanisms turned out to be redundant.

**F-2 — modern sessions never reach the legacy handler.**

```rust
// rmcp handler/server.rs:185-201
ClientRequest::SubscribeRequest(request) => {
    if !legacy_request { Err(McpError::method_not_found::<SubscribeRequestMethod>()) }
    else { self.subscribe(request.params, context).await.map(ServerResult::empty) }
}
// rmcp service.rs:196-202
!uses_discover_lifecycle && protocol_version.is_none_or(|v| v < &ProtocolVersion::V_2026_07_28)
```

**F-9 — and the gate is symmetric.** `subscriptions/listen` is version-gated the
same way (`handler/server.rs:146-149` returns `method_not_found` for legacy
sessions). A session is either legacy (subscribe works, listen 404s) or modern
(listen works, subscribe 404s).

Therefore gate **G-2 and the proposed `unsupported_protocol_method` error kind
are both unnecessary**, which also deletes an `ERRORS.md` /
`agent-error-contract.md` spec change and three files of Phase E scope. The
residual double-registration risk is HTTP-only: `legacy_request` is computed
per request, so one logical client can send POST #1 headerless (→ legacy →
subscribe) and POST #2 with the modern header (→ listen), producing two
registry entries. That is an F-0 artifact, not a protocol one.

**Session identity is a solved problem.** `context.protocol_version()` is public
API (`rmcp service.rs:1221-1229`), and rmcp reconstructs `peer_info` in
stateless mode specifically so it works inside handlers
(`tower.rs:1959-1963`). `Peer<RoleServer>` has no `Eq`/`Hash`, but on stdio
there is exactly one `LabMcpServer` for the process, so a per-instance cell is
correct there. **The original plan had the polarity backwards** — it feared a
shared instance leaking across sessions; the actual hazard is that on HTTP the
instance is *not* shared, so every POST mints a fresh identity.

---

## 3. The capability flag is shared — the "just stop advertising it" fix needs care

```rust
// rmcp model.rs:2013-2018 — SubscriptionFilter::supported_by
resource_subscriptions: capabilities.resources.as_ref()
    .is_some_and(|resources| resources.subscribe == Some(true))
    .then(|| self.resource_subscriptions.clone()).flatten(),
```

MCP uses **one flag** — `resources.subscribe` — for both the deprecated RPC pair
and the modern `subscriptions/listen` mechanism. Dropping
`.enable_resources_subscribe()` (`server.rs:414-415`) globally would silence the
legacy lie *and* destroy modern resource subscriptions, including Labby's own
upstream negotiation, which calls `supported_by` at
`pool/notifications.rs:270`.

**But a targeted fix does exist.** `compat_legacy_initialize` already returns a
*modified* `ServerInfo`, and only legacy clients pass through it — modern
clients negotiate via `discover`, which returns `get_info()` untouched
(`server.rs:485-489`):

```rust
// server.rs:381-383 (current)
let mut info = self.get_info();
info.protocol_version = request.protocol_version;
Ok(info)
```

Clearing `capabilities.resources.subscribe` there is roughly ten lines and
restores capability honesty for legacy sessions with zero effect on the modern
path.

> **Note on C-1 as originally written.** "Advertised ⟺ the methods work, there
> is no third state" is unachievable by construction for *any* MCP server: the
> shared flag means supporting `subscriptions/listen` necessarily advertises the
> deprecated RPCs. The violation is an upstream spec artifact, not a Labby bug.
> The version-conditional fix is what makes it honest *in practice*.

---

## 4. Phase D was dead in both branches

Verified from rmcp's own documentation:

```
service/client.rs:1058-1059  "Notifications routed to the returned Subscription are not also
                              delivered through ClientHandler callbacks."
service/client.rs:322-324    "Call Peer::listen again after reconnecting; subscription
                              streams are not resumable."
service/server.rs:139-144    accepted: Arc<SubscriptionFilter>     // plain Arc, no setter
```

So the planned `BridgeClientHandler::on_resource_updated` cannot work:

- If the bridge opens a `listen()`, notifications go to the `Subscription`
  receiver and the callback never fires.
- If it does not, the daemon never registers it as a peer and never fans out to
  it at all.

Compounding: `live_gateway.rs:434-437` negotiates
`ClientLifecycleMode::Discover{V_2026_07_28}` over `StreamableHttpClientWorker`,
so the bridge→daemon hop is both modern-protocol *and* stateless HTTP — no push
channel exists. And `BridgeClientHandler` is a zero-field unit struct
(`bridge.rs:69`) with nowhere to hold state.

**Correct design, if the bridge is ever built:** open the daemon-side `listen()`
**once at bridge connect** with `resource_subscriptions` set to the full
`subscribable_resource_uris_snapshot()`, and filter downstream *locally* in the
bridge. Subscribe/unsubscribe becomes a local `BTreeSet` insert/remove — no wire
cost, no loss window.

The alternative (re-listen per subscribe) costs, for a client subscribing to N
URIs one at a time: N round trips, N × 3-6 ms of daemon CPU, 3N registry
write-locks, 2N `peer.connect`/`peer.disconnect` INFO lines — and, worse, a
**deterministic loss window**: between `cancel()` and the new ack there is no
subscription at all, so subscribing to URI #10 silently drops updates for URI #1.

---

## 5. Security

**Gate G-4 can never fire on the only working transport.** `run_stdio` hardcodes
`McpRouteScope::Root` (`serve.rs:1651`); the bridge always connects to `/mcp`
root with its own `LABBY_MCP_HTTP_TOKEN` (`live_gateway.rs:99,420-441`); and
`Root::allows_upstream` returns `true` unconditionally (`route_scope.rs:73-78`).
Protected routes exist only as separate *HTTP* paths (`serve.rs:1812-1838`).

C-4's indistinguishability is still worth building — it becomes live the moment
protected stdio routing exists, and the modern path has the analogous bug
(`lab-1415y`) — but the plan must not claim it provides protection today.

**The real marginal risk:** this feature converts a bridge client's existing
one-shot read access into a **standing, push-based watch channel across every
upstream**, uncapped (§6) and unreaped (§1.1), through a code path that
structurally cannot honor the `protected_mcp_routes` primitive. Not an
authentication bypass — anyone who can exec `labby mcp` already holds the token
— but a materially higher-value target inside the same trust boundary.

**Missing auth gate (F-8), re-rated.** `handlers_resources.rs:686-693` gates
`ui://` *reads* behind `code_mode_read_scope_allowed`; neither the planned
`subscribe` nor the existing `accepted_subscription_filter` applies any auth
check. Severity drops to **low/informational for the current tree** — stdio
carries no `AuthContext`, and OAuth sessions are HTTP where subscribe is dead —
but it is live on `main` on the modern path and should be its own bead.

**Gate ordering (F-7).** `CONTRACT.md` ordered G-3 (snapshot) before G-4 (route
scope); the implementation sketch did the reverse. **The sketch is correct** —
the contract's order leaks, because an out-of-scope URI on a reconnecting
upstream would return `retry_later` while an unknown URI returns `rediscover`,
which is exactly the distinguishability C-4 forbids. This must be corrected in
the document even if nothing is ever implemented.

**No subscription cap (F-5-sec).** No MCP gateway in the ecosystem caps this,
but the Python SDK hard-caps its client-side backlog at
`_MAX_PENDING_EVENTS = 1024` with a comment that unbounded URI-driven state must
not grow memory. Combined with §1.1 and §5, a cap becomes **normative, not
optional**, and must be keyed on session identity or the HTTP
double-registration path bypasses it.

---

## 6. Performance — most of it evaporates at real scale

Re-rated against the actual topology (§1.1), where the working deployment has
n = 1 peer.

| ID | Finding | Original | Re-rated |
|---|---|---|---|
| P-1 | `catalog_notifications.rs:342-350` computes `visible_contract().await` *before* consulting `wants_tool_list_changed()`, so legacy peers pay a 3-6 ms serialize+SHA-256 they discard | CRIT | **LOW** — one wasted recompute per `tools/list_changed`, an event minutes apart, inside a loop iteration that may already have spent 60 s elsewhere. Do the one-line hoist for readability, not urgency |
| P-2 | `notify_resource_update_peers:272` deep-clones the whole registry before filtering | CRIT | **LOW** — 1-2 µs at n=1/T=15; ~25 µs at T=300. Take it as a free side effect of the Phase C edit; do not write it up |
| P-3 | `prune_closed_peers` has one production call site (`server.rs:538`, inside `listen()`) | CRIT | **CRIT, reclassified** — harmless on stdio, unbounded on the daemon (§1.1). **The fix is a transport gate, not more pruning** |
| P-4 | Seed legacy peers with an empty `ToolCatalogSnapshot` — C-13 makes `last_contract` permanently unreadable | MED | **LOW but do it** — 3-6 ms off a client-facing path |
| LOW-1 | O(n²) `position()` scans in `notify_catalog_peers` | LOW | **Delete from the register** — nanoseconds at n≤5 |

**Corrected threshold.** The prior pass cited `DISCOVERY_TIMEOUT = 15s`. For
stdio upstreams — most of a homelab gateway — `helpers.rs:37,44` selects
`STDIO_DISCOVERY_TIMEOUT = 60s`, awaited inline in the single broadcast consumer
(`peers.rs:317`). With capacity 1024 (`pool/notifications.rs:17`), loss begins
around **~17 events/s**, not 68. Unreachable in steady state; reachable in a
burst from a filesystem-watching upstream.

**New — lock ordering.** Filtering under the read lock means a `std::sync::RwLock`
is acquired while holding the tokio registry lock. A future subscribe-path
refactor doing `peers.read().await` → find → `subscriptions.write()` would
deadlock a tokio *worker*, not just a task. Needs an explicit stated invariant:
the registry lock is always outermost.

**New — observability holes.** `notify_resource_update_peers` contains **zero**
tracing calls (compare `notify_catalog_peers`, which logs `peer.gc` with
`pruned_count`/`active_count`). And the `Lagged` warn (`peers.rs:327-329`)
carries only `skipped` — no `surface`/`service`/`action`, so it never appears in
a `surface="mcp"` query. An accepted loss mode that cannot be observed is not
accepted, it is invisible. **~20 lines total; the highest value-per-line work in
the epic.**

---

## 7. Agent claims that did NOT survive verification

Recorded so they are not re-imported later.

| Claim | Reality |
|---|---|
| "Inbound upstream `list_changed` plumbing does not exist" (memory `lab-l3o8.4`) | **Stale.** It exists at `pool/relay.rs:441` and `pool/notifications.rs:410`. Superseded by this research |
| "`subscriptions/listen` is not version-gated, so C-11 is unenforced" (F-9, agent-native pass) | **Wrong.** `handler/server.rs:146-149` gates it. The SDK enforces C-11 on both sides |
| "Just drop `.enable_resources_subscribe()`" (simplicity pass, hedged) | **Incomplete** — would break the modern path (§3). The version-conditional form works |
| "`LegacyPeer` was demoted from production to test-only" (my own README) | **Wrong.** It was *born* `#[cfg(test)]` in `41fbdae5f`; there was no `NotificationTarget` enum before that |
| "The audit branch is a major conflict risk" (my own risk register) | **Overstated.** It touches neither `LegacyPeer`, the prune logic, nor subscribe. Risk is textual merge noise in `server.rs`/`bridge.rs` only |

---

## 8. History — no evidence of deliberately-undone work

| Commit | Date | Relevance |
|---|---|---|
| `6996d4a50` | 2026-07-07 | Bridge `subscribe`/`unsubscribe` forwarding **was** implemented |
| `41fbdae5f` | 2026-07-25 | rmcp-3 stateless migration: removed that forwarding, created `NotificationTarget` with `LegacyPeer` already `#[cfg(test)]`. Removal tracked rmcp dropping the `Peer<RoleClient>` convenience method — the wire types still exist |
| `e47996e20` | 2026-07-27 | "address stateless review findings" — correction #1 |
| `9aa1f912b` | 2026-07-27 | "preserve legacy lifecycle compatibility" — correction #2, partially *reversed* the migration's removal of legacy `initialize` |
| `23b2afbb5` | 2026-07-25 | Added `prune_closed_peers` after 119 dead sessions accumulated in a day |
| `41db7d4a2` | 2026-08-03 | Generation-guard design, to fix a documented multi-hop reconnect race — supports C-5 |
| `21ef0c93e` | 2026-08-02 | Added `enable_resources_subscribe()` to unlock the **modern** `listen()` path. The capability-honesty side effect was never weighed |

**The pattern that matters:** `41fbdae5f` → `e47996e20` → `9aa1f912b` → #211 is
a fourth pass over one migration, and each correction re-adds legacy surface to
a design that deliberately dropped it. That argues for making the boundary
explicit rather than patching it again.

`docs/services/GATEWAY.md` still claims *"Labby does not accept the legacy
initialize / notifications/initialized lifecycle"* — **stale**, reversed by
`9aa1f912b`. Worth fixing regardless of this epic's outcome.
