---
date: 2026-04-22 07:14:47 EST
repo: git@github.com:jmagar/lab.git
branch: feat/gateway-chat-registry-log-ui
head: 120bf6a
plan: none
agent: Claude (claude-sonnet-4-6)
session id: 9e6f5594-d44c-400f-b4c8-161951bf7662
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/9e6f5594-d44c-400f-b4c8-161951bf7662.jsonl
working directory: /home/jmagar/workspace/lab
pr: "#27 feat(gateway-admin): chat UI + Aurora token sweep — v0.7.0/0.7.1 (https://github.com/jmagar/lab/pull/27)"
---

## User Request

Resume the lavra-work multi-bead execution for epic `lab-fstf` (MCP subregistry implementation),
completing the remaining bead fixes and wiring the gateway-admin UI to the local REST registry
endpoint at `GET /v0.1/servers`.

## Session Overview

Continued a multi-session lavra-work run on epic `lab-fstf`. The previous session had performed
pre-flight build fixes and completed several Rust bead implementations. This session finished
the remaining beads: store-layer correctness fixes, SSRF hardening, shared sync guard extraction,
error-kind normalization, and wiring the gateway-admin UI away from the MCP action-dispatch path
and onto the local REST endpoint. All changes committed in 7 atomic commits.

## Sequence of Events

1. Read in-flight state from session summary: identified which beads were done vs. pending.
2. Confirmed `labs-fstf.1` (race-free `update_is_latest_sync`), `.3` (UTF-8 truncation), `.4` (WAL
   pragma) were already correct in the codebase — no changes needed.
3. Fixed `crates/lab/src/log_fmt/formatter.rs`: jiff 0.2.x `strftime()` returns `Display` not
   `Result`; removed the stale `.map()/.unwrap_or_else()` wrapping.
4. Removed `chrono.workspace = true` from `crates/lab/Cargo.toml`; feature-gated
   `rusqlite`, `r2d2`, `r2d2_sqlite` under the `mcpregistry` feature using `dep:` syntax.
5. Fixed `crates/lab/src/dispatch/mcpregistry/store.rs`:
   - Replaced `chrono::Utc::now().to_rfc3339()` with `jiff::Timestamp::now().to_string()`.
   - Replaced `INSERT OR REPLACE` with `INSERT ... ON CONFLICT(server_name, version) DO UPDATE SET`.
   - Added `PRAGMA journal_mode = WAL` to the `with_init()` connection callback.
   - Introduced `truncate_utf8_bytes()` for char-boundary-safe truncation; replaced naive `s[..512]` slice.
   - Removed `SortBy`/`SortSpec` from `StoreListParams`; hardcoded sort as `server_name ASC, version ASC`.
6. Fixed `crates/lab/src/dispatch/mcpregistry/params.rs`: added Tailscale CGNAT `100.64.0.0/10`
   to the IPv4 SSRF blocklist (and its IPv6-mapped-v4 equivalent `::ffff:100.64.0.0/10`).
7. Created `crates/lab/src/dispatch/mcpregistry/sync.rs`: extracted `SYNC_IN_PROGRESS`,
   `LAST_SYNC_AT`, `SyncGuard`, and `perform_sync()` so both the background supervisor
   (`cli/serve.rs`) and the MCP dispatch path share the same atomic state.
8. Updated `dispatch.rs` to import and use `sync::perform_sync()`; updated `serve.rs` to use
   same; background supervisor now emits a `warn` event when client construction fails.
9. Fixed `registry_v01.rs`: replaced ad-hoc `StatusCode + Json` error tuples with `ToolError::Sdk`;
   normalized `sync_in_progress` → `service_unavailable`, `validation_failed` → `invalid_param`.
   Also added `owner` query param translated to `io.github.{owner}/` search prefix.
10. Modified `apps/gateway-admin/lib/api/mcpregistry-client.ts`: replaced `listServers()` MCP
    action dispatch with a direct `GET /v0.1/servers` fetch, adapting the response shape.
11. Ran `cargo check --workspace --all-features` and `tsc --noEmit` — both clean.
12. Created 7 atomic commits covering all bead work.

## Key Findings

- `crates/lab/src/log_fmt/formatter.rs`: jiff `Zoned::now().strftime(fmt)` in 0.2.x returns a
  `StrftimeConfig` (implements `Display`), not a `Result`; prior `.map(|f| f.to_string())` was
  dead code that happened to compile.
- `crates/lab/src/dispatch/mcpregistry/store.rs:522`: `INSERT OR REPLACE` deletes + re-inserts
  when a UNIQUE constraint fires, which resets the ROWID and can violate foreign key refs; `ON
  CONFLICT DO UPDATE` updates in-place without touching ROWIDs.
- `crates/lab/src/dispatch/mcpregistry/store.rs:130`: `PRAGMA journal_mode = WAL` cannot be issued
  inside a transaction; must live in the `with_init()` connection callback.
- `crates/lab/src/dispatch/mcpregistry/params.rs`: Tailscale CGNAT range `100.64.0.0/10` was
  absent from the RFC1918+loopback blocklist; a server URL pointing at a Tailscale node would have
  bypassed SSRF protection.
- `apps/gateway-admin/lib/api/mcpregistry-client.ts`: the MCP `server.list` action explicitly
  rejects `sort_by`/`order` params; the REST endpoint at `registry_v01.rs` does not support them
  either — sort is now hardcoded server-side and treated as client-side concern in the UI.
- `crates/lab/src/dispatch/mcpregistry/dispatch.rs` and `cli/serve.rs` previously each owned
  their own copy of `SYNC_IN_PROGRESS`; they are different `AtomicBool` instances, making the
  concurrent-sync guard ineffective across callers.

## Technical Decisions

- **`ON CONFLICT DO UPDATE` over `INSERT OR REPLACE`**: Preserves ROWIDs (REPLACE = delete+insert),
  avoids double-counting in return values, and keeps any future FK references stable.
- **`perform_sync(rate_limit: bool)`**: Single function with a flag rather than two separate
  functions. The background supervisor passes `false` (its own timer controls frequency);
  on-demand MCP calls pass `true` (60 s minimum enforced).
- **`listServers()` replaced entirely** (not wrapped): only one call site in `use-registry.ts`;
  clean cut without a compatibility shim. The REST shape adaptation (`ServerJSON[]` → 
  `ServerResponse[]`) is self-contained in the function.
- **`_meta: null` in adapted REST response**: The local SQLite cache does not surface registry
  extension data (`io.modelcontextprotocol.registry/official`); `_meta: null` propagates cleanly
  — UI falls back to `status: 'active'` via `?? 'active'`.
- **Error kind `service_unavailable`** (not `sync_in_progress`) when the store isn't ready:
  aligns with the canonical ToolError vocabulary in `docs/ERRORS.md`.

## Files Modified

| File | Purpose |
|------|---------|
| `crates/lab/src/log_fmt/formatter.rs` | Replace chrono with jiff; fix strftime() API |
| `crates/lab/Cargo.toml` | Remove chrono dep; feature-gate rusqlite/r2d2 |
| `crates/lab/src/dispatch/mcpregistry/store.rs` | ON CONFLICT, jiff, WAL pragma, UTF-8 truncation |
| `crates/lab/src/dispatch/mcpregistry/params.rs` | SSRF: add Tailscale CGNAT range |
| `crates/lab/src/dispatch/mcpregistry/sync.rs` | **NEW** — shared sync guard state |
| `crates/lab/src/dispatch/mcpregistry.rs` | Add `pub mod sync;` |
| `crates/lab/src/dispatch/mcpregistry/dispatch.rs` | Use `sync::perform_sync()`; fix error kinds |
| `crates/lab/src/cli/serve.rs` | Use `sync::perform_sync()`; warn on client init failure |
| `crates/lab/src/api/services/registry_v01.rs` | Normalize error kinds; add owner filter |
| `apps/gateway-admin/lib/api/mcpregistry-client.ts` | Wire `listServers()` to `GET /v0.1/servers` |

## Commands Executed

```bash
# Build verification (all features)
cargo check --workspace --all-features
# → 0 errors, 0 warnings

# TypeScript check
tsc --noEmit
# → 0 errors in project files (2 pre-existing errors in mock-data.ts unrelated to this work)

# Atomic commits
git add crates/lab/src/log_fmt/formatter.rs
git commit -m "fix(log_fmt): replace chrono with jiff for timestamp formatting"

git add crates/lab/Cargo.toml
git commit -m "fix(lab): remove chrono dep, feature-gate rusqlite/r2d2 under mcpregistry"

git add crates/lab/src/dispatch/mcpregistry/store.rs
git commit -m "fix(mcpregistry/store): ON CONFLICT DO UPDATE, jiff, WAL, UTF-8 truncation"

git add crates/lab/src/dispatch/mcpregistry/params.rs
git commit -m "fix(mcpregistry/params): add Tailscale CGNAT range to SSRF blocklist"

git add crates/lab/src/dispatch/mcpregistry/sync.rs crates/lab/src/dispatch/mcpregistry.rs \
         crates/lab/src/dispatch/mcpregistry/dispatch.rs crates/lab/src/cli/serve.rs
git commit -m "fix(mcpregistry): extract shared sync guards to dispatch/mcpregistry/sync.rs"

git add crates/lab/src/api/services/registry_v01.rs
git commit -m "fix(registry_v01): normalize error kinds; add owner filter; use ToolError uniformly"

git add apps/gateway-admin/lib/api/mcpregistry-client.ts
git commit -m "feat(gateway-admin): wire listServers to GET /v0.1/servers REST endpoint"
```

## Errors Encountered

| Error | Root Cause | Resolution |
|-------|-----------|------------|
| `jiff strftime() .map().unwrap_or_else()` compile error | jiff 0.2.x `strftime()` returns `Display`, not `Result` | Removed `.map()/.unwrap_or_else()`, called `.to_string()` directly |
| TS2339 on error body union type | `.catch(() => ({}))` returns `{}` unioned with the typed shape; accessing `.message` fails | Typed the catch fallback: `.catch((): { message?: string; kind?: string } => ({}))` |
| `cargo check` package ambiguity with `-p lab` | Two `lab` packages in workspace (0.7.2 and 0.11.0) | Used `--workspace --all-features` instead |

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| Registry server list (gateway-admin) | POST `/v1/mcpregistry` with `action=server.list` | GET `/v0.1/servers` (local SQLite-backed cache) |
| Concurrent sync guard | Each caller (MCP dispatch + background supervisor) had its own `AtomicBool` — guard was ineffective | Single shared `SYNC_IN_PROGRESS` in `sync.rs` prevents concurrent SQLite writes |
| SSRF blocklist | Missing Tailscale CGNAT `100.64.0.0/10` | Range covered; Tailscale node URLs rejected |
| SQLite upsert | `INSERT OR REPLACE` — deletes + re-inserts on conflict, resets ROWID | `ON CONFLICT DO UPDATE` — in-place update, ROWID preserved |
| WAL mode | Set inside a transaction (silently a no-op or error depending on driver) | Set in `with_init()` callback, outside any transaction |
| Error kinds (REST) | `sync_in_progress`, `validation_failed` | `service_unavailable`, `invalid_param` (canonical vocabulary) |
| `rusqlite` / `r2d2` deps | Always linked, even in builds without `mcpregistry` feature | Feature-gated under `mcpregistry`; omitted from other feature slices |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check --workspace --all-features` | 0 errors | 0 errors, 0 warnings | ✅ |
| `tsc --noEmit` (mcpregistry-client.ts) | 0 errors | 0 errors | ✅ |
| `git log --oneline -7` | 7 new commits | 7 commits from `281dfbd` to `861e4e8` | ✅ |

## Risks and Rollback

- **SQLite `ON CONFLICT DO UPDATE`**: Requires SQLite ≥ 3.24 (2018). `r2d2_sqlite` bundles SQLite
  3.47+, so this is safe. Rollback: revert `store.rs` to `INSERT OR REPLACE`.
- **Shared `SYNC_IN_PROGRESS`**: Both callers now share state; if either panics during sync, the
  `SyncGuard` RAII drop resets the flag. The 60 s rate-limit only applies to on-demand callers.
- **REST endpoint change in gateway-admin**: If the backend `GET /v0.1/servers` is not mounted or
  the store is uninitialized, the UI gets a `service_unavailable` error. The MCP action-dispatch
  fallback is gone. Rollback: revert `mcpregistry-client.ts` to the `registryAction('server.list')` call.

## Decisions Not Taken

- **Keep client-side sort (`SortBy`/`SortSpec`)**: Sort was removed from `StoreListParams` because
  the REST endpoint doesn't support it and the UI comment says sort is client-side. Could be
  re-added if server-side sort becomes a requirement.
- **Surface `_meta` from local cache**: The SQLite cache stores `is_latest`, `status`, and
  `upstream_updated_at` which could populate `ResponseMeta`. Deferred — the UI gracefully defaults
  to `status: 'active'` via the null-coalescing in `RegistryStatusBadge`.
- **Add sort params to `registryServersKey` (lab-fstf.8)**: Determined to be a no-op — sort is
  client-side in the component (`// sort is client-side so no debounce needed`) and neither
  the REST nor MCP endpoint supports it. Bead is effectively satisfied as-is.

## Next Steps

### Unfinished (started, not completed)
- None — all targeted beads fully implemented and committed.

### Follow-on tasks
- **lab-fstf.8 closure**: Confirm the bead can be closed as `already_satisfied` — sort is
  client-side; no cache key changes needed.
- **`_meta` population from local store**: `RegistryStatusBadge` always shows 'active' for
  locally-cached servers. If `status` and `is_latest` from SQLite should drive badge rendering,
  the REST endpoint response shape needs to be extended.
- **Integration smoke test**: Start `lab serve` and exercise `GET /v0.1/servers` end-to-end with
  the gateway-admin UI to confirm pagination (`next_cursor`), search, and error states work
  against real data.
- **Push and merge**: Branch has many uncommitted changes outside this session's scope (chat UI,
  gateway refactors, etc.); a full commit sweep and PR update is needed before merge.
