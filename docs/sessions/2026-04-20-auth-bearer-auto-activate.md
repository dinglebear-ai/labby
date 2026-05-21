# Session: Bearer Auth Auto-Activation

**Date:** 2026-04-20  
**Branch:** fix/auth  
**Author:** Jacob Magar

---

## Session Overview

Removed the requirement to set `NEXT_PUBLIC_STANDALONE_BEARER_AUTH=true` alongside `NEXT_PUBLIC_API_TOKEN`. Bearer mode now activates automatically whenever `NEXT_PUBLIC_API_TOKEN` is set. The practical motivation was enabling browser automation tools (e.g. Chrome DevTools / Playwright) to authenticate against the gateway admin UI without disabling OAuth.

---

## Timeline

1. **Auth system audit** — Inspected the full auth stack: Rust `Auth` enum (`lab-apis/src/core/auth.rs`), axum middleware (`router.rs`), TypeScript client (`auth-mode.ts`, `gateway-request.ts`).
2. **Server-side finding** — Confirmed `authenticate_request` in `router.rs:153` already tries static bearer → OAuth JWT → session cookie in priority order. No server changes needed.
3. **Fail-closed behavior noted** — A bad bearer token short-circuits session fallback (router.rs:232–236). Deliberate security design; left unchanged.
4. **Root cause identified** — `isStandaloneBearerAuthMode` required both `NEXT_PUBLIC_API_TOKEN` **and** `NEXT_PUBLIC_STANDALONE_BEARER_AUTH=true`. Setting only the token silently fell through to session auth.
5. **Implementation** — Simplified `isStandaloneBearerAuthMode` to check token only. Updated `logs-stream.ts` guard. Updated tests.
6. **Verification** — Ran test suite; confirmed same 4 pre-existing failures, no new failures introduced.

---

## Key Findings

- `router.rs:153–309` (`authenticate_request`) already supports bearer + session as parallel alternatives — the server was never the problem.
- `router.rs:232–236`: when a bearer token is present but **invalid**, the server returns 401 immediately without falling through to session cookie auth. This is intentional fail-closed behavior.
- `auth-mode.ts:11–15` was the gating point: `isStandaloneBearerAuthMode` ANDed the token check with `hasStandaloneBearerAuthOverride`, requiring an explicit second env var.
- `logs-stream.ts:27`: EventSource cannot send `Authorization` headers; the guard preventing bearer mode there is still correct. Changed from auto-detecting via `isStandaloneBearerAuthMode` to checking `options?.standaloneBearerAuth === true` explicitly.
- 4 pre-existing test failures exist (unrelated to this change): `log timeline` UI test, `buildGatewayEndpointPreview` gateway test, `connectLogStream malformed frames` flaky timing test, `logoutBrowserSession` state test.

---

## Technical Decisions

- **Dropped `NEXT_PUBLIC_STANDALONE_BEARER_AUTH` entirely** rather than keeping it as an opt-out. The env var was only needed to distinguish "I have a token for some other reason" from "I want bearer mode" — but in practice, if a token is configured, bearer mode is always the intent.
- **Kept `credentials: 'omit'` in bearer mode** (`gateway-request.ts:36`). When a bearer token is present, cookies are suppressed. This prevents confusion where both a token and session cookie would be sent on the same request.
- **`connectLogStream` guard made explicit-only** (`logs-stream.ts:21`). Rather than auto-detecting bearer mode from the env var, the throw now only fires when the caller passes `standaloneBearerAuth: true`. This means calling `connectLogStream` without options works normally (uses session cookies via `withCredentials: true`) even when `NEXT_PUBLIC_API_TOKEN` is set — graceful degradation instead of a crash.
- **Did not change server-side fail-closed behavior.** Allowing a bad bearer token to fall through to session auth would mask broken config silently. Left `router.rs:232–236` unchanged.

---

## Files Modified

| File | Purpose |
|------|---------|
| `apps/gateway-admin/lib/auth/auth-mode.ts` | Removed `hasStandaloneBearerAuthOverride`; simplified `isStandaloneBearerAuthMode` to check token only; removed `standaloneBearerAuth` param from `shouldBypassBrowserSessionAuth` |
| `apps/gateway-admin/lib/api/logs-stream.ts` | Replaced auto-detect via `isStandaloneBearerAuthMode` with explicit `=== true` check; removed unused import |
| `apps/gateway-admin/lib/api/session.test.ts` | Updated `isStandaloneBearerAuthMode` and `shouldBypassBrowserSessionAuth` tests to match new behavior |
| `apps/gateway-admin/README.md` | Removed `NEXT_PUBLIC_STANDALONE_BEARER_AUTH` from docs and example command |
| `docs/LOCAL_LOGS.md` | Updated SSE streaming note to reflect that bearer mode is now auto-detected from token |

---

## Commands Executed

```bash
# Verified pre-existing test failures (stash approach)
cd apps/gateway-admin && pnpm test
git stash && pnpm test  # confirmed same 4 failures pre-existed
git stash pop

# Final test run with changes applied
pnpm test
# Result: same 4 pre-existing failures, all auth-related tests pass
```

---

## Behavior Changes (Before / After)

| Scenario | Before | After |
|----------|--------|-------|
| `NEXT_PUBLIC_API_TOKEN=x` only | Falls through to session auth (no bearer header sent) | Bearer mode activates; `Authorization: Bearer x` sent |
| `NEXT_PUBLIC_API_TOKEN=x` + `NEXT_PUBLIC_STANDALONE_BEARER_AUTH=true` | Bearer mode active | Bearer mode active (same result, second var ignored) |
| Neither env var set | Session auth (CSRF + cookies) | Session auth (CSRF + cookies) — unchanged |
| `connectLogStream` with token in env, no explicit option | Threw if `STANDALONE_BEARER_AUTH=true`; silent otherwise | Never throws (explicit `standaloneBearerAuth: true` option required to throw) |
| OAuth-protected deployment + token set | Blocked by session auth flow | Bearer token bypasses OAuth session requirement |

---

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `pnpm test` (before changes, stashed) | 4 failures | 4 failures (log-timeline, buildGatewayEndpointPreview, connectLogStream malformed, logoutBrowserSession) | ✅ baseline confirmed |
| `pnpm test` (after changes) | Same 4 failures, new auth tests pass | Same 4 failures; `isStandaloneBearerAuthMode activates whenever a token is set` ✔; `shouldBypassBrowserSessionAuth bypasses hosted auth when a token is set or in mock mode` ✔ | ✅ |

---

## Source IDs + Collections Touched

None — no vector store or embedding operations in this session.

---

## Risks and Rollback

- **Risk**: Any deployment that sets `NEXT_PUBLIC_API_TOKEN` for a reason other than bearer auth (e.g., some other purpose where the token was incidentally present) will now activate bearer mode unintentionally. Assess existing deployments before upgrading.
- **Risk**: Live log streaming (`/logs/stream` SSE endpoint) is silently unavailable in bearer mode. The EventSource will fail to authenticate and trigger `onError`. The UI displays this gracefully but the feature is non-functional when a token is set.
- **Rollback**: Revert `auth-mode.ts` — restore `hasStandaloneBearerAuthOverride` and the two-arg `isStandaloneBearerAuthMode`. Restore `logs-stream.ts` `modeOverride` logic. Restore tests. Restore docs.

---

## Decisions Not Taken

- **Allow bad bearer token to fall through to session auth** — Rejected. Would mask misconfigured tokens silently. Current fail-closed behavior in `router.rs:232–236` is correct.
- **Send both bearer + CSRF on every request** — Rejected. Redundant; bearer wins server-side; `credentials: 'omit'` in bearer mode is an intentional security measure.
- **Make `connectLogStream` call `onError` instead of throwing** — Not implemented (deferred). Callers that pass `standaloneBearerAuth: true` explicitly still get a thrown error. The log console (`log-console.tsx:191`) never passes this option, so it's unaffected.

---

## Open Questions

- Does any existing deployment set `NEXT_PUBLIC_API_TOKEN` for a purpose unrelated to bearer auth? If so, that deployment will break.
- Should live log streaming be disabled/hidden in the UI when bearer mode is active, rather than silently failing via EventSource auth error?
- The `gatewayRequestInit keeps credentialed requests when a token is present without standalone bearer mode` test name (`gateway-request.test.ts:15`) is now misleading — the described scenario no longer exists. The test still exercises a valid code path (explicit `standaloneBearerAuth: false` override) but should be renamed.

---

## Next Steps

- Consider renaming the stale test description at `apps/gateway-admin/lib/api/gateway-request.test.ts:15`.
- Investigate and fix the 4 pre-existing test failures (out of scope for this session).
- If log streaming matters in bearer-mode deployments, add UI affordance to disable/hide the live stream panel when `isStandaloneBearerAuthMode()` is true.
