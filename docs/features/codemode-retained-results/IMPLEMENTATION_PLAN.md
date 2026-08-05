# IMPLEMENTATION PLAN — Code Mode Retained Results

Build sequence for [#274](https://github.com/dinglebear-ai/labby/issues/274), grounded in `origin/main` @ `132448802`. Every
`file:line` below was read, not assumed. Beads: epic `lab-zca58`, children `.1`–`.6`.

> **Revised 2026-08-05** after an 8-agent research pass. The architecture changed:
> retention is no longer a `CodeModeHost` trait extension. Read [MODELS.md](./MODELS.md) §1–2
> before this document.

---

## 0. Current-state map (verified)

| Concern | Location | Today |
|---|---|---|
| Envelope budget | `labby-runtime/src/gateway_config.rs:149-159` | 24 KB / 6000 tokens / divisor 4 |
| Shape policy default | `gateway_config.rs:84`, `:205` | **`Off`** — the string marker never fires by default |
| Result shaping | `labby-codemode/src/shape.rs:27-55` | `shape_final_result` |
| String marker | `shape.rs:99-135` | opt-in only; **no `next_action` field** |
| Envelope truncation | `truncate.rs:19-101` | markers the result, then trims logs |
| JSON marker | `truncate.rs:178-193` | the real path for every caller |
| Shrink invariant | `truncate.rs:40-48` | markers only if `marker_len < original_len` |
| Orchestration | `execute.rs:61-165` | clone raw → shape → truncate → log |
| `result_shaping` reset | `execute.rs:136-138` | dropped when truncation replaces the result |
| Internal namespace | `execute.rs:35`, `:366-372`, `:412-415` | bypasses `scope.allows()` |
| Internal ceiling | `config.rs:16-24` + **`runner_drive.rs:456-486`** | 32/run, enforced at the **enqueue site** |
| **Fail-open shim** | **`runner_drive.rs:974-1005`** | settles over-ceiling internal calls with `{"ranked":[]}` |
| Per-run counter precedent | `runner_drive.rs:140` | plain `usize` on `DriveState`, not an atomic |
| Config pattern | `config.rs:41-114` | env → `OnceLock` → default, **resolved lazily per use** |
| Config install site | `manager/pool_lifecycle.rs:265-268` | inside reload, **after** manager construction |
| **Store precedent** | **`types.rs:605-690`** | `CodeModeSourceStore` — owner-scoped, `Arc` on manager |
| Manager wiring | `manager.rs:115`, `manager/code_mode_runtime.rs:65-79` | inherent methods, not the trait |
| Surface | `mcp/call_tool_codemode.rs:527`, `:552-556`, `:724-739` | holds `route_scope`, `actor_key`, `surface` |
| Actor identity | `mcp/context.rs:122` | `actor_key_from_extensions` |
| Bearer subject | `labby-auth/src/middleware.rs:321` | literal `"static-bearer"` for all bearer callers |
| Tool description | `mcp/call_tool_codemode.rs:179-305`, cap `:307` | the only place helpers are documented; 8192-byte cap |
| MCP App UI | `mcp/assets/code_mode_app.html:747-749` | hardcodes "Narrow the query"; ignores `next_action` |
| Broker lifetime | `broker.rs:1-4` | fresh per request |
| Error mappings | `labby-runtime/src/agent_error.rs:421,482,498` | `origin/recovery/side_effects_for_kind` |
| Poison idiom | `code_mode_host.rs:392-394` | `PoisonError::into_inner` |

Workspace deps: `serde_json 1.0.149`, `uuid 1.23.1` (+`getrandom 0.4.2`), `jiff 0.2.24`.
**`labby-codemode` has neither `uuid` nor `jiff` in its manifest** — both must be added.

---

## Bead 1 — `lab-zca58.1` Result store core

**Files:** `result_store.rs` (new), `tests_result_store.rs` (new), `lib.rs`, `config.rs`, `Cargo.toml`
**Depends on:** nothing. **Budget:** impl <500 LOC + sibling test file.

### 1.1 Manifest and config

Add `uuid` and `jiff` (workspace deps) to `crates/labby-codemode/Cargo.toml`.

Three env knobs only, resolved **lazily per use** (`config.rs:59-84` shape):

```rust
const DEFAULT_RETAIN_TTL_SECS: u64 = 300;
const DEFAULT_RETAIN_MAX_TOTAL_MIB: usize = 16;

static RETENTION_CONFIG_DEFAULTS: std::sync::OnceLock<Option<RetentionConfigDefaults>> =
    std::sync::OnceLock::new();

pub fn install_retention_config_defaults(defaults: Option<RetentionConfigDefaults>);

/// Resolved on every use, NOT captured at store construction —
/// `install_*` runs in `reload_with_origin_unlocked` (manager/pool_lifecycle.rs:265),
/// after the manager exists, exactly like the existing budget knobs.
pub(crate) fn retention_config() -> RetentionConfig;
```

Everything else is a `const` ([TYPES.md](./TYPES.md) §1.5), matching the crate's restraint.

### 1.2 Store

Types per [TYPES.md](./TYPES.md) §1.1–1.6, §2. The three algorithms that must be exact:

```rust
pub fn store(
    &self, cfg: RetentionConfig, owner: &OwnerKey,
    value: Arc<Value>, size_bytes: usize, now: SystemTime,
) -> Option<StoredHandle> {
    if !cfg.enabled || size_bytes > RETAIN_ENTRY_MAX_BYTES {
        return None;
    }

    // Anything evicted is moved here and dropped AFTER the guard is released:
    // dropping one 15.3 MB parsed entry under the lock measured 367 ms, which
    // blocks an OS thread, not just a task.
    let mut condemned: Vec<Arc<RetainedEntry>> = Vec::new();
    let stored = {
        let mut inner = self.lock();          // PoisonError::into_inner
        inner.purge_expired_into(now, &mut condemned);

        // Evict only THIS owner's oldest — never a stranger's live entry.
        // Both quotas matter: the entry-count cap stops 32 tiny results (which
        // need no upstream call at all) from occupying every global slot.
        while inner.owner_would_exceed(owner, size_bytes) {
            if !inner.evict_oldest_owned_by(owner, &mut condemned) { break; }
        }

        if inner.total_bytes + size_bytes > cfg.global_max_bytes
            || inner.entries.len() + 1 > RETAIN_MAX_ENTRIES
        {
            inner.counters.stores_rejected += 1;
            None                                   // caller falls back to truncation
        } else {
            Some(inner.insert(owner, value, size_bytes, now, cfg.ttl))
        }
    };
    drop(condemned);                               // outside the lock
    stored
}
```

```rust
pub fn fetch(
    &self, owner: &OwnerKey, handle: &RetainedHandle, now: SystemTime,
) -> Result<Arc<RetainedEntry>, RetainedMiss> {
    let mut condemned = None;
    let result = {
        let mut inner = self.lock();
        match inner.entries.get(handle).cloned() {
            // Ownership BEFORE state — a foreign caller must not learn that this
            // handle exists. Capability sets compare by containment.
            Some(e) if !owner.may_read(&e.owner) => Err(RetainedMiss::NotFound),
            Some(e) if e.expires_at <= now => {
                condemned = inner.reap(handle);
                Err(RetainedMiss::NotFound)
            }
            Some(e) => { inner.counters.fetch_hits += 1; Ok(e) }
            None => { inner.counters.fetch_misses += 1; Err(RetainedMiss::NotFound) }
        }
    };
    drop(condemned);
    result
}
```

`lock()` is `self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)` —
the workspace idiom (`code_mode_host.rs:392`). A `.ok()?` would disable retention
permanently and silently; mapping poisoning to a miss would tell agents to re-run
a query when the store is actually dead.

### 1.3 Tests (sibling `tests_result_store.rs`, `now` injected — no sleeps)

`store_fetch_round_trip`; **`foreign_session_is_not_found`**;
**`two_bearer_callers_in_separate_sessions_are_isolated`**;
`foreign_actor_is_not_found`; `foreign_route_scope_is_not_found`;
`narrower_capability_cannot_read_broader`; `broader_capability_can_read_narrower`;
`expired_is_not_found`; `malformed_handle_rejected_by_parse`;
`per_caller_byte_quota_evicts_only_own_oldest`; `per_caller_entry_quota_blocks_slot_hogging`;
`global_cap_rejects_rather_than_evicting_stranger`; `entry_over_cap_rejected`;
`disabled_always_rejects`; `eviction_is_deterministic_by_admission_seq`;
`ttl_boundary`; `concurrent_stores_respect_caps`; `fetch_racing_eviction_is_all_or_nothing`;
`poisoned_lock_still_serves`; `evicted_arcs_drop_outside_lock`; `counters_track_each_event`.

---

## Bead 2 — `lab-zca58.2` Retention hook + marker embedding

**Files:** `execute.rs`, `truncate.rs`, `types.rs` (`RetentionContext`), `mcp/call_tool_codemode.rs`
**Depends on:** Bead 1. **Budget:** ~450 LOC incl. tests.
**Not touched:** `host.rs` (no trait change), `shape.rs` (see SPEC FR-2.4).

### 2.1 The seam — a broker field, not a parameter

`RetentionContext` is a **field on `CodeModeBroker`** (`broker.rs:22-30`), set by
a `with_retention()` constructor beside the existing `new()`. A parameter on
`execute()` cannot reach the handlers: the chain runs through `enqueue_tool_call`
(`runner_drive.rs:761`), a free function with an explicit `'a` lifetime tie, so a
parameter would mean editing seven signatures. The field also matches how
`ui_capture` already carries run-scoped state, and it changes **one** construction
site rather than three production call sites plus ~20 test sites.

`execute.rs` then reads `self.retention` in the handlers verbatim, and passes
`self.retention.as_ref()` where the marker is planned.

**Three call sites exist**, not two — `mcp/call_tool_codemode.rs:587`,
`cli/gateway/code.rs:111`, and `dispatch/snippets/dispatch.rs:319`. The snippets
surface was missed in earlier drafts; decide deliberately whether snippet
execution retains.

The surface builds the context (and **must** withhold it when the owner identity
is not per-caller — SPEC FR-5.2a):

```rust
// crates/labby/src/mcp/call_tool_codemode.rs — where route_scope, actor_key,
// and surface all exist. `None` here is how CLI one-shot runs are gated.
let retention = self.gateway.retained_results().map(|store| RetentionContext {
    store,
    owner: OwnerKey {
        // The isolation boundary. `self` is a LabMcpServer, so this is already
        // in scope — one id per transport session (server.rs:242), which is what
        // separates two agents sharing a single bearer token.
        session: self.relay_session_id,
        actor_key: self.request_actor_key(context)
            .unwrap_or(OwnerKey::TRUSTED_LOCAL).to_string(),
        route_scope: self.route_scope.label().to_string(),
        capability_fingerprint: capability_filter.fingerprint(),
    },
    paging_used: AtomicUsize::new(0),
});
```

`retained_results()` is a **gateway inherent method**, sibling to
`record_code_mode_source` / `resolve_code_mode_source`
(`manager/code_mode_runtime.rs:65-79`). `CodeModeHost` is untouched.

### 2.2 Decide first, then mint — no rollback

The previous draft stored the value, built the marker, and called `release()` on
decline. That was unsound: `store()` evicts the **caller's own older entries**
before inserting, and `release()` does not restore them — so a declined marker
would silently destroy live handles from earlier executions, which the obvious
test would not catch. It also let a provisional entry count against other owners'
admissions, and stranded quota on a panic in that window.

None of it is necessary. The three retention fields are **fixed width**:
`result_handle` is always `cmr_` + 32 hex, `retained_until` is a fixed-width
RFC 3339 stamp, and `retained_bytes` **is** `original_len`, already computed at
`truncate.rs:41`. So the shrink decision is arithmetic:

```rust
// Pure — no store, no allocation of the payload. Lives in truncate.rs.
fn plan_result_marker(
    result: &Value, divisor: u32,
    artifacts: &[CodeModeArtifactReceipt],
    retention_will_apply: bool,
) -> Option<MarkerPlan> {
    let original_len = serde_json::to_string(result).map(|s| s.len()).unwrap_or(0);
    // Build with a placeholder handle: same width as a real one.
    let probe = truncation_marker(result, divisor, artifacts, retention_will_apply.then(placeholder));
    let marker_len = serde_json::to_string(&probe).map(|s| s.len()).unwrap_or(0);
    (marker_len < original_len).then(|| MarkerPlan { original_len, marker_len })
}
```

The **commit** then happens in `execute.rs`, where `raw_response` is in scope —
which is what makes SPEC FR-1.7 satisfiable at all. Retaining inside
`truncate_execution_response` would capture the value **after** `execute.rs:107`
assigned `shaped.result`, so under `result_shape_policy = Truncate` the agent
would get a handle to a preview it already has.

Two further corrections to the old sketch:

- **`Arc::clone(result_arc)` refers to a type that does not exist.**
  `types.rs:313` is `Option<Value>`. Written as-is it degrades to
  `Arc::new(result.clone())` — a deep clone costing ~150–240 ms at the 4 MiB
  entry cap. Either take ownership (`response.result.take()`, wrap, and recover
  with `Arc::try_unwrap` if the marker is declined), or scope
  `Option<Arc<Value>>` explicitly: 8 non-test sites, and it also deletes the two
  **pre-existing** deep clones at `execute.rs:99` and `:123`.
- **The over-budget test must be an explicit budget check.** `truncate.rs:44` is
  only `marker_len < original_len` — about a 1.1 KB floor, not a budget — so a
  10 KB result in a logs-dominant response would otherwise be retained despite
  being far under the 24 KB budget (SPEC FR-1.8).

### 2.3 Marker fields

```rust
fn truncation_marker(
    value: &Value, divisor: u32,
    artifacts: &[CodeModeArtifactReceipt],
    retained: Option<&RetainedMarkerFields>,
) -> Value {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
    let mut marker = json!({
        "truncated": true,
        "original_size": serialized.len(),
        "original_tokens": estimated_tokens(serialized.len(), divisor),
        "preview": utf8_prefix_by_bytes(&serialized, 1024).to_string(),
        "artifacts": artifacts,
        "next_action": next_action_text(retained),
    });
    if let Some(r) = retained
        && let Some(obj) = marker.as_object_mut()
    {
        obj.insert("result_handle".into(), json!(r.handle.as_str()));
        obj.insert("retained_until".into(), json!(jiff::Timestamp::try_from(r.retained_until)
            .map(|t| t.to_string()).unwrap_or_default()));
        obj.insert("retained_bytes".into(), json!(r.retained_bytes));
    }
    marker
}

/// Reduce-before-return LEADS (#217). The paging clause names `result_handle` —
/// the field — because the existing next_action is self-contained and models are
/// trained to read it alone.
fn next_action_text(retained: Option<&RetainedMarkerFields>) -> &'static str {
    if retained.is_some() {
        "Reduce inside the sandbox before returning — project fields, filter rows, \
         or slice arrays. This result was retained: pass the `result_handle` value \
         below to codemode.fetch(handle) or codemode.slice(handle, path, range) in a \
         later execution instead of re-running an expensive or rate-limited call."
    } else {
        "Use a narrower query, request fewer fields, or split the work across \
         multiple codemode calls."
    }
}
```

The `else` branch is byte-identical to `truncate.rs:191` — that is what keeps the
disabled path a byte-for-byte match. `jiff` is the workspace timestamp crate
(`jiff::Timestamp::now().to_string()` is used at `gateway_config.rs:344` and four
other sites); there is no `rfc3339()` helper to invent.

### 2.4 Traps

1. **The shrink guard** (`truncate.rs:44`) stays authoritative, with rollback (§2.2).
2. **`result_shaping = None`** (`execute.rs:136-138`) — the handle rides in the
   marker value, never in shaping metadata. Most likely mistake.
3. **`shape.rs` is not modified.** Its marker fires only under an opt-in policy
   that is not the default, has no `next_action` to carry guidance, and its
   `room = budget - marker_prefix.len()` (`shape.rs:117`) would silently shrink
   the preview.

### 2.5 Tests

`retention_marker_carries_handle_and_expiry`; `disabled_output_is_byte_identical`;
`store_full_falls_back_to_truncation`; `under_budget_result_is_not_retained`;
`logs_dominant_response_mints_no_handle`; `handle_survives_result_shaping_reset`;
`declined_marker_releases_the_entry`; `marker_with_handle_still_shrinks_envelope`;
`next_action_names_result_handle_field`; `bearer_callers_are_distinct_owners`;
`cli_surface_builds_no_context`; `multibyte_preview_still_utf8_safe`.

---

## Bead 3 — `lab-zca58.3` Handlers, slice engine, error kinds

**Files:** `result_slice.rs` (new), `execute.rs`, **`runner_drive.rs`**, `broker.rs`,
`error_contract.rs`, `labby-runtime/src/agent_error.rs`, `docs/dev/ERRORS.md`,
`docs/contracts/code-mode-tool-errors.md`, `docs/contracts/schemas/code-mode-call-error.schema.json`
**Depends on:** Beads 1, 2. **Budget:** ~800 LOC incl. tests.

### 3.1 The fail-open shim must be amended first

This is the finding that would have silently defeated the whole fail-closed
requirement. `runner_drive.rs:456-460` counts **every** `__lab_internal::`-prefixed
id against the shared 32-call ceiling, and `enqueue_internal_call_over_ceiling`
(`runner_drive.rs:974-1005`) settles over-ceiling calls **before**
`dispatch_internal_call` runs. Its `else` branch would hand the paging helpers
`{"ranked": []}` — a wrong-shaped success. The existing comment there warns
against exactly this: *"keep this explicit so a future third internal tool doesn't
inherit that accident."*

The naive two-line edit is **wrong twice over**. Setting
`is_internal = !is_paging && …` makes paging fall into the `else` arm at
`runner_drive.rs:465`, charging it to the **ordinary `callTool` budget** — the
exact exemption FR-8.3 requires — and on overflow routing it to
`reject_tool_call_over_budget`, whose unconditional `state.calls.push(...)`
(`:1084`) puts the call **and its params, including the full handle** into
`response.calls`. `redact_trace_params` keys on `token`/`secret`/`password`
(`trace.rs:369-384`), so `handle` is not redacted. And an empty `if is_paging {}`
inside `enqueue_internal_call_over_ceiling` cannot "fall through" — that
function's only job is to push a settled future (`:995-1008`).

The correct shape is a **three-way classification at the call site**
(`runner_drive.rs:456-497`), with paging incrementing neither existing counter:

```rust
// Shared with the execute.rs dispatch match — one list, not two literal copies,
// or the two sites drift and paging silently re-enters the fail-open path.
const PAGING_TOOL_IDS: &[&str] = &[
    "__lab_internal::codemode_result_fetch",
    "__lab_internal::codemode_result_slice",
];

// Gate on retention being enabled: a bare string match would change call-budget
// accounting on the DISABLED path, violating NFR-1.
let is_paging = retention_enabled && PAGING_TOOL_IDS.contains(&id.as_str());
let is_internal = !is_paging && id.starts_with("__lab_internal::");

if is_paging {
    state.paging_calls_enqueued = state.paging_calls_enqueued.saturating_add(1);
    if state.paging_calls_enqueued > RETAIN_CALLS_PER_RUN {
        // Fail CLOSED via the existing structured-error settler — not the
        // fail-open value, and not by letting the handler meter it downstream,
        // which would allow a while(true) loop to enqueue unbounded futures.
        enqueue_tool_call_error(seq, id, params, call_budget_exceeded_paging(), ...);
        continue;
    }
} else if is_internal { /* unchanged */ } else { /* unchanged */ }
```

Metering at **enqueue** (not in the handler) is the point: the existing ceiling
gates there precisely so a runaway loop cannot push thousands of boxed futures
into `pending_tool_calls` before the first error. Paging remains exempt from the
`callTool` budget and the trace, consistent with other internal calls — the
settlement-site `is_internal` recomputation (`runner_drive.rs:~1152`) is a prefix
match and already excludes paging from the trace on the success path.

### 3.2 Slice engine

```rust
pub fn select(
    root: &Value, pointer: &str, range: Option<SliceRange>, max_bytes: usize,
) -> Result<SliceSelection, SliceError> {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(SliceError::InvalidPointer { pointer: pointer.into() });
    }
    let target = if pointer.is_empty() { root } else {
        root.pointer(pointer).ok_or_else(|| SliceError::PathNotFound { pointer: pointer.into() })?
    };

    let (value, range_applied, source_length) = match (range, target) {
        (None, v) => (v.clone(), None, None),
        (Some(r), Value::Array(items)) => {
            let (s, e) = clamp(r, items.len());
            (Value::Array(items[s..e].to_vec()), Some((s, e)), Some(items.len()))
        }
        (Some(r), Value::String(text)) => {
            // MEASURED: the naive three-pass form (chars().count() + two
            // char_indices().nth()) is 5.2-15.3 ms on a 4 MiB ASCII string —
            // SLOWER than the Vec<char> bug it replaces (6.1 ms). `nth()` is not
            // specialized and degrades superlinearly under cache pressure.
            // ASCII fast path + offset second scan measures 0.10 ms.
            let total = entry.char_len();          // cached OnceLock — 63 of 64
                                                   // paging calls skip this pass
            let (s, e) = clamp(r, total);
            let (a, b) = if text.is_ascii() {
                (s, e)                             // char index == byte index
            } else {
                let a = text.char_indices().nth(s).map_or(text.len(), |(i, _)| i);
                // Offset from `a`, so this walks (e - s) chars — bounded by
                // RETAIN_SLICE_MAX_BYTES — not `e` chars from the start.
                let b = text[a..].char_indices().nth(e - s).map_or(text.len(), |(i, _)| a + i);
                (a, b)
            };
            (Value::String(text[a..b].to_string()), Some((s, e)), Some(total))
        }
        (Some(_), other) => return Err(SliceError::RangeNotApplicable { value_type: type_name(other) }),
    };

    let selection_bytes = serde_json::to_vec(&value).map(|b| b.len()).unwrap_or(usize::MAX);
    if selection_bytes > max_bytes {
        return Err(SliceError::TooLarge { selection_bytes, max_bytes });
    }
    Ok(SliceSelection { value, selection_bytes, range_applied, source_length })
    //
    // NOTE — the arms above must size the TARGET before materializing it.
    // As sketched, `codemode.slice(handle, "")` with no range deep-clones the
    // whole 4 MiB entry (~32-50 MiB resident, ~150-240 ms), serializes it, and
    // only then returns retained_value_too_large — making the ERROR path the
    // most expensive path in the feature, repeatable 64 times per run. Use an
    // early-aborting counting writer over the borrowed `&Value` and clone only
    // what survives the check; likewise for `items[s..e].to_vec()`.
}

/// Array.prototype.slice clamping; start >= end is empty, never an error.
fn clamp(r: SliceRange, len: usize) -> (usize, usize) {
    let s = r.start.unwrap_or(0).min(len);
    let e = r.end.unwrap_or(len).min(len);
    (s, e.max(s))
}
```

### 3.3 Handlers

Registered in `dispatch_internal_call` (`execute.rs:415+`) **only when a
`RetentionContext` exists** — so with retention off the existing `_ =>` arm
(`execute.rs:516-519`) returns the defined `unknown_tool`, and no out-of-contract
"disabled" kind is needed.

```rust
"codemode_result_fetch" => {
    self.paging_budget()?;                       // fail-CLOSED
    let ctx = self.retention.as_ref().ok_or_else(unknown_tool)?;
    let handle = parse_handle_param(&params)?;   // → retained_handle_malformed
    // Owner comes from the context — derived ONCE at the surface, not rebuilt
    // per call, and the handler enforces it because internal dispatch bypasses
    // scope.allows() (execute.rs:366-372).
    let entry = ctx.store.fetch(&ctx.owner, &handle, SystemTime::now())
        .map_err(miss_to_tool_error)?;
    Ok(fetch_metadata_json(&entry, RETAIN_SLICE_MAX_BYTES))
}
"codemode_result_slice" => {
    self.paging_budget()?;
    let ctx = self.retention.as_ref().ok_or_else(unknown_tool)?;
    let handle = parse_handle_param(&params)?;
    let entry = ctx.store.fetch(&ctx.owner, &handle, SystemTime::now())
        .map_err(miss_to_tool_error)?;
    let selection = result_slice::select(
        &entry.value,
        params.get("path").and_then(Value::as_str).unwrap_or(""),
        parse_range_param(&params)?,
        RETAIN_SLICE_MAX_BYTES,      // from the 64 MiB sandbox heap, NOT calltool cap
    ).map_err(slice_error_to_tool_error)?;
    Ok(slice_response_json(&handle, selection))
}
```

`fetch_metadata_json` emits the root-shape metadata only. A paging loop is sized
by a zero-width probe slice, which already returns `source_length` — `array_lengths`
was cut (SPEC FR-3.3).

The paging budget is a plain counter on the **per-request broker** (`broker.rs:1-4`),
mirroring `DriveState::internal_calls_enqueued` (`runner_drive.rs:140`); that state
is single-threaded per execution, so no atomics. On the shared store it would
become a process-global ceiling.

### 3.4 Errors and docs (same change)

Five kinds with the **specified guidance** from [CONTRACT.md](./CONTRACT.md) §3.1 — not the generic
arms of `recovery_for_kind` (`agent_error.rs:498-606`), which would tell an agent
to "correct the parameters and retry" a handle that can never come back. Use
`origin: discovery` + `rediscover` + `never` for the miss, matching every existing
discovery-origin kind (`agent_error.rs:458-461`, `:516-523`).

Also reconcile `code-mode-call-error.schema.json`: it lists 6 origins, the Rust
enum has 9. That is a live bug on `main` (`unknown_tool` already emits `discovery`).

### 3.5 Tests

Slice: `pointer_root`, `pointer_nested`, `pointer_escapes_tilde_and_slash`,
`pointer_missing_is_path_not_found`, `invalid_pointer_syntax`,
**`pointer_rejects_leading_zero_index`**, **`pointer_dash_token_is_path_not_found`**,
`array_range_clamps_both_ends`, `inverted_range_is_empty`,
`string_range_never_splits_multibyte`, `string_slice_allocates_no_vec_char`,
`range_on_object_is_not_applicable`, `oversized_selection_errors_not_truncates`.
Handlers: `fetch_metadata_shape`, `zero_width_probe_reports_source_length`,
`fetch_inlines_small_value`, `fetch_omits_large_value_with_guidance`,
`cross_owner_fetch_is_not_found`, **`paging_survives_shared_internal_ceiling`**,
`paging_ceiling_fails_closed`, `handlers_absent_without_context_return_unknown_tool`,
`error_guidance_matches_contract`, `schema_conformance_*`.

---

## Bead 4 — `lab-zca58.4` Sandbox helpers, typings, tool description

**Files:** `preamble.rs`, `ts_signatures.rs`, `tests_ts_signatures.rs`, **`mcp/call_tool_codemode.rs`** (description)
**Depends on:** Bead 3. **Budget:** ~400 LOC incl. tests.

Generate the helpers (conditionally on a `RetentionContext`), propagating errors
rather than swallowing them — `describe()` deliberately degrades on failure
(`preamble.rs:396-410`), but paging must surface `retained_*` kinds.

```rust
codemode.fetch = async function(handle) {{
  return await callTool("__lab_internal::codemode_result_fetch", {{ handle: handle }});
}};
codemode.slice = async function(handle, path, range) {{
  var params = {{ handle: handle }};
  if (path !== undefined && path !== null) {{ params.path = path; }}
  if (range !== undefined && range !== null) {{ params.range = range; }}
  return await callTool("__lab_internal::codemode_result_slice", params);
}};
```

**Reserve `fetch`/`slice` conditionally.** Adding them unconditionally to
`CODEMODE_TOP_LEVEL_RESERVED` (`preamble.rs:98`) would rename a real upstream
namespace called `fetch` even with retention off, via `namespace_segment()`
(`preamble.rs:156-163`) — a behavior change on the disabled path.

**Update the tool description** (`call_tool_codemode.rs:179-305`). This is the only
surface where the existing helpers are documented, and it already describes the
truncation marker at `:288-291` — which goes stale the moment retention ships.
`codemode.describe()`/`search()` cannot see these helpers by construction (they
close over the catalog index), so the description is load-bearing. Respect the
8192-byte cap (`:307`, asserted at `call_tool_codemode/tests.rs:187`); the body is
~5593 bytes today. Include the corrected worked example from [TYPES.md](./TYPES.md) §3.1.

**Tests:** `helpers_absent_without_context`, `helpers_present_with_context`,
`reserved_names_conditional`, `dts_includes_both_signatures`,
`guidance_puts_reduce_before_paging`, `description_within_byte_cap`,
`description_documents_result_handle`, plus a runner round-trip against a stub.

---

## Bead 5 — `lab-zca58.5` Gateway wiring, lifecycle, observability

**Files:** `gateway/manager.rs`, `manager/core.rs`, `manager/code_mode_runtime.rs`,
`manager/pool_lifecycle.rs`, `dispatch/doctor/`
**Depends on:** Beads 2, 3. **Budget:** ~450 LOC incl. tests.
**Not touched:** `crates/labby/src/config.rs` — see below.

1. Add `code_mode_retained_results: Arc<RetainedResultStore>` to `GatewayManager`
   (`manager.rs:115` neighborhood), constructed in `manager/core.rs:148`, sibling
   to `code_mode_source_store`.
2. Expose `retained_results()` as a **gateway inherent method**
   (`manager/code_mode_runtime.rs:65-79` pattern). No trait change.
3. `store.flush()` in `reload_with_origin_unlocked` (`pool_lifecycle.rs:229+`) —
   draining under the guard, dropping after (a flush at cap is ~1.5 s of
   deallocation). Note in docs that *any* `gateway.reload` drops retained results.
4. Seed config defaults next to `install_call_budget_config_defaults`
   (`pool_lifecycle.rs:265-268`) — **not** in `crates/labby/src/config.rs`, which
   has zero `labby_codemode` references and would need a `#[cfg(feature = "gateway")]`
   gate, creating the plan's only cross-slice coupling.
5. Observability under `service = "code_mode"`: `result.retain`, `result.fetch`,
   `result.slice`, `result.evict`, `result.expire`. Handle **prefix only**; never
   payload bytes.
6. **Give operators a read path** for `counters()` — a `doctor` check is cheapest.
   Agents can read the store; without this, operators cannot answer "is the store
   full?" except by log-grep, and `stores_rejected` is precisely the "raise the
   caps" signal.

**Tests:** `store_present_only_when_enabled`, `reload_flush_empties_store`,
`flush_drops_outside_lock`, `config_toml_defaults_apply_after_reload`,
`env_overrides_config_toml`, `logs_never_contain_full_handle`,
`doctor_reports_retention_counters`.

---

## Bead 6 — `lab-zca58.6` E2E, docs, generated artifacts

**Files:** `crates/labby` E2E tests, `docs/dev/CODE_MODE.md`,
**`mcp/assets/code_mode_app.html`**, `docs/generated/*`
**Depends on:** Beads 4, 5. **Budget:** ~550 LOC incl. tests.

1. E2E: execution 1 over budget → handle; execution 2 fetches/slices → small
   answer. **Assert zero upstream calls with a counting host** — `CountingHost`
   (`runner_drive.rs:1401`) or `RecordingHost` (`:1654`). Asserting an empty
   `response.calls` proves nothing: internal calls are excluded from the trace by
   construction (`runner_drive.rs:443-452`, test at `:1324`).
2. Negative twins: foreign owner, past TTL, retention disabled (byte-identical).
3. Concurrency: parallel over-budget executions never breach caps.
4. **Regression guards for the measured hazards**: resident-footprint ratio,
   lock-hold duration on flush/eviction, and slice cost over a large string.
5. Fix `code_mode_app.html:747-749`, which hardcodes *"Narrow the query or return
   a smaller object"* and ignores `next_action` — it would tell a `codemode_ui`
   user to re-run a query whose result was in fact retained and pageable.
6. `docs/dev/CODE_MODE.md` gains a "Retained results" section; then
   `just docs-generate` + `just docs-check`.

---

## Verification

```bash
just test
```

```bash
just lint
```

```bash
just docs-check
```

Per-bead: `cargo nextest run -p labby-codemode`.
CI gates: `test`, `clippy`, `fmt`, `docs-check`, `mcp-regressions`,
`codemode-runner-smoke`, `feature-slices` (**both `gateway` and `fs`** — `fs` is
the slice that step 5.4 would have broken).

## Sequencing

The earlier graph was **inverted**: bead 2's surface code calls
`self.gateway.retained_results()`, which bead 5 delivers — so bead 2 could not
compile. Bead 5 splits, and three units of bead 3 have no dependency on 1 or 2 at
all, which also unblocks parallelism:

```
0  schema-origin fix        ── independent PR, land FIRST (pre-existing bug on main)
3a runner_drive amendment   ── independent (hardening of shipped code)
3b slice engine             ── independent (pure fn over &Value)

1 store ──▶ 5a manager field + accessor ──▶ 2 hook+marker ──▶ 3c handlers ──▶ 4 helpers+description ──┐
                                                                    │                                 ├──▶ 6 E2E+docs
                                                                    └──▶ 5b flush + observability ────┘
```

- **0** — the 6-vs-9 `origin` schema drift is live on `main` and independent.
  Landing it inside a disabled-by-default feature PR sweeps unrelated drift into
  the commit, against repo convention.
- **3a / 3b** can run concurrently with bead 1, which otherwise sits alone on the
  critical path.
- **5a before 2**, **5b after 3**.

## Risk register

| Risk | Where | Mitigation |
|---|---|---|
| Paging silently fails open | `runner_drive.rs:974` | §3.1 amendment + `paging_survives_shared_internal_ceiling` |
| Orphaned entry holds quota | `truncate.rs:44` decline | §2.2 rollback + `declined_marker_releases_the_entry` |
| Bearer callers share an owner | `middleware.rs:321` | `actor_key`; `bearer_callers_are_distinct_owners` |
| Handle lost to `result_shaping` reset | `execute.rs:136` | Handle in marker value; test |
| Resident memory 8–13x the cap | store | Defaults re-derived; ratio regression test |
| Lock stall on eviction/flush | store | Drop outside guard; hold-duration test |
| Slice OOMs the runner | ceiling choice | `RETAIN_SLICE_MAX_BYTES` from sandbox heap |
| Ownership check omitted | `execute.rs:366` bypass | Handler-side check + cross-owner test |
| Disabled path drifts | marker text | Byte-identical snapshot test |
| Stale published error schema | contracts | Reconciled in §3.4 |
