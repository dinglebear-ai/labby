# Specification — Legacy resource subscription passthrough

- **Issue:** [dinglebear-ai/labby#211](https://github.com/dinglebear-ai/labby/issues/211)
- **Epic bead:** `lab-n27j2` · **Base:** `132448802` · **Branch:** `feat/resource-subscriptions-211`
- **Status:** Revised after research + engineering review (2026-08-05). **Scope reduced.**

Read [FINDINGS.md](FINDINGS.md) first — it carries the verified evidence this
document is built on.

---

## 1. Problem statement

Labby advertises the MCP `resources.subscribe` capability but does not implement
`resources/subscribe` or `resources/unsubscribe`. Pre-`2026-07-28` clients
receive an **advertised capability that does not work**: rmcp's default trait
body returns `method_not_found`.

Verified on `origin/main`:

| Fact | Evidence |
|---|---|
| Capability advertised when a gateway manager is configured | `crates/labby/src/mcp/server.rs:414-415` |
| No `subscribe`/`unsubscribe` impl on `LabMcpServer` | `git grep -nE "fn (subscribe\|unsubscribe)" origin/main -- crates/labby/src/mcp/server.rs` → no match |
| rmcp's default body returns method-not-found | `rmcp-3.1.0/src/handler/server.rs:434-451` |
| Legacy clients are a real, currently-served population | `server.rs:360-384` (`compat_legacy_initialize`), restored by commit `9aa1f912b` |

The issue's own "Plan" section is **stale**: it asks to add
`.enable_resources_subscribe()` (already present since `21ef0c93e`) and to build
`resources/updated` aggregation (already built).

### 1.1 …but the gap is not the defect

Review established that filling the gap is not the right fix. The defect is
better stated as: **Labby advertises a capability it cannot deliver on its
primary network transport.** See [FINDINGS §1](FINDINGS.md#1-the-decisive-finding--f-0).

On HTTP there is no server→client channel outside an open POST, and a
`subscribe` returning `Ok(())` closes its own transport. Implementing the
handlers naively would replace a loud, recoverable `-32601` with a silent
`Ok(())` that never delivers — worse, and invisible to any test that only
asserts the return value.

## 2. What already exists (do not rebuild)

The delivery pipeline is complete and serves the modern `subscriptions/listen`
path:

```
upstream → RelayConnectionHandler::on_resource_updated   (pool/relay.rs:441)
         → gateway_resource_uri() rewrite                 (pool/notifications.rs:140)
         → UpstreamNotificationEvent::ResourceUpdated     (broadcast, cap 1024)
         → run_upstream_notifications                     (mcp/peers.rs:321)
         → notify_resource_update_peers                   (mcp/catalog_notifications.rs:269-302)
         → route-scope + subscription filter, timeout, prune
         → downstream peer
```

Also present: `subscribable_resource_uris_snapshot()`
(`pool/notifications.rs:136`), generation guards (`41db7d4a2`),
`accepted_subscription_filter()` (`server.rs:492-529`), `listen()`
(`server.rs:531-575`), `prune_closed_peers()` (`peers.rs:161-166`).

## 3. Scope

### 3.1 Approved — P0

| ID | Requirement |
|---|---|
| **R0.1** | Clear `capabilities.resources.subscribe` for legacy sessions in `compat_legacy_initialize` (`server.rs:378-384`), so the capability is advertised only where it can be honored. |
| **R0.2** | Do **not** remove `.enable_resources_subscribe()` from `get_info()`. MCP shares one flag between the deprecated RPCs and `subscriptions/listen`; removing it breaks the modern path and Labby's own upstream negotiation (`pool/notifications.rs:270`). See [FINDINGS §3](FINDINGS.md#3-the-capability-flag-is-shared--the-just-stop-advertising-it-fix-needs-care). |
| **R0.3** | Fix two false comments claiming one `LabMcpServer` per session (`cli/serve.rs:1693`, `mcp/server.rs:38-42`). True on stdio, false on HTTP — they are what misled the original plan. |
| **R0.4** | Tests: a legacy `initialize` does not advertise `resources.subscribe`; a modern `discover` still does; a modern `listen` with `resource_subscriptions` still works end to end. |
| **R0.5** | Document in `docs/surfaces/MCP.md` that legacy resource subscription is not offered, and why. |

### 3.2 Deferred — gated on demand

**R1–R7** (the handler build, per-peer tracking, `LegacyPeer` promotion, bridge
support) are **blocked on identifying a real legacy stdio consumer.** If none
surfaces, #211 closes as resolved by P0.

Should they be approved, the constraints in [CONTRACT.md](CONTRACT.md) and
[FINDINGS §4-6](FINDINGS.md#4-phase-d-was-dead-in-both-branches) are binding —
particularly gate **G-0**, the deletion of **G-2**, and the bridge's
single-listen design.

### 3.3 Split out — unblocked, independently valuable

These are pre-existing, benefit the **modern** path today, and should not wait
on this epic:

| ID | Work | Evidence |
|---|---|---|
| **S1** | Missing auth-scope gate on subscription acceptance | FINDINGS §5 |
| **S2** | `ui://` route-scope bypass (`lab-1415y`) | `server.rs:509-517` |
| **S3** | Standard fields on the `Lagged` warn; tracing in `notify_resource_update_peers` | FINDINGS §6 |
| **S4** | `prune_closed_peers` call site independent of `listen()` | FINDINGS §1.1 |
| **S5** | P-1 predicate short-circuit; P-2 filter-under-read-lock | FINDINGS §6 |
| **S6** | `docs/services/GATEWAY.md` stale claim about legacy `initialize` | FINDINGS §8 |

### 3.4 Non-goals

| ID | Non-goal | Rationale |
|---|---|---|
| **N1** | Upstream-side legacy subscribe | Labby subscribes upstream only via `subscriptions/listen` |
| **N2** | HTTP subscription support (2025-era GET/SSE stream + session management) | **Not a technical impossibility — a product decision.** MCP defines this channel; Labby declines it by implementing only 2026-07-28 transport semantics. Serving legacy subscribe over HTTP requires supporting *both* transport eras. Out of scope here, but see [FINDINGS §1.2](FINDINGS.md#12-the-real-decision) — an earlier draft wrongly dismissed this as a rewrite |
| **N3** | Eliminating broadcast-lag loss | Pre-existing; affects both paths. Make it *observable* (S3) rather than fixing it |
| **N4** | Server-initiated unsubscribe on upstream removal | MCP has no such method |

## 4. Functional requirements — P0

`compat_legacy_initialize` is the only legacy entry point; modern clients use
`discover`, which returns `get_info()` unmodified (`server.rs:485-489`).
Clearing the flag there is therefore precise:

| Session type | Lifecycle | `resources.subscribe` advertised | `resources/subscribe` | `subscriptions/listen` |
|---|---|---|---|---|
| Legacy (< 2026-07-28) | `initialize` | **no** (R0.1) | `-32601` (rmcp default) | `-32601` (rmcp, `handler/server.rs:146-149`) |
| Modern (2026-07-28) | `discover` | yes | `-32601` (rmcp, `:185-201`) | works |

Both rows are internally consistent: each session type is told exactly what it
can use. That is capability honesty in the only form MCP's shared flag permits.

## 5. Non-functional requirements

- **Observability** (`docs/dev/OBSERVABILITY.md`): P0 adds no request path. The
  observability debt it exposes is tracked as S3.
- **Error contract**: P0 introduces no new error kinds. The originally-planned
  `unsupported_protocol_method` is **dropped** — rmcp enforces that gate itself,
  which also removes an `ERRORS.md` / `agent-error-contract.md` spec change.
- **Style** (root `CLAUDE.md`): no `mod.rs`; native `async fn` in trait; no
  `#[async_trait]`; `tracing` only.
- **Verification**: `just lint`, `just test`, `just docs-check`, plus
  `cargo check -p labby --no-default-features --features gateway`.

## 6. Acceptance criteria — P0

1. A legacy `initialize` response does **not** advertise `resources.subscribe`.
2. A modern `discover` response **does** advertise it.
3. A modern `subscriptions/listen` carrying `resource_subscriptions` still
   receives `resources/updated` end to end — proving R0.2 was honored.
4. Labby's upstream subscription negotiation is unaffected
   (`pool/notifications.rs:270` still sees the flag).
5. The two false comments are corrected.
6. `docs/surfaces/MCP.md` states the boundary and the reason.
7. `just lint`, `just test`, `just docs-check` green.

> **The original acceptance criterion 10 — "delivery verified on stdio and
> streamable-HTTP" — is void.** It asserts something F-0 proves impossible. Any
> future handler build must replace it with a *negative* assertion: HTTP
> subscribe is rejected and the registry length is unchanged.

## 7. Open items

All three original open items are **resolved**; see
[PROGRESS.md](PROGRESS.md#open-items--all-resolved).

- **O-1** (session identity) — `context.protocol_version()` is public API; the
  original plan had the polarity of the risk backwards.
- **O-2** (bridge protocol) — yes, the bridge negotiates `2026-07-28`, and the
  problem is worse than the version: there is no push channel at all.
- **O-3** (conformance suites) — `mcp-regressions` uses explicit test-name
  filters (not auto-discovery); `mcp-conformance` runs only against
  `2026-07-28` and will never exercise a legacy subscribe.

## 8. References

- [FINDINGS.md](FINDINGS.md) — verified evidence, including refuted claims
- [CONTRACT.md](CONTRACT.md) — wire contract, if the build is ever approved
- [TYPES.md](TYPES.md) · [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) · [PROGRESS.md](PROGRESS.md)
