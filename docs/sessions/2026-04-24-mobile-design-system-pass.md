---
date: 2026-04-24 00:52:01 EST
repo: git@github.com:jmagar/lab.git
branch: bd-security/marketplace-p1-fixes
head: f8de5bde
agent: Codex
session id: 019dbca9-8a30-7c61-aa72-38dee280e3fc
working directory: /home/jmagar/workspace/lab
worktree: /home/jmagar/workspace/lab
pr: "#29 fix(marketplace): P1 security fixes — path traversal, symlink following, installPath validation — https://github.com/jmagar/lab/pull/29"
---

# User Request

Initial request: `i need you to do a thorough mobile optimization pass over /marketplaces uses chrome dev tools mcp`.

The request expanded during the session to include:
- gateway detail mobile UX cleanup
- unified gateway catalog behavior for tools/resources/prompts
- granular resource and prompt exposure management with backend support
- a full admin-page Chrome DevTools MCP audit against the design contract
- final mobile spot checks across the highest-traffic routes

# Session Overview

This session started as a `/marketplace` mobile optimization pass and expanded into a broader gateway-admin UX and design-system remediation. The work landed in three layers:

1. Gateway detail and marketplace UX redesign
   - reworked mobile list/search patterns
   - moved gateway toggles out of crowded headers into a dedicated settings surface
   - unified tools/resources/prompts browsing into a `Catalog` view driven by icon filters
2. Gateway backend/config parity work
   - added granular `resources` and `prompts` exposure support to match existing tool exposure behavior
   - wired the UI to save those policies through the real gateway mutation path
3. Full design-system and mobile verification pass
   - audited the admin routes with Chrome DevTools MCP
   - fixed hydration, mock-mode failures, missing accessible names, and several mobile layout regressions
   - updated the design contract with missing responsive and unavailable-state rules

By the end of the session, the verified admin routes were usable on mobile and free of product-console errors, with the only residual console noise coming from Next.js dev font preload warnings.

# Sequence of Events

1. `/marketplace` mobile redesign began.
   - The user asked for a mobile optimization pass on the marketplace route using Chrome DevTools MCP.
   - The first direction explored live mockups before applying code changes.

2. The scope expanded to reuse gateway patterns.
   - The user directed reuse of the gateways search/filter shell and dense list/card pattern for marketplace.
   - Marketplace mobile controls were redesigned around the gateway search/filter approach instead of separate search, sort, and stats rows.

3. Gateway detail header and toggle UX were redesigned.
   - Redundant status iconography and oversized header controls were removed.
   - The gateway detail page moved mutable controls into a dedicated `Settings` tab while preserving `Config`.
   - Header actions were tightened to icon-first mobile treatments.

4. Gateway catalog navigation was simplified.
   - `Tools`, `Resources`, and `Prompts` tabs were removed from the top-level detail navigation.
   - A single `Catalog` tab was introduced with icon filters that switch primitive views.
   - The icon chips were corrected to behave like the former tabs rather than toggle on/off chips.

5. Missing granular resource/prompt management was identified and implemented.
   - Existing UI only supported gateway-level `proxy_resources` and `proxy_prompts` switches.
   - Backend/config/runtime support for `expose_resources` and `expose_prompts` was added.
   - New resource/prompt management tables were added to the real frontend.

6. Systematic debugging resolved gateway detail blockers.
   - `gatewayApi is not defined` blocked resource/prompt saves.
   - Mock persistence paths and gateway state wiring were fixed.
   - A backend 500/save-path problem was traced through the gateway mutation flow and addressed.
   - Hydration issues and accidental manage-mode defaults were removed.

7. A full admin-route design-system audit followed.
   - The user asked for a full pass over the pages against `docs/design/design-system-contract.md`.
   - Mock-mode routes that still called unavailable backend paths were converted to local mock-aware behavior for preview mode.
   - Accessibility issues, deterministic timestamp formatting, and duplicated mobile/desktop control shells were fixed.

8. A second round of focused visual polish was done.
   - `overview`, `chat`, `settings`, `activity`, and `logs` each received a live mobile pass.
   - Only routes with concrete visible issues were patched.

9. Final spot checks completed the session.
   - `gateways`, `gateway detail`, `marketplace`, and `registry` were verified on mobile in Chrome DevTools MCP.
   - No further high-value fixes were identified.

# Key Findings

- The design system was missing an explicit rule for unavailable backend states. This was codified so product pages show calm Aurora support panels instead of raw transport strings: [design-system-contract.md](/home/jmagar/workspace/lab/docs/design/design-system-contract.md:409).
- The design system was also missing an explicit rule preventing mobile from rendering both desktop and mobile control shells simultaneously: [design-system-contract.md](/home/jmagar/workspace/lab/docs/design/design-system-contract.md:467).
- Deterministic UI timestamp helpers were added and then consumed by activity, logs, and design-system surfaces to remove locale/timezone-driven drift and hydration risk: [format-ui-time.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/format-ui-time.ts:31), [activity/page.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/app/(admin)/activity/page.tsx:237), [log-event-inspector.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/logs/log-event-inspector.tsx:36).
- Gateway detail now exposes a unified `Catalog` surface and real per-resource/per-prompt management instead of only coarse gateway-level toggles: [gateway-detail-content.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/gateway/gateway-detail-content.tsx:673), [gateway-detail-content.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/gateway/gateway-detail-content.tsx:827), [gateway-detail-content.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/gateway/gateway-detail-content.tsx:860).
- Backend gateway patch parameters now support `proxy_prompts`, `expose_resources`, and `expose_prompts`, which was necessary to make the new UI real instead of mock-only: [params.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/gateway/params.rs:110), [params.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/gateway/params.rs:114), [params.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/gateway/params.rs:116).
- Gateway config patching persists the new per-primitive allowlists in the control-plane config: [config.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/gateway/config.rs:154), [config.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/gateway/config.rs:165), [config.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/gateway/config.rs:171).
- Mock-mode preview for OAuth now intentionally reports unavailable state instead of generating product 404 noise: [upstream-oauth-section.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/upstream-oauth/upstream-oauth-section.tsx:38), [use-upstream-oauth.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/hooks/use-upstream-oauth.ts).
- The logs toolbar now has explicit accessible identifiers and a denser mobile layout: [log-toolbar.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/logs/log-toolbar.tsx:109), [log-toolbar.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/logs/log-toolbar.tsx:127), [log-toolbar.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/logs/log-toolbar.tsx:153).
- The chat composer now has a concrete field name and a denser mobile shell: [chat-input.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/chat/chat-input.tsx:147).
- Mock logs explicitly surface preview-mode messages for gateway and registry so the activity/log routes remain useful without a live backend: [logs-client.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/api/logs-client.ts:15), [logs-client.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/api/logs-client.ts:38).

# Technical Decisions

- Reused the gateway mobile search/filter shell in marketplace rather than inventing a marketplace-specific mobile toolbar.
  - This reduced one-off responsive logic and aligned the list surfaces with the design contract.

- Replaced separate `Tools/Resources/Prompts` top-level tabs with a single `Catalog` tab.
  - The user explicitly wanted the primitive-type icons to act like the former tabs.
  - This reduced top-level navigation and kept the detail page focused on search/browse tasks.

- Kept gateway-level `proxy_resources` / `proxy_prompts` toggles while adding per-item allowlists.
  - This preserved coarse kill-switch behavior while enabling granular exposure control.

- Used mock-aware client behavior for preview mode instead of allowing repeated 404s from unavailable backend routes.
  - This kept the live preview usable and made the UI reflect environment state instead of transport failures.

- Standardized timestamps through a shared helper rather than route-local `Intl.DateTimeFormat(undefined, ...)` usage.
  - This directly addressed hydration drift and made Chrome verification stable.

- Limited later passes to routes with concrete visible issues rather than rewriting already-stable pages.
  - This kept the session focused on actual defects instead of cosmetic churn.

# Files Modified

Frontend and design-system work:
- [apps/gateway-admin/components/marketplace/marketplace-list-content.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/marketplace/marketplace-list-content.tsx): marketplace mobile hero, search/filter shell, tabs, and summary behavior.
- [apps/gateway-admin/components/gateway/gateway-detail-content.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/gateway/gateway-detail-content.tsx): gateway header cleanup, `Catalog` tab, `Settings` tab, resource/prompt management, mobile density fixes.
- [apps/gateway-admin/components/gateway/primitive-exposure-table.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/gateway/primitive-exposure-table.tsx): shared management table for resources and prompts.
- [apps/gateway-admin/components/gateway/gateway-filters.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/gateway/gateway-filters.tsx): accessible names and mobile filter behavior.
- [apps/gateway-admin/components/logs/log-console.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/logs/log-console.tsx): mobile header-action density and copy-feedback behavior.
- [apps/gateway-admin/components/logs/log-toolbar.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/logs/log-toolbar.tsx): accessible IDs/names and denser mobile toolbar layout.
- [apps/gateway-admin/components/logs/log-event-inspector.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/logs/log-event-inspector.tsx): deterministic time formatting and Aurora token cleanup.
- [apps/gateway-admin/components/chat/chat-shell.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/chat/chat-shell.tsx): mock-mode ACP unavailable behavior and mobile header density.
- [apps/gateway-admin/components/chat/chat-input.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/chat/chat-input.tsx): mobile composer tightening and `chat-message` field name.
- [apps/gateway-admin/components/chat/message-thread.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/chat/message-thread.tsx): stronger mobile empty state panel.
- [apps/gateway-admin/components/design-system/patterns-section.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/design-system/patterns-section.tsx): deterministic time formatting and token cleanup.
- [apps/gateway-admin/components/design-system/command-palette-demo.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/design-system/command-palette-demo.tsx): accessible search naming and markup cleanup.
- [apps/gateway-admin/components/design-system/command-palette-row.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/design-system/command-palette-row.tsx): deterministic recent-time formatting.
- [apps/gateway-admin/components/registry/server-filters.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/registry/server-filters.tsx): named/accessible registry filter controls.
- [apps/gateway-admin/components/registry/registry-list-content.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/registry/registry-list-content.tsx): registry mobile layout cleanup.
- [apps/gateway-admin/components/registry/server-detail-panel.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/registry/server-detail-panel.tsx): registry detail polish.
- [apps/gateway-admin/components/registry/install-dialog.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/registry/install-dialog.tsx): registry install surface fixes.
- [apps/gateway-admin/components/setup/setup-page-content.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/setup/setup-page-content.tsx): Aurora-compliant setup shell and accessible target input.
- [apps/gateway-admin/components/upstream-oauth/upstream-oauth-section.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/components/upstream-oauth/upstream-oauth-section.tsx): unavailable-state panel for mock preview.
- [apps/gateway-admin/app/(admin)/page.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/app/(admin)/page.tsx): overview warning banner and recent-gateway density polish.
- [apps/gateway-admin/app/(admin)/activity/page.tsx](/home/jmagar/workspace/lab/apps/gateway-admin/app/(admin)/activity/page.tsx): deterministic timestamps.
- [docs/design/design-system-contract.md](/home/jmagar/workspace/lab/docs/design/design-system-contract.md): added missing contract rules discovered during the audit.
- [apps/gateway-admin/lib/format-ui-time.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/format-ui-time.ts): shared deterministic date/time formatter.

Mock-mode and route-stability work:
- [apps/gateway-admin/lib/api/marketplace-client.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/api/marketplace-client.ts): mock marketplace behavior and safer demo data.
- [apps/gateway-admin/lib/api/mcpregistry-client.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/api/mcpregistry-client.ts): mock registry behavior and mock metadata/install paths.
- [apps/gateway-admin/lib/api/logs-client.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/api/logs-client.ts): mock log event/stat responses.
- [apps/gateway-admin/lib/api/logs-stream.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/api/logs-stream.ts): mock SSE behavior.
- [apps/gateway-admin/lib/chat/use-session-events.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/chat/use-session-events.ts): mock-mode event-stream suppression.
- [apps/gateway-admin/lib/hooks/use-upstream-oauth.ts](/home/jmagar/workspace/lab/apps/gateway-admin/lib/hooks/use-upstream-oauth.ts): mock-mode unavailable behavior.

Backend/resource-prompt exposure work:
- [crates/lab/src/dispatch/gateway/config.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/gateway/config.rs): persistence for `proxy_prompts`, `expose_resources`, and `expose_prompts`.
- [crates/lab/src/dispatch/gateway/params.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/gateway/params.rs): patch parameter support for new exposure fields.
- [crates/lab/src/dispatch/gateway/types.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/gateway/types.rs): gateway config/runtime representation of new exposure fields.
- [crates/lab/src/dispatch/gateway/manager.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/gateway/manager.rs): gateway catalog diff + propagation behavior.
- [crates/lab/src/dispatch/upstream/pool.rs](/home/jmagar/workspace/lab/crates/lab/src/dispatch/upstream/pool.rs): upstream prompt/resource exposure handling.

# Commands Executed

Critical shell commands observed during the session:

- `cd /home/jmagar/workspace/lab/apps/gateway-admin && NEXT_PUBLIC_MOCK_DATA=true LAB_ALLOWED_DEV_ORIGINS=10.1.0.6 npx next dev --webpack -H 0.0.0.0 -p 3200`
  - Result: dev server started on `0.0.0.0:3200` and was used for all live Chrome DevTools verification.

- `rm -rf /home/jmagar/workspace/lab/apps/gateway-admin/.next`
  - Result: removed a corrupted Next.js build cache after repeated manifest `ENOENT` errors in dev mode.

- `TZ=America/New_York date '+%Y-%m-%d %H:%M:%S EST'`
  - Result: `2026-04-24 00:52:01 EST`.

- `git remote get-url origin`
  - Result: `git@github.com:jmagar/lab.git`.

- `git branch --show-current`
  - Result: `bd-security/marketplace-p1-fixes`.

- `git rev-parse --short HEAD`
  - Result: `f8de5bde`.

- `git log --oneline -5`
  - Result: captured the latest five commits for repo context.

- `git status --short`
  - Result: captured the dirty working tree and newly added files at session end.

- `git log --oneline --name-only -10`
  - Result: captured recent commit history and touched files for nearby context.

- `git worktree list | grep "$(pwd)" | head -1`
  - Result: `/home/jmagar/workspace/lab  f8de5bde [bd-security/marketplace-p1-fixes]`.

- `gh pr view --json number,title,url 2>/dev/null || echo "none"`
  - Result: active PR metadata for PR `#29`.

Chrome DevTools MCP actions were used extensively instead of shell commands for verification:
- route navigation
- mobile viewport emulation (`390x844x3,mobile,touch`)
- ARIA/text snapshots
- console inspection
- screenshots for `overview`, `docs`, `chat`, `settings`, `activity`, `logs`, `gateways`, `gateway detail`, `marketplace`, and `registry`

# Errors Encountered

- Next.js dev server manifest failures
  - Symptom: `/` returned `500`, and the server logged `ENOENT` for `.next/dev/server/app-paths-manifest.json` and `.next/dev/routes-manifest.json`.
  - Root cause: corrupted or incomplete dev build cache in `.next` during repeated live verification/reload cycles.
  - Resolution: removed `.next` and restarted `next dev --webpack`.

- Missing frontend import for resource/prompt save path
  - Symptom: `Save changes` on resource management threw `gatewayApi is not defined` in the actual frontend.
  - Root cause: missing `gatewayApi` import in the save callback path.
  - Resolution: patched the import and reverified the real UI.

- Backend/API failure for granular resource/prompt persistence
  - Symptom: a live save request returned `500` after the frontend import issue was fixed.
  - Root cause: incomplete gateway mutation / persistence path for new resource and prompt exposure fields.
  - Resolution: patched gateway params/config/runtime handling and the mock-aware mutation path, then reverified end-to-end in Chrome.

- Hydration and locale-driven UI drift
  - Symptom: design-system/log surfaces produced hydration mismatches and unstable timestamps.
  - Root cause: route-local `Intl.DateTimeFormat(undefined, ...)` usage and non-deterministic client/server formatting.
  - Resolution: introduced `format-ui-time.ts` and switched affected surfaces to the shared deterministic helper.

- Mock preview routes emitted avoidable 404 noise
  - Symptom: marketplace, registry, logs, upstream OAuth, and chat ACP surfaces produced missing-backend or missing-provider errors in preview mode.
  - Root cause: preview-mode UI still hit live backend endpoints.
  - Resolution: switched those clients/hooks to mock-aware behavior and explicit unavailable-state panels.

# Behavior Changes (Before/After)

- Marketplace mobile
  - Before: separate search/sort/stats controls, clipping/overflow risk, denser desktop-shaped cards.
  - After: gateway-style search/filter shell, compact summary treatment, stronger mobile list density.

- Gateway detail navigation
  - Before: separate `Tools`, `Resources`, and `Prompts` top-level tabs.
  - After: single `Catalog` tab with icon filters that switch primitive views.

- Gateway detail exposure management
  - Before: tools had granular control; resources/prompts only had coarse gateway-level switches.
  - After: resources and prompts have real per-item management tables and save through the actual gateway mutation path.

- Gateway detail header
  - Before: redundant connected/status iconography and oversized/toggle-heavy header usage.
  - After: denser icon-first header actions, compact command snippet row, settings controls moved out of the status cluster.

- Mock preview route behavior
  - Before: several pages surfaced raw 404/transport failures when the backend path was intentionally absent.
  - After: pages show mock content or explicit unavailable-state Aurora panels.

- Logs mobile toolbar
  - Before: toolbar behaved like a compressed desktop toolbar and still triggered unnamed-form warnings.
  - After: search remains full width, window/limit controls are explicit and named, mobile action row is compact, and warnings are gone.

- Chat mobile
  - Before: empty state was visually loose, header status pill consumed unnecessary width, and the composer missed a concrete field name.
  - After: stronger empty-state panel, icon-first status on mobile, denser composer, and named message field.

# Verification Evidence

| command | expected | actual | status |
|---|---|---|---|
| Chrome DevTools MCP: `/gateway?id=gw-2` resource save flow | resource exposure changes persist without frontend/runtime errors | resource exposure toggles saved successfully after `gatewayApi`/backend fixes | pass |
| Chrome DevTools MCP: `/gateway?id=gw-2` prompt save flow | prompt exposure changes persist without frontend/runtime errors | prompt exposure toggles saved successfully after backend/config/runtime fixes | pass |
| Chrome DevTools MCP: `/design-system` reload + console check | no hydration/product errors | hydration errors removed; only dev warnings remained | pass |
| Chrome DevTools MCP: `/chat` mobile snapshot + console | mock preview should not call unavailable ACP endpoints; no unnamed form field warning | ACP unavailable panel shown; message field named; no product errors | pass |
| Chrome DevTools MCP: `/logs` mobile snapshot + console | toolbar should be mobile-usable and free of unnamed form field warnings | denser mobile toolbar rendered; unnamed form warnings removed | pass |
| Chrome DevTools MCP: `/settings` mobile snapshot + console | route should render cleanly in mobile layout | layout stable; no product errors | pass |
| Chrome DevTools MCP: `/activity` mobile snapshot + console | route should render cleanly with deterministic timestamps | layout stable; no product errors | pass |
| Chrome DevTools MCP: `/gateways` mobile snapshot + console | route should remain stable after gateway/mobile redesign | route rendered cleanly; no product errors | pass |
| Chrome DevTools MCP: `/marketplace` mobile snapshot + console | route should use the new mobile search/filter/list layout without product errors | route rendered cleanly; no product errors | pass |
| Chrome DevTools MCP: `/registry` mobile snapshot + console | route should render cleanly with named controls and no product errors | route rendered cleanly; no product errors | pass |

# Risks and Rollback

- Risk: this session changed both frontend interaction patterns and backend gateway exposure semantics in the same working tree.
- Risk: preview-mode route stability now depends on mock-aware client branches in multiple files.
- Rollback path: revert the gateway-admin frontend files listed above plus the gateway exposure backend files if the granular resource/prompt behavior needs to be removed together.
- Rollback path: if only preview-mode behavior needs to be reverted, confine rollback to the `apps/gateway-admin/lib/api/*`, `lib/chat/*`, and `lib/hooks/use-upstream-oauth.ts` changes.

# Decisions Not Taken

- Did not keep `Tools`, `Resources`, and `Prompts` as separate top-level tabs on gateway detail.
  - Rejected because the user explicitly wanted the primitive-type icons to replace the old tab switching behavior.

- Did not keep raw backend/HTTP error strings in preview mode.
  - Rejected because the design-system contract and the user’s audit goal required intentional unavailable-state UI.

- Did not broaden later polish passes to every route after they were already stable.
  - Rejected to avoid unnecessary churn once Chrome DevTools verification showed the routes were clean.

# References

- [docs/design/design-system-contract.md](/home/jmagar/workspace/lab/docs/design/design-system-contract.md)
- PR `#29`: `https://github.com/jmagar/lab/pull/29`
- Live dev server used for verification: `http://10.1.0.6:3200`

# Open Questions

- Transcript path: no concrete transcript file path was exposed by the environment during this session.
- Active plan path: no concrete active plan file path was exposed by the environment during this session.
- Dirty working tree attribution: `git status --short` includes many modified files outside the gateway-admin surfaces touched in this session; this document lists the files known to have been modified during the conversation, not a forensic attribution for every dirty file in the repository.

# Next Steps

Unfinished work from this session:
- None identified during the final Chrome DevTools spot check. The remaining console warnings were limited to Next.js dev font preload warnings.

Follow-on tasks not yet started:
- Run a desktop-width verification sweep on the highest-traffic routes if parity evidence is needed in addition to the mobile verification already completed.
- Decide whether the Next.js dev font preload warnings are worth cleaning up, since they are still present even though product-console errors are gone.
- If desired, separate the gateway-admin/frontend work from unrelated dirty-tree Rust/device/ACP changes before preparing a reviewable branch diff.
