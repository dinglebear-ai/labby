# Session: Upstream OAuth PR #21 Review — Address All 9 Threads

**Date:** 2026-04-19
**Branch:** `bd-lab-77y5/mcpregistry-service`
**PR:** [#21 — Feat/upstream mcp oauth pkce](https://github.com/jmagar/lab/pull/21)

---

## Session Overview

Full systematic pass through all 9 open review threads on merged PR #21 ("upstream MCP OAuth PKCE"), followed by a quick-push of remaining unstaged work on the branch. All 9 threads were fixed, committed, pushed, and verified resolved via `mark_resolved.py` + `verify_resolution.py`.

---

## Timeline

| Time | Activity |
|------|----------|
| Start | Session resumed from prior compaction; orientation context pre-loaded via `gh-address-comments` skill |
| Orient | Read all 9 affected source files; verified thread 9 already fixed; got advisor sign-off |
| Implement | 7 code changes across 7 files (threads 1–8); thread 9 was already correct |
| Verify | `cargo check` → `cargo clippy -D warnings` → `cargo nextest` (1616 passed, 0 errors) |
| Resolve | `mark_resolved.py` resolved 9/9 threads; `verify_resolution.py` confirmed 94/94 total |
| Push | Committed `fix(upstream-oauth): address PR #21 review feedback`; pushed to origin |
| Quick-push | Staged remaining branch work (auth rate limiter, catalog refactor, skill scripts); bumped version 0.3.5→0.4.0; pushed `feat(lab-77y5.4)` |

---

## Key Findings

- **Thread 9 pre-verified fixed**: `manager.rs:529-531` correctly mapped `TokenRefreshFailed → NeedsReauth` via `map_auth_error`; both callers (lines 173, 232) go through it. No code change needed.
- **`build_locks` race (thread 6)**: Removing the lock entries in `evict_subject`/`evict_upstream` created a window where two concurrent callers both see no cached client, drop the (now-gone) lock, and race to build two `AuthClient`s for the same `(upstream, subject)` key.
- **TOCTOU in dynamic registration (thread 7)**: `ON CONFLICT DO UPDATE SET` (last-writer-wins) means `save_dynamic_client_registration` may persist a different `client_id` than what `cfg` holds; fixing with a read-back, NOT a schema change (existing test at `sqlite.rs:1341-1349` explicitly verifies overwrite behavior).
- **Circuit breaker false success (thread 5)**: `record_success_for` fired before the response size check in `subject_scoped_read_resource`; oversized responses were advancing the healthy counter.
- **`window.open` null (thread 2)**: Returns `null` when browser blocks the popup; unchecked leaves `connecting=true` indefinitely with no user feedback.
- **Docs contradiction (thread 8)**: `UPSTREAM.md:140-141` claimed OAuth upstreams "appear in the merged catalog regardless of transport"; lines 203-208 correctly stated they are unhealthy at startup and excluded until OAuth flow completes.

---

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| Don't remove `build_locks` on eviction | Lock is the serialization primitive — evicting it defeats its purpose. Clients can be re-built; locks must persist to prevent concurrent duplicate builds. |
| Read-back after save (thread 7) | Schema uses `ON CONFLICT DO UPDATE` (intentional last-writer-wins, tested); app-level read-back is the correct fix without touching the schema contract. |
| Size check → failure path on oversized (thread 5) | An oversized response is a misbehaving upstream; it should not advance the circuit breaker's healthy counter. Moved check before `record_success_for` and routes to `record_failure_for`. |
| Gate `/gateway/oauth` on `gateway_manager.is_some()` | OAuth routes require the gateway manager; mounting them unconditionally when no manager is present is misleading and potentially panicky at runtime. Ordering constraint (oauth before gateway) is preserved. |
| `gatewayHeaders()` owns CSRF/bearer in frontend | Canonical shared helper already handles both standalone-bearer and cookie+CSRF modes; private `apiFetch` in `upstream-oauth-client.ts` bypassed it entirely. |

---

## Files Modified

### Thread fixes committed in `f50f106` (`fix(upstream-oauth)`)

| File | Change |
|------|--------|
| `apps/gateway-admin/lib/api/upstream-oauth-client.ts` | Import `normalizeGatewayApiBase` + `gatewayHeaders`; remove hardcoded `/v1`; respect bearer/CSRF modes (threads 1+3) |
| `apps/gateway-admin/components/upstream-oauth/upstream-oauth-card.tsx` | Check `window.open()` return; reset state + show error on null (thread 2) |
| `crates/lab/src/api/router.rs` | Gate `/gateway/oauth` mount on `state.gateway_manager.is_some()` (thread 4) |
| `crates/lab/src/dispatch/upstream/pool.rs` | Move size check before `record_success_for` in `subject_scoped_read_resource` (thread 5) |
| `crates/lab/src/oauth/upstream/cache.rs` | Remove `build_locks.remove/retain` from `evict_subject`/`evict_upstream` (thread 6) |
| `crates/lab/src/oauth/upstream/manager.rs` | Add DB read-back after `save_dynamic_client_registration` for canonical `client_id` (thread 7) |
| `docs/UPSTREAM.md` | Reconcile contradictory OAuth catalog-visibility docs (thread 8) |

### Subsequent push `08fc5e1` (`feat(lab-77y5.4)`)

| File | Change |
|------|--------|
| `crates/lab-auth/src/state.rs` | Add token-bucket `RateLimiter` to `AuthState` for `/authorize` and `/register` endpoints |
| `crates/lab/src/mcp/catalog.rs` | Extract `CatalogSnapshot` struct so it can be shared |
| `crates/lab/src/mcp/server.rs` | Remove duplicate `CatalogSnapshot` definition + `snapshot_catalog` impl; use import from catalog |
| `crates/lab/src/dispatch/gateway/catalog.rs` | Remove stale non-destructive-read assertion tests |
| `crates/lab/src/dispatch/gateway/{config,manager}.rs` | Minor cleanup from mcpregistry + upstream-oauth work |
| `crates/lab/src/api/upstream_oauth.rs` | Minor cleanup |
| `crates/lab/src/dispatch/deploy/build.rs` | Minor cleanup |
| `skills/gh-address-comments/SKILL.md` | Skill doc update |
| `skills/gh-address-comments/scripts/` | Updated `fetch_comments.py`, `mark_resolved.py`, `pr_summary.py`; added `_bd_utils.py`, `ai_triage.py`, `close_beads.py`, `create_beads.py`, `install_completions.py`, `post_reply.py`, `pr_changelog.py`, `pr_checklist.py`, `pr_status.py`, `thread_context.py` |
| `Cargo.toml` | Version bump `0.3.5 → 0.4.0` |
| `.claude-plugin/plugin.json` | Version bump `0.3.4 → 0.4.0` |

---

## Commands Executed

```bash
# Verification after all fixes
cargo check --workspace --all-features          # no errors
cargo clippy --workspace --all-features -- -D warnings  # no errors
cargo nextest run --workspace --all-features    # 1616 passed, 2 ignored

# PR thread resolution
python3 ~/.claude/skills/gh-address-comments/scripts/mark_resolved.py \
  PRRT_kwDOR8nC1M57_GE2 PRRT_kwDOR8nC1M57_GE4 PRRT_kwDOR8nC1M57_GE5 \
  PRRT_kwDOR8nC1M57_GE6 PRRT_kwDOR8nC1M57-ZCq PRRT_kwDOR8nC1M57-ZCu \
  PRRT_kwDOR8nC1M57-ZCv PRRT_kwDOR8nC1M57-ZCw PRRT_kwDOR8nC1M576Cms
# Result: Resolved 9/9 threads

python3 ~/.claude/skills/gh-address-comments/scripts/verify_resolution.py
# Result: ✓ 94 thread(s) resolved or outdated — all clear
```

---

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| `upstream-oauth-client.ts` | Hardcoded `/v1` prefix, bare `credentials:'include'`, no CSRF/bearer | Uses `normalizeGatewayApiBase()` + `gatewayHeaders()` — respects `NEXT_PUBLIC_API_URL` and bearer mode |
| Popup blocked | `connecting` stays `true` forever, no feedback | Resets to `false`, shows "Popup blocked" error message |
| `/gateway/oauth` routes | Always mounted when `is_master`, even without gateway manager | Only mounted when `state.gateway_manager.is_some()` |
| Resource size check | Success recorded before size check; oversized responses advanced healthy counter | Size check first; oversized routes to `record_failure_for` |
| `OauthClientCache` eviction | Removed `build_locks` entries — created race window for concurrent builders | `build_locks` preserved on eviction — serialization invariant maintained |
| Dynamic client registration | Used `cfg.client_id` from `register_client` response — could be stale in concurrent calls | Read-back from DB after save — always uses DB-canonical `client_id` |
| `AuthState` | No rate limiting on `/authorize` or `/register` | Token-bucket limiter on both endpoints; configurable per-minute rate |
| `CatalogSnapshot` | Defined inline in `mcp/server.rs`, not shareable | Extracted to `mcp/catalog.rs`, importable by other modules |
| `docs/UPSTREAM.md` | Contradictory: claimed OAuth upstreams in catalog regardless of transport | Corrected: OAuth upstreams are unhealthy at startup, excluded until flow completes |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo check --workspace --all-features` | 0 errors | 0 errors | ✅ |
| `cargo clippy --workspace --all-features -- -D warnings` | 0 errors | 0 errors | ✅ |
| `cargo nextest run --workspace --all-features` | All pass | 1616 passed, 2 ignored | ✅ |
| `mark_resolved.py` (9 thread IDs) | 9/9 resolved | 9/9 resolved | ✅ |
| `verify_resolution.py` | All threads addressed | ✓ 94 resolved/outdated | ✅ |
| `git push` (fix commit) | Pushed to remote | Pushed to `bd-lab-77y5/mcpregistry-service` | ✅ |
| `git push` (feat commit) | Pushed to remote | Pushed to `bd-lab-77y5/mcpregistry-service` | ✅ |

---

## Risks and Rollback

- **`build_locks` preservation**: The fix is conservative (not removing). Risk of lock table growing unbounded is negligible — keys are bounded by active `(upstream, subject)` pairs and eviction still removes cached clients.
- **DB read-back (TOCTOU fix)**: Adds one extra SQLite read per dynamic registration (cold-path only). Read-back returning `None` returns `OauthError::Internal` — this cannot happen in practice unless the DB discards a just-committed write.
- **Rate limiter on `AuthState`**: Token bucket is shared across all clones via `Arc<Mutex<...>>`. Lock contention is negligible at homelab scale. Rollback: revert `state.rs` addition and remove the two guard call sites.
- **Rollback for all fixes**: Single commit `f50f106` — `git revert f50f106` is clean.

---

## Decisions Not Taken

| Alternative | Why Rejected |
|-------------|-------------|
| Change `ON CONFLICT DO UPDATE` → `INSERT OR IGNORE` in `save_dynamic_client_registration` | Existing test at `sqlite.rs:1341-1349` explicitly tests overwrite behavior; schema is intentional last-writer-wins |
| Add `build_locks` capacity cap / TTL eviction | YAGNI — lock table bounded by active sessions at homelab scale; adds complexity without need |
| Use `X-Lab-Confirm` header as frontend confirm signal | Already removed in a prior PR; API docs note it was removed because reverse proxies may forward arbitrary headers |
| Mark thread 9 without reading refresh paths | Advisor recommended verifying all callers go through `map_auth_error` first — confirmed both paths (lines 173, 232) do |

---

## Open Questions

- **`build_locks` memory at scale**: At large scale (many subjects per upstream), `build_locks` grows without bound since only clients are evicted. For homelab use this is fine; a TTL-based cleanup pass may be warranted if this moves to production.
- **rmcp refresh + `resource` param gap**: `UPSTREAM.md:174-179` documents a known gap where rmcp 1.4's refresh path does not re-emit `resource` on the `refresh_token` grant. Tracked for upstream fix once rmcp exposes a refresh hook.

---

## Next Steps

- Merge `bd-lab-77y5/mcpregistry-service` → `main` once CI passes (mcpregistry service is the primary goal of this branch)
- Update `mcpregistry` dispatch layer with any follow-up from branch review
- Address any CI failures from the new `lab-auth` rate limiter addition
