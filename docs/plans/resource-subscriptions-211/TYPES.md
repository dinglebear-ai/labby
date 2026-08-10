# Types and models — Legacy resource subscriptions

Rust types this work introduces, changes, or consumes. Companion to
[SPEC.md](SPEC.md) and [CONTRACT.md](CONTRACT.md). Base commit `132448802`.

Code blocks marked **(current)** are verbatim from `origin/main`. Blocks marked
**(proposed)** are the target shape.

> **Status: conditional.** The handler build is deferred pending a real legacy
> stdio consumer ([SPEC §3.2](SPEC.md#32-deferred--gated-on-demand)). The
> approved P0 work introduces **no new types** — it clears one capability field
> in `compat_legacy_initialize`. Everything below applies only if the full build
> is approved. Corrections from the engineering review are marked **[revised]**.

## 0. What the review changed

| Item | Original | Revised |
|---|---|---|
| `unsupported_protocol_method` error kind | new kind + constructor + `ERRORS.md` spec change + `labby-runtime` table entries | **deleted** — rmcp enforces the gate itself (`handler/server.rs:185-201`) |
| `rediscover_advice()` helper | build explicitly | **deleted** — `recovery_for_kind("not_found")` already defaults to `Rediscover`/`Never` (`labby-runtime/src/agent_error.rs:459,517`) |
| New `subscription_errors.rs` module | new file | **fold into `resource_errors.rs`** — one constructor does not warrant a module |
| Poisoned-lock handling | `is_ok_and` (fail closed, silent) | `unwrap_or_else(\|e\| e.into_inner())` + `ERROR` log, per house pattern `lab-2ehcf`/`lab-2xkf` |
| `last_contract` seeding | mirror `listen()`'s real `visible_contract()` | **seed empty** — C-13 makes it permanently unreadable; a real one costs 3-6 ms on a client-facing path |
| Test-fixture migration | choose option (a) or (b) | **(b) is the only option** — `SubscriptionSink` has private fields and a private constructor (`rmcp service/server.rs:139-158`) |

---

## 1. Consumed from rmcp 3.1.0

Exact pin: `rmcp = "=3.1.0"` in `[workspace.dependencies]`.

### 1.1 Handler methods (current)

`rmcp-3.1.0/src/handler/server.rs:435-450` — both carry `#[deprecated]`, so an
implementation needs `#[allow(deprecated)]`, matching the existing precedent on
`get_info` (`crates/labby/src/mcp/server.rs:387`).

```rust
fn subscribe(
    &self,
    request: SubscribeRequestParams,
    context: RequestContext<RoleServer>,
) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ {
    std::future::ready(Err(McpError::method_not_found::<SubscribeRequestMethod>()))
}

#[deprecated(
    note = "resources/unsubscribe is legacy-only; subscriptions/listen is cancelled through its request lifecycle"
)]
fn unsubscribe(
    &self,
    request: UnsubscribeRequestParams,
    context: RequestContext<RoleServer>,
) -> impl Future<Output = Result<(), McpError>> + MaybeSendFuture + '_ { /* ... */ }
```

The default body is exactly the bug: an advertised capability answering
`method_not_found`.

### 1.2 Param types (current)

`rmcp-3.1.0/src/model.rs:1806-1815`:

```rust
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SubscribeRequestParams {
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<RequestMetaObject>,
    pub uri: String,
}
```

`UnsubscribeRequestParams` is structurally identical.
`ResourceUpdatedNotificationParam::new(uri)` is already used at
`crates/labby/src/mcp/peers.rs:120-123`.

### 1.3 Error codes (current)

`rmcp-3.1.0/src/model.rs:550,594`:

```rust
pub const RESOURCE_NOT_FOUND: Self = Self(-32002);

pub fn resource_not_found(message: impl Into<Cow<'static, str>>, data: Option<Value>) -> Self {
    Self::new(ErrorCode::RESOURCE_NOT_FOUND, message, data)
}
```

## 2. `NotificationTarget` — the central change

### 2.1 Current

`crates/labby/src/mcp/peers.rs:41-46`:

```rust
#[derive(Clone)]
pub(crate) enum NotificationTarget {
    #[cfg(test)]
    LegacyPeer(Peer<RoleServer>),
    Subscription(SubscriptionSink),
}
```

Its predicates return `true` unconditionally for `LegacyPeer` — fine for a test
fixture, wrong for production (`peers.rs:47-81`):

```rust
pub(crate) fn wants_tool_list_changed(&self) -> bool {
    match self {
        #[cfg(test)]
        Self::LegacyPeer(_) => true,
        Self::Subscription(sink) => sink.accepted().tools_list_changed == Some(true),
    }
}
// wants_resource_list_changed, wants_prompt_list_changed: same shape
pub(crate) fn wants_resource_update(&self, uri: &str) -> bool {
    match self {
        #[cfg(test)]
        Self::LegacyPeer(_) => true,
        Self::Subscription(sink) => sink
            .accepted()
            .resource_subscriptions
            .as_ref()
            .is_some_and(|uris| uris.iter().any(|accepted| accepted == uri)),
    }
}
```

Shipping this as-is would deliver **every** resource update to **every** legacy
subscriber and enrol them in catalog `list_changed` fan-out — violating
contracts C-9 and C-13.

### 2.2 Proposed

```rust
/// Live, mutable subscription set for one legacy session.
///
/// `RegisteredPeer` is `Clone` and the fan-out clones a registry snapshot
/// before delivering (`catalog_notifications.rs:270`), so a plain `BTreeSet`
/// field would freeze at snapshot time and miss subscribes that land during
/// delivery. The `Arc` keeps every clone pointing at one set.
pub(crate) type LegacySubscriptions = Arc<RwLock<BTreeSet<String>>>;

#[derive(Clone)]
pub(crate) enum NotificationTarget {
    /// A pre-2026-07-28 session tracked through `resources/subscribe`.
    LegacyPeer {
        peer: Peer<RoleServer>,
        subscriptions: LegacySubscriptions,
    },
    Subscription(SubscriptionSink),
}
```

Predicate changes:

| Method | `LegacyPeer` returns | Contract |
|---|---|---|
| `wants_tool_list_changed` | `false` | C-13 |
| `wants_resource_list_changed` | `false` | C-13 |
| `wants_prompt_list_changed` | `false` | C-13 |
| `wants_resource_update(uri)` | set membership | C-9 |
| `is_closed` | `peer.is_transport_closed()` (unchanged) | — |
| `notify_resource_updated` | unchanged | — |

**`wants_resource_update` is synchronous** and would need to read an async
`RwLock`. Two viable resolutions, decided in Phase A:

1. Use `std::sync::RwLock` — the guard never crosses an `.await` here, so this
   is sound and keeps the predicate synchronous. Preferred.
2. Keep `tokio::sync::RwLock` and make the predicate async, which forces the
   `filter_map` at `catalog_notifications.rs:271` to become a two-pass
   collect-then-filter. More churn, no benefit.

> **Why `Arc<RwLock<BTreeSet>>` and not `DashMap`:** each entry is owned by
> exactly one session and mutated only by that session's request handlers.
> There is no cross-shard contention to relieve, and `BTreeSet` gives
> deterministic iteration for tests. Fleet precedent for `DashMap` (bead
> `lab-n07n`) concerns registries with many concurrent writers — and bead
> `lab-jnxon` is a live memory-exhaustion bug caused by an unevicted `DashMap`,
> which argues against it here.
>
> **Why not `arc-swap`** *(the obvious in-repo alternative — it is already a
> dependency and used at `pool.rs:122` for exactly this shape)*: that precedent
> is a read-mostly, rarely-rebuilt *global*. A per-session subscription set is
> neither, and copy-on-write would allocate the whole set on every
> subscribe/unsubscribe. The read is already ~1% of per-event cost. **[revised]**

### 2.2b Lock-ordering invariant **[revised]**

Choosing `std::sync::RwLock` makes it possible to filter under the registry read
lock — the change that makes fan-out cheap. It also creates a hazard the first
draft did not name:

> **The peer-registry lock is always outermost. The subscription-set lock is
> never held across a registry acquisition.**

A future subscribe-path refactor doing `peers.read().await` → find entry →
`subscriptions.write()` would deadlock — and because the inner lock is a `std`
lock, it wedges the tokio **worker thread**, not just the task. No test will
catch it. State the invariant in code next to `LegacySubscriptions`.

### 2.3 Test-fixture migration

`RegisteredPeer::with_last_contract_for_test`, `stale_for_test`, and
`current_for_test` (`peers.rs:167-215`) construct
`NotificationTarget::LegacyPeer(peer)`. Each must pass a subscription set.
Existing fan-out tests rely on the old unconditional `true` for
`wants_*_list_changed`; they exercise `list_changed`, not resource updates, so
they need a fixture whose predicates still return `true`. Options, decided in
Phase A:

- Give the test constructors a `Subscription`-backed target instead; or
- Add a `#[cfg(test)]` variant retaining permissive predicates, keeping the new
  production `LegacyPeer` strict.

Do **not** relax the production predicates to keep the fixtures compiling —
that reintroduces the C-13 violation.

## 3. Unchanged types this work depends on

### 3.1 `RegisteredPeer` (current) — `peers.rs:20-33`

```rust
#[derive(Clone)]
pub struct RegisteredPeer {
    pub(crate) registration_id: u64,
    pub(crate) target: NotificationTarget,
    pub(crate) contract: crate::mcp::peer_contract::PeerContract,
    pub(crate) last_contract: crate::mcp::catalog::ToolCatalogSnapshot,
}
```

`registration_id` comes from a process-global counter (`peers.rs:35-39`) and is
the identity used for removal — JSON-RPC request IDs are client-local and cannot
identify a shared registry entry.

A new constructor is needed:

```rust
impl RegisteredPeer {
    /// Register a legacy `resources/subscribe` session with an empty set.
    pub(crate) fn from_legacy_peer(
        peer: Peer<RoleServer>,
        contract: crate::mcp::peer_contract::PeerContract,
        last_contract: crate::mcp::catalog::ToolCatalogSnapshot,
    ) -> Self { /* ... */ }
}
```

### 3.2 `PeerRegistry` (current) — `peers.rs:146`

```rust
pub type PeerRegistry = Arc<RwLock<Vec<RegisteredPeer>>>;
```

A `Vec` scanned linearly. Fine at current scale; the fan-out already clones it
wholesale.

### 3.3 Deliverable-URI snapshot (current)

`crates/labby-gateway/src/upstream/pool/notifications.rs:136`:

```rust
pub fn subscribable_resource_uris_snapshot(&self) -> Arc<BTreeSet<String>>
```

Sole source of truth for contract C-2. Already consumed by
`accepted_subscription_filter` (`server.rs:497-501`).

### 3.4 Agent error context (current)

`crates/labby-runtime/src/agent_error.rs:301-324`:

```rust
pub struct AgentErrorContext {
    pub service: Option<String>,
    pub action: Option<String>,
    pub tool: Option<String>,
    pub upstream: Option<String>,
    pub command: Option<String>,
    pub prompt: Option<String>,
    pub resource: Option<String>,
    pub cause: Option<String>,
    pub origin: Option<AgentErrorOrigin>,
    pub recovery: Option<AgentRecoveryAdvice>,
    pub side_effects: Option<AgentSideEffectRisk>,
}
```

Constructed via `AgentErrorContext::for_service_action(service, action)`
(`agent_error.rs:328`). Rendered by
`crate::mcp::agent_error::{resource_not_found, invalid_params}`, which wrap
`build_agent_error_value` and stamp `contract_version: 1`.

**Note the `service` convention** (`resource_errors.rs:44-50`): `service` names
the *denying surface* (`"labby"`), never the requested target. `read_resource`
puts the requested service in a separate `denied_service` key — a pattern the
subscription surface must **not** copy, since it would distinguish out-of-scope
from not-found and break C-4.

## 4. New error constructors (proposed) **[revised]**

> **Fold these into the existing `crates/labby/src/mcp/resource_errors.rs`** — a
> new module for what is now a single constructor is over-modularization. The
> `unsupported_on_modern_session` constructor below is **deleted**: rmcp rejects
> modern sessions before the handler runs, so the gate and its error kind never
> existed as real work. Keep only `not_subscribable`.
>
> Note also that `context.origin = Discovery` and
> `side_effects = NoneExpected` are **redundant** for the `not_found` kind —
> `origin_for_kind` and `side_effects_for_kind` already classify it that way
> (`labby-runtime/src/agent_error.rs:459,487`). Only the `retry_later` recovery
> override does real work.

The shape below is retained for reference, with those corrections applied in
Phase B rather than rewritten here:

```rust
//! Model-actionable errors for legacy MCP resource subscriptions.

use labby_runtime::agent_error::{AgentErrorContext, AgentErrorOrigin, AgentSideEffectRisk};
use rmcp::ErrorData;

use crate::mcp::agent_error::{
    invalid_params as invalid_params_agent_error,
    resource_not_found as resource_not_found_agent_error,
};

fn context(uri: &str, action: &'static str) -> AgentErrorContext {
    let mut context = AgentErrorContext::for_service_action("labby", action);
    context.resource = Some(uri.to_string());
    context.origin = Some(AgentErrorOrigin::Discovery);
    context.side_effects = Some(AgentSideEffectRisk::NoneExpected);
    context
}

/// Gates G-3 and G-4. One constructor for both, so the two cases cannot drift
/// apart and become distinguishable (contract C-4).
#[must_use]
pub(crate) fn not_subscribable(uri: &str, action: &'static str, reconnecting: bool) -> ErrorData {
    let mut context = context(uri, action);
    context.recovery = Some(if reconnecting {
        // Catalog-listed but not yet acknowledged: the upstream is reconnecting.
        retry_later_advice()
    } else {
        rediscover_advice()
    });
    resource_not_found_agent_error(
        format!("resource is not subscribable: {uri}. Call resources/list and retry with a subscribable URI."),
        None,
        &context,
    )
}

/// Gate G-2: a 2026-07-28 session must use subscriptions/listen.
#[must_use]
pub(crate) fn unsupported_on_modern_session(uri: &str, action: &'static str) -> ErrorData {
    let mut context = context(uri, action);
    context.recovery = Some(revise_and_retry_advice());
    invalid_params_agent_error(
        "unsupported_protocol_method",
        format!("resources/{action} is legacy-only; this session negotiated 2026-07-28. Use subscriptions/listen."),
        None,
        &context,
    )
}
```

`retry_later_advice` / `rediscover_advice` / `revise_and_retry_advice` are thin
`AgentRecoveryAdvice` builders; confirm whether `labby_runtime::agent_error`
already exports equivalents before adding them.

## 5. Type-level invariants

| Invariant | Enforced by |
|---|---|
| A legacy session's set is shared across registry clones | `Arc` in `LegacySubscriptions` |
| Subscribe is idempotent | `BTreeSet::insert` |
| No duplicate delivery per session | one registry entry per session |
| Legacy subscribers get no `list_changed` | `wants_*_list_changed → false` |
| Not-found and out-of-scope indistinguishable | single `not_subscribable` constructor |
| No lock guard across `.await` | `std::sync::RwLock`, guard dropped before delivery |
