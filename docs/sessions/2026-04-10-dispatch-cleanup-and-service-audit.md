# Session: Dispatch Cleanup and Service Compliance Audit

**Date:** 2026-04-10  
**Branch:** `feat/lab-operational`  
**Outcome:** Clean compile, 0 errors, 0 unused-import warnings. All 4 migrated services pass compliance audit.

---

## Session Overview

Two major activities:

1. **Simplify pass** — Three parallel review agents (reuse, quality, efficiency) analyzed the staged diff (~80KB, ~95 files). Fixed duplicate helper functions across dispatch params modules, upgraded `require_str` signature, and eliminated double serialization in `output.rs`.

2. **Service compliance audit** — Three parallel research agents checked whether the four migrated dispatch services (`bytestash`, `radarr`, `sabnzbd`, `unifi`) conform to `docs/DISPATCH.md`, `docs/ERRORS.md`, `docs/SERVICE_ONBOARDING.md`, and `docs/SERVICE_LAYER_MIGRATION.md`. Found and fixed 4 violations.

---

## Timeline

| Time | Activity |
|------|----------|
| Session start | `/simplify` invoked — agents launched against git diff |
| Simplify phase | Agents report: duplicate helpers in 3 params.rs files; double serialization; `request_id` parameter sprawl |
| Simplify fixes | Updated `dispatch/helpers.rs`, rewrote 3 params.rs files as re-exports, fixed `output.rs` render |
| Research phase | `/lavra:lavra-research` invoked — 3 agents audit 4 services against docs |
| Research fixes | Fixed sabnzbd inline params, unifi empty-string guard, bytestash API import path, bytestash MCP dead re-exports |
| Cleanup | Resolved 2 unused-import warnings from the re-export additions |

---

## Key Findings

### From Simplify Agents

- **Duplicate helpers** (`dispatch/radarr/params.rs`, `dispatch/sabnzbd/params.rs`, `dispatch/unifi/params.rs`): `to_json`, `require_str`, `require_i64` each copied 2–4 times. All were byte-identical to functions already in `dispatch/helpers.rs`.
- **`require_str` signature mismatch**: Central `dispatch/helpers.rs` returned owned `String`; per-service copies returned borrowed `&'a str` (more efficient). Upgraded central version to `&'a str`.
- **Double serialization** (`output.rs:59`): `render()` called `serde_json::to_value(value)` then immediately `serde_json::to_string(&value)` for the `Json` path — one full allocation wasted per call.
- **`handle_action` parameter sprawl** (`api/services/helpers.rs`): Adding `request_id: Option<&str>` as a 6th positional param instead of folding into `DispatchContext`. Noted but not fixed (would require broader struct change).

### From Service Audit Agents

- **`dispatch/sabnzbd/dispatch.rs:37-68`**: Inline param coercion (`limit`, `kbps`) in dispatch code — should live in `params.rs`.
- **`dispatch/unifi/client.rs:8-9`**: `std::env::var("UNIFI_URL").ok()?` does not filter empty strings. Set-but-empty env var would pass `""` to `UnifiClient::new` instead of returning `None`.
- **`api/services/bytestash.rs:19`**: `crate::mcp::envelope::ToolError` import path — resolves to same type but creates forbidden `api -> mcp` module dependency.
- **`api/services/bytestash.rs:25`**: `surface: "api"` — all other API handlers use `"http"` per dispatch/CLAUDE.md convention.
- **`mcp/services/bytestash.rs:6`**: `pub use crate::dispatch::bytestash::{ACTIONS, dispatch}` — dead code; `mcp/registry.rs` imports directly from `dispatch::bytestash`, bypassing the MCP shim entirely.

### What Was Clean

- All `From<ServiceError> for ToolError` impls correctly in `dispatch/error.rs`, feature-gated. Zero misplaced conversions.
- No secrets logged in any dispatch path. `auth.login`/`auth.register` in bytestash explicitly documented.
- CLI shims: zero `cli -> mcp` violations across all 4 services.
- Coverage docs for all 4 services exist and were recently updated.
- `confirmation_required` emitted only from `api/services/helpers.rs`, not dispatch internals.

---

## Technical Decisions

- **Re-export rather than delete** for `params.rs`: Per-service domain files (`movies.rs`, `config.rs`, etc.) import `use super::params::{require_str, to_json}`. Using `pub use` in `params.rs` preserves those import paths with zero churn in 15+ domain files.
- **`require_str` upgraded to `&'a str`**: The borrowed version is strictly better — callers using `&id` auto-deref either way. Safe because all callers use the result immediately (no struct lifetime binding).
- **sabnzbd `client_from_env` re-export reverted**: Added for consistency with other services, but no caller uses it (`cli/health.rs` has no sabnzbd health check). Adding dead code was wrong; reverted.
- **`visible_width` ANSI optimization skipped**: Would require threading `RenderContext` through the entire table rendering call stack (~5 functions). Minor optimization, large scope — deferred.
- **`execute_dispatch`/`run_action_command` convergence skipped**: `cli/radarr.rs` has a local `execute_dispatch` that duplicates `cli/helpers.rs::run_action_command`. Requires understanding `CommandOutcome` abstraction first; left for a dedicated radarr CLI cleanup pass.

---

## Files Modified

| File | Change |
|------|--------|
| `crates/lab/src/dispatch/helpers.rs` | `require_str` returns `&'a str`; added `require_i64` |
| `crates/lab/src/dispatch/radarr/params.rs` | Replaced body with `pub use` re-exports from `dispatch::helpers` |
| `crates/lab/src/dispatch/sabnzbd/params.rs` | Replaced body with re-exports; added `require_u64` and `opt_u32` helpers |
| `crates/lab/src/dispatch/sabnzbd/dispatch.rs` | Replaced inline `limit`/`kbps` extraction with `opt_u32`/`require_u64` |
| `crates/lab/src/dispatch/sabnzbd.rs` | (No `client_from_env` re-export — reverted; no caller exists) |
| `crates/lab/src/dispatch/unifi/params.rs` | Removed duplicate `to_json`, `require_str`, `require_i64`; kept service-specific helpers; added `pub use` re-exports |
| `crates/lab/src/dispatch/unifi/client.rs` | Added `.filter(|v| !v.is_empty())` to `UNIFI_URL` and `UNIFI_API_KEY` env reads |
| `crates/lab/src/output.rs` | `render()` now calls `serde_json::to_string(value)` directly for Json path; only converts to `Value` for Human path |
| `crates/lab/src/api/services/bytestash.rs` | Error type: `mcp::envelope::ToolError` → `dispatch::error::ToolError`; surface: `"api"` → `"http"` |
| `crates/lab/src/mcp/services/bytestash.rs` | Removed dead `pub use {ACTIONS, dispatch}`; test now imports from `crate::dispatch::bytestash` directly |

---

## Commands Executed

```bash
# Verify compilation after each change set
rtk cargo check --all-features
# → 0 errors (both times)

# Final check for unused import warnings
cargo check --all-features 2>&1 | grep -E "^(error|warning: unused)"
# → (empty — clean)
```

---

## Behavior Changes (Before/After)

| Surface | Before | After |
|---------|--------|-------|
| `render()` JSON path | `to_value(T)` + `to_string(Value)` — 2 allocations | `to_string(T)` — 1 allocation |
| UniFi client init | Empty `UNIFI_URL=""` accepted, passed to constructor | Empty string treated as unset, returns `None` |
| `api/services/bytestash` | Import path through `mcp::envelope` | Direct path through `dispatch::error` |
| `api/services/bytestash` surface field | `"api"` | `"http"` (matches all other API handlers) |
| `dispatch/sabnzbd` speed-limit error | Inline `MissingParam` construction | Standard `require_u64` helper (same error shape) |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `rtk cargo check --all-features` | 0 errors | 0 errors, 166 warnings (pre-existing) | ✓ PASS |
| `cargo check --all-features \| grep "unused import"` | 0 lines | 0 lines | ✓ PASS |
| Grep `mcp::envelope` in `api/services/` | 0 matches | 0 matches | ✓ PASS |
| Grep `From.*Error.*for ToolError` in `mcp/\|api/` | 0 matches | 0 matches | ✓ PASS |

---

## Decisions Not Taken

- **`DispatchContext` with `request_id` field**: Folding `request_id: Option<&str>` into `DispatchContext` would eliminate the 6-param `handle_action` signature. Not done — `DispatchContext` docs explicitly state it should remain minimal (`surface` + `instance` only until a second proven need). The doc cites this exact decision (`SERVICE_LAYER_MIGRATION.md:168`).
- **`collect_headers` dedup optimization**: Using `IndexSet` instead of `Vec::contains` for O(1) dedup in `output.rs`. Not done — only matters for large tables with many columns; not a hot path in practice.
- **`visible_width` ANSI short-circuit**: Skipping `strip_ansi_escapes::strip_str` when `!ctx.color`. Requires threading `RenderContext` through 5 table helper functions. Deferred.
- **`cli/radarr.rs` `execute_dispatch` consolidation**: Dead-code duplication with `cli/helpers.rs::run_action_command`. Not done — requires understanding `CommandOutcome` struct and how `success_note`/`print_result` differ from the shared helper.

---

## Open Questions

- **MCP/CLI surface observability**: `handle_action` in `api/services/helpers.rs` emits full structured dispatch events. MCP and CLI surfaces have no equivalent wrapper — dispatch logs on those surfaces are absent. Needs a dedicated audit pass.
- **`sabnzbd` health check**: `cli/health.rs` has health probes for radarr, unifi, bytestash but not sabnzbd. Is this intentional (service not reliable enough) or a gap?
- **`dispatch/sabnzbd/catalog.rs` pattern**: Uses a plain `static ACTIONS: &[ActionSpec]` wrapped in a function, where radarr/unifi use `LazyLock` assembly. Functionally identical for a flat catalog but inconsistent. Worth standardizing if a third pattern doesn't emerge.
- **`api/CLAUDE.md` stale guidance**: States "use `crate::mcp::envelope::ToolError`" — contradicts the current dispatch-layer architecture. Should be updated to say `crate::dispatch::error::ToolError`.

---

## Next Steps

- Update `api/CLAUDE.md` to reference `crate::dispatch::error::ToolError` instead of `crate::mcp::envelope::ToolError`
- Add sabnzbd health check to `cli/health.rs` (matches pattern of other 3 services)
- Audit MCP and CLI dispatch instrumentation against `docs/OBSERVABILITY.md`
- Add typed MCP elicitation for destructive actions (currently only HTTP surface gates on `confirm: true`)
