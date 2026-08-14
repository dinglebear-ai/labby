# Implementation plan — Legacy resource subscription passthrough

- **Issue:** [#211](https://github.com/dinglebear-ai/labby/issues/211) · **Epic:** `lab-n27j2`
- **Integrated base:** `origin/main` at `313e58969` · **Branch:** `feat/resource-subscriptions-211`
- **Status:** Revised after review. **P0 is implemented; Phases A–E are deferred.**

Read [FINDINGS.md](FINDINGS.md) first, then [SPEC.md](SPEC.md) and
[CONTRACT.md](CONTRACT.md). Code marked **(current)** is verbatim from
`origin/main`; **(target)** shows structure and the invariant being enforced,
not a patch to apply blindly.

---

## Work map

| Bead | Work | Status | Size |
|---|---|---|---|
| **P0** | Version-conditional capability advertisement + comment fixes | **implemented** | done |
| **S1–S6** | Split-out beads (see [SPEC §3.3](SPEC.md#33-split-out--unblocked-independently-valuable)) | unblocked | ~half day each |
| **A–E** | Handler build, pool API, bridge | **blocked on demand** | — |

---

# P0 — Restore capability honesty

**File:** `crates/labby/src/mcp/server.rs` · **Bead:** `lab-n27j2.4` (supersedes `lab-n27j2.1` as the first deliverable)

## P0.1 Why this shape

Two facts make this precise rather than a hack:

1. **`compat_legacy_initialize` is the only legacy entry point.** Modern clients
   negotiate through `discover`, which returns `get_info()` untouched
   (`server.rs:485-489`).
2. **It already returns a modified `ServerInfo`** — so mutating capabilities
   there is an established pattern, not a new one.

```rust
// server.rs:378-383  (current)
context.peer.set_peer_info(request.clone());
let mut info = self.get_info();
// The legacy wire protocol requires echoing the negotiated version in
// initialize/result. This is an edge adapter only; internal handling
// remains stateless and all modern clients use server/discover.
info.protocol_version = request.protocol_version;
Ok(info)
```

## P0.2 The change

```rust
// (target)
let mut info = self.get_info();
info.protocol_version = request.protocol_version;

// MCP shares one `resources.subscribe` flag between the deprecated
// resources/subscribe RPCs and the modern subscriptions/listen mechanism
// (rmcp model.rs:2013-2018 — SubscriptionFilter::supported_by). We must keep
// advertising it on the discover path or modern subscriptions break, including
// our own upstream negotiation (pool/notifications.rs:270). But a legacy
// session can only reach the deprecated RPCs, which rmcp answers with
// method_not_found (handler/server.rs:434-451) — so advertising the flag here
// promises something that cannot work. Clear it for legacy sessions only.
if let Some(resources) = info.capabilities.resources.as_mut() {
    resources.subscribe = None;
}
Ok(info)
```

Confirm the exact field shape against `ServerCapabilities` before writing —
`enable_resources_subscribe` sets `c.subscribe = Some(true)`
(`rmcp model/capabilities.rs:417-422`), so `None` (not `Some(false)`) is the
correct clear.

## P0.3 Correct two false comments

Both claim one `LabMcpServer` per session. True for stdio, **false for HTTP**,
where `get_service()` runs per POST (`rmcp tower.rs:1947`). These are what led
the original plan to a per-instance session cell.

- `crates/labby/src/cli/serve.rs:1693` — "constructs a new `LabMcpServer` per session"
- `crates/labby/src/mcp/server.rs:38-42` — "Each transport session … builds one `LabMcpServer`, so the id is stable for a session's lifetime"

Replace with the accurate statement: one instance per stdio process; one per
HTTP POST under stateless mode, all sharing the peer registry
(`serve.rs:1643,1753`).

## P0.4 Tests

- [x] Legacy `initialize` response does **not** advertise `resources.subscribe`
- [x] Modern `discover` remains unmodified
- [x] Modern `subscriptions/listen` with `resource_subscriptions` still delivers
      `resources/updated` end to end — this is the regression guard for R0.2
- [x] Upstream negotiation unaffected: `SubscriptionFilter::supported_by` still
      sees the flag (`pool/notifications.rs:270`)

## P0.5 Docs

`docs/surfaces/MCP.md`: state that legacy resource subscription is not offered,
that the modern path is `subscriptions/listen`, and that the shared capability
flag is why the boundary is drawn at the session level. Then
`just docs-generate` and `just docs-check`.

## P0.6 Verify

```bash
just lint
just test
just docs-check
cargo check -p labby --no-default-features --features gateway
```

---

# Deferred — Phases A–E

**Do not start without an identified legacy stdio consumer.** Recorded so the
work is resumable, with the review's corrections folded in.

## Phase 0 (new) — transport gate

The first thing any handler build must do. Reject `subscribe` wherever a
server-initiated notification cannot be delivered — a hard structural check on
the transport, never a config flag, so the HTTP silent-accept cannot be
reintroduced by a later refactor.

`transport_label` already exists (`server.rs:263`, set at `serve.rs:1647,1795`).
Gate **G-0**, with `recovery.action = "do_not_retry"`.

Without this, the build ships a worse capability violation than the one it
fixes. See [FINDINGS §1](FINDINGS.md#1-the-decisive-finding--f-0).

## Phase A — promote `LegacyPeer`

**File:** `crates/labby/src/mcp/peers.rs`

```rust
// peers.rs:41-46  (current)
#[derive(Clone)]
pub(crate) enum NotificationTarget {
    #[cfg(test)]
    LegacyPeer(Peer<RoleServer>),
    Subscription(SubscriptionSink),
}
```

Target: carry `Arc<std::sync::RwLock<BTreeSet<String>>>` alongside the peer;
`wants_resource_update` becomes set membership; the three `wants_*_list_changed`
return `false` (C-13). Full shape in [TYPES.md](TYPES.md).

Corrections from review:

- **Fixture migration is one call site**, not a sub-phase —
  `catalog_notifications.rs:409-411`. And option (a) (a `Subscription`-backed
  fixture) is *impossible*: `SubscriptionSink` has private fields and a private
  constructor (`rmcp service/server.rs:139-158`). Keep a `#[cfg(test)]`
  permissive variant; it is the only option, not a compromise.
- **Seed `last_contract` empty.** C-13 makes it permanently unreadable for a
  legacy peer, so seeding a real `visible_contract()` wastes 3-6 ms on a
  client-facing path (`ToolCatalogSnapshot` has no `Default` — `catalog.rs:77`).
- **Poisoned locks**: use `unwrap_or_else(|e| e.into_inner())` per house pattern
  (`lab-2ehcf`, `lab-2xkf`) — a `BTreeSet<String>` has no invariants a panic
  could violate. Log `ERROR` if it happens; a silently fail-closed session that
  never recovers is worse than the panic.
- **Lock ordering invariant**: the registry lock is always outermost; the
  subscription-set lock is never held across a registry acquisition. A future
  `peers.read().await` → find → `subscriptions.write()` would wedge a tokio
  *worker*, and no test would catch it.

## Phase B — handlers

**File:** a new `crates/labby/src/mcp/handlers_subscriptions.rs` — **not**
`server.rs`, which is already 1299 lines. Follow the dominant pattern
(`handlers_resources.rs`, `handlers_prompts.rs`): thin wrappers in `server.rs`,
logic in the handler module. Register in `crates/labby/src/mcp.rs`; no `mod.rs`.

Gate order per [CONTRACT §3.1](CONTRACT.md#31-gate-table-normative-first-match-wins):
**G-0 → G-1 → G-3 (route scope) → G-4 (snapshot) → G-5 (cap)**. Note this
**corrects the first draft**, which put snapshot before route scope and leaked
the reconnect signal to out-of-scope callers.

Deleted from the original plan:

- **G-2 and `negotiated_modern_protocol()`** — rmcp enforces it
  (`handler/server.rs:185-201`), symmetrically with `listen()` (`:146-149`).
- **The `unsupported_protocol_method` error kind**, and with it the
  `ERRORS.md` / `agent-error-contract.md` spec change and the
  `labby-runtime` classification-table entries it would have needed.
- **`rediscover_advice()`** — `recovery_for_kind("not_found")` already defaults
  to `Rediscover`/`Never` (`labby-runtime/src/agent_error.rs:459,517`). Only the
  `retry_later` branch needs an explicit override.

Put the remaining constructor(s) in the existing
`crates/labby/src/mcp/resource_errors.rs` — a new module for two functions is
over-modularization.

### Phase B0 (new) — pool API, its own bead

`upstream_owning_resource` and `catalog_lists_resource` **do not exist**, and the
gap is bigger than a rename: `entry.resource_uris` holds native URIs,
`subscribable_resource_uris_snapshot()` holds gateway-form, and
`gateway_resource_uri` is `pub(super)`. This needs **new public API on
`UpstreamPool` in `labby-gateway`** — a cross-crate change the original plan
budgeted at zero.

```rust
// (target) — takes gateway-form, rewrites internally
fn upstream_owning_gateway_uri(&self, gateway_uri: &str) -> Option<String>
```

Do not widen `gateway_resource_uri`'s visibility. Build from the reverse lookup
at `pool/resources_read.rs:95-150`; do not write a parallel implementation.

Independently landable and testable — hence its own bead.

## Phase C — fold into A/B

~10 lines, unobservable without Phase B. Two review corrections:

- The variant match **is** load-bearing: `Subscription::is_closed()` returns
  `false` unconditionally by design (`peers.rs:85-92`), so a bare
  `!is_closed()` retain would stop pruning dead subscription channels. Move it
  behind a named predicate on `NotificationTarget` rather than inlining it in
  `catalog_notifications.rs`.
- **The escape clause needs a bound.** "A legacy peer may be merely slow"
  justifies surviving one timeout, not thirty — a wedged peer would otherwise
  tax every future event by the full 5 s timeout, because `join_all` waits for
  the slowest. Add a consecutive-failure counter (k=3) and a `WARN` on delivery
  failure; `notify_resource_update_peers` currently has **no tracing at all**.

## Phase D — bridge, redesigned

The original design was dead in both branches
([FINDINGS §4](FINDINGS.md#4-phase-d-was-dead-in-both-branches)). Replacement:
one daemon-side `listen()` opened at bridge connect with a superset filter,
filtered locally in the bridge. Never re-listen per subscribe (C-15b).

Four gaps the original did not budget:

1. **No owner for the subscription.** Today's relay is driven by
   `SubscriptionContext` and lives exactly as long as a downstream `listen`
   request. A legacy client never calls `listen`, so this becomes a spawned
   background task plus mutable state on `BridgeServerHandler` — the first
   background-task lifecycle in `bridge.rs`.
2. **Downstream disconnect leaks the task** unless tied to a `CancellationToken`
   or a `Drop` impl that does not exist.
3. **A dropped daemon stream fails silently** — there is no downstream request
   to fail into. Needs explicit reconnect-with-backoff.
4. **Test-shape trap:** `wire_bridge` uses `FakeDaemonHandler` (`bridge.rs:705`).
   A fake daemon emitting a *bare* notification would exercise
   `on_resource_updated` and pass while production fails. The test must drive
   the notification through a real `subscriptions/listen`.

Sized honestly, Phase D exceeds A+B combined. The original budgeted two code
snippets.

## Phase E — docs and conformance

- `mcp-regressions` (`.github/workflows/ci.yml:548-588`) is a list of explicit
  `cargo test <name filter>` invocations — **new tests are not auto-discovered**;
  add a filter line.
- `mcp-conformance` drives the pinned rmcp fixture at
  `MCP_SPEC_VERSION=2026-07-28` (`scripts/ci/mcp-conformance.sh:37-39`) and will
  **never** send a legacy `resources/subscribe`. "Conformance wired" can only
  mean regression rows.
- The C-1 assertion must be transport-aware: *advertised ⟺ a subscription on
  this transport can actually receive a notification.* A test asserting only
  `Ok(())` would certify the F-0 lie.

---

## Risk register

| Risk | Mitigation |
|---|---|
| P0 breaks modern subscriptions | R0.2 + acceptance criteria 3 and 4 are the guard. `SubscriptionFilter::supported_by` reads the same flag |
| Future refactor reintroduces HTTP silent-accept | G-0 as a structural transport check, not config |
| Implementer reaches for the prefix shortcut in `accepted_subscription_filter` | C-3, and B0 building on `resources_read.rs:95-150` |
| `audit/mcp-2026-07-28-capabilities` conflict | **Overstated in the first draft.** It touches neither `LegacyPeer`, the prune logic, nor subscribe. Textual merge noise in `server.rs`/`bridge.rs` only; re-diff before the PR |
| Base has advanced | Rebased cleanly onto `origin/main` at `313e58969` on 2026-08-13; citations were rechecked during implementation |

## Definition of done — P0

[SPEC §6](SPEC.md#6-acceptance-criteria--p0) criteria 1-7 pass; `just lint`,
`just test`, `just docs-check` green; `docs/surfaces/MCP.md` states the
boundary; the two false comments are corrected.
