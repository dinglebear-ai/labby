# Wire contract — Legacy resource subscriptions

Normative contract for `resources/subscribe`, `resources/unsubscribe`, and
`notifications/resources/updated`.

**Status: conditional.** [SPEC.md](SPEC.md) defers the handler build pending a
real legacy stdio consumer. This document is binding *if* that build is ever
approved. Revised after engineering review — the gate table below **corrects a
leak** present in the first draft, so this file is worth keeping current even if
nothing ships.

Evidence: [FINDINGS.md](FINDINGS.md). Base commit `132448802`.

---

## 1. Capability advertisement

`resources.subscribe` is advertised iff the `gateway` feature is compiled in and
a gateway manager is configured (`crates/labby/src/mcp/server.rs:412-417`).

**C-1 (revised).** The original formulation — *"advertised ⟺ the methods work;
there is no third state"* — is **unachievable by construction** and has been
replaced. MCP uses one flag, `resources.subscribe`, for both the deprecated RPC
pair and `subscriptions/listen` (`rmcp model.rs:2013-2018`). Any server
supporting the modern mechanism necessarily advertises the deprecated one.

The achievable invariant is **per-session**:

> A session must be advertised exactly the subscription mechanism it can use.
> Legacy sessions (`initialize`) must not be told `resources.subscribe` is
> available; modern sessions (`discover`) must be.

`compat_legacy_initialize` (`server.rs:378-384`) is the enforcement point — it
already returns a modified `ServerInfo`, and modern clients never pass through
it.

## 2. URI forms

| Form | Example | Subscribable | Notes |
|---|---|---|---|
| Rewritten upstream resource | `lab://upstream/github/repo/status` | Yes | via `gateway_resource_uri()` (`pool/notifications.rs:140`) |
| Native upstream mcp-ui widget | `ui://quick-shell/app.html` | Yes | deliberately **not** rewritten |
| Local catalog / actions / Code Mode app | `lab://catalog`, `ui://lab/code-mode/*` | No | never emit updates |

**C-2.** Deliverability = membership in
`UpstreamPool::subscribable_resource_uris_snapshot()`
(`pool/notifications.rs:136`). Single source of truth.

**C-3.** Prefix inspection is **not** a valid ownership test — `ui://` URIs carry
no `lab://upstream/<name>/` segment. Ownership must come from a reverse lookup
through the pool (the mechanism at `handlers_resources.rs:686-701`). Deriving
ownership from the prefix is bug `lab-1415y`; do not reproduce it.

> **Namespace hazard.** `entry.resource_uris` holds **native** URIs;
> `subscribable_resource_uris_snapshot()` holds **gateway-form** URIs; and
> `gateway_resource_uri` is `pub(super)`, unreachable from `crates/labby`. Any
> new pool API must take a gateway-form URI and do the rewrite internally —
> e.g. `fn upstream_owning_gateway_uri(&self, gateway_uri: &str) -> Option<String>`.
> Do **not** widen `gateway_resource_uri`'s visibility; that leaks the rewrite
> convention across the crate boundary and invites the prefix parsing C-3
> forbids.

## 3. `resources/subscribe`

**Params** (`rmcp model.rs:1806-1815`): `SubscribeRequestParams { uri: String, meta: Option<RequestMetaObject> }`.
**Result:** empty.

### 3.1 Gate table (normative, first match wins)

| # | Condition | Code | `data.kind` | `recovery.action` |
|---|---|---|---|---|
| **G-0** | transport cannot carry a server-initiated notification | `-32601` | — | — |
| G-1 | gateway feature off / no gateway manager | `-32601` | — | — |
| G-3 | owning upstream not permitted by this session's route scope | `-32002` | `not_found` | `rediscover` |
| G-4 | URI not in the deliverable snapshot | `-32002` | `not_found` | `rediscover` \| `retry_later` |
| **G-5** | session is at its subscription cap | `-32602` | `budget_exceeded` | `reduce_work` |
| — | accept | — | — | — |

**Three changes from the first draft, all load-bearing:**

- **G-0 is new and mandatory.** Accepting a subscribe that can never fire is a
  capability violation on the same axis this work exists to fix — and a worse
  one, because the client loses its error signal. See
  [FINDINGS §1](FINDINGS.md#1-the-decisive-finding--f-0).
- **G-2 is deleted.** rmcp rejects modern sessions before the handler runs
  (`handler/server.rs:185-201`) and gates `subscriptions/listen` symmetrically
  for legacy ones (`:146-149`). The hand-rolled check and its
  `unsupported_protocol_method` error kind were both dead weight.
- **Route scope now precedes snapshot membership** (former G-4 before former
  G-3). **The original order leaked:** an out-of-scope URI on a reconnecting
  upstream would return `retry_later` while an unknown URI returned
  `rediscover` — precisely the distinguishability C-4 forbids.

**C-4 (scope-denial indistinguishability).** G-3 and G-4 responses must be
byte-identical apart from the echoed URI. Route scope is a *visibility*
boundary; a distinguishable denial lets a scoped client enumerate upstreams it
cannot see. Enforce with one shared constructor, and assert equality in tests —
not merely that both are errors.

> **C-4 covers wire-visible bytes, not timing.** The out-of-scope path performs
> one extra synchronous set lookup; the residual is accepted as negligible.
>
> **C-4 protects nothing today.** All stdio sessions run at
> `McpRouteScope::Root` (`serve.rs:1651`; `Root::allows_upstream` is
> unconditionally `true`, `route_scope.rs:73-78`), so G-3 can never fire on the
> only transport where the feature works. Build it anyway — it becomes live the
> moment protected stdio routing exists — but do not credit it with current
> protection. Ship a regression test asserting stdio sessions are `Root`-scoped
> so a future reader does not assume CI exercises G-3.
>
> **Precedent is real but not universal.** Upstream resource *reads* already
> collapse unknown and out-of-scope to not-found
> (`resource_proxy.rs:220-226,285-300`), and SEP-1881 (Draft, tools-only) states
> the principle. IBM ContextForge is a live counter-example, raising distinct
> `PermissionError` vs `ResourceNotFoundError`. C-4 is Labby's deliberate
> choice, not an unquestioned norm.

**C-5 (reconnect nuance).** Within G-4, when the URI is catalog-listed but
absent from the ack snapshot — the upstream-reconnect window that the generation
guard (`41db7d4a2`) deliberately creates — use `retry_later` /
`same_arguments: "conditional"`. Otherwise `rediscover` / `"never"`. This is the
only permitted variation, and it is keyed on catalog presence, **never** on
route scope.

**C-6.** Subscribe is idempotent, by `BTreeSet` semantics rather than by a check.

**C-17 (new).** `-32002` is era-correct here. SEP-2164 (merged 2026-05-18) moved
resource-not-found to `-32602` for `2026-07-28`, retaining `-32002` only as a
client-side back-compat accept. Because this handler serves *only*
pre-`2026-07-28` sessions, `-32002` is right — do not "fix" it.

## 4. `resources/unsubscribe`

Gates G-0 and G-1 apply; G-3/G-4/G-5 do not — a client must always be able to
stop.

**C-7.** Unsubscribing a never-subscribed URI returns `Ok(())` and logs `WARN`.

**C-8.** When the last URI is removed, the session's `RegisteredPeer` is removed.

**C-8b (new).** Unsubscribe must **never** register. Two concurrent unsubscribes
of the last two URIs must converge on a single removal — so the accessor must be
non-creating and removal must be keyed on `registration_id`. Test it: final
registry length 0.

## 5. `notifications/resources/updated`

**C-9 (delivery predicate).** Exactly the existing predicate at
`catalog_notifications.rs:271-277` — route scope on the event's *owning
upstream*, plus the session's own subscription set.

**C-10.** The URI carried downstream is the gateway form — byte-identical to
what the client passed to `subscribe`.

**C-11.** One session subscribed to one URI receives one notification per event.
**Guaranteed by the SDK**, not by Labby: a session is either legacy or modern
and cannot hold both registration types. The residual HTTP double-registration
path (per-request classification, `tower.rs:2087-2099`) is an F-0 artifact,
eliminated by G-0.

**C-12 (at-most-once).** Delivery is not guaranteed:

| Mode | Cause | Threshold |
|---|---|---|
| Fan-out race | subscribe lands after the registry snapshot | — |
| Post-unsubscribe straggler | snapshot predates the unsubscribe | — |
| Delivery timeout | `resolved_catalog_notification_timeout()` | 5 s default |
| Broadcast lag | `RecvError::Lagged`, capacity 1024 | **~17 ev/s** with a stdio upstream mid-`tools/list_changed` (`STDIO_DISCOVERY_TIMEOUT` = 60 s, `helpers.rs:37,44`); ~205 ev/s otherwise |
| Upstream removed | MCP has no server-initiated unsubscribe | — |

**C-12b (new).** An accepted loss mode must be *observable*. The `Lagged` warn
(`peers.rs:327-329`) carries only `skipped` — no `surface`/`service`/`action`,
so it never appears in a `surface="mcp"` query, and
`notify_resource_update_peers` has no tracing at all. Fix both (tracked as S3);
otherwise C-12 documents a failure nobody can detect.

## 6. `*/list_changed` isolation

**C-13.** A session registered solely through `resources/subscribe` receives no
`*/list_changed` notifications. The production `LegacyPeer` variant must return
`false` from all three predicates — the `#[cfg(test)]` variant returns `true`
unconditionally (`peers.rs:49-71`) and must not ship as-is.

> This is a *semantic* gate, not a performance one:
> `catalog_notifications.rs:342-350` computes `visible_contract().await` before
> consulting the predicate, so a legacy peer still pays the recompute. Hoisting
> the predicate is tracked as S5.

## 7. Bridge

**C-14.** The bridge forwards daemon errors unaltered. Already true —
`bridge.rs:127-138` matches `ServiceError::McpError` first and returns the
daemon's `ErrorData` verbatim. No work needed.

**C-15 (rewritten).** The original — "add `on_resource_updated` to
`BridgeClientHandler`" — **cannot work in either branch.** `Peer::listen`'s own
documentation (`rmcp service/client.rs:1058-1059`) states that notifications
routed to the returned `Subscription` are *not also* delivered through
`ClientHandler` callbacks; and without a `listen()` the daemon never registers
the bridge as a peer at all.

The correct design: open **one** daemon-side `listen()` at bridge connect, with
`resource_subscriptions` set to the full subscribable snapshot, and filter
downstream locally in the bridge. Subscribe/unsubscribe becomes a local
`BTreeSet` mutation.

**C-15b (new).** Do **not** re-listen per subscribe. `SubscriptionSink.accepted`
is a plain `Arc<SubscriptionFilter>` with no setter (`service/server.rs:139-144`)
and streams are "not resumable" (`service/client.rs:322-324`), so extending
means cancel + re-listen. Beyond the cost (N round trips, N × 3-6 ms daemon CPU,
2N `peer.connect`/`peer.disconnect` INFO lines for N URIs), the gap between
`cancel()` and the new ack is a **deterministic loss window for every
already-subscribed URI** — subscribing to URI #10 would silently drop an update
for URI #1.

**C-16.** Resource updates only; no `list_changed` relay for legacy bridge
clients.

**C-18 (new).** Bridged subscriptions run at `McpRouteScope::Root` by
construction — `connect_service` always targets `/mcp` root with the bridge's
own token (`live_gateway.rs:99,420-441`) and cannot honor `protected_mcp_routes`.
State this in code near the bridge changes so no reader assumes C-4 provides
isolation there.

## 8. Invariant summary

| ID | Invariant | Change |
|---|---|---|
| C-1 | Per-session: advertise exactly the mechanism the session can use | **revised** — original was unachievable |
| C-2 | Deliverability = ack-snapshot membership | — |
| C-3 | Ownership by reverse lookup, never by prefix | — |
| C-4 | Scope denial indistinguishable from not-found (wire bytes) | scoped; noted as currently unreachable |
| C-5 | `retry_later` only in the reconnect window | — |
| C-6 | Subscribe idempotent | — |
| C-7 | Unsubscribe idempotent | — |
| C-8 | Empty set ⇒ entry removed | — |
| C-8b | Unsubscribe never registers; concurrent unsubscribe converges | **new** |
| C-9 | Delivery predicate unchanged | — |
| C-10 | Gateway-form URI round-trips | — |
| C-11 | One notification per session per event | now SDK-guaranteed |
| C-12 | At-most-once delivery | thresholds quantified |
| C-12b | Accepted loss modes must be observable | **new** |
| C-13 | No `list_changed` for legacy subscribers | — |
| C-14 | Bridge passes daemon errors through unaltered | already holds |
| C-15 | Bridge relays via one daemon-side `listen()`, filtered locally | **rewritten** |
| C-15b | Never re-listen per subscribe | **new** |
| C-16 | No `list_changed` relay to legacy bridge clients | — |
| C-17 | `-32002` is era-correct; do not "fix" to `-32602` | **new** |
| C-18 | Bridged subscriptions are Root-scoped by construction | **new** |

Gates: **G-0** (transport, new) · G-1 (gateway) · G-3 (route scope) ·
G-4 (snapshot) · **G-5** (cap, new). G-2 deleted.
