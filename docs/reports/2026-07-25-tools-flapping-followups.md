# Tools "flapping" — what's fixed, what's left, and why

Status as of 2026-07-25, after `#260`, `#262`, `#264`, `#267`.

## TL;DR

The gateway is no longer emitting spurious `tools/list_changed`. Measured on the
live gateway over 90 minutes with real clients connected:

| Signal | Count |
|---|---|
| `catalog.notify` (notifications sent) | **0** |
| `catalog.notify.churn` (burst warnings) | **0** |
| `during_tool_call=true` (mid-turn invalidation) | **0** |
| `catalog.refresh.finish` with a visible change | **0** |

Two things remain, and they are unrelated to each other:

1. **A gateway bug** — the connected-peer registry is never pruned, so dead
   sessions accumulate without bound. Fixed here.
2. **Client-side session churn** — Codex opens a new MCP session per
   interaction. This is what still *looks* like flapping, and it cannot be
   fixed from the gateway. Needs action on `winhost`.

---

## 1. Peer registry is never pruned (gateway bug)

### Evidence

```
peer.gc events in 6 hours:        0
peer_count before service restart: 119
peer_count after restart:          1 → 10 and climbing
```

Every `peer.connect` increments; nothing ever decrements.

### Why it happens

Peers are removed in exactly one place — inside the notification fanout
(`crates/labby/src/mcp/catalog_notifications.rs`), which prunes peers whose
notification failed:

```rust
let alive: Vec<RegisteredPeer> = evaluated.into_iter().zip(results)
    .filter_map(|(evaluated, ok)| ok.then(|| evaluated.into_published()))
    .collect();
*guard = alive;
```

There is no other removal path. Pruning was therefore a **side effect of
sending notifications**.

This is the uncomfortable part: **fixing the flapping removed the mechanism
that was garbage-collecting dead peers.** Before `#260`/`#264`/`#267`, the
gateway emitted spurious notifications frequently, and each fanout swept the
registry. Now that notifications are correctly rare — zero, in a 90-minute
window — the sweep effectively never runs.

So this is not a regression in the usual sense (the coupling always existed),
but it was latent and is now load-bearing.

### Impact

- Unbounded growth. Each entry holds a `Peer<RoleServer>`, a `PeerContract`
  (two `Arc`s + route scope), and a `last_contract` tool-name set. Small per
  entry, unbounded in aggregate. 119 accumulated in roughly a day.
- `peer_count`, `peers_notified`, and `peers_skipped` are inflated, which
  degrades exactly the telemetry added to diagnose flapping.
- When a real notification finally fires, it fans out to a pile of dead
  sessions, each burning the notification timeout before being pruned.

### Fix

Sweep closed peers on `peer.connect`, using `rmcp::Peer::is_transport_closed()`
— already used for the same purpose in
`crates/labby-gateway/src/upstream/pool/relay.rs`.

Pruning on connect (rather than on a timer) bounds growth by connection rate
with no background task, and needs no new lifecycle hook. Only definitively
closed transports are dropped, so a live-but-idle session is never evicted —
evicting a live peer would silently cost it notifications, which is worse than
holding a dead one.

---

## 2. Codex opens a new MCP session per interaction (client-side)

### Evidence

```
12:17:58  session.init → server.info → peer.connect(8)  → 1 call_tool
12:18:06  session.init → server.info → peer.connect(9)  → 1 call_tool
12:51:13  session.init ×2            → peer.connect(10) → read_resource
```

Two sessions **8 seconds apart**. The HTTP MCP session TTL is 300s, so this is
not expiry — the client simply is not reusing its session.

### Why it looks like flapping

Every new session re-runs `initialize` → `tools/list`. From the user's side,
the tool list is being rebuilt constantly, which is indistinguishable from the
server invalidating it. The difference is visible only in the logs: the server
sent **zero** `tools/list_changed`, so nothing was invalidated — the client
discarded and rediscovered on its own.

### Current config on `winhost`

`~/.codex/config.toml` (Codex CLI **0.144.6**):

```toml
[mcp_servers.labby]
url = "https://dinglebear.ai/mcp"
```

No transport or session options set.

### What to try — UNVERIFIED, needs testing on `winhost`

I have not reproduced or confirmed any of the following. Listing them as
candidates, in order of likelihood, explicitly so nobody mistakes them for a
diagnosis:

1. **Codex's streamable-HTTP client may not persist `Mcp-Session-Id` between
   calls.** Codex has carried an `experimental_use_rmcp_client` option for the
   rmcp-based HTTP client; if this build supports it, enabling it is the first
   thing to test.
2. **Codex version.** 0.144.6 — check whether a newer build changes HTTP MCP
   session handling.
3. **The OAuth token is re-fetched per call**, forcing a fresh session. The
   scope fix (`#268`) landed immediately before these observations, so the
   client may simply have been re-authenticating.

### How to confirm a fix

On the gateway, watch `peer.connect` frequency against actual tool calls:

```bash
incus exec labby -- journalctl -u labby.service --since "-30 min" -o cat \
  | grep -cE "action=peer.connect"
incus exec labby -- journalctl -u labby.service --since "-30 min" -o cat \
  | grep -cE "action=call_tool"
```

Healthy: connects ≪ calls (one session serving many calls). Currently they are
roughly 1:1, which is the bug.

---

## What is explicitly NOT the problem

Recorded so these get ruled out fast next time:

- **Notification churn.** Zero notifications sent. `#260`/`#264`/`#267` hold.
- **Mid-turn invalidation.** `during_tool_call=true` never fired.
- **Catalog instability.** No reconcile reported a visible-contract change.
- **The scope rejection** (`#268`) — separate issue, fixed, and it predated the
  flapping reports by three days.
