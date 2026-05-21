# Labby Exportability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `apps/gateway-admin` build cleanly as a static export with no Next runtime dependency.

**Architecture:** Remove all runtime dependence on Next server features by shifting gateway data access to browser-side API calls against the Rust backend and by replacing the dynamic gateway detail route with an export-safe static route shape. Keep the current UI behavior, but make the app a pure static client over `lab serve`.

**Tech Stack:** Next.js 16 app router, React 19, SWR, static export, Rust `lab serve` HTTP API

---

### Task 1: Lock in export-safe routing

**Files:**
- Modify: `apps/gateway-admin/app/(admin)/gateways/[id]/page.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-table.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-detail-content.tsx`
- Create: `apps/gateway-admin/app/(admin)/gateway/page.tsx`
- Test: `apps/gateway-admin` build output

- [ ] Replace the runtime dynamic `[id]` route with a static page that reads the gateway id from search params.
- [ ] Update all navigation/actions to link to the new export-safe route.
- [ ] Verify the detail page still loads and handles missing ids gracefully.
- [ ] Run `pnpm build` and confirm the dynamic route no longer appears in output.

### Task 2: Remove Next API route dependency

**Files:**
- Modify: `apps/gateway-admin/lib/api/gateway-client.ts`
- Modify: `apps/gateway-admin/lib/hooks/use-gateways.ts`
- Delete: `apps/gateway-admin/app/api/gateways/route.ts`
- Delete: `apps/gateway-admin/app/api/gateways/[id]/route.ts`
- Delete: `apps/gateway-admin/app/api/gateways/[id]/test/route.ts`
- Delete: `apps/gateway-admin/app/api/gateways/[id]/reload/route.ts`
- Delete: `apps/gateway-admin/app/api/gateways/[id]/exposure/route.ts`
- Delete: `apps/gateway-admin/app/api/gateways/[id]/exposure/preview/route.ts`
- Modify: `apps/gateway-admin/lib/server/*` only if needed for tests or mock data cleanup
- Test: `apps/gateway-admin/lib/server/**/*.test.ts`

- [ ] Change the client API base to target the Rust backend directly via a public env/configured origin.
- [ ] Preserve auth behavior by sending the browser token/header model expected by `lab serve`.
- [ ] Remove the Next route handlers once no client path depends on them.
- [ ] Keep mock-data mode working for local UI iteration.
- [ ] Run `pnpm test` and `pnpm build` to confirm the app no longer emits dynamic API routes.

### Task 3: Enable static export configuration

**Files:**
- Modify: `apps/gateway-admin/next.config.mjs`
- Modify: `apps/gateway-admin/package.json`
- Modify: `apps/gateway-admin/README.md`
- Test: `pnpm build`

- [ ] Set Next export mode in config.
- [ ] Add any required base-path/trailing-slash/export-safe settings.
- [ ] Update docs/scripts so local dev still uses `next dev`, while production output is export-oriented.
- [ ] Run `pnpm build` and confirm all app routes are static.

### Task 4: Verify export artifact usability

**Files:**
- Modify only if verification reveals gaps
- Test: exported `out/` artifact, browser smoke check

- [ ] Serve the export artifact with a simple static server.
- [ ] Verify the list page loads.
- [ ] Verify the detail page route works via the new static path shape.
- [ ] Verify create/test/reload flows still talk to the Rust backend.
- [ ] Capture any remaining blockers for the later Axum embedding step.
