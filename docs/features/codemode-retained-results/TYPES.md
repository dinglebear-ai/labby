# TYPES — Code Mode Retained Results

Concrete definitions. Rust follows crate conventions (edition 2024,
`unsafe_code = "forbid"`, no `#[async_trait]`, no `mod.rs`, native `async fn` in
traits). TypeScript is what the sandbox sees.

> **Revised after research (2026-08-05).** The `CodeModeHost` trait accessors are
> gone — see [MODELS.md](./MODELS.md) §1–2. Ownership keys on `actor_key`, not `sub`. The
> worked example in §3.1 was rewritten; the previous one returned an empty array
> for its own payload shape.

---

## 1. Rust — public vocabulary

New: `crates/labby-codemode/src/result_store.rs` (+ sibling `tests_result_store.rs`).

### 1.0 Crate dependencies to add

`crates/labby-codemode/Cargo.toml` currently has **no** `uuid` and **no** `jiff`.
Both are existing workspace dependencies, so this adds no new third-party crate
to the tree, but the manifest edit is real work and `just deny` gates it:

```toml
uuid = { workspace = true }   # v4 + serde already enabled workspace-wide
jiff = { workspace = true }   # RFC 3339 formatting, the workspace's timestamp crate
```

`ulid` is already present (`artifacts.rs:11`) and is how the codebase mints other
opaque ids, but it is **not** used here: a ULID embeds a 48-bit creation
timestamp, and a capability token should not leak when it was minted or sort
predictably. UUIDv4 gives 122 bits of `getrandom`-backed entropy with no structure.

### 1.1 Handle

```rust
use std::fmt;

/// Opaque, unguessable retained-result handle: `cmr_` + 32 lowercase hex chars.
///
/// Encodes no ownership, path, timestamp, or secret — a lookup key only.
/// `Display`/`as_str` print the full handle; use [`log_prefix`] for anything
/// reaching a log, because a handle is a capability token.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RetainedHandle(String);

impl RetainedHandle {
    const PREFIX: &'static str = "cmr_";
    const HEX_LEN: usize = 32;

    #[must_use]
    pub fn generate() -> Self {
        Self(format!("{}{}", Self::PREFIX, uuid::Uuid::new_v4().simple()))
    }

    /// Parse a caller-supplied handle. Rejection is `retained_handle_malformed`,
    /// never a not-found, so a typo stays distinguishable from a miss.
    pub fn parse(raw: &str) -> Result<Self, MalformedHandle> {
        let Some(hex) = raw.strip_prefix(Self::PREFIX) else {
            return Err(MalformedHandle);
        };
        if hex.len() != Self::HEX_LEN || !hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            return Err(MalformedHandle);
        }
        Ok(Self(raw.to_string()))
    }

    /// Short prefix safe to log (`cmr_9f2c41d8`).
    #[must_use]
    pub fn log_prefix(&self) -> &str {
        &self.0[..Self::PREFIX.len() + 8]
    }

    #[must_use]
    pub fn as_str(&self) -> &str { &self.0 }
}

impl fmt::Display for RetainedHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MalformedHandle;
```

### 1.2 Owner key

Mirrors `CodeModeSourceLookup` (`types.rs:643-690`), the shipping precedent.

```rust
/// Authorization identity a retained entry belongs to.
///
/// `session` carries the isolation. Auth identity alone cannot: bearer auth
/// assigns every caller the constant subject "static-bearer"
/// (labby-auth/src/middleware.rs:321) and `derive_actor_key` sees only that
/// string (`:491`), so `actor_key` is constant across bearer callers — as is
/// `route_scope.label()` on the default route. Two agents sharing one token do
/// still occupy two transport sessions, so `relay_session_id` isolates them
/// with no change to `labby-auth`. Issue #274 sanctions per-session ownership.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OwnerKey {
    /// `next_relay_session_id()` (mcp/server.rs:242) — one per `LabMcpServer`,
    /// i.e. one per transport session. Stable for the session's lifetime and
    /// never reused across sessions (server.rs:37-42).
    pub session: u64,
    /// Defense in depth; real weight under OAuth, constant under bearer.
    pub actor_key: String,
    pub route_scope: String,
    /// `ToolScope::fingerprint()` (types.rs:907) — compared by CONTAINMENT.
    pub capability_fingerprint: String,
}

impl OwnerKey {
    pub const TRUSTED_LOCAL: &'static str = "trusted-local";

    /// True when `self` may read an entry stored under `stored`.
    ///
    /// Session, actor, and route must match exactly; the stored capability set
    /// must be WITHIN the reader's current set, so a narrower later execution
    /// cannot read a broader earlier one. Mirrors
    /// `source_capability_within_lookup` (types.rs:690+).
    #[must_use]
    pub fn may_read(&self, stored: &OwnerKey) -> bool {
        self.session == stored.session
            && self.actor_key == stored.actor_key
            && self.route_scope == stored.route_scope
            && capability_within(&stored.capability_fingerprint, &self.capability_fingerprint)
    }
}
```

### 1.3 Retention context — the seam

```rust
/// Everything the kernel needs for retention, built ONCE per execution by the
/// surface (crates/labby/src/mcp/call_tool_codemode.rs), which is the only layer
/// holding `route_scope`, `actor_key`, and `surface` together.
///
/// `None` disables retention for this execution — how CLI one-shot runs are
/// gated (FR-9.2), and how bearer-sentinel identities are refused (FR-5.2a).
#[derive(Debug)]
pub struct RetentionContext {
    pub store: Arc<RetainedResultStore>,
    pub owner: OwnerKey,
    /// Per-run paging ceiling. MUST be atomic: `dispatch_internal_call` takes
    /// `&self`, so a bare `usize` cannot be mutated, and a `Cell` is not `Sync`
    /// — which would break the `Send` bound the rmcp `ServerHandler` boundary
    /// requires. Sandbox JS can also issue concurrent paging calls via
    /// `Promise.all`, which an unsynchronized counter would race. The
    /// `DriveState::internal_calls_enqueued` precedent is `&mut self` and does
    /// not transfer.
    pub paging_used: std::sync::atomic::AtomicUsize,
}
```

**This lives as a field on `CodeModeBroker`** (`broker.rs:22-30`), beside
`ui_capture`, set by a `with_retention()` constructor — **not** as a new
`execute()` parameter. The chain from `execute()` to `dispatch_internal_call`
passes through the free function `enqueue_tool_call` (`runner_drive.rs:761`) with
an explicit `'a` lifetime tie, so a parameter would require editing seven
signatures and ~20 test construction sites; the field changes one.

### 1.4 Entry and miss

```rust
/// A retained final result. `Arc`-shared so a hit is a refcount bump.
#[derive(Debug)]
pub struct RetainedEntry {
    pub handle: RetainedHandle,
    pub owner: OwnerKey,
    /// Parsed — JSON Pointer resolution needs it parsed. NOTE: resident size is
    /// 7.9x–12.6x `size_bytes` for MCP-shaped data (MODELS §6.1).
    pub value: Arc<Value>,
    /// Serialized length, measured once (hoisted from shaping — NFR-5).
    pub size_bytes: usize,
    /// Monotonic admission counter — the eviction sort key. NOT `SystemTime`,
    /// which an NTP step can reorder (SPEC FR-7.5).
    pub seq: u64,
    pub created_at: SystemTime,
    pub expires_at: SystemTime,
}

/// Why a lookup failed. `NotFound` deliberately covers unknown, foreign-owned,
/// and expired: all three carry an identical recovery contract, and
/// distinguishing the foreign case would be a handle-existence oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedMiss {
    NotFound,
}

#[derive(Debug, Clone)]
pub struct StoredHandle {
    pub handle: RetainedHandle,
    pub retained_until: SystemTime,
    pub retained_bytes: usize,
}
```

### 1.5 Config and counters

```rust
/// Caps. Resolved LAZILY per use — `install_*_config_defaults` runs inside
/// `reload_with_origin_unlocked` (manager/pool_lifecycle.rs:265), AFTER manager
/// construction, so a config captured at construction ignores config.toml.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionConfig {
    pub enabled: bool,             // env: LABBY_CODE_MODE_RETAIN_RESULTS
    pub ttl: Duration,             // env: LABBY_CODE_MODE_RETAIN_TTL_SECS
    pub global_max_bytes: usize,   // env: LABBY_CODE_MODE_RETAIN_MAX_TOTAL_MIB
}

// Everything else is a const, matching the crate's restraint (config.rs keeps
// MAX_INTERNAL_CALLS_PER_RUN and MAX_SNIPPET_RESOLVES_PER_RUN as constants).
pub(crate) const RETAIN_ENTRY_MAX_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const RETAIN_PER_CALLER_MAX_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const RETAIN_PER_CALLER_MAX_ENTRIES: usize = 8;
pub(crate) const RETAIN_MAX_ENTRIES: usize = 32;
pub(crate) const RETAIN_CALLS_PER_RUN: usize = 64;
/// Derived from the 64 MiB QuickJS sandbox heap and the measured parse
/// expansion — NOT from `calltool_result_max_bytes` (8 MiB), which would let a
/// successful host-side slice OOM the runner (SPEC FR-7.4).
pub(crate) const RETAIN_SLICE_MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetentionCounters {
    pub stored_bytes: u64,
    pub entry_count: u64,
    pub stores_admitted: u64,
    pub stores_rejected: u64,   // the "raise the caps" signal — surface it (FR-11.5)
    pub evictions: u64,
    pub expirations: u64,
    pub fetch_hits: u64,
    pub fetch_misses: u64,
}
```

### 1.6 Store API

```rust
/// Bounded, TTL-expiring, per-caller-quota'd in-memory store.
///
/// INVARIANT: never `.await` under the inner guard. The compiler will not catch
/// this — `dispatch_internal_call` is an inherent `async fn` whose future is only
/// required `Send` at the outermost spawn point.
#[derive(Debug)]
pub struct RetainedResultStore {
    inner: std::sync::Mutex<StoreInner>,
}

impl RetainedResultStore {
    #[must_use]
    pub fn new() -> Self;

    /// Admit a value, or `None` when the caller must fall back to hard
    /// truncation. Follows MODELS §5's ordered algorithm; never evicts another
    /// owner's live entry.
    ///
    /// Evicted `Arc`s are moved out and dropped AFTER the guard is released —
    /// dropping one 15.3 MB parsed entry under the lock measured 367 ms.
    pub fn store(
        &self,
        cfg: RetentionConfig,
        owner: &OwnerKey,
        value: Arc<Value>,
        size_bytes: usize,
        now: SystemTime,
    ) -> Option<StoredHandle>;

    /// Resolve a handle. Ownership is checked before state, so a foreign caller
    /// cannot learn that a handle exists (MODELS §3.5).
    pub fn fetch(
        &self,
        owner: &OwnerKey,
        handle: &RetainedHandle,
        now: SystemTime,
    ) -> Result<Arc<RetainedEntry>, RetainedMiss>;

    /// Drop everything (gateway reload). Drains under the guard, then hands the
    /// drained entries to `spawn_blocking` — ~384 ms of deallocation at the
    /// 16 MiB cap, on the operator-visible reload path, is past the point where
    /// occupying a tokio worker is acceptable.
    pub fn flush(&self);

    /// Remove a single entry. NOT used by the marker path — the shrink decision
    /// is made arithmetically before minting, so there is no rollback (FR-1.5).
    /// Exists only for targeted eviction, and takes an owner because the next
    /// caller to reach for it will otherwise have built an unauthenticated
    /// cross-tenant eviction primitive.
    pub fn release(&self, owner: &OwnerKey, handle: &RetainedHandle) -> bool;

    pub fn purge_expired(&self, now: SystemTime) -> usize;

    #[must_use]
    pub fn counters(&self) -> RetentionCounters;
}
```

`now` is injected so TTL and eviction are testable without sleeping.

### 1.7 Slice engine

`crates/labby-codemode/src/result_slice.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SliceRange { pub start: Option<usize>, pub end: Option<usize> }

#[derive(Debug, Clone, PartialEq)]
pub struct SliceSelection {
    pub value: Value,
    pub selection_bytes: usize,
    pub range_applied: Option<(usize, usize)>,
    pub source_length: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SliceError {
    InvalidPointer { pointer: String },
    PathNotFound { pointer: String },
    RangeNotApplicable { value_type: &'static str },
    TooLarge { selection_bytes: usize, max_bytes: usize },
}

/// Resolve `pointer`, apply `range`, enforce `max_bytes`.
///
/// Never truncates — an oversized selection is `TooLarge`, so a paged
/// reconstruction cannot be silently corrupted.
///
/// String ranges use `char_indices()` to find byte offsets and slice `&text[a..b]`.
/// They MUST NOT collect `Vec<char>`: measured 5.2 ms + 16 MiB allocated per call
/// on a 4 MiB string versus 0.002 ms and zero allocation.
pub fn select(
    root: &Value,
    pointer: &str,
    range: Option<SliceRange>,
    max_bytes: usize,
) -> Result<SliceSelection, SliceError>;
```

`serde_json::Value::pointer` implements RFC 6901 including `~0`/`~1`, rejects
leading-zero array indices, and fails the `-` token — all verified, all pinned by
test rather than assumed.

### 1.8 Marker carrier

```rust
/// Retention facts embedded in the JSON truncation marker. `None` everywhere
/// when retention did not happen — which is what keeps the disabled path
/// byte-identical (NFR-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedMarkerFields {
    pub handle: RetainedHandle,
    pub retained_until: SystemTime,
    pub retained_bytes: usize,
}
```

## 2. Internal structures

```rust
#[derive(Debug, Default)]
struct StoreInner {
    entries: HashMap<RetainedHandle, Arc<RetainedEntry>>,
    /// Deterministic eviction order, sorted by admission `seq`. A plain Vec —
    /// the operation admission needs is "oldest owned by OWNER", which a globally
    /// ordered BTreeSet cannot answer in O(log n) anyway (MODELS §5.2).
    order: Vec<(u64, RetainedHandle)>,
    per_owner: HashMap<OwnerKey, OwnerUsage>,
    total_bytes: usize,
    next_seq: u64,
    counters: RetentionCounters,
}

#[derive(Debug, Default, Clone, Copy)]
struct OwnerUsage { bytes: usize, entries: usize }
```

Paging budget lives on the **per-request broker** (`broker.rs:1-4`: "a fresh
broker is constructed per request"), never on the shared store — otherwise the
ceiling becomes process-global and every caller shares 64 calls forever:

```rust
/// Per-execution paging ceiling. A plain counter, mirroring
/// `DriveState::internal_calls_enqueued` (runner_drive.rs:140) — that state is
/// single-threaded per execution, so no atomics are needed. Unlike the internal
/// ceiling, this one FAILS CLOSED.
pub(crate) struct PagingBudget { used: usize, limit: usize }
```

## 3. TypeScript — ambient sandbox declarations

Emitted only when a `RetentionContext` exists.

> `preamble.rs` emits **runtime JS**, not declarations (`preamble.rs:10-12`). These
> types therefore reach the model through the `codemode` tool description
> (`mcp/call_tool_codemode.rs:179-305`), which is the only surface where the
> existing helpers are documented and which Bead 4 must update — subject to its
> 8192-byte cap (`:307`).

```typescript
type RetainedHandle = string;
type RetainedValueType = "object" | "array" | "string" | "number" | "boolean" | "null";

interface RetainedFetchResponse {
  handle: RetainedHandle;
  size_bytes: number;
  token_estimate: number;
  value_type: RetainedValueType;
  array_length?: number;        // when value_type === "array"
  string_length?: number;       // chars, not bytes
  object_keys?: string[];       // first 100 root keys
  object_key_count?: number;
  // NOTE: `array_lengths` was considered and CUT (SPEC FR-3.3) — a zero-width
  // probe slice already returns `source_length`, and building the map would walk
  // an unbounded root key count on every fetch.
  created_at: string;
  expires_at: string;           // fixed at creation; fetching does NOT extend it
  value_omitted: boolean;
  value?: unknown;              // iff !value_omitted
  guidance?: string;            // iff value_omitted
}

interface RetainedSliceResponse {
  handle: RetainedHandle;
  path: string;
  value: unknown;
  selection_bytes: number;
  truncated: false;             // always — slice errors rather than truncating
  range_applied?: { start: number; end: number };
  source_length?: number;
}

declare namespace codemode {
  /** Metadata about a retained oversized result, plus its value when small
   *  enough to inline. Does NOT extend the TTL. */
  function fetch(handle: RetainedHandle): Promise<RetainedFetchResponse>;

  /** Select a bounded part of a retained result without re-calling upstream.
   *  @param path  RFC 6901 JSON Pointer; "" selects the root (`~1` = "/", `~0` = "~").
   *  @param range Array element indices, or string CHAR indices. Clamps like
   *               Array.prototype.slice. Rejects with retained_value_too_large
   *               rather than returning a partial selection. */
  function slice(
    handle: RetainedHandle,
    path?: string,
    range?: { start?: number; end?: number },
  ): Promise<RetainedSliceResponse>;
}
```

### 3.1 Worked example

Retention triggers at ~24 KB while `fetch` inlines up to 1 MiB, so **the common
case needs no paging loop at all**. Lead with that:

```javascript
// A previous execution returned { result_handle, retained_until }.
const meta = await codemode.fetch(handle);

// Common case: the whole value came back inline. Reduce and return.
if (!meta.value_omitted) {
  return meta.value.items.slice(0, 20).map(({ id, name }) => ({ id, name }));
}

// Large value: page it. A zero-width probe returns source_length, so the loop
// can be sized without any extra response field (see SPEC FR-3.3).
const probe = await codemode.slice(handle, "/items", { start: 0, end: 0 });
const total = probe.source_length ?? 0;
const names = [];
let width = 500;

for (let start = 0; start < total; ) {
  try {
    const page = await codemode.slice(handle, "/items", { start, end: start + width });
    for (const item of page.value) names.push(item.name);
    start += width;
  } catch (e) {
    const kind = JSON.parse(e.message)?.kind;
    if (kind === "retained_value_too_large" && width > 25) {
      width = Math.floor(width / 4);   // narrow and retry the same window
      continue;
    }
    break;                             // expired, exhausted, or gone: use what we have
  }
}
return names.slice(0, 20);             // still reduce before returning
```

The previous version of this example read `meta.array_length` (which is only set
when the **root** is an array) while slicing `/items` (which implies an object
root), so `total` was `0`, the loop never ran, and it returned `[]`. The plan
requires this example to be embedded verbatim in the guidance text, so it has to
be correct — models copy examples more reliably than prose, which is the whole
premise of #217.

## 4. Type-to-schema mapping

| Rust | TypeScript | Schema |
|---|---|---|
| `RetainedMarkerFields` on the marker | — | `code-mode-retained-result-marker.schema.json` |
| fetch handler output | `RetainedFetchResponse` | `code-mode-retained-fetch-response.schema.json` |
| slice handler input | `(handle, path, range)` | `code-mode-retained-slice-request.schema.json` |
| `SliceSelection` | `RetainedSliceResponse` | `code-mode-retained-slice-response.schema.json` |
| `RetainedMiss`, `SliceError` | rejected promise | `code-mode-call-error.schema.json` |

Conformance is asserted by serializing the real types and validating against the
published schemas, so they cannot drift silently.
