# MODELS — Code Mode Retained Results

Data model, ownership, state machine, and lifecycle.

> **Revised after research (2026-08-05).** The v1 draft hung retention off two new
> `CodeModeHost` trait accessors. Research proved that seam cannot work: the trait
> is implemented once, by the long-lived shared `GatewayManager`, which has no
> per-request `route_scope`, `actor_key`, or `surface`. Retention is now a
> **context passed into the kernel by the surface**, modeled on the existing
> `CodeModeSourceStore`. See §2 and [PROGRESS.md](./PROGRESS.md) decision log D-1/D-13.

Types in [TYPES.md](./TYPES.md); normative rules in [SPEC.md](./SPEC.md).

---

## 1. The precedent this is built on

`CodeModeSourceStore` ([types.rs:605-690](../../../crates/labby-codemode/src/types.rs)) is a near-exact
existing solution to the same problem — bounded in-memory, owner-scoped, id
lookup — and the design now follows it rather than inventing a parallel one:

| Aspect | `CodeModeSourceStore` (shipping) | Retained results (this feature) |
|---|---|---|
| Type lives in | `labby-codemode` | `labby-codemode` — same |
| Instance lives on | `Arc<Mutex<…>>` field on `GatewayManager` | Same |
| Reached via | Gateway inherent methods (`manager/code_mode_runtime.rs:65-79`) called from the **surface** (`mcp/call_tool_codemode.rs:724-739`) | Same |
| On the `CodeModeHost` trait? | **No** | **No** (was yes in v1 — removed) |
| Owner identity | `actor_key` + `route_scope` + `capability_filter_fingerprint` | Same |
| Capability comparison | **Containment**, not equality (`source_capability_within_lookup`) | Same |

Sibling `Arc`-on-manager state confirming this is the house pattern:
`code_mode_history`, `step_buffers`, `code_mode_catalog_render_cache`,
`code_mode_embedding_cache`, `code_mode_runner_pool`. None is on the trait.

## 2. Entity model

```
RetainedResultStore  1 ──── * RetainedEntry     (live values, payload-bearing)
                     1 ──── 1 RetentionConfig   (immutable caps)
                     1 ──── 1 RetentionCounters (observability)

RetentionContext  ──── 1 Arc<RetainedResultStore>  +  1 OwnerKey
                       (built ONCE per execution, at the surface)
```

`RetentionContext` is the whole seam. The surface — which has `route_scope`,
`actor_key`, and `surface` in scope — builds it and passes it into `execute()`
alongside the existing `caller` / `surface` / `config` / `scope` / `execution_id`
parameters. The kernel uses it without ever learning gateway vocabulary, and
`CodeModeHost` is untouched.

This single change resolves four separate research findings: the unreachable
route scope, the impossible CLI surface gating, the trait-cohesion objection, and
the per-paging-call fingerprint rebuild (the owner is derived once, not 64 times).

**Tombstones were cut.** The v1 draft kept a 256-entry payload-free ring purely to
distinguish *expired* and *evicted* from *not found*. All three carry an
identical recovery contract, so the distinction was diagnostic only — and the
same reasoning already justified collapsing `flushed` into `evicted`. Cutting it
removes a struct family, a ring buffer, two error kinds, and tests across three
beads.

## 3. Ownership model

### 3.1 The owner key

```
OwnerKey = (relay_session_id, actor_key, route_scope, capability_fingerprint)
```

| Component | Source | Why |
|---|---|---|
| `relay_session_id` | `self.relay_session_id` — `mcp/server.rs:271`, minted at `:242` | **The isolation boundary.** One per transport session; see §3.2 |
| `actor_key` | `actor_key_from_extensions` — `mcp/context.rs:122` | Defense in depth; real weight under OAuth |
| `route_scope` | `self.route_scope.label()` — `mcp/call_tool_codemode.rs:527` | Route isolation |
| `capability_fingerprint` | `ToolScope::fingerprint()` — `types.rs:907` | Stops capability laundering |

`surface_tag` was dropped — a session is inherently one transport, and
`code_mode_surface()` returned a constant anyway.

### 3.2 Why the session id, and why auth identity alone fails

The v1 draft keyed on the JWT `sub`; the second draft switched to `actor_key`
believing that fixed bearer-mode collapse. **It does not.** Verified chain:

```
middleware.rs:321   let sub = "static-bearer".to_string();          // constant
middleware.rs:322   derive_actor_key(deriver, &sub)
middleware.rs:491   fn derive_actor_key(deriver, subject) { deriver(subject) }
middleware.rs:52    type ActorKeyDeriver = dyn Fn(&str) -> Option<Arc<str>>;  // subject ONLY
activity.rs:87-97   HMAC-SHA256(installation_secret, subject.as_bytes())
```

A pure function of a constant is a constant, so **every bearer caller on an
installation shares one `actor_key`**. And on the default route
`route_scope.label()` is the constant `"root"` (`mcp/route_scope.rs:56`), while
capabilities come from the same `static_token_scopes`. In the default deployment
— `AuthMode::#[default] Bearer`, root route — **all four owner components are
constant**, and the cross-tenant read this whole design exists to prevent happens
exactly as before.

Hashing the *token* instead would not help either: there is exactly one
`static_token`, so it yields a single identity too. Genuine per-caller **auth**
identity under bearer needs per-agent credentials — a separate feature.

**The session id solves it without touching `labby-auth`.** Two agents sharing
one bearer token still occupy two transport sessions.
`next_relay_session_id()` (`mcp/server.rs:242`) mints one id per `LabMcpServer`,
and each HTTP factory invocation or the single stdio server builds exactly one,
so the id is stable for a session and unique across sessions (`server.rs:37-42`).
The codebase already leans on this exact property to keep a cached upstream relay
connection bound to one downstream agent and never reused across agents — the
same isolation boundary retention needs, already load-bearing elsewhere.

Issue #274 sanctions it explicitly: *"Per-caller **or per-session** ownership so
handles cannot cross authorization boundaries."*

This matters because the repo's own CLAUDE.md documents handing the bearer token
to automation tooling — "several distinct agents share one token" is the
documented shape, not an edge case. Under session keying those agents are
isolated; under auth-identity keying they were not.

**Trade-off, stated plainly:** a handle does not survive a reconnect. Given a
300 s TTL and paging that happens within a session, that is acceptable — and it
is arguably the *correct* reading of the issue's requirement that retained values
not outlive the authorization context that created them.

### 3.3 Capability comparison is containment, not equality

`source_capability_within_lookup` (`types.rs:690+`) checks that the stored
capability set is **within** the looking-up caller's set, rather than requiring
string equality. Retention adopts the same comparison: a caller may read what it
retained under a capability set no broader than the one it currently holds. A
narrower later execution therefore cannot read a broader earlier one — the
laundering case this component exists to prevent — while a harmless re-fetch
under an identical-or-broader grant still works.

### 3.4 Accepted coarseness

Trusted-local callers have no `actor_key`, so they collapse into one owner.
Acceptable: trusted-local already implies full local trust and can read the
workspace directly through the `state` local provider.

### 3.5 Containment direction — verify by asymmetric test, not by inspection

`may_read` checks `capability_within(stored, self)`: a read succeeds iff the
stored set is **within** the reader's current set. Walking both directions — a
narrower reader against a broader entry is denied; a broader reader against a
narrower entry is allowed — matches the intent and mirrors
`source_capability_within_lookup` (`types.rs:690-707`). This is easy to invert
silently, so the paired tests `narrower_capability_cannot_read_broader` and
`broader_capability_can_read_narrower` are both mandatory; a one-line mirror
claim is not evidence.

### 3.5 The ordering rule

Ownership resolves before state:

```
lookup(owner, handle):
  1. parse handle       → malformed?   ⇒ retained_handle_malformed
  2. find entry
       absent           ⇒ retained_result_not_found
       owner mismatch   ⇒ retained_result_not_found   ← indistinguishable
  3. check expiry
       expired          ⇒ retained_result_not_found (and reap)
       live             ⇒ Hit(Arc<RetainedEntry>)
```

**Divergence from `CodeModeSourceStore`, deliberately.** That store returns
*distinguishable* `Forbidden` messages for wrong-actor vs wrong-scope. Retention
collapses them, because the two features have different exposure: source
promotion is `lab:admin`-only, so a distinguishing error reveals nothing to
anyone who isn't already trusted, whereas retention is available to **any**
`can_execute` caller, where a distinct "exists but not yours" would be a
handle-existence oracle across tenants.

## 4. Entry state machine

```
                    store() admitted
      (nothing) ─────────────────────────▶ ┌──────┐
           ▲                               │ Live │◀── fetch/slice (no TTL change)
           │                               └──┬───┘
           │           TTL elapsed │ quota eviction │ reload flush
           │                       └────────┼────────┘
           └────────────────────────────────┘
                    reaped ⇒ retained_result_not_found
```

| Transition | Trigger | Counter |
|---|---|---|
| → Live | `store()` admitted | `stored_bytes`, `entry_count` ↑ |
| Live → Live | `fetch` / `slice` | `fetch_hits` ↑ |
| Live → gone | TTL elapsed | `expirations` ↑ |
| Live → gone | own-quota eviction, or reload flush | `evictions` ↑ |

## 5. Admission model

```
admit(owner, value, size_bytes) -> Option<StoredHandle>

  ① retention disabled                        ⇒ None
  ② size_bytes > entry_max_bytes              ⇒ None
  ③ purge_expired()
  ④ while caller_bytes(owner) + size_bytes > per_caller_max
        OR caller_entries(owner) + 1 > per_caller_max_entries:
        evict oldest entry OWNED BY `owner`   (none left ⇒ break)
  ⑤ if total_bytes + size_bytes > global_max
        OR entry_count + 1 > max_entries      ⇒ None      ← reject, never steal
  ⑥ insert; return StoredHandle
```

Every `None` falls back to today's hard truncation with no handle.

**Step ④ gained a per-caller entry count.** Research found that a byte-only quota
left a trivial cross-tenant denial of service: 32 tiny over-budget results — each
producible with no upstream call at all, e.g. `return "x".repeat(25000)` — occupy
every one of the 32 global entry slots using under 1 MB, blocking all other
callers for the full TTL. A per-caller entry cap closes it.

### 5.1 Why reject rather than evict a stranger

1. **Security.** Evicting strangers is a cheap cross-tenant DoS.
2. **Semantics.** "Hard truncation remains the fallback when the store is full"
   is unimplementable if admission always evicts to make room.

### 5.2 Determinism

"Oldest" is by a monotonic **admission sequence number**, not `SystemTime`.
Research flagged that a wall-clock key is non-monotonic — an NTP step reorders
"oldest" and shifts live TTLs — so the determinism claim would have held in tests
(where `now` is injected) and failed in production.

The ordered index is a `Vec` scan, not a `BTreeSet`. The v1 draft justified a
`BTreeSet` with "O(log n) oldest-first eviction", but the operation admission
actually needs is *oldest owned by this owner*, and a globally-ordered index has
no owner component — so it degrades to an O(n) scan inside an O(n) loop anyway.
At `n ≤ 32` a sorted `Vec` scan is equally deterministic, simpler, and allocation-free.

## 6. Size accounting model

| Quantity | Measured | Where |
|---|---|---|
| `size_bytes` | **Once**, hoisted from the shaping path | Reused for caps and `fetch` metadata |
| `token_estimate` | `ceil(size_bytes / divisor)`, floor 1 | Same estimator as `truncate.rs:174` |
| `selection_bytes` | Per slice, on the selection only | Compared against the slice ceiling (§7) |

### 6.1 Serialized bytes are not resident bytes

The caps are defined over **serialized** bytes; the store holds a **parsed**
`serde_json::Value`. Research measured the expansion on MCP-shaped data:

| Shape | Serialized | Parsed | Ratio |
|---|---|---|---|
| 40k small flat objects | 3.13 MB | 39.3 MB | **12.6x** |
| 8k medium nested objects | 1.73 MB | 13.5 MB | **7.9x** |
| 500k integers | 3.39 MB | 37.7 MB | **11.1x** |
| single 4 MiB string | 4.19 MB | 4.19 MB | 1.0x |

(`size_of::<Value>()` is 72 bytes here, because `labby-codemode` enables
`preserve_order` — `Cargo.toml:23`.)

So a 64 MiB serialized cap meant up to **~800 MiB resident**, and one 16 MiB
entry meant ~200 MiB — over three times the entire 64 MiB Javy runner heap. The
v1 NFR-3 guarantee was wrong by an order of magnitude. Defaults are re-derived
accordingly in SPEC FR-12, and the multiplier is now stated rather than assumed.

## 7. Ceiling model

| Ceiling | Value | Derived from | Over-limit |
|---|---|---|---|
| `callTool` fan-out | `max_calltool_per_run()` | config | error |
| Internal calls | `MAX_INTERNAL_CALLS_PER_RUN = 32` | const | **fail-open** |
| **Paging calls** | `RETAIN_CALLS_PER_RUN = 64` (const) | — | **fail-closed** |
| **Slice selection** | `RETAIN_SLICE_MAX_BYTES = 1 MiB` (const) | the **64 MiB sandbox heap** | error |

The slice ceiling is deliberately *not* `calltool_result_max_bytes` (8 MiB,
`config.rs:34`). Research found that 8 MiB of object-heavy JSON parses to well
over the QuickJS heap, so a *successful* host-side slice could OOM the runner —
and the agent's recovery advice would be exactly wrong, because the call
succeeded. Deriving it from the sandbox heap with the measured expansion factor
gives ~1 MiB.

Paging must fail **closed**. The internal-call ceiling fails open by design so
`codemode.search()` still works with an empty ranking; an empty *page* mid-stream
would silently corrupt an agent's reconstruction. See IMPLEMENTATION_PLAN §3 for
the `runner_drive.rs` change this requires — it is not automatic.

## 8. Lifecycle model

```
labby serve start
  └─ retention enabled? ──no──▶ no store; no RetentionContext; behavior = main
        │yes
        ▼
  Arc<RetainedResultStore> on GatewayManager (sibling of code_mode_source_store)
        │
        ├─ surface builds RetentionContext { store, owner } per execution
        ├─ execution 1 over budget ─▶ admit ─▶ handle in marker
        ├─ execution 2 same owner  ─▶ fetch/slice ─▶ paged value
        ├─ lazy TTL sweep on access + admission
        ├─ gateway reload ─▶ flush
        └─ process exit ─▶ gone
```

`RetentionConfig` must be resolved **lazily per use**, not captured at store
construction: `install_call_budget_config_defaults` runs inside
`reload_with_origin_unlocked` (`manager/pool_lifecycle.rs:265`), i.e. *after* the
manager is built. Existing knobs survive that because they resolve per call
(`runner_drive.rs:143`). A config captured at construction would silently ignore
every `config.toml` retention setting.

### 8.1 Authorization-context lifetime

Reload flush plus a short TTL bound exposure. Research found the codebase does
have revocation call sites — `labby-auth/src/token.rs:588` (`revoke`),
`sqlite.rs:331`, `sqlite/tokens.rs:450` — each of which knows the subject being
revoked. A targeted `evict_by_actor()` from those sites would close the window
rather than merely bounding it. Recorded as a follow-up rather than v1 scope, but
"no hook exists" was too strong a claim and has been corrected.

## 9. Surface applicability

| Surface | Store outlives execution | `RetentionContext` |
|---|---|---|
| `labby serve` (HTTP MCP) | Yes | **Some** |
| `labby mcp` (stdio) | Yes — one long-lived process, one `GatewayManager` | **Some** |
| CLI one-shot (`cli/gateway/code.rs:114`) | No | **None** |

Gating happens where the context is built, in the surface. There is no separate
"CLI host" — both paths drive the same `GatewayManager` — which is precisely why
the v1 trait accessor could not express this.

## 10. Concurrency model

- One `std::sync::Mutex` — confirmed the workspace idiom for short, I/O-free
  sections (`broker.rs:26`, `pool.rs:72`, `manager/core.rs:145`).
  `tokio::sync::Mutex` appears only where a guard is held across `.await`.
- **Never `.await` under the guard.** The compiler will not enforce this here, so
  it is a documented invariant on `StoreInner`.
- **Evicted entries are dropped outside the lock.** Research measured 367 ms to
  drop one 15.3 MB-serialized parsed array, and ~1.5 s for a full flush at cap —
  with the lock held, blocking an OS thread. Removal moves `Arc`s into a local
  `Vec`, releases the guard, and drops after. This is the real stall risk, not
  the lock type.
- Poisoning uses `unwrap_or_else(PoisonError::into_inner)` — the workspace idiom
  (`code_mode_host.rs:392`). The v1 draft's `.lock().ok()?` would have disabled
  retention permanently and silently, and its `fetch` mapped poisoning to
  not-found, telling agents to re-run a query when the store was actually dead.
- A fetch racing eviction returns a complete `Arc` or a structured miss, never a
  partial: the `Arc` is cloned under the guard and outlives the entry's removal.
