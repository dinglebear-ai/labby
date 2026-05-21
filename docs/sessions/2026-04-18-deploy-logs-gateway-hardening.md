# Session: Deploy Runner Hardening, Logs Fleet Ingestion, Gateway Auth

**Date:** 2026-04-18  
**Branch:** `wip/logs-origin-main`  
**Commits this session:** `20de079`, `3fb1054`, `cee722e`

---

## Session Overview

Three major areas of work completed:

1. **Fleet syslog ingestion** — peer nodes can now forward their local journal/syslog to the master via `POST /v1/logs/ingest`; `lab logs forward` CLI command added; `LogQuery` extended with `source_node_ids`/`source_kinds` filters.
2. **Deploy service hardening** — `DefaultRunner` runner refactored with `_impl` inherent methods to fix HRTB Send constraint (Rust #100013); `dispatch_mcp` boxed-future entry point added; `rollback_impl` parallelized; `shell_single_quote` duplicate removed in favour of shared `lab_apis::core::ssh::shell_quote`.
3. **Gateway form auth UX** — bearer token UI redesigned with auth-mode/auth-source two-level RadioGroup; `GatewayWriteConfig` type extracted; env names prefixed `LAB_GW_`; accessibility regression (`<label>` → `<div>`) fixed.

---

## Timeline

| Step | Activity |
|------|----------|
| 1 | Implemented `PeerIngestRequest`/`PeerIngestResponse` types and `source_node_ids`/`source_kinds` on `LogQuery` |
| 2 | Added `POST /v1/logs/ingest` endpoint gated on `LAB_LOGS_INGEST_ENABLED=true` |
| 3 | Implemented `lab logs forward` CLI command — journald-first, syslog fallback, channel+timer batch flush |
| 4 | Committed syslog ingestion work (`20de079`) |
| 5 | Executed deploy plan from `docs/superpowers/plans/2026-04-18-deploy-implementation-v2.md` |
| 6 | Deploy runner stages (`preflight`, `transfer_and_install`, `restart`, `verify`) implemented in `runner.rs` |
| 7 | HRTB Send fix: split trait methods into `plan_impl`/`run_impl`/`rollback_impl` inherent methods; added `dispatch_mcp` static-future entry point |
| 8 | `/simplify` pass 1: identified `shell_single_quote` duplicate, `<label>` regression, sequential rollback |
| 9 | Fixed all three: removed duplicate, restored `<label>`, parallelized rollback |
| 10 | Committed hardening work (`cee722e`), bumped `0.3.4 → 0.3.5`, pushed |

---

## Key Findings

- **HRTB Send bug (Rust #100013):** `Box::pin` in MCP registry requires `'static`-bounded futures; `&self`-capturing RPITIT futures across `.await` fail the higher-ranked check. Fix: all synchronous `&self` access extracted before any `await`, results moved into `async move` blocks owning no borrows of `self`.
- **`shell_single_quote` duplicate:** `runner.rs:138` had an identical implementation to `lab_apis::core::ssh::shell_quote`. The runner already imported `SshHostTarget`/`SshSession` from the same module — just needed to add `shell_quote` to the import.
- **`<label>` accessibility regression:** Changeset switched RadioGroup item wrappers from `<label>` to `<div>`, breaking click-to-select and screen reader association. Reverted in `gateway-form-dialog.tsx:499-539`.
- **Sequential rollback:** `rollback_impl` used a plain `for` loop while `run_impl` used `buffer_unordered`. Fixed by converting to `stream::iter().map(...).buffer_unordered(max_parallel)`.
- **Per-request env read:** `ingest_peer_events` called `std::env::var("LAB_LOGS_INGEST_ENABLED")` on every request. Fixed with `OnceLock<bool>` in `dispatch/logs/client.rs::is_ingest_enabled()`.

---

## Technical Decisions

- **`dispatch_mcp` separate from `dispatch_with_runner`:** Duplication is unavoidable — MCP needs a `Pin<Box<dyn Future + 'static>>` while CLI/HTTP can use a plain `async fn`. Attempting to unify causes lifetime errors.
- **`LAB_GW_` prefix for gateway bearer env names:** Prevents collision with user-defined env vars; makes auto-generated names identifiable as lab-managed.
- **`rollback_impl` uses `effective_max_parallel` from config:** Rollback has no per-call `max_parallel` field in `DeployRequest`; falls back to the same config default used by `run_impl`.
- **`estimate_free_bytes` fallback stays `u64::MAX`:** Returning an error would break builds on systems where `df` is unavailable (containers, restricted sandboxes). Added `tracing::warn!` for visibility while preserving best-effort behavior.
- **Journald bridge via `spawn_blocking` + `mpsc` channel:** Blocking `BufReader` on journald stdout can't `select!` on a flush timer in async context. Bridge: blocking reader → channel → async loop with `interval(2s)` flush.

---

## Files Modified

### New Files
| File | Purpose |
|------|---------|
| `crates/lab/src/dispatch/logs/forward.rs` | Extracted journald/syslog forward logic from `cli/logs.rs` |
| `crates/lab-apis/src/core/ssh.rs` (extended) | Added `shell_quote()`, `reap_and_fail!` macro, shell-quoted `run_command` |
| `docs/DEPLOY_SERVICE.md` | Operator documentation for deploy service |

### Modified Files
| File | Change |
|------|--------|
| `crates/lab/src/dispatch/deploy/runner.rs` | `_impl` split, `systemctl_argv`, `normalize_arch`, `shell_quote` import, parallel rollback |
| `crates/lab/src/dispatch/deploy/dispatch.rs` | Added `dispatch_mcp` boxed-future entry point |
| `crates/lab/src/dispatch/deploy/build.rs` | POSIX `df -k`, `expected_artifact_path_for` cross-platform |
| `crates/lab/src/dispatch/deploy/catalog.rs` | Added `ParamSpec` entries to all actions |
| `crates/lab/src/dispatch/logs/client.rs` | Added `is_ingest_enabled()` with `OnceLock` |
| `crates/lab/src/api/services/logs.rs` | Rate-limited vs fatal error separation; `is_ingest_enabled()` |
| `crates/lab/src/api/error.rs` | Deploy error kinds → HTTP status codes |
| `crates/lab/src/cli/logs.rs` | Use `env_non_empty`, delegate forward to `dispatch/logs/forward.rs` |
| `apps/gateway-admin/lib/types/gateway.ts` | Extracted `GatewayWriteConfig` type |
| `apps/gateway-admin/lib/gateway-env.ts` | `LAB_GW_` prefix on auto-generated env names |
| `apps/gateway-admin/components/gateway/gateway-form-dialog.tsx` | Auth mode/source RadioGroup UI; `<label>` restored |
| `Cargo.toml` | Version `0.3.4 → 0.3.5` |

---

## Commands Executed

```bash
# Verify pre-existing Send errors (not introduced by changes)
rtk cargo check --all-features  # 2 pre-existing errors in dispatch/deploy/dispatch.rs

# Compile check after all simplify fixes
rtk cargo check --all-features  # same 2 pre-existing errors, no regressions

# Push
rtk git push origin wip/logs-origin-main
```

---

## Behavior Changes (Before/After)

| Surface | Before | After |
|---------|--------|-------|
| `POST /v1/logs/ingest` | Route didn't exist | Accepts peer log batches when `LAB_LOGS_INGEST_ENABLED=true`; returns 429 when queue full |
| `lab logs forward` | Command didn't exist | Reads journald or `/var/log/syslog`, batches to master with 2s flush timer |
| `LogQuery` | No node/kind filters | `source_node_ids` and `source_kinds` filter peer events |
| `rollback_impl` | Sequential per-host loop | Parallelized via `buffer_unordered(max_parallel)` |
| `ingest_peer_events` | `std::env::var` call per request | `OnceLock<bool>` — reads env once, caches forever |
| Gateway RadioGroup | `<div>` wrapper (broken click) | `<label>` wrapper (click-to-select works) |
| Gateway bearer env names | `GITHUB_AUTH_HEADER` | `LAB_GW_GITHUB_AUTH_HEADER` |
| `shell_single_quote` | Duplicate local fn in runner.rs | Removed; uses `lab_apis::core::ssh::shell_quote` |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check --all-features` | Only pre-existing Send errors | 2 pre-existing Send errors | ✅ |
| `git push` | Branch pushed to remote | `ok wip/logs-origin-main` | ✅ |
| `grep shell_single_quote runner.rs` | No matches | No matches | ✅ |
| `grep 'LAB_LOGS_INGEST_ENABLED' api/services/logs.rs` | No inline env reads | No matches | ✅ |

---

## Risks and Rollback

- **`LAB_GW_` prefix change** is a breaking change for any existing gateways whose `bearer_token_env` was auto-generated with the old `*_AUTH_HEADER` format. Existing gateways using a manually-entered env name are unaffected.
  - **Rollback:** Revert `apps/gateway-admin/lib/gateway-env.ts:36`.
- **Rollback parallelization:** If per-host ordering was relied upon (unlikely for rollback), sequential behavior can be restored by reverting `runner.rs` rollback loop.
- **`OnceLock` for ingest flag:** `LAB_LOGS_INGEST_ENABLED` can no longer be toggled at runtime without restarting. This is intentional (feature flags should be startup-time).

---

## Decisions Not Taken

- **`reap_and_fail!` macro → helper function:** The macro uses `return Err(...)` which can't be expressed as a regular function. Kept as macro.
- **`effective_*` methods → parameterized helper:** Four near-duplicate methods in `DefaultRunner`. A macro/generic would reduce lines but add complexity. Deferred.
- **`authMode`/`authSource` → union type in React:** Valid refactor but would change the form reset logic significantly. Deferred.
- **`estimate_free_bytes` returning `Err` on df failure:** Would break builds in containers. Kept `Ok(u64::MAX)` fallback with warning log.

---

## Open Questions

- The two pre-existing `Send` errors in `dispatch/deploy/dispatch.rs` are around `&'static DefaultRunner` HRTB. They appear to be Rust compiler limitations — need to investigate whether a workaround exists or if they'll resolve in a future stable release.
- `--source-node-id` / `--source-kind` CLI flags for `lab logs local search` are mentioned in `LOCAL_LOGS.md` as "not yet wired to CLI args". These remain unwired.
- `DeployRunner::plan` and `DeployRunner::rollback` trait methods remain via delegation to `_impl`. The `config_list` returns sync `Result` but the trait declares it `async` — inconsistency worth cleaning up.

---

## Next Steps

- Wire `--source-node-id` and `--source-kind` to `LogQuery` in `cli/logs.rs` LocalSearchArgs
- Resolve the two pre-existing HRTB Send errors in `dispatch_mcp`
- Clean up `config_list` sync/async inconsistency in `DeployRunner` trait
- Add `DeployError::Aborted` variant for fail-fast skipped hosts (currently uses stringly-typed `"aborted"`)
- Open PR for `wip/logs-origin-main` → `main`
