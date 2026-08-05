# PROGRESS — Code Mode Retained Results (#274)

**Branch:** `feat/codemode-retained-results-274` · **Worktree:** `.worktrees/feat-codemode-retained-results-274`
**Base:** `origin/main` @ `132448802` · **Epic:** `lab-zca58` · **Children:** `lab-zca58.1`–`.6`
**Last updated:** 2026-08-05 (post-research revision)

Beads are the source of truth for task state (`bd show lab-zca58`); this is the roll-up.

---

## Status

| Phase | Bead | Status | Blocked by |
|---|---|---|---|
| Design artifacts | — | ✅ Complete |  |
| Research (8 agents) | — | ✅ Complete |  |
| Design revision (research) | — | ✅ Complete |  |
| Engineering review (4 agents) | — | ✅ Complete |  |
| Revision #2 (eng-review) | — | ✅ Complete |  |
| Revision #3 (session-keyed ownership) | — | ✅ Complete | O-11 resolved |
| 1. Result store core | `lab-zca58.1` | ⬜ Not started | — |
| 2. Retention hook + marker | `lab-zca58.2` | ⬜ Not started | 1 |
| 3. Handlers + slice + errors | `lab-zca58.3` | ⬜ Not started | 1, 2 |
| 4. Helpers + typings + description | `lab-zca58.4` | ⬜ Not started | 3 |
| 5. Gateway wiring + observability | `lab-zca58.5` | ⬜ Not started | 2, 3 |
| 6. E2E + docs + UI | `lab-zca58.6` | ⬜ Not started | 4, 5 |

**0 / 6 implementation beads complete.** No production code written.

---

## Acceptance criteria

| # | Criterion | Bead | Test | Status |
|---|---|---|---|---|
| AC-1 | Preview + opaque handle | 2 | `retention_marker_carries_handle_and_expiry` | ⬜ |
| AC-2 | Pages without re-running upstream | 3,4,6 | `e2e_paging_makes_no_upstream_calls` (counting host) | ⬜ |
| AC-3 | Scoped to caller, expires | 1,2 | `foreign_actor_is_not_found`, `bearer_callers_are_distinct_owners`, `ttl_boundary` | ⬜ |
| AC-4 | Bounded under concurrent load | 1 | `concurrent_stores_respect_caps`, resident-ratio guard | ⬜ |
| AC-5 | Structured errors | 3 | `error_guidance_matches_contract` | ⬜ |
| AC-6 | Truncation remains fallback | 2 | `disabled_output_is_byte_identical`, `declined_marker_releases_the_entry` | ⬜ |
| AC-7 | Reduce-before-return primary | 2,4 | `guidance_puts_reduce_before_paging` | ⬜ |

---

## Research pass — 2026-08-05

Eight domain-matched agents. `learnings-researcher` was dropped (no `.lavra/memory`
in this repo); `kieran-typescript-reviewer` and `deployment-verification-agent`
were not dispatched (no meaningful domain match).

| Agent | Outcome |
|---|---|
| architecture-strategist | **Design pivot** — trait seam unworkable; `CodeModeSourceStore` precedent |
| security-sentinel | **HIGH** — bearer subject collapse; entry-count DoS; fingerprint unreachable |
| performance-oracle | **HIGH, measured** — 8–13x resident memory; 367 ms drop-under-lock; quadratic slice |
| code-simplicity-reviewer | Cut tombstones; 7 config knobs → 3; 7 error kinds → 5 |
| pattern-recognition-specialist | Fail-open shim above the handler; wrong trait precedent; 500-LOC rule |
| agent-native-reviewer | **HIGH** — worked example returned `[]`; tool description never updated |
| best-practices-researcher | Hand-rolled store validated; RFC 7233 precedent; 2 untested RFC 6901 cases |
| framework-docs-researcher | All library claims verified; `jiff` not `rfc3339()`; SystemTime caveat |

### Design changes applied

| # | Change | Driver |
|---|---|---|
| 1 | **Retention moved off the `CodeModeHost` trait** to a surface-built `RetentionContext` | The trait's only implementor is the shared `GatewayManager`, which has no per-request `route_scope`/`actor_key`/`surface` |
| 2 | **Owner keys on `actor_key`, not `sub`** | Bearer is the default auth mode and assigns every caller `sub = "static-bearer"` |
| 3 | Owner adds `route_scope`; capability compared by **containment** | Matches `CodeModeSourceStore::resolve` |
| 4 | **Retention runs after the marker decision, with rollback** | Otherwise a declined marker strands an entry holding quota for the full TTL |
| 5 | **`runner_drive.rs` fail-open shim amended** | It settles over-ceiling internal calls with `{"ranked":[]}` before the handler runs |
| 6 | **Tombstones cut**; 7 error kinds → 5 | All miss kinds share one recovery contract |
| 7 | **7 config knobs → 3 env + consts**; defaults re-derived | Crate precedent; and the caps were 8–13x off on resident memory |
| 8 | **Slice ceiling from the 64 MiB sandbox heap**, not the 8 MiB calltool cap | A successful slice could OOM the runner |
| 9 | **`char_indices()`, never `Vec<char>`** | 5.2 ms + 16 MiB per call → 0.002 ms + zero allocation |
| 10 | **Evicted `Arc`s dropped outside the lock** | 367 ms measured for one entry; ~1.5 s for a flush |
| 11 | **Per-caller entry-count quota added** | 32 tiny results, no upstream call needed, blocked every other caller |
| 12 | **Worked example rewritten**; `array_lengths` added to `fetch` | The old one returned `[]` for its own payload shape |
| 13 | **Tool description + MCP App UI added to scope** | The only discovery surface, and the UI contradicts retention |
| 14 | `shape.rs` string marker **left alone** | Non-default policy, no `next_action`, shrinks its own preview |
| 15 | Specified `recovery.guidance` per kind; `rediscover` not `revise_and_retry` | Generic arms give advice that cannot succeed |
| 16 | `uuid` + `jiff` added to the crate manifest | Neither is currently a `labby-codemode` dependency |
| 17 | Tests move to a sibling `tests_result_store.rs` | Crate's 500-LOC rule and its own escape hatch |
| 18 | Eviction sorts by admission counter, not `SystemTime` | Wall clock is non-monotonic |
| 19 | Poisoning uses `PoisonError::into_inner` | `.ok()?` disables retention silently and forever |
| 20 | Operator read path for counters (doctor) | Agents could read the store; operators could not |

---

## Engineering review — 2026-08-05 (post-research)

Four agents against the revised plan, framed adversarially about the revision.

| # | Finding | Sev | Applied |
|---|---|---|---|
| E-1 | **The `actor_key` fix is illusory.** `derive_actor_key` takes only the subject string (`middleware.rs:491`, deriver type `:52`), the deriver is `HMAC(secret, subject)` (`activity.rs:87`), and bearer's subject is the constant `"static-bearer"` (`middleware.rs:321`). With `route_scope.label()` = `"root"` by default, **all four owner components are constant** in the default deployment. | **Critical** | Superseded by D-15: ownership keys on `relay_session_id`, which isolates two agents sharing one bearer token. The interim OAuth gate was removed. |
| E-2 | `RetentionContext` as an `execute()` parameter **cannot reach the handlers** — the chain crosses the free fn `enqueue_tool_call` (`runner_drive.rs:761`). Plan contradicted itself (§2.1 parameter vs §3.3 `self.retention`). | **Critical** | FR-6.3: broker field via `with_retention()`; 1 site instead of ~23 |
| E-3 | `is_internal = !is_paging && …` charges paging to the **ordinary `callTool` budget** and, on overflow, leaks the full handle into `response.calls` (`:1084`; `handle` is not a redacted key, `trace.rs:369`). It also fires on the **disabled** path, breaking NFR-1. | **Critical** | FR-8.2/8.2a/8.2c: three-way classification, gated on enabled, metered at enqueue |
| E-4 | The `char_indices` "fix" is **slower than the bug** — 5.2–15.3 ms vs 6.1 ms on 4 MiB ASCII; the "0.002 ms" claim was off by ~3 orders of magnitude. | **Critical** | FR-4.8 rewritten: ASCII fast path + offset second scan + cached `char_len`; test asserts timing, not allocation |
| E-5 | `store()`-then-`release()` is **unsound**: admission evicts the caller's *own older entries* first, and release does not restore them — a declined marker silently destroys live handles. | **Critical** | FR-1.5: decide arithmetically first (fields are fixed width); rollback removed entirely |
| E-6 | `Arc::clone(result_arc)` refers to a type that doesn't exist (`types.rs:313` is `Option<Value>`); degrades to a ~150–240 ms deep clone. | High | §2.2: take-and-wrap, or scope `Option<Arc<Value>>` (also deletes 2 pre-existing clones) |
| E-7 | Retention inside `truncate_execution_response` captures the **post-shaping** value, so FR-1.3 is unimplementable there. | High | FR-1.7: pure `plan_result_marker` + commit in `execute.rs` where `raw_response` lives |
| E-8 | `truncate.rs:44` is a ~1.1 KB floor, **not** a budget check — a 10 KB logs-dominant result would be wrongly retained. FR-1.2 misread it. | High | FR-1.8: explicit budget predicate |
| E-9 | `select()` clones the target **before** the size check — `slice(h, "")` deep-clones 4 MiB then errors, making the error path the most expensive path. | High | §3.2: size before materializing, early-aborting counting writer |
| E-10 | Paging id lists would exist in two places, one qualified and one bare-suffix → silent drift back to fail-open. | High | FR-8.2b: shared `const PAGING_TOOL_IDS` |
| E-11 | `RETAIN_CALLS_PER_RUN × RETAIN_SLICE_MAX_BYTES` = 64 × 1 MiB = **exactly the 64 MiB sandbox heap**; an accumulating loop OOMs the runner ~page 25. | High | FR-8.5: co-derive the pair |
| E-12 | Paging budget "plain counter, no atomics" **will not compile** — `&self`, and `Cell` isn't `Sync`, breaking the rmcp `Send` bound. | High | FR-6.5: `AtomicUsize` |
| E-13 | Bead graph inverted: bead 2 calls `retained_results()`, delivered by bead 5. | High | Resequenced 0 → 3a/3b ∥ 1 → 5a → 2 → 3c → 4 → 5b → 6 |
| E-14 | A **third** `execute_with_raw_response` caller was never mentioned (`snippets/dispatch.rs:319`). | Med | FR-6.3a: explicit decision required |
| E-15 | `flush()` had prose only, and the "~1.5 s" figure was scaled from the retired 64 MiB cap. | Med | FR-7.6: ~384 ms, `spawn_blocking`, sketch required |
| E-16 | `release()` was used but absent from the public API, with no owner argument — a future reuse becomes a cross-tenant eviction primitive. | Med | TYPES §1.6: `release(owner, handle)`, documented |
| E-17 | Only **2 concurrent callers** can hold a full quota (8 MiB per-caller vs 16 MiB global). | Med | SPEC FR-12 states the ceiling; 2 MiB per-caller recommended for 8 |
| E-18 | `array_lengths` was justified by a worked example written to require it; a zero-width probe already returns `source_length`. | Med | FR-3.3 cut; example uses the probe |
| E-19 | `retained_calls_exhausted` duplicates `call_budget_exceeded`'s exact triple. | Low | Folded; 5 kinds → 4 |
| E-20 | Schema-origin drift is pre-existing on `main`; bundling it here sweeps unrelated drift into the commit. | Low | Split to bead 0, lands first |
| E-21 | `fetch()`'s `match` on a temporary conflicts with `inner.reap()`; expired arm never increments `fetch_misses`. | Low | Bind first; count the miss |
| E-22 | Deferred: MCP App UI fix, doctor read path. Kept: tool description (only discovery surface). | Low | Noted below |

**Validated, do not revisit:** moving off the `CodeModeHost` trait; ownership-before-state
ordering; leaving `shape.rs` alone; dropping evicted `Arc`s outside the guard;
`PoisonError::into_inner`; catching the fail-open shim before writing code;
counting-host assertion for AC-2; hand-rolled store over `moka`/`dashmap`.

## Decision log

| # | Decision | Status | Where |
|---|---|---|---|
| D-1 | Owner = (surface, **actor_key**, route_scope, capability containment) | **Revised** — was `sub` | MODELS §3 |
| D-2 | Reject when full; never evict a stranger | Holds | MODELS §5.1 |
| D-3 | Unauthorized ≡ not-found | Holds; divergence from `CodeModeSourceStore` now justified (it is admin-only) | CONTRACT §3.2 |
| D-4 | Paging fails closed on its own ceiling | Holds — but requires the `runner_drive.rs` amendment | SPEC FR-8.2 |
| D-5 | JSON Pointer; clamped ranges; char-indexed strings | Holds; RFC 7233 cited as precedent | CONTRACT §5 |
| D-6 | `fetch` metadata-first | Holds; `array_lengths` added | CONTRACT §4.2 |
| D-7 | Handle minted once, embedded in the marker | **Revised** — JSON marker only, after the shrink decision | SPEC FR-2.4, FR-1.5 |
| D-8 | Ships disabled | Holds | SPEC FR-12 |
| D-9 | Fixed TTL | Holds | SPEC FR-7.1 |
| D-10 | `cmr_` + UUIDv4 | Holds; `ulid` rejected (embeds a timestamp) | TYPES §1.0 |
| D-11 | In-memory; flush on reload | Holds; revocation hooks exist and are a named follow-up | MODELS §8.1 |
| D-12 | Bounded tombstones | **Cut** | SPEC §7.1 |
| D-13 | Retention is a surface-built context, not a trait extension | Holds — **delivered as a broker field**, not an `execute()` parameter | MODELS §1–2, SPEC FR-6.3 |
| D-14 | Caps are serialized bytes; resident is 8–13x | Holds | MODELS §6.1 |
| D-15 | **Ownership keys on `relay_session_id`**, not auth identity. Two agents sharing one bearer token occupy separate transport sessions, so they are isolated with no `labby-auth` change. `actor_key`/`route_scope`/capability stay as defense in depth; `surface_tag` dropped. Handles do not survive a reconnect. | **Revised — supersedes the OAuth gate** | SPEC FR-5.1–5.2c, MODELS §3 |
| D-16 | No store-then-rollback; the shrink decision is arithmetic on fixed-width fields | **New** | SPEC FR-1.5 |
| D-17 | Paging is a third classification at the enqueue site, gated on retention being enabled | **New** | SPEC FR-8.2–8.2c |
| D-18 | 4 error kinds, not 5 — paging overrun reuses `call_budget_exceeded` | **Revised** | CONTRACT §3 |
| D-19 | `array_lengths` cut; the loop is sized by a zero-width probe slice | **Revised** | SPEC FR-3.3 |

---

## Verification gates

| Gate | Command | Status |
|---|---|---|
| Workspace tests | `just test` | ⬜ |
| Lint + fmt + drift | `just lint` | ⬜ |
| Generated docs | `just docs-check` | ⬜ |
| Dependency policy | `just deny` (manifest gains `uuid`, `jiff`) | ⬜ |
| CI `mcp-regressions` | Actions | ⬜ |
| CI `codemode-runner-smoke` | Actions | ⬜ |
| CI `feature-slices` (gateway **and fs**) | Actions | ⬜ |

---

## Open items

| # | Item | Status |
|---|---|---|
| O-1 | Error schema lists 6 origins, Rust has 9 — **live bug on `main`**, not introduced here | ⚠️ Fix in Bead 3 |
| O-2 | Token revocation: hooks exist (`token.rs:588`, `sqlite.rs:331`, `sqlite/tokens.rs:450`); targeted `evict_by_actor` would close the TTL window | 🔮 Follow-up |
| O-3 | Trusted-local callers share one owner | 📋 Accepted |
| O-4 | Per-execution retention opt-out | 🔮 Future |
| O-5 | Per-`callTool` retention | 🔮 Non-goal |
| O-6 | stdio MCP host lifetime | ✅ **Resolved** — `labby mcp` is one long-lived process with one `GatewayManager` |
| O-7 | `CodeModeExecutionResponse.result` as `Option<Arc<Value>>` would delete two pre-existing deep clones | 🔮 Follow-up bead |
| O-8 | `utf8_prefix_by_bytes` is duplicated in `shape.rs:137` and `truncate.rs:195` | 🔮 Cleanup |
| O-9 | Only ~2 callers can hold a full per-caller quota at the 16 MiB global default | 📋 Accepted; `stores_rejected` is the signal |
| O-10 | MCP resources route (`lab://codemode/retained/<handle>`) | 📋 Considered, not adopted (SPEC §7.5) |
| O-11 | Bearer-mode isolation | ✅ **Resolved** — keyed on `relay_session_id`. Changing `ActorKeyDeriver` would not have worked anyway: there is one `static_token`, so hashing it yields one identity too. Verified the id is stable per session and unique across sessions (`server.rs:37-42`), and is already load-bearing for relay-connection isolation. No `labby-auth` change; the `SHARED_TOKEN_OK` knob is gone. |
| O-12 | Whether snippet execution (`snippets/dispatch.rs:319`) retains | ⬜ Open |
| O-13 | `Option<Arc<Value>>` for `CodeModeExecutionResponse.result` — now partly in scope for bead 2 (8 sites; deletes 2 pre-existing deep clones) | 🔮 Scope decision |
| O-14 | `gateway.reload` flushes the whole store; recovery guidance says "re-run the query" for what was an operator action. Question whether the flush is needed at all | ⬜ Open |
| O-15 | `OwnerKey.surface` never differentiates two live owners today (`code_mode_surface()` returns a constant) | 📋 Accepted |
| O-16 | Deferred to the enablement PR: MCP App UI text fix, doctor counters read path | 📋 Deferred |

---

## Session log

| Date | Change |
|---|---|
| 2026-08-05 | Reviewed #274; researched `labby-codemode`; SpecFlow analysis; created epic `lab-zca58` + 6 children; created worktree off `origin/main`; authored spec, contract, 4 schemas, models, types, plan, tracker. |
| 2026-08-05 | Ran `/lavra-research` with 8 domain-matched agents. 20 design changes applied across SPEC/CONTRACT/MODELS/TYPES/PLAN, including an architecture pivot off the `CodeModeHost` trait and two security fixes. Findings logged to epic and beads `.1`/`.2`/`.3`. Still no production code. |
| 2026-08-05 | Ran `/lavra-eng-review` (architecture, security, performance, simplicity) against the revised plan. 22 findings, 5 critical — including that the previous round's `actor_key` security fix was **illusory**, that the `RetentionContext` seam as specified could not reach the handlers, and that the `char_indices` performance fix was **slower than the bug**. All applied. Still no production code. |
| 2026-08-05 | Resolved O-11 by keying ownership on `relay_session_id` instead of auth identity — verified stable per session and unique across sessions (`mcp/server.rs:37-42`, minted at `:242`), already in scope on `LabMcpServer`, and already load-bearing for relay-connection isolation. Changing `ActorKeyDeriver` was rejected: with one `static_token`, hashing the token yields one identity too. The interim OAuth gate and `SHARED_TOKEN_OK` knob are removed. Trade-off accepted and documented: handles do not survive a reconnect. |
