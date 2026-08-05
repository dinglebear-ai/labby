# SPEC — Retain and Page Oversized Code Mode Results by Handle

**Issue:** [#274](https://github.com/dinglebear-ai/labby/issues/274) (Phase 2 of [#217](https://github.com/dinglebear-ai/labby/issues/217), closed)
**Status:** Proposed — revised 2026-08-05 after 8-agent research pass
**Branch:** `feat/codemode-retained-results-274`
**Owner crate:** `labby-codemode` (store, slice engine), `labby-gateway` + `labby` (lifecycle, surface wiring)

Requirement levels follow RFC 2119.

---

## 1. Problem

A Code Mode final result is capped by the response envelope budget —
`max_response_bytes` 24 KB and `max_response_tokens` 6000
([gateway_config.rs:149-159](../../../crates/labby-runtime/src/gateway_config.rs)). Over-budget results are replaced by a
marker carrying a 1 KB preview and the instruction to re-run with a narrower
query ([truncate.rs:191](../../../crates/labby-codemode/src/truncate.rs)).

That is right for cheap idempotent reads. It is wrong when the call consumed a
rate limit, ran an expensive aggregation, returned a large immutable snapshot,
cannot be repeated safely, or would return different data next time. The data
already exists on the host and is being discarded.

## 2. Solution

When a final result exceeds the budget **and** retention is enabled, the host
retains the complete pre-shaping value in a bounded in-memory store, mints an
opaque handle, and returns the existing marker **plus** that handle. A later
execution by the same owner pages it with `codemode.fetch(handle)` and
`codemode.slice(handle, path, range)` — no upstream re-call.

Retention is a **selective fallback**. Reduce-before-return (#217) stays primary.

## 3. Scope

**In:** retaining the final result; two sandbox helpers and their host handlers; a
bounded TTL store with per-caller quotas; new error kinds plus contract/schema
updates; observability; config, defaulting to disabled.

**Out (non-goals):** durable storage; cross-owner memory; retaining every result;
per-`callTool` retention; a "cheaply reproducible" heuristic (§7.3).

## 4. Functional requirements

### FR-1 — Retention trigger

- **FR-1.1** Retention **MUST** be attempted iff: retention is enabled; a
  `RetentionContext` is present for this execution; and the **final result alone**
  is over the envelope budget.
- **FR-1.2** **MUST NOT** trigger for a logs-dominant response, where the result
  is within budget and the envelope is over because of `logs`/`calls`
  (`truncate.rs:36-48` already leaves such a result intact).
- **FR-1.3** The retained value **MUST** be the pre-shaping result.
- **FR-1.4** **MUST** be attempted at most once per execution.
- **FR-1.5** Retention **MUST** run **after** the decision that a marker will be
  emitted, and **MUST NOT** use a store-then-roll-back scheme. The retention
  fields are fixed width — `result_handle` is always `cmr_` + 32 hex,
  `retained_until` is a fixed-width RFC 3339 stamp, and `retained_bytes` **is**
  `original_len`, already in hand at `truncate.rs:41` — so the shrink decision is
  **computable before minting anything**. Build the marker with a placeholder
  handle, evaluate `marker_len < original_len` on that, and mint only if it wins.

  A rollback is not merely unnecessary but unsound: `store()` evicts the
  **caller's own older entries** to make room *before* inserting, and a
  `release()` of the new handle does not restore them. A declined marker would
  therefore silently destroy live handles from earlier executions — data loss
  that the obvious test (`declined_marker_releases_the_entry`) would not catch.
  It also opens a window in which a provisional entry counts against other
  owners' admissions, and a panic in that window strands quota for a full TTL.
- **FR-1.7** Retention **MUST** read the pre-shaping value. By the time
  `truncate_execution_response` runs, `execute.rs:107` has already assigned
  `shaped.result`; under `result_shape_policy = Truncate` that is a preview
  string, so retaining there would hand the agent a handle to a preview it
  already has. The pre-shaping value exists only as `raw_response` at
  `execute.rs:89`. Satisfy this by splitting a pure
  `plan_result_marker(...) -> Option<MarkerPlan>` from the commit, and performing
  the store in `execute.rs` where `raw_response` is in scope.
- **FR-1.8** The over-budget predicate **MUST** be an explicit budget test.
  `truncate.rs:44`'s guard is only `marker_len < original_len` — roughly a 1.1 KB
  floor, **not** a budget check — so a 10 KB result in a logs-dominant response is
  markered today and would be wrongly retained despite being far under the 24 KB
  budget. Earlier drafts claimed this guard already implemented FR-1.2; it does not.
- **FR-1.6** The over-budget predicate **MUST** be the one that actually governs
  the marker. `effective_result_budget` (`shape.rs:57`) and `response_within_budget`
  (`truncate.rs:152`) are different tests; the implementation **MUST NOT** add a
  third.

### FR-2 — Handle issuance

- **FR-2.1** A successful store **MUST** return a handle matching `^cmr_[0-9a-f]{32}$`.
- **FR-2.2** Handles **MUST NOT** encode ownership, paths, secrets, or structure.
- **FR-2.3** Handle and expiry **MUST** live in the marker value itself, not only
  in `result_shaping` — `execute.rs:136-138` discards that metadata on exactly
  the path that needs it.
- **FR-2.4** The **JSON** marker (`truncate.rs:178`) **MUST** carry the handle.
  The string marker (`shape.rs:99`) **MUST NOT** be modified: it fires only when
  `result_shape_policy` is explicitly `Truncate`, which is not the default
  (`gateway_config.rs:84`, `:205`), has no `next_action` field to carry guidance,
  and its preview would be silently shortened by the added header. A caller who
  opts into that policy gets handle-less truncation; this **MUST** be documented.
- **FR-2.5** Adding handle fields **MUST NOT** break `marker_len < original_len`
  (`truncate.rs:44`). If it would, the marker is emitted without retention and
  the entry is not admitted (FR-1.5).
- **FR-2.6** `next_action` **MUST** name the field the handle lives in
  (`result_handle`), not merely the parameter name. The existing `next_action` is
  self-contained; the retention variant **MUST** stay self-contained too.

### FR-3 — `codemode.fetch(handle)`

- **FR-3.1** **MUST** always return metadata: `size_bytes`, `token_estimate`,
  `value_type`, `created_at`, `expires_at`, `value_omitted`, plus type-specific
  `array_length` / `string_length` / `object_keys` + `object_key_count`.
- **FR-3.2** **MUST** inline `value` when within the slice ceiling (FR-7.4);
  otherwise **MUST** set `value_omitted: true` and supply `guidance`.
- **FR-3.3** *(cut)* `array_lengths` is **not** part of v1. A zero-width probe —
  `codemode.slice(handle, "/items", {start:0,end:0})` — already returns
  `source_length` (FR-4.6), so the primitive exists and costs one host-local
  call against the paging budget, never an upstream re-call. Building the map
  would also walk a root object with an unbounded key count on every fetch. The
  worked example uses the probe pattern instead.
- **FR-3.4** **MUST NOT** extend the TTL.

### FR-4 — `codemode.slice(handle, path, range)`

- **FR-4.1** `path` **MUST** be an RFC 6901 JSON Pointer; `""` selects the root.
- **FR-4.2** `range` is legal only on arrays and strings; otherwise `retained_slice_invalid`.
- **FR-4.3** Range semantics **MUST** clamp, `Array.prototype.slice`-style;
  `start >= end` yields empty, not an error. (RFC 7233 §2.1 uses the same
  clamping for HTTP Range, and §4.4 errors only on zero overlap — the closest
  external precedent.)
- **FR-4.4** String ranges **MUST** index chars, never bytes.
- **FR-4.5** A selection over the slice ceiling **MUST** raise
  `retained_value_too_large` with the actual size. **MUST NOT** truncate silently.
- **FR-4.6** **MUST** include `source_length` when a range was applied.
- **FR-4.7** **MUST NOT** extend the TTL.
- **FR-4.8** String slicing **MUST NOT** materialize `Vec<char>`, and **MUST NOT**
  use the naive three-pass `chars().count()` + two `char_indices().nth()` form
  either — measured, that form is **slower than the bug it replaces** (5.2–15.3 ms
  vs 6.1 ms on a 4 MiB ASCII string; an earlier draft's "0.002 ms" claim was
  wrong by three orders of magnitude). The required shape is:
  (a) an `is_ascii()` fast path where char index equals byte index (0.10 ms at
  4 MiB); (b) for non-ASCII, one prefix scan to `start`, then a **second scan
  offset from the first** walking only `end - start` chars, which
  `RETAIN_SLICE_MAX_BYTES` already bounds; (c) `char_len` cached on the entry
  (`OnceLock<usize>`), since entries are immutable and a paging loop slices the
  same string up to 64 times. The corresponding test **MUST** assert timing or
  pass count, not merely the absence of a `Vec<char>` — an allocation-only
  assertion passes on the slower implementation.

### FR-5 — Ownership

- **FR-5.1** Entries **MUST** be keyed by `(relay_session_id, actor_key,
  route_scope, capability_fingerprint)`.
- **FR-5.2 — `relay_session_id` is the component that carries isolation.**
  It is minted once per `LabMcpServer` instance by `next_relay_session_id()`
  (`mcp/server.rs:242`), and each transport session — one HTTP factory
  invocation, or the single stdio server — builds exactly one `LabMcpServer`,
  so the id is **stable for a session's lifetime and unique across sessions**
  (`server.rs:37-42`). The codebase already relies on precisely this property to
  bind a cached upstream relay connection to one downstream agent so it is never
  reused across agents; retention needs the identical boundary.
- **FR-5.2a — Auth identity alone is insufficient, which is why the session id
  is required.** Bearer auth sets `sub = "static-bearer"`
  (`labby-auth/src/middleware.rs:321`); `derive_actor_key` passes **only that
  string** to the deriver (`middleware.rs:491`, type at `:52`); the concrete
  deriver is `HMAC-SHA256(secret, subject)` (`observability/activity.rs:87-97`).
  A pure function of a constant is a constant, so **every bearer caller shares
  one `actor_key`** — and on the default route `route_scope.label()` is likewise
  the constant `"root"` (`mcp/route_scope.rs:56`). Hashing the token instead
  would not help: there is exactly one `static_token`, so it too yields a single
  identity. Genuine per-caller auth identity under bearer would require
  per-agent credentials, which is a separate feature.

  Two agents sharing one bearer token nevertheless occupy **separate sessions**,
  so `relay_session_id` isolates them without any change to `labby-auth`. Issue
  #274 sanctions this directly: *"Per-caller **or per-session** ownership."*
- **FR-5.2b** `actor_key`, `route_scope`, and the capability fingerprint remain
  in the tuple as defense in depth — they carry real weight under OAuth and cost
  nothing under bearer. `surface_tag` is dropped: a session is inherently one
  transport, and `code_mode_surface()` returned a constant anyway.
- **FR-5.2c** A handle is therefore **not usable after a reconnect**, and this
  **MUST** be documented. That is a correctness property, not a limitation: it is
  the strongest available reading of the issue's requirement that retained values
  not outlive the authorization context that created them.
- **FR-5.3** Capability comparison **MUST** be containment, not equality,
  matching `source_capability_within_lookup`.
- **FR-5.4** A foreign-owned handle **MUST** be indistinguishable from unknown.
- **FR-5.5** Ownership **MUST** be enforced in the handler. Internal dispatch
  bypasses `scope.allows()` (`execute.rs:366-372`) — the mechanism behind the
  historical `describe_types` leak.
- **FR-5.6** The `OwnerKey` **MUST** be derived once per execution, at the
  surface, and carried on the `RetentionContext` — not rebuilt per paging call.

### FR-6 — Architecture

- **FR-6.1** Retention **MUST NOT** extend the `CodeModeHost` trait. The trait's
  sole production implementor is the shared long-lived `GatewayManager`, which
  has no per-request `route_scope`, `actor_key`, or `surface`.
- **FR-6.2** The store **MUST** be an `Arc` field on `GatewayManager`, sibling to
  `code_mode_source_store` / `code_mode_history` / `code_mode_runner_pool`,
  reached through gateway inherent methods called from the surface.
- **FR-6.3** The `RetentionContext` **MUST** be a field on `CodeModeBroker`
  (`broker.rs:22-30`), set via a `with_retention` constructor — **not** a new
  `execute()` parameter. A parameter cannot reach the handlers: the chain from
  `execute()` to `dispatch_internal_call` passes through `enqueue_tool_call`
  (`runner_drive.rs:761`), a **free function** with an explicit `'a` lifetime tie,
  so threading it would mean editing seven signatures. The broker field is also
  how `ui_capture` already carries run-scoped state, and it changes **one**
  construction site instead of three production call sites plus ~20 test sites.
- **FR-6.3a** All **three** `execute_with_raw_response` call sites must be
  considered: `mcp/call_tool_codemode.rs:587`, `cli/gateway/code.rs:111`, and
  `dispatch/snippets/dispatch.rs:319`. The snippets surface was missing from
  earlier drafts; whether snippet execution retains **MUST** be an explicit
  decision, not an accident of which sites got edited.
- **FR-6.3b** `relay_session_id` is a field on `LabMcpServer`
  (`mcp/server.rs:271`) and `call_tool_codemode.rs` is `impl LabMcpServer`, so
  the owner is built from `self.relay_session_id` with no new plumbing. The CLI
  path does not construct a `LabMcpServer` at all, which is consistent with
  FR-9.2 gating it to `None`.
- **FR-6.5** The paging counter **MUST** be an `AtomicUsize` (or `Mutex`), not a
  plain field. `dispatch_internal_call` takes `&self`, so a bare `usize` cannot
  be mutated and a `Cell` is not `Sync` — which would break the `Send` bound the
  rmcp `ServerHandler` boundary requires. The `DriveState::internal_calls_enqueued`
  precedent is `&mut self` and does not transfer. Sandbox JS can also issue
  concurrent paging calls via `Promise.all`, which an unsynchronized counter would
  race.
- **FR-6.4** `RetentionConfig` **MUST** be resolved lazily per use.
  `install_*_config_defaults` runs in `reload_with_origin_unlocked`
  (`manager/pool_lifecycle.rs:265`), after manager construction, so a config
  captured at construction would ignore every `config.toml` setting.

### FR-7 — Lifecycle and bounds

- **FR-7.1** TTL **MUST** be fixed at creation, never sliding.
- **FR-7.2** The store **MUST** enforce per-entry size, per-caller bytes,
  **per-caller entry count**, global bytes, and global entry count. The
  per-caller entry count closes a DoS in which 32 tiny over-budget results —
  producible with no upstream call at all — occupy every global slot for a full TTL.
- **FR-7.3** Admission **MUST** follow MODELS §5's order and **MUST NOT** evict
  another owner's live entry.
- **FR-7.4** The slice/inline ceiling **MUST** derive from the 64 MiB sandbox
  heap, **not** `calltool_result_max_bytes` (8 MiB). 8 MiB of object-heavy JSON
  parses well past the QuickJS heap, so a *successful* slice could OOM the runner.
- **FR-7.5** Eviction ordering **MUST** use a monotonic admission counter, not
  `SystemTime`, which is non-monotonic under NTP steps.
- **FR-7.6** Evicted entries **MUST** be dropped **outside** the store lock —
  measured 367 ms for one 15.3 MB entry. At the re-derived 16 MiB global cap a
  full flush is **~384 ms** (an earlier "~1.5 s" figure was scaled from the
  retired 64 MiB cap). Dropping outside the lock stops other *store* users from
  blocking but still occupies a tokio worker, so `flush()` — the largest drop,
  on the operator-visible reload path — **MUST** additionally hand the drained
  entries to `spawn_blocking`. `flush()` **MUST** have an explicit code sketch;
  prose alone is what the drop-ordering rule exists to prevent.
- **FR-7.7** Lock poisoning **MUST** use `PoisonError::into_inner`.
- **FR-7.8** The store **MUST** flush on gateway reload, **MUST NOT** persist.

### FR-8 — Budget

- **FR-8.1** Paging **MUST** consume a dedicated per-run ceiling, separate from
  `MAX_INTERNAL_CALLS_PER_RUN`.
- **FR-8.2** Over-ceiling paging **MUST** fail closed, and the classification at
  `runner_drive.rs:456-497` **MUST** become **three-way** — `paging` / `internal` /
  `ordinary` — where paging increments **neither** existing counter.
  Simply setting `is_internal = !is_paging && …` is wrong twice over: paging
  would fall into the `else` arm and be charged to the ordinary `callTool`
  budget (violating FR-8.3), and `enqueue_internal_call_over_ceiling`
  (`runner_drive.rs:974`) becomes unreachable for paging, so the fail-open
  `{"ranked":[]}` it would otherwise return is dead code rather than a fixed
  bug. The exemption **MUST** live at the call site (`runner_drive.rs:481`), not
  inside that settler, whose only job is to push a settled future.
- **FR-8.2a** Paging **MUST** be metered at the **enqueue** site, not only in the
  handler. Moving the meter downstream lets a `while(true)` paging loop enqueue
  unbounded boxed futures into `pending_tool_calls` before any of them errors —
  the reason the existing ceiling gates at enqueue. Over-ceiling paging calls
  **MUST** settle via the existing `enqueue_tool_call_error` (`runner_drive.rs:949`).
- **FR-8.2b** The paging id list **MUST** be a single shared
  `const PAGING_TOOL_IDS`, referenced by both `runner_drive.rs` and the
  `execute.rs` dispatch match. Two independently-typed literal lists — one
  fully-qualified, one bare-suffix — would silently drift back into the
  fail-open path.
- **FR-8.2c** The paging classification **MUST NOT** apply when retention is
  disabled. A bare string match with no enabled-gate changes call-budget
  accounting on the disabled path, violating NFR-1.
- **FR-8.5** `RETAIN_CALLS_PER_RUN × RETAIN_SLICE_MAX_BYTES` **MUST** be derived
  **together** against the 64 MiB sandbox heap. At 64 × 1 MiB the product is
  exactly the heap, so an accumulating paging loop OOMs the runner around page
  25 after every individual call succeeded. Either lower the pair (e.g.
  64 × 256 KiB = 16 MiB) or document the accumulation hazard in the helper
  guidance and tool description.
- **FR-8.3** Paging calls **MUST** stay exempt from the `callTool` budget and trace.
- **FR-8.4** The paging budget **MUST** live on the per-request broker
  (`broker.rs:1-4`), never on the shared store, or the ceiling becomes process-global.

### FR-9 — Surface applicability

- **FR-9.1** Retention **MUST** be available only where the store outlives the
  execution: `labby serve` and `labby mcp` (both one long-lived `GatewayManager`).
- **FR-9.2** One-shot CLI executions (`cli/gateway/code.rs:114`) **MUST** receive
  `None`. There is no separate CLI host — both paths drive the same manager.
- **FR-9.3** With no context, helpers **MUST NOT** be generated.
- **FR-9.4** Reserved-name registration **MUST** be conditional. Unconditionally
  adding `fetch`/`slice` to `CODEMODE_TOP_LEVEL_RESERVED` (`preamble.rs:98`) would
  rename a real upstream namespace called `fetch` even with retention off,
  violating byte-compatibility.

### FR-10 — Errors

**Four** kinds (down from seven; see §7.1). All **MUST** be `CodeModeCallError`
envelopes. `recovery.guidance` **MUST** be specified per kind, not inherited from
`recovery_for_kind`'s generic arms — reusing them yields "correct the parameters
and retry" for a miss, which can never succeed. New kinds **MUST** land in
`docs/dev/ERRORS.md` and `docs/contracts/code-mode-tool-errors.md` in the same
change. There **MUST NOT** be an out-of-contract `retention_disabled` kind: the
handlers are registered only when a context exists, so the existing `_ =>` arm
(`execute.rs:516-519`) returns the defined `unknown_tool`.

### FR-11 — Observability

- **FR-11.1** Structured events under `service = "code_mode"`: retain, fetch,
  slice, evict, expire.
- **FR-11.2** Counters: stored bytes, entry count, evictions, expirations,
  fetches, misses, rejections.
- **FR-11.3** Full handles **MUST NOT** be logged — prefix only.
- **FR-11.4** Retained payload bytes **MUST NOT** be logged.
- **FR-11.5** Counters **MUST** have at least one operator read path (doctor or
  gateway action). Agents can read the store; operators currently could not, and
  the failure they will be asked about — "the store is full" — is otherwise
  visible only by log-grepping.

### FR-12 — Configuration

Env → `config.toml` → default, resolved lazily.

| Knob | Default | Notes |
|---|---|---|
| `LABBY_CODE_MODE_RETAIN_RESULTS` | `false` | Master switch; ships dark |
| `LABBY_CODE_MODE_RETAIN_TTL_SECS` | `300` | Fixed from creation |
| `LABBY_CODE_MODE_RETAIN_MAX_TOTAL_MIB` | `16` | Serialized; ~130–200 MiB resident |

**Concurrency ceiling — state it, operators will ask.** With
`RETAIN_PER_CALLER_MAX_BYTES` at 8 MiB against a 16 MiB global cap, exactly
**2 concurrent callers** can hold a full quota (4 by entry count); everyone else
silently receives today's handle-less truncation. Lowering the per-caller cap to
2 MiB gives 8 concurrent callers at identical worst-case resident memory, and is
the recommended default. `stores_rejected` is the signal, and the doctor check
(FR-11.5) **MUST** surface it.

Everything else is a `const`, matching the crate's restraint — `config.rs` exposes
only two env knobs and keeps every per-run ceiling
(`MAX_INTERNAL_CALLS_PER_RUN`, `MAX_SNIPPET_RESOLVES_PER_RUN`) as constants:

| Constant | Value |
|---|---|
| `RETAIN_ENTRY_MAX_BYTES` | 4 MiB serialized (~32–50 MiB resident) |
| `RETAIN_PER_CALLER_MAX_BYTES` | 8 MiB |
| `RETAIN_PER_CALLER_MAX_ENTRIES` | 8 |
| `RETAIN_MAX_ENTRIES` | 32 |
| `RETAIN_CALLS_PER_RUN` | 64 |
| `RETAIN_SLICE_MAX_BYTES` | 1 MiB (derived from the 64 MiB sandbox heap) |

Invalid values **MUST** warn and fall back, matching `config.rs:70-83`.

## 5. Non-functional requirements

- **NFR-1 — Byte compatibility.** Disabled ⇒ byte-identical to `main`.
- **NFR-2 — Bounded under concurrency.** Caps hold exactly; a fetch racing
  eviction returns a complete value or a structured miss.
- **NFR-3 — Memory.** Resident footprint is `RETAIN_MAX_TOTAL_MIB` **times the
  parsed-value expansion factor**, measured at **7.9x–12.6x** for MCP-shaped data
  (1.0x for single large strings). At the 16 MiB default that is ~130–200 MiB.
  The v1 claim that resident memory was bounded by the serialized cap was wrong
  by an order of magnitude.
- **NFR-4 — Latency.** No store operation may hold the lock across a large drop
  (FR-7.6) or an `.await`.
- **NFR-5 — Common-path cost.** Retention **MUST NOT** add a full serialization
  to under-budget executions; `size_bytes` is hoisted from the existing shaping
  measurement (`shape.rs:34-38`) or taken with a counting writer.
- **NFR-6 — Style.** `unsafe_code = "forbid"`, no `#[async_trait]`, no `mod.rs`,
  implementation files under 500 LOC with tests in a sibling `tests_*.rs`
  (the crate's own convention: `tests_normalize.rs`, `tests_ids_schema.rs`).

## 6. Acceptance criteria

| # | Issue criterion | Requirements | Verified by |
|---|---|---|---|
| AC-1 | Preview + opaque handle | FR-1, FR-2 | `retention_marker_carries_handle_and_expiry` |
| AC-2 | Later execution pages without re-running upstream | FR-3, FR-4 | `e2e_paging_makes_no_upstream_calls` — **must use a counting host** (`CountingHost`, `runner_drive.rs:1401`); asserting an empty `response.calls` is a tautology, since internal calls are excluded from the trace by construction (`runner_drive.rs:443-452`) |
| AC-3 | Scoped to caller/session, expires | FR-5, FR-7.1 | `foreign_owner_is_indistinguishable_from_unknown`, `bearer_callers_are_distinct_owners`, `entry_expires_at_ttl` |
| AC-4 | Bounded under concurrent load | FR-7.2–7.6, NFR-2/3 | `concurrent_stores_respect_caps`, `resident_footprint_ratio_is_bounded` |
| AC-5 | Structured errors | FR-10 | `retention_error_matrix` |
| AC-6 | Truncation remains fallback | FR-1.5, NFR-1 | `disabled_output_is_byte_identical`, `store_full_falls_back_to_truncation`, `declined_marker_admits_nothing` |
| AC-7 | Reduce-before-return primary | FR-2.6 | `marker_text_puts_reduce_before_paging` |

## 7. Decisions and rationale

### 7.1 Tombstones cut; seven error kinds became five

`retained_result_not_found`, `_expired`, and `_evicted` shared an identical
`origin`/`recovery`/`same_arguments`/`side_effects` contract — the distinction was
diagnostic only, and the same argument already justified reporting a flush as an
eviction. Cutting the tombstone ring removes a struct family, two kinds, and
tests across three beads. A paging-ceiling overrun then reused the existing
`call_budget_exceeded` (identical `budget`/`reduce_work`/`never` triple) rather
than minting a fifth kind. Remaining: `retained_result_not_found`,
`retained_handle_malformed`, `retained_slice_invalid`, `retained_value_too_large`.

### 7.2 Unauthorized collapses into not-found

Distinguishing them is a handle-existence oracle. This **diverges** from
`CodeModeSourceStore`, which returns distinguishable `Forbidden` messages — but
that store is `lab:admin`-only, so it discloses nothing to an untrusted caller,
whereas retention serves any `can_execute` caller.

### 7.3 Over-budget + enabled is the whole trigger

No reproducibility signal exists at the response boundary. Documented as an
approximation; per-execution opt-out is a follow-up.

### 7.4 Contract drift found during specification

`docs/contracts/schemas/code-mode-call-error.schema.json` enumerates six `origin`
values; `AgentErrorOrigin` (`agent_error.rs:240-250`) has nine. This is a **live
bug on `main`** — `origin_for_kind` already emits `discovery` for `unknown_tool`
today — not something this feature introduces. It must be fixed here regardless.

### 7.5 MCP resources considered, not adopted

A retained handle could also be addressable as `lab://codemode/retained/<handle>`
via the existing prefix dispatch (`mcp/resource_proxy.rs`). Rejected for v1: MCP
resources have no meaning *inside* the sandbox, where paging happens, so it would
be a second surface serving outer clients only. Revisit if operators need to read
retained values directly (see FR-11.5, which addresses the operator gap more cheaply).

## 8. Risks

| Risk | Mitigation |
|---|---|
| Handle leaks via logs | Prefix-only, asserted |
| Owner key too coarse | `actor_key` + route + capability containment; bearer test |
| Marker growth breaks shrink guard | FR-2.5 + FR-1.5 rollback |
| Resident memory surprise | NFR-3 states the measured multiplier; defaults re-derived |
| Lock stall on eviction | FR-7.6 drop outside lock |
| Slice OOMs the runner | FR-7.4 ceiling from sandbox heap |
| Paging silently fails open | FR-8.2 `runner_drive.rs` amendment + test |
| Retention outlives revoked auth | TTL + flush; targeted eviction is a named follow-up |

## 9. References

- Issue [#274](https://github.com/dinglebear-ai/labby/issues/274), parent [#217](https://github.com/dinglebear-ai/labby/issues/217)
- [CONTRACT.md](./CONTRACT.md), [MODELS.md](./MODELS.md), [TYPES.md](./TYPES.md), [IMPLEMENTATION_PLAN.md](./IMPLEMENTATION_PLAN.md), [PROGRESS.md](./PROGRESS.md)
- `docs/dev/CODE_MODE.md`, `docs/dev/ERRORS.md`, `docs/dev/OBSERVABILITY.md`
- RFC 6901 (JSON Pointer), RFC 7233 §2.1/§4.4 (range clamping precedent)
