# Gateway Admin Local Auth Modes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add explicit local auth modes for `gateway-admin`, including a real-backend local auth bypass that keeps UI session consumers working.

**Architecture:** Extend the existing auth mode layer with a development-only local bypass flag, synthesize an authenticated browser session in `session-store`, then surface the selected mode in Settings and README. Keep mock mode intact and leave hosted auth as the default path.

**Tech Stack:** Next.js 16, React 19, TypeScript, existing browser-session auth store, Aurora admin UI

---

### Task 1: Define local auth-bypass mode

**Files:**
- Modify: `apps/gateway-admin/lib/auth/auth-mode.ts`

- [ ] Add a dedicated local bypass detector based on `NEXT_PUBLIC_LOCAL_AUTH_BYPASS`
- [ ] Gate local bypass to development-only execution
- [ ] Keep mock mode and hosted auth mode behavior unchanged

### Task 2: Synthesize a stable local browser session

**Files:**
- Modify: `apps/gateway-admin/lib/auth/session-store.ts`

- [ ] Add a deterministic synthetic authenticated session for local bypass mode
- [ ] Initialize session state from that synthetic session when bypass is active
- [ ] Make `loadBrowserSession()` short-circuit to the synthetic session in bypass mode
- [ ] Make `logoutBrowserSession()` behave safely in bypass mode without breaking the UI session shape

### Task 3: Surface auth mode in Settings

**Files:**
- Modify: `apps/gateway-admin/lib/dashboard/admin-insights.ts`
- Modify: `apps/gateway-admin/app/(admin)/settings/page.tsx`

- [ ] Extend settings snapshot inputs to include local bypass mode
- [ ] Report `Local dev bypass` distinctly from `Browser session`
- [ ] Keep runtime labels aligned with mock vs live backend behavior

### Task 4: Document local workflows

**Files:**
- Modify: `apps/gateway-admin/README.md`

- [ ] Document the three supported local modes
- [ ] Make `real backend + local auth bypass` the recommended default local workflow
- [ ] Show concrete startup commands for frontend and backend
