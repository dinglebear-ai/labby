# Global Command Palette Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a near-real, fully interactive global `cmd+k` command palette prototype in `apps/gateway-admin` that follows the approved Aurora hybrid-spotlight design and lives in the design-system sandbox.

**Architecture:** Add the command-palette prototype as a shared design-system pattern rather than a throwaway page. Use the existing `cmdk` primitive in `components/ui/command.tsx`, wrap it with Aurora-specific product components under `components/design-system/command-palette/`, and drive the experience with local mock data plus client-side ranking/filter state. Update the `/design-system` sandbox so the new interaction pattern is visible in the canonical reference surface.

**Tech Stack:** Next.js 16, React 19, TypeScript, `cmdk`, existing Radix/shadcn primitives, Aurora token classes from `components/aurora/tokens.ts`, local `tsx --test` component tests

---

## File Structure

### Existing files to modify

- `apps/gateway-admin/components/design-system/design-system-shell.tsx`
  - Add the new command-palette section to the sandbox layout.
- `apps/gateway-admin/components/design-system/patterns-section.tsx`
  - Remove any overlap if the new section supersedes part of the current “application patterns” showcase.
- `apps/gateway-admin/components/design-system/demo-data.ts`
  - Add route metadata or small shared sandbox labels only if needed by the new section.
- `apps/gateway-admin/components/ui/command.tsx`
  - Tighten the primitive defaults only where needed for Aurora-compatible behavior that should apply across the product.
- `apps/gateway-admin/app/globals.css`
  - Add missing Aurora utility support only if the prototype needs a shared utility that does not exist already.

### New files to create

- `apps/gateway-admin/components/design-system/command-palette-section.tsx`
  - Main sandbox section entry point.
- `apps/gateway-admin/components/design-system/command-palette-demo.tsx`
  - Client component containing palette open state, query state, active-item state, and simulated selection outcome.
- `apps/gateway-admin/components/design-system/command-palette-data.ts`
  - Mock destinations, actions, recents, and default grouping metadata.
- `apps/gateway-admin/components/design-system/command-palette-model.ts`
  - Pure helpers for filtering, ranking, grouping, and default active-row selection.
- `apps/gateway-admin/components/design-system/command-palette-row.tsx`
  - Aurora-styled mixed-result row renderer.
- `apps/gateway-admin/components/design-system/command-palette-preview.tsx`
  - Compact simulated outcome/status panel for selected items.
- `apps/gateway-admin/components/design-system/command-palette-section.test.tsx`
  - Section-level rendering and state coverage.
- `apps/gateway-admin/components/design-system/command-palette-model.test.ts`
  - Ranking and grouping tests.

### Files to inspect while implementing

- `apps/gateway-admin/components/aurora/tokens.ts`
- `apps/gateway-admin/components/design-system/navigation-section.tsx`
- `apps/gateway-admin/components/design-system/patterns-section.tsx`
- `apps/gateway-admin/components/ui/dialog.tsx`
- `apps/gateway-admin/components/ui/command.tsx`
- `apps/gateway-admin/components/ui/kbd.tsx`

---

### Task 1: Define the prototype data and ranking model

**Files:**
- Create: `apps/gateway-admin/components/design-system/command-palette-data.ts`
- Create: `apps/gateway-admin/components/design-system/command-palette-model.ts`
- Test: `apps/gateway-admin/components/design-system/command-palette-model.test.ts`

- [ ] **Step 1: Write the failing model tests**

Add tests that define the expected behavior:

- mixed result types are returned for an empty query
- `gate` ranks `Gateway Admin` before `Reload gateways`
- `logs` ranks `Logs` before `Tail local logs`
- result groups stay lightweight (`best match`, `suggested actions`, `recent context`)
- the active row defaults to the first ranked item

- [ ] **Step 2: Run the model test file to verify it fails**

Run:

```bash
cd apps/gateway-admin
pnpm test -- components/design-system/command-palette-model.test.ts
```

Expected: FAIL because the model and/or test target does not exist yet.

- [ ] **Step 3: Create the mock data file**

Add a typed local data set with at least:

- destinations: `Gateway Admin`, `Logs`, `Settings`, `Registry`
- actions: `Reload gateways`, `Rotate MCP token`, `Tail local logs`, `Open setup`
- recents: at least two recent entities/views, including `edge-proxy-prod`

Include:

- `id`
- `type`
- `title`
- `description`
- `keywords`
- `group`
- `icon`
- `actionHint`
- `priority`

- [ ] **Step 4: Create the ranking/filter model**

Implement pure helpers for:

- default suggestions for empty query
- keyword matching
- priority-aware sorting
- group assignment
- default active-item selection

Keep the helpers free of React and DOM concerns.

- [ ] **Step 5: Run the model tests to verify they pass**

Run:

```bash
cd apps/gateway-admin
pnpm test -- components/design-system/command-palette-model.test.ts
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add \
  apps/gateway-admin/components/design-system/command-palette-data.ts \
  apps/gateway-admin/components/design-system/command-palette-model.ts \
  apps/gateway-admin/components/design-system/command-palette-model.test.ts
git commit -m "feat: add command palette ranking model"
```

---

### Task 2: Build Aurora-styled result-row and preview primitives

**Files:**
- Create: `apps/gateway-admin/components/design-system/command-palette-row.tsx`
- Create: `apps/gateway-admin/components/design-system/command-palette-preview.tsx`
- Modify: `apps/gateway-admin/components/aurora/tokens.ts`
- Test: `apps/gateway-admin/components/design-system/command-palette-section.test.tsx`

- [ ] **Step 1: Write the failing component tests for row rendering**

Cover:

- result row renders title, description, type tag, and action hint
- active row gets stronger Aurora emphasis than inactive rows
- type signaling is not color-only
- preview panel updates based on the selected item

- [ ] **Step 2: Run the section test file to verify it fails**

Run:

```bash
cd apps/gateway-admin
pnpm test -- components/design-system/command-palette-section.test.tsx
```

Expected: FAIL because the components do not exist yet.

- [ ] **Step 3: Add any missing Aurora token helpers**

Only if necessary, extend `components/aurora/tokens.ts` with small reusable class strings for:

- command-palette shell
- command-palette row
- quiet type tag
- preview/status surface

Do not add raw color values. Reuse the existing token contract.

- [ ] **Step 4: Implement the row component**

Use:

- `Inter`-based dense row text
- compact, consistent heights
- truncation-first layout
- Aurora radius tokens only
- quiet `Destination` / `Action` / `Recent` tags
- stronger active-row treatment using border + subtle glow + surface deepening

- [ ] **Step 5: Implement the preview component**

Render a compact outcome panel that changes based on the active result:

- destination: “opens page” summary
- action: “simulated run” summary
- recent: “reopen context” summary

Keep the copy concise and operational.

- [ ] **Step 6: Run the section tests to verify the new primitives satisfy rendering expectations**

Run:

```bash
cd apps/gateway-admin
pnpm test -- components/design-system/command-palette-section.test.tsx
```

Expected: PASS for row/preview rendering coverage that exists so far.

- [ ] **Step 7: Commit**

```bash
git add \
  apps/gateway-admin/components/aurora/tokens.ts \
  apps/gateway-admin/components/design-system/command-palette-row.tsx \
  apps/gateway-admin/components/design-system/command-palette-preview.tsx \
  apps/gateway-admin/components/design-system/command-palette-section.test.tsx
git commit -m "feat: add command palette presentation components"
```

---

### Task 3: Build the interactive command-palette demo

**Files:**
- Create: `apps/gateway-admin/components/design-system/command-palette-demo.tsx`
- Modify: `apps/gateway-admin/components/ui/command.tsx`
- Test: `apps/gateway-admin/components/design-system/command-palette-section.test.tsx`

- [ ] **Step 1: Add failing interaction tests**

Cover:

- palette opens in the sandbox section
- typing filters the list
- arrow keys move the active row
- `Enter` selects the active row
- `Escape` closes the dialog
- empty query shows suggested mixed results
- no-results query shows the operational empty state

- [ ] **Step 2: Run the section tests to verify interaction coverage fails**

Run:

```bash
cd apps/gateway-admin
pnpm test -- components/design-system/command-palette-section.test.tsx
```

Expected: FAIL because the interactive demo is not implemented yet.

- [ ] **Step 3: Implement the client demo component**

Use the existing `cmdk` primitive exported by `components/ui/command.tsx` and compose:

- `CommandDialog`
- `CommandInput`
- `CommandList`
- grouped command items
- local state for query, active item, open/closed, and simulated outcome

Do not fetch data. Use the local model from Task 1.

- [ ] **Step 4: Tighten the primitive only where shared behavior is justified**

If the prototype needs primitive-level changes, keep them generic and Aurora-safe:

- preserve `aurora-scrollbar`
- avoid product-code shadcn color drift
- keep primitive imports on `@/`

Do not dump page-specific styling into `components/ui/command.tsx`.

- [ ] **Step 5: Implement empty/loading handling**

Add:

- default suggestions state
- lightweight loading simulation state if needed
- concise empty/no-results state

All states must remain inside the Aurora product language.

- [ ] **Step 6: Run the section tests to verify interaction passes**

Run:

```bash
cd apps/gateway-admin
pnpm test -- components/design-system/command-palette-section.test.tsx
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add \
  apps/gateway-admin/components/ui/command.tsx \
  apps/gateway-admin/components/design-system/command-palette-demo.tsx \
  apps/gateway-admin/components/design-system/command-palette-section.test.tsx
git commit -m "feat: add interactive command palette demo"
```

---

### Task 4: Integrate the palette into the design-system sandbox

**Files:**
- Create: `apps/gateway-admin/components/design-system/command-palette-section.tsx`
- Modify: `apps/gateway-admin/components/design-system/design-system-shell.tsx`
- Modify: `apps/gateway-admin/components/design-system/patterns-section.tsx`
- Modify: `apps/gateway-admin/components/design-system/demo-data.ts`
- Test: `apps/gateway-admin/components/design-system/design-system-shell.test.tsx`

- [ ] **Step 1: Add a failing shell test**

Cover:

- the design-system shell renders the command-palette section
- the new section appears as part of the direct-url-only sandbox flow

- [ ] **Step 2: Run the shell test to verify it fails**

Run:

```bash
cd apps/gateway-admin
pnpm test -- components/design-system/design-system-shell.test.tsx
```

Expected: FAIL because the shell does not include the new section yet.

- [ ] **Step 3: Implement the section wrapper**

The section should:

- use `AURORA_STRONG_PANEL`
- present the required eyebrow + heading copy
- embed the interactive demo and preview area
- explain that this is the canonical command-palette reference pattern

- [ ] **Step 4: Update the sandbox shell**

Insert the new section in the most appropriate place for a shared interaction pattern. Likely after navigation and before data display, unless the existing shell reads better with it in the patterns area.

- [ ] **Step 5: Reduce or adjust overlap in `patterns-section.tsx`**

If the current patterns section duplicates command-palette concepts after the new section lands, trim the overlap rather than leaving two competing references.

- [ ] **Step 6: Run the shell test to verify the sandbox integration passes**

Run:

```bash
cd apps/gateway-admin
pnpm test -- components/design-system/design-system-shell.test.tsx
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add \
  apps/gateway-admin/components/design-system/command-palette-section.tsx \
  apps/gateway-admin/components/design-system/design-system-shell.tsx \
  apps/gateway-admin/components/design-system/patterns-section.tsx \
  apps/gateway-admin/components/design-system/demo-data.ts \
  apps/gateway-admin/components/design-system/design-system-shell.test.tsx
git commit -m "feat: add command palette to design system sandbox"
```

---

### Task 5: Final polish and contract-specific cleanup

**Files:**
- Modify: `apps/gateway-admin/app/globals.css`
- Modify: `apps/gateway-admin/components/design-system/command-palette-section.tsx`
- Modify: `apps/gateway-admin/components/design-system/command-palette-demo.tsx`
- Modify: `apps/gateway-admin/components/design-system/command-palette-row.tsx`
- Test: `apps/gateway-admin/components/design-system/command-palette-section.test.tsx`
- Test: `apps/gateway-admin/components/design-system/design-system-shell.test.tsx`

- [ ] **Step 1: Add any final failing assertions for contract-specific details**

Cover only if not already asserted:

- `aurora-scrollbar` on scrollable result list
- quiet type tags
- no-results copy stays concise
- focus-visible treatment is present on interactive elements

- [ ] **Step 2: Run the relevant tests to verify they fail**

Run:

```bash
cd apps/gateway-admin
pnpm test -- \
  components/design-system/command-palette-section.test.tsx \
  components/design-system/design-system-shell.test.tsx
```

Expected: FAIL if the missing contract details are not in place.

- [ ] **Step 3: Apply final cleanup**

Ensure:

- no raw color values are introduced
- no product-code `text-muted-foreground`, `bg-card`, `bg-background`, or `border-border` usage slips in
- radii use Aurora tokens
- spacing stays on the compact Aurora cadence
- focus and motion stay restrained

- [ ] **Step 4: Run the relevant tests to verify they pass**

Run:

```bash
cd apps/gateway-admin
pnpm test -- \
  components/design-system/command-palette-section.test.tsx \
  components/design-system/design-system-shell.test.tsx \
  components/design-system/command-palette-model.test.ts
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add \
  apps/gateway-admin/app/globals.css \
  apps/gateway-admin/components/design-system/command-palette-section.tsx \
  apps/gateway-admin/components/design-system/command-palette-demo.tsx \
  apps/gateway-admin/components/design-system/command-palette-row.tsx \
  apps/gateway-admin/components/design-system/command-palette-section.test.tsx \
  apps/gateway-admin/components/design-system/design-system-shell.test.tsx \
  apps/gateway-admin/components/design-system/command-palette-model.test.ts
git commit -m "chore: polish command palette sandbox"
```

---

## Notes For The Implementer

- Keep the prototype inside the design-system sandbox. Do not add it to the primary sidebar.
- Prefer extending shared Aurora recipes over page-local Tailwind one-offs.
- Keep `components/ui/**` primitive-safe and generic. Product styling belongs in `components/design-system/**`.
- If the result list becomes scrollable, preserve `aurora-scrollbar`.
- Use `@/` imports throughout product code.
- Treat the spec at `docs/superpowers/specs/2026-04-22-global-command-palette-design.md` as authoritative for behavior and hierarchy.

## Suggested Verification Commands

```bash
cd apps/gateway-admin
pnpm test -- components/design-system/command-palette-model.test.ts
pnpm test -- components/design-system/command-palette-section.test.tsx
pnpm test -- components/design-system/design-system-shell.test.tsx
```
