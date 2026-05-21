# Marketplace Page — Implementation Session

**Date:** 2026-04-22  
**Branch:** `fix/auth`  
**Working directory:** `apps/gateway-admin/`

> Historical note: this session document describes the initial marketplace page delivery. The current implementation no longer uses `plugin-detail-dialog.tsx`; plugin details now live on the dedicated route `/marketplace/plugin?id=<pluginId>`, and the `Files` tab is an editable CodeMirror surface with workspace save/deploy flow.

---

## Session Overview

Built the complete Marketplace page for gateway-admin from scratch: a full-page UI for discovering, browsing, and installing Claude Code plugins from multiple upstream sources. The session covered three phases carried over from a prior context window:

1. **Aurora design-system-contract polishing pass** — eliminated all raw `rgba()`/hex violations from `/tmp/lab-marketplace/index.html` mockup
2. **Design spec authoring** — wrote `docs/superpowers/plans/DESIGN_SPEC.md` documenting all components, tokens, TypeScript models, and scrollbar contract
3. **Implementation plan authoring** — invoked `superpowers:writing-plans` to produce a 12-task TDD plan at `docs/superpowers/plans/2026-04-22-marketplace-page.md`
4. **Full inline execution** — executed all 12 tasks in this session via `superpowers:executing-plans`, delivering 14 created/modified files in 3 commits

---

## Timeline

| Time | Activity |
|------|----------|
| Session start | Context resumed post-compaction; plan already written |
| Step 1 | Read plan (30k tokens, read in 6 chunks) |
| Step 2 | Verified existing codebase patterns (`cn` at `@/lib/utils`, `AppHeader` exists, prismjs not present) |
| Step 3 | Task 1 — `lib/types/marketplace.ts` |
| Step 4 | Task 2 — API client + mock data + 17-test suite (all passing) |
| Step 5 | Task 3 — SWR hooks (`useMarketplaces`, `usePlugins`, `useMarketplaceMutations`) |
| Step 6 | Tasks 4–12 — all 8 components, page route, sidebar wiring |
| Step 7 | `pnpm add prismjs && pnpm add -D @types/prismjs` |
| Step 8 | Lint fix: `_pluginId` → `void pluginId`; ternary → `if` in `toggleFolder` |
| Step 9 | 3 atomic commits pushed to `fix/auth` |

---

## Key Findings

- **`cn` is at `@/lib/utils`** — plan incorrectly specified `@/lib/utils/utils`; corrected in all 8 components
- **Pre-existing TS errors** — `components/chat/mock-data.ts` had 7 pre-existing `TS2322` errors (`'agent'` not assignable to `ACPRole`); unrelated to marketplace work
- **ESLint underscore convention not configured** — `_pluginId` was still flagged by `@typescript-eslint/no-unused-vars`; fixed with `void pluginId` pattern
- **Ternary-as-statement** — `next.has(dir) ? next.delete(dir) : next.add(dir)` flagged by `no-unused-expressions`; replaced with `if` block
- **pnpm lockfile had duplicate key** — `server-only@0.0.1` appeared twice; pnpm warned but continued; prismjs installed cleanly
- **tsx is at `~/.local/share/pnpm/tsx`** — `npx tsx` does not work; must invoke as bare `tsx`

---

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| `void pluginId` for stub params | Matches ESLint config that doesn't honor `_` prefix; avoids disabling the rule |
| `color-mix(in_srgb,...)` for all transparent tints | Aurora design contract; no raw rgba in component rules |
| Mock data in `marketplace-client.ts` | Backend wire-up is deferred; stubs return real-looking data with ~600ms delays |
| `structuredClone()` on mock arrays | Prevents mutation of shared mock constants across SWR calls |
| Prism loaded as module (not CDN) | SSR-compatible; works with Next.js 16 app router |
| `'use client'` on all components | Required for hooks, event handlers, and browser APIs (clipboard, Prism) |
| Page route is thin RSC wrapper | `MarketplaceListContent` is client; page.tsx is server; `metadata` export works correctly |

---

## Files Modified

| Action | File | Purpose |
|--------|------|---------|
| Create | `lib/types/marketplace.ts` | `Marketplace`, `Plugin`, `Artifact`, `ArtifactLang`, `MarketplaceState` types |
| Create | `lib/api/marketplace-client.ts` | Mock data (4 marketplaces, 10 plugins, 2 artifact trees), fetch fns, `detectArtifactLang`, `getArtifacts` |
| Create | `lib/api/marketplace-client.test.ts` | 17 unit tests for API client (all passing) |
| Create | `lib/hooks/use-marketplace.ts` | `useMarketplaces`, `usePlugins`, `useMarketplaceMutations` SWR hooks with Sonner toasts |
| Create | `components/marketplace/marketplace-card.tsx` | Plugin grid card: GitHub avatar, status badge (Installed/Update), tags, version chip |
| Create | `components/marketplace/mkt-source-card.tsx` | Marketplace source card: avatar, owner, installed/available counts, auto-update pulse |
| Create | `components/marketplace/marketplace-stats-strip.tsx` | Stat chips strip: installed/sources/updates with semantic color icon backgrounds |
| Create | `components/marketplace/add-marketplace-modal.tsx` | Radix Dialog form: repo, git URL, name, auto-update toggle |
| Create | `components/marketplace/plugin-info-panel.tsx` | Info tab: description, count chips, details table, included items list, README renderer |
| Create | `components/marketplace/plugin-files-panel.tsx` | Initial Files tab: collapsible tree + read-only Prism viewer; later replaced by editable CodeMirror workspace flow |
| Create | `components/marketplace/plugin-detail-dialog.tsx` | Initial modal detail shell; later removed in favor of `/marketplace/plugin?id=<pluginId>` |
| Create | `components/marketplace/marketplace-list-content.tsx` | Page body: Browse/Installed/Marketplaces tabs, search input, sort select, stats strip, grids, initial modal state |
| Create | `app/(admin)/marketplace/page.tsx` | Route entry point (RSC wrapper with `metadata`) |
| Modify | `components/app-sidebar.tsx` | Added `ShoppingBag` import + `Marketplace` nav entry after `Registry` |

---

## Commands Executed

```bash
# Test run (all 17 pass)
tsx --test lib/api/marketplace-client.test.ts

# Install Prism
pnpm add prismjs && pnpm add -D @types/prismjs

# TypeScript check (no marketplace errors; 7 pre-existing chat errors)
npx tsc --noEmit 2>&1 | grep -i marketplace

# Lint check (clean after two fixes)
npx eslint components/marketplace/ app/(admin)/marketplace/ lib/types/marketplace.ts lib/api/marketplace-client.ts lib/hooks/use-marketplace.ts

# Commits
git commit -m "feat(marketplace): types, API client (mock data), and SWR hooks"
git commit -m "feat(marketplace): all UI components — cards, panels, dialogs, modal"
git commit -m "feat(marketplace): route + sidebar nav entry — Marketplace page complete"
```

---

## Verification Evidence

| Check | Expected | Actual | Status |
|-------|----------|--------|--------|
| `tsx --test lib/api/marketplace-client.test.ts` | 17 pass, 0 fail | 17 pass, 0 fail | ✅ |
| `tsc --noEmit \| grep marketplace` | no output | no output | ✅ |
| ESLint on all marketplace files | no issues | No issues found | ✅ |
| `git log --oneline -3` | 3 feat(marketplace) commits | 802d67e, 3674c5b, 120bf6a | ✅ |
| Pre-existing TS errors | unrelated to our work | 7 errors in `chat/mock-data.ts` only | ✅ |

---

## Behavior Changes (Before / After)

| Surface | Before | After |
|---------|--------|-------|
| Sidebar navigation | Registry → Setup | Registry → **Marketplace** → Setup |
| `/marketplace` route | 404 | Full marketplace page with Browse/Installed/Marketplaces tabs |
| Plugin browsing | Not available | 10 plugins across 4 marketplaces; filterable, sortable grid |
| Plugin detail | Not available | Initial modal with Info tab + read-only Files tab; later replaced by dedicated route and editable workspace-backed Files tab |
| Install/uninstall | Not available | Optimistic mutation via SWR + Sonner toasts |
| Add marketplace | Not available | Modal form with GitHub repo / git URL / auto-update options |

---

## Risks and Rollback

- **Risk:** `pnpm` lockfile has a duplicate `server-only` key. Currently a warning only; if it causes CI failures, fix by deduplicating `pnpm-lock.yaml` manually or running `pnpm install --fix-lockfile`.
- **Risk:** All data is mock. The `installPlugin`, `uninstallPlugin`, `addMarketplace` stubs sleep for 400–800ms and do nothing. Backend wire-up is deferred.
- **Rollback:** `git revert 120bf6a 3674c5b 802d67e` removes all three commits cleanly. The sidebar and route are independent — reverting `components/app-sidebar.tsx` alone is sufficient to hide the page from navigation.

---

## Decisions Not Taken

- **React Testing Library for component tests** — plan used a minimal `typeof MarketplaceCard === 'function'` smoke test instead; full DOM rendering tests require jsdom setup that was removed from this project during the pnpm install (vitest + jsdom were dropped)
- **Prism via CDN** — would avoid the pnpm add but breaks SSR; module import is correct for Next.js 16
- **Inline `dangerouslySetInnerHTML` XSS risk** — the README renderer uses `renderMd()` which only operates on canned mock content; real wire-up should sanitize with DOMPurify before rendering user-supplied content

---

## Open Questions

- The `components/chat/mock-data.ts` file has 7 pre-existing `TS2322` errors (`'agent'` not assignable to `ACPRole`). These should be fixed separately — they will block a clean `tsc --noEmit` run on CI if strict mode is enforced.
- The `pnpm-lock.yaml` has a duplicate `server-only@0.0.1` key (lines 4109/4111). This is a pre-existing issue but could cause `pnpm install --frozen-lockfile` failures in CI.
- The `MarketplaceListContent` Refresh button has a no-op `onClick={() => {}}`. Wire it to `mutate()` from both SWR hooks when the backend is ready.

---

## Next Steps

1. **Wire backend** — replace `fetchMarketplaces()`, `fetchPlugins()`, `installPlugin()`, `uninstallPlugin()`, `addMarketplace()` stubs in `marketplace-client.ts` with real HTTP calls to `GET /api/marketplaces`, `GET /api/plugins`, etc.
2. **Fix chat mock-data TS errors** — `components/chat/mock-data.ts:139` et al; unrelated but blocking clean tsc
3. **Sanitize README renderer** — add DOMPurify before `dangerouslySetInnerHTML` when real plugin content is fetched
4. **Refresh button** — call `mutate()` on both `useMarketplaces` and `usePlugins` in `MarketplaceListContent`
5. **Fix lockfile** — resolve duplicate `server-only` key in `pnpm-lock.yaml`
6. **Add update-all action** — bulk "Update All" button for plugins with `hasUpdate: true`
