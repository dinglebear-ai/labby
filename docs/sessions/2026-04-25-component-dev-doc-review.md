---
date: 2026-04-25 16:34:42 EST
repo: git@github.com:jmagar/lab.git
branch: bd-security/marketplace-p1-fixes
head: f168964b
plan: none
agent: Claude (claude-sonnet-4-6)
session id: 8ceac97e-dec1-4351-b1f9-3b57834b06bf
transcript: /home/jmagar/.claude/projects/-home-jmagar-workspace-lab/8ceac97e-dec1-4351-b1f9-3b57834b06bf.jsonl
working directory: /home/jmagar/workspace/lab
pr: "#29 fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation — https://github.com/jmagar/lab/pull/29"
---

## User Request

Review `docs/design/component-development.md` in full, cross-reference it against the actual codebase, then identify all issues and suggest improvements — and then implement all suggestions.

## Session Overview

Full audit of the component development process document against its live implementation. Found a critical whitelist drift bug, two pre-existing build breaks, missing tests, and documentation gaps. Implemented all fixes: bug patch, 14 new tests (11 frontend + 3 backend), two pre-existing Rust build breaks resolved, CI test script updated, process document substantially revised, and the missing marketplace feature design spec authored.

## Sequence of Events

1. Read `docs/design/component-development.md` (provided inline by user)
2. Spawned Explore agent to map the `apps/gateway-admin` codebase — layout, dev routes, preview-mode, API clients, marketplace components, design system contract, backend routes
3. Read key files directly to verify agent findings: `preview-mode.ts`, `marketplace/types.ts`, `app/dev/layout.tsx`, `app/dev/marketplace/page.tsx`, `crates/lab/src/api/router.rs` (full 1651 lines)
4. Identified six issues; presented them to user; user requested all be addressed
5. Fixed whitelist drift in `router.rs` — added `"plugin.components"` to `DEV_MARKETPLACE_READ_ACTIONS`
6. Attempted to run backend tests; discovered two pre-existing build breaks on the branch: `serve_dev_mockup` / `serve_dev_named_mockup` stubs missing from `web.rs`, and `router.rs` using deprecated axum `:capture` syntax instead of `{capture}`
7. Fixed both pre-existing breaks: added the two stubs to `web.rs`, corrected route syntax in `router.rs`
8. Added three backend integration tests to `router.rs` for the `/dev/api/marketplace` endpoint
9. Wrote 11 frontend unit tests in `apps/gateway-admin/lib/dev/preview-mode.test.ts`
10. Added `lib/dev/**/*.test.ts` to the `test` script in `package.json`
11. Updated `docs/design/component-development.md`: backend guard architecture section, whitelist sync rule, extension protocol, AppSidebar note, completion checklist
12. User requested two remaining issues: missing marketplace spec + unverified AppSidebar unauthenticated behavior
13. Traced `useBrowserSession()` → `session-store.ts` → `loadBrowserSession()` → `AuthBootstrap` — confirmed `AuthBootstrap` is only in the `(admin)` layout, not the `/dev/` layout; sidebar is safe for unauthenticated visitors
14. Authored `apps/gateway-admin/docs/marketplace-catalog.md` — full retroactive feature spec for the marketplace catalog
15. Updated document to replace speculative AppSidebar note with verified finding and generalized rule

## Key Findings

- **Critical bug** (`router.rs:28-44`): `"plugin.components"` was in the frontend `READ_ONLY_ACTIONS` set (`preview-mode.ts:11`) but absent from the backend `DEV_MARKETPLACE_READ_ACTIONS` constant. Any `/dev/*` call to `plugin.components` would pass the frontend guard but receive a 403 from the backend with no useful error visible to the user.
- **Pre-existing build break 1** (`router.rs:662-663`): Routes used Express-style `:name` path capture syntax; axum 0.8 requires `{name}`. The compiler error was `"Path segments must not start with ':'`.
- **Pre-existing build break 2** (`router.rs:660-663`): `crate::api::web::serve_dev_mockup` and `serve_dev_named_mockup` were referenced but never defined in `web.rs`.
- **AppSidebar is safe**: `loadBrowserSession()` is only called from `AuthBootstrap`, which is only mounted in `app/(admin)/layout.tsx`. On `/dev/*` routes the session store stays at `loading`; the user card in the sidebar footer renders only for `authenticated` status → no API calls, no 401 errors, no broken layout.
- **Whitelist lists were defined in two places with no enforcement**: `preview-mode.ts` (frontend) and `router.rs` (backend) — no shared type or test to catch future drift.
- **No marketplace spec**: `apps/gateway-admin/docs/` contained only `gateway-detail-redesign.md`; the process document requires a spec to exist before implementation.

## Technical Decisions

- **Fixed whitelist drift rather than removing the duplicate**: The dual-layer enforcement (frontend blocks early for UX, backend blocks authoritatively for security) is correct architecture; the fix is to keep both lists in sync, not collapse them.
- **Added `serve_dev_mockup` / `serve_dev_named_mockup` as thin wrappers over `serve_web_request`**: The dev page routes serve the Next.js SPA, same as any other client-side route. A dedicated wrapper name makes the intent explicit and keeps the router readable.
- **Backend tests assert 403, not 200**: The whitelist test checks that allowed actions are not blocked (status ≠ 403), since the dispatch itself returns various codes depending on backend state. Only mutating action tests assert exactly 403 with `kind: "dev_preview_read_only"`.
- **Marketplace spec written retroactively to document current approved state**: The process doc requires a spec before implementation, but the implementation already existed. The spec records the current design as approved, notes gaps, and locks the current behavior as the baseline.
- **AppSidebar issue resolved as documentation, not code change**: The code was already correct; the doc was speculative. The fix was to verify and record the finding rather than add defensive code that wasn't needed.

## Files Modified

| File | Change |
|------|--------|
| `crates/lab/src/api/router.rs` | Added `"plugin.components"` to `DEV_MARKETPLACE_READ_ACTIONS`; fixed route syntax `/:name` → `/{name}`; added 3 backend integration tests |
| `crates/lab/src/api/web.rs` | Added `serve_dev_mockup` and `serve_dev_named_mockup` stubs (both delegate to `serve_web_request`) |
| `apps/gateway-admin/lib/dev/preview-mode.test.ts` | Created — 11 unit tests for `isDevPreviewReadOnlyAction`, `isDevPreviewRoute`, `assertDevPreviewCanRunAction`, `devPreviewActionUrl` |
| `apps/gateway-admin/package.json` | Added `lib/dev/**/*.test.ts` to the `test` script |
| `docs/design/component-development.md` | Backend Guard section: added Rust implementation location, whitelist sync rule, service extension protocol; Step 8: replaced speculative AppSidebar note with verified finding; Completion Checklist: added whitelist sync, test requirements, new component verification rule |
| `apps/gateway-admin/docs/marketplace-catalog.md` | Created — full feature design spec for the marketplace catalog component |

## Commands Executed

| Command | Result |
|---------|--------|
| `cargo test --all-features -p "path+file:///…/crates/lab#0.11.0" -- "tests::dev_marketplace"` | 6 passed (3 new tests × 2 filter passes) |
| `pnpm tsx --test lib/dev/preview-mode.test.ts` | 11 passed, 0 failed |
| `pnpm test` | 182 passed, 0 failed (full frontend suite) |
| `rtk cargo check --all-features` | 2 crates compiled, 0 errors |

## Errors Encountered

- **`serve_dev_mockup` not found** — pre-existing: `router.rs` referenced functions not yet implemented in `web.rs`. Root cause: branch work-in-progress left stubs unimplemented. Resolution: added the two functions to `web.rs`.
- **`Path segments must not start with ':'`** — pre-existing: axum 0.8 path capture syntax changed from `:name` to `{name}`. Root cause: routes written with pre-0.8 syntax. Resolution: updated route strings in `router.rs`.
- **`cargo test -p lab` ambiguous package** — `lab` name collision with a crates.io crate. Resolution: used full path spec `path+file:///…/crates/lab#0.11.0`.

## Behavior Changes (Before/After)

| Area | Before | After |
|------|--------|-------|
| `plugin.components` action in `/dev/*` | Frontend allowed it, backend returned 403 `dev_preview_read_only` | Both layers allow it; action reaches dispatch correctly |
| `/dev` and `/dev/:name` GET routes | Failed to compile (missing stubs + wrong syntax) | Build clean; routes serve the SPA index via `serve_dev_mockup` |
| `preview-mode.ts` test coverage | Zero | 11 unit tests covering all exported functions |
| `/dev/api/marketplace` backend test coverage | Zero | 3 integration tests (allowed actions, blocked actions, no-auth access) |
| Frontend test script | Did not include `lib/dev/**` | `lib/dev/**/*.test.ts` included in `pnpm test` |

## Verification Evidence

| Command | Expected | Actual | Status |
|---------|----------|--------|--------|
| `cargo test -- tests::dev_marketplace` | 3 tests pass | 6 pass (3 tests × 2 filter scopes) | ✅ |
| `pnpm tsx --test lib/dev/preview-mode.test.ts` | 11 pass, 0 fail | 11 pass, 0 fail | ✅ |
| `pnpm test` | All pass, no regressions | 182 pass, 0 fail | ✅ |
| `rtk cargo check --all-features` | 0 errors | 0 errors | ✅ |

## Risks and Rollback

- **`serve_dev_mockup` stubs** are thin wrappers that call `serve_web_request`. If `web_assets_dir` is not configured, they return 404 — same behavior as any other SPA route. No new surface area.
- **`/dev/api/marketplace` is unauthenticated by design**. The handler only exposes read actions whitelisted in `DEV_MARKETPLACE_READ_ACTIONS`. The backend test `dev_marketplace_requires_no_auth` asserts this contract is maintained.
- Rollback: revert `router.rs` and `web.rs` to the pre-session state. The whitelist drift bug would return.

## Decisions Not Taken

- **Sharing the whitelist as a single source of truth** (e.g., a build-time generated file or a TypeScript constant imported by a test): rejected as over-engineering for the current scale. The documentation and dual-layer test coverage are the enforcement mechanism.
- **Adding skeleton loading state to the marketplace catalog**: deferred; noted as a known gap in the spec.
- **Moving `lib/dev/**` tests to a separate test target**: rejected; folding into the existing `tsx --test` command is simpler and keeps the test matrix flat.

## Open Questions

- MCP servers are currently mocked in `MOCK_MCP_SERVERS`. When the MCP registry API is available, `marketplace-catalog.md` notes that `useMcpServers()` should replace the static mock.
- The Installed tab shows plugins only. If MCP server and ACP agent install tracking is added, the installed view scope needs extending.
- Plugin detail view (`plugin-detail-content.tsx`) is not linked from the catalog cards. Whether the `MarketplaceCard` should have a detail href is unresolved.

## Next Steps

**Unfinished from this session:** none — all identified issues were addressed.

**Follow-on tasks not yet started:**
- Verify `/dev/marketplace` in a running browser for an unauthenticated user to confirm the sidebar and marketplace data both load without console errors (can only be done with a running server + web assets)
- Replace `MOCK_MCP_SERVERS` with a live `useMcpServers()` hook once the MCP registry backend exposes a `mcp.list` action
- Consider adding skeletons to the marketplace grid for perceived performance on slow connections
