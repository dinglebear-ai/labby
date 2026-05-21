# Gateways Mobile Density Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rework the `/gateways` mobile experience so the top summary compresses into a compact telemetry strip, search becomes the primary control with embedded filter access, and each mobile gateway row shows dense inline metadata by default without separate detail cards or a dedicated tools column.

**Architecture:** Keep the existing desktop `GatewayListView`, `GatewayFilters`, and `GatewayTable` structure intact, but introduce mobile-only rendering changes inside the existing components. Reuse current filter state and summary state instead of changing data shape, and drive the mobile pass through focused component updates plus `node:test` snapshot assertions that lock in the new Aurora-aligned structure.

**Tech Stack:** Next.js app router, React server/client components, Tailwind utility classes with Aurora semantic tokens, `node:test`, `react-dom/server`.

---

## File Structure

### Existing files to modify

- `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
  - Owns the `/gateways` page layout, summary surface, header actions, search/filter placement, and the desktop/mobile split between filters and table content.
- `apps/gateway-admin/components/gateway/gateway-filters.tsx`
  - Owns the search control, desktop filter panel, and mobile filter affordance. This is the right place to embed mobile filter access into the search row and keep secondary filters hidden by default.
- `apps/gateway-admin/components/gateway/gateway-table.tsx`
  - Owns mobile and desktop table renderings. This is where the mobile row structure, density, and inline metrics need to change.
- `apps/gateway-admin/components/gateway/gateway-list-content.test.tsx`
  - Snapshot-style assertions for page-level layout and primary actions.
- `apps/gateway-admin/components/gateway/gateway-filters.test.tsx`
  - Snapshot-style assertions for filter rendering and search affordances.
- `apps/gateway-admin/components/gateway/gateway-table.test.tsx`
  - Snapshot-style assertions for Aurora table surfaces and mobile row output.

### Files to leave alone unless blocked

- `apps/gateway-admin/app/(admin)/gateways/page.tsx`
  - Thin page wrapper only. No change expected.
- `apps/gateway-admin/components/gateway/gateway-list-state.ts`
  - Filtering semantics and data shaping remain unchanged.
- `apps/gateway-admin/lib/types/gateway.ts`
  - No backend or type-shape changes are in scope.

## Task 1: Lock the mobile page layout contract in tests

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-list-content.test.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-list-content.tsx`

- [ ] **Step 1: Write the failing test expectations for the mobile page contract**

Update `apps/gateway-admin/components/gateway/gateway-list-content.test.tsx` so the page-level test stops asserting the old summary-card wording and density controls on mobile, and instead asserts the new structure:

```tsx
assert.doesNotMatch(markup, /Discovered tools/)
assert.doesNotMatch(markup, /aria-label="Comfortable density"/)
assert.doesNotMatch(markup, /aria-label="Condensed density"/)
assert.match(markup, /aria-label="Open filters"/)
assert.match(markup, /aria-label="Switch to tools view"/)
assert.match(markup, /data-mobile-summary="configured"/)
assert.match(markup, /data-mobile-summary="healthy"/)
assert.match(markup, /data-mobile-summary="disconnected"/)
assert.match(markup, /data-mobile-summary="tools"/)
```

- [ ] **Step 2: Run the list-content test to verify it fails**

Run:

```bash
pnpm --dir apps/gateway-admin test -- --test-name-pattern="gateway list view renders quick-lens cards and primary actions"
```

Expected:

- FAIL because the current markup still renders large labeled summary cards and density toggle buttons.

- [ ] **Step 3: Update `GatewayListView` mobile header and summary structure**

In `apps/gateway-admin/components/gateway/gateway-list-content.tsx`:

- keep desktop summary cards and density controls intact
- introduce a mobile-only compact summary strip directly under the page title
- render four icon-and-number summary chips with explicit attributes for testing:

```tsx
<button data-mobile-summary="configured" ...>
  <Cable className="size-3.5" />
  <span>{summary.configured}</span>
</button>
```

- replace the current mobile `Open filters` button + density buttons row with:
  - compact tools-entry action (`aria-label="Switch to tools view"`)
  - compact add action
- ensure density controls remain desktop-only

- [ ] **Step 4: Run the list-content test to verify it passes**

Run:

```bash
pnpm --dir apps/gateway-admin test -- --test-name-pattern="gateway list view renders quick-lens cards and primary actions"
```

Expected:

- PASS with markup showing the mobile telemetry strip and without mobile density controls.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-list-content.tsx apps/gateway-admin/components/gateway/gateway-list-content.test.tsx
git commit -m "feat: compress gateways mobile header and summary"
```

## Task 2: Embed mobile filter access into search and hide secondary filters by default

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-filters.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-filters.test.tsx`

- [ ] **Step 1: Write the failing test expectations for mobile search/filter integration**

Extend `apps/gateway-admin/components/gateway/gateway-filters.test.tsx` with assertions that capture the new mobile structure without breaking desktop assertions:

```tsx
assert.match(markup, /aria-label="Open filters"/)
assert.match(markup, /data-mobile-search="gateways"/)
assert.doesNotMatch(markup, /Open filters<\/span>/)
```

For tools mode, also assert the search row still changes placeholder correctly:

```tsx
assert.match(markup, /Search tools, descriptions, or gateways/)
assert.match(markup, /aria-label="Open filters"/)
```

- [ ] **Step 2: Run the filters tests to verify they fail**

Run:

```bash
pnpm --dir apps/gateway-admin test -- --test-name-pattern="gateway filters render aurora checkbox groups and clear state affordance|tools filters render exposure segmented control and gateway facets"
```

Expected:

- FAIL because there is no embedded mobile filter control inside the search row today.

- [ ] **Step 3: Rework `GatewayFilters` mobile rendering**

In `apps/gateway-admin/components/gateway/gateway-filters.tsx`:

- keep the existing desktop panel (`lg:block`) behavior
- replace the mobile “filters sheet trigger as a standalone button row” presentation with:
  - a single search row containing search icon, input, clear affordance, and filter toggle button
  - the filter toggle button embedded into the same search container
- keep `Sheet`-backed controls only if needed for interaction parity, but do not expose secondary controls by default in the mobile page layout
- render active mobile filter chips below the search row only when there are active filters or the mobile filter section is open
- ensure all colors come from Aurora tokens and no raw color values are introduced

- [ ] **Step 4: Run the filters tests to verify they pass**

Run:

```bash
pnpm --dir apps/gateway-admin test -- --test-name-pattern="gateway filters render aurora checkbox groups and clear state affordance|tools filters render exposure segmented control and gateway facets"
```

Expected:

- PASS with the embedded mobile filter control present and desktop filter markup still intact.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-filters.tsx apps/gateway-admin/components/gateway/gateway-filters.test.tsx
git commit -m "feat: embed gateways mobile filters into search control"
```

## Task 3: Flatten the mobile gateway table into a two-column dense layout

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-table.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-table.test.tsx`

- [ ] **Step 1: Write the failing test for the new mobile row structure**

Update `apps/gateway-admin/components/gateway/gateway-table.test.tsx` so it asserts the mobile row shows inline metrics and no longer emits the old separate tools/resources/prompts summary treatment:

```tsx
assert.match(markup, />14 tools</)
assert.match(markup, />4 res</)
assert.match(markup, />2 prompts</)
assert.doesNotMatch(markup, />14\\/18</)
assert.doesNotMatch(markup, /TOOLS|RESOURCES|PROMPTS/)
```

If the exact text differs, prefer precise assertions on the new inline fragments instead of broad regexes.

- [ ] **Step 2: Run the table test to verify it fails**

Run:

```bash
pnpm --dir apps/gateway-admin test -- --test-name-pattern="gateway table uses aurora lifted surfaces and muted operational pills"
```

Expected:

- FAIL because the current mobile row still renders the old card-like structure and ratio output.

- [ ] **Step 3: Replace the mobile row composition in `GatewayTable`**

In `apps/gateway-admin/components/gateway/gateway-table.tsx`:

- keep desktop table structure and sorting behavior unchanged
- rewrite only the `md:hidden` mobile rendering so each row shows:
  - gateway name + status dot
  - endpoint/transport line
  - inline metrics strip with:
    - exposed/discovered tools expressed clearly as `14 tools` or similar
    - resources
    - prompts
    - runtime age
- remove mobile-only detail cards and any stacked metrics blocks
- remove the separate mobile tools column concept entirely
- keep the right side compact:
  - state label
  - warning/secondary status
  - overflow menu
- preserve existing action menu wiring and existing probe/reload/delete hooks

Concrete target shape:

```tsx
<div className="gateway-subline">
  <span className="mini-inline"><Wrench ... />14 tools</span>
  <span className="mini-inline"><FileText ... />4 res</span>
  <span className="mini-inline"><MessageSquare ... />2 prompts</span>
  <span className="mini-inline"><Activity ... />42m</span>
</div>
```

- [ ] **Step 4: Run the table test to verify it passes**

Run:

```bash
pnpm --dir apps/gateway-admin test -- --test-name-pattern="gateway table uses aurora lifted surfaces and muted operational pills"
```

Expected:

- PASS with inline mobile metrics present and without the old ratio string.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-table.tsx apps/gateway-admin/components/gateway/gateway-table.test.tsx
git commit -m "feat: flatten gateways mobile rows into inline dense metadata"
```

## Task 4: Integrate page-level spacing and mobile-first ordering

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-filters.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-table.tsx`

- [ ] **Step 1: Write a final integration assertion in the page-level test**

Add page-level assertions to `apps/gateway-admin/components/gateway/gateway-list-content.test.tsx` that confirm the mobile structure works together:

```tsx
assert.match(markup, /data-mobile-summary="configured"/)
assert.match(markup, /data-mobile-search="gateways"/)
assert.match(markup, /tools<\/span>/)
assert.doesNotMatch(markup, /Discovered tools/)
```

- [ ] **Step 2: Run the list-content test to verify it fails if integration is incomplete**

Run:

```bash
pnpm --dir apps/gateway-admin test -- --test-name-pattern="gateway list view renders quick-lens cards and primary actions"
```

Expected:

- FAIL until the page-level ordering and composed mobile output are fully aligned.

- [ ] **Step 3: Finalize spacing, ordering, and mobile visibility rules**

In `apps/gateway-admin/components/gateway/gateway-list-content.tsx` and related components:

- ensure mobile order is exactly:
  1. title/actions
  2. compact telemetry strip
  3. search row with embedded filter access
  4. optional compact filter chips
  5. dense table
- ensure desktop retains:
  - larger summary cards
  - desktop filter rail
  - desktop density toggles
- ensure no raw hex, rgba, or hsl values are introduced in product component class names
- ensure any overflow containers use `aurora-scrollbar` if overflow utilities are added

- [ ] **Step 4: Run the targeted UI tests to verify the integrated pass**

Run:

```bash
pnpm --dir apps/gateway-admin test -- --test-name-pattern="gateway list view renders quick-lens cards and primary actions|gateway filters render aurora checkbox groups and clear state affordance|tools filters render exposure segmented control and gateway facets|gateway table uses aurora lifted surfaces and muted operational pills"
```

Expected:

- PASS for all four tests.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-list-content.tsx apps/gateway-admin/components/gateway/gateway-filters.tsx apps/gateway-admin/components/gateway/gateway-table.tsx apps/gateway-admin/components/gateway/gateway-list-content.test.tsx apps/gateway-admin/components/gateway/gateway-filters.test.tsx apps/gateway-admin/components/gateway/gateway-table.test.tsx
git commit -m "feat: complete gateways mobile density pass"
```

## Task 5: Design-system follow-through

**Files:**
- Inspect: `docs/design-system-contract.md`
- Optional Modify: `apps/gateway-admin/app/(admin)/design-system/page.tsx` or the design-system section file that demonstrates gateway page patterns

- [ ] **Step 1: Check whether the mobile search-integrated filter pattern already exists in the design-system sandbox**

Inspect the existing design-system gateway/pattern references before adding anything new.

Run:

```bash
rg -n "Gateways|filter|search" apps/gateway-admin/app/(admin)/design-system apps/gateway-admin/components/design-system
```

Expected:

- Either an existing pattern can be reused, or there is no representative mobile pattern yet.

- [ ] **Step 2: If the pattern is missing, add a minimal sandbox example**

If no equivalent pattern exists, add or update the smallest relevant design-system demo so the search-integrated mobile filter row is represented there. Do not build a full second gateway page in the sandbox.

- [ ] **Step 3: Run any design-system-specific test only if one already exists for the touched sandbox file**

Run the narrowest existing test that covers the touched sandbox surface.

Expected:

- PASS, or no-op if there is no existing test coverage.

- [ ] **Step 4: Commit**

```bash
git add apps/gateway-admin/app/(admin)/design-system page.tsx apps/gateway-admin/components/design-system
git commit -m "docs: reflect gateways mobile filter pattern in design system"
```

## Verification Checklist

Run after implementation tasks are complete:

```bash
pnpm --dir apps/gateway-admin test -- --test-name-pattern="gateway list view renders quick-lens cards and primary actions|gateway filters render aurora checkbox groups and clear state affordance|tools filters render exposure segmented control and gateway facets|gateway table uses aurora lifted surfaces and muted operational pills"
```

Manual browser check on `/gateways` mobile viewport:

- telemetry strip is icon + number only
- search sits directly below the strip
- filter access is embedded into the search control
- extra filters are hidden by default
- mobile rows show tools, resources, prompts, and runtime inline
- no separate mobile tools column
- no resource/prompt/runtime cards
- desktop layout still shows its existing broader structure

## Notes for the implementing worker

- Do not change backend data shape or filter semantics.
- Do not rewrite desktop table sorting for this task.
- Do not introduce new page-local design tokens.
- Prefer small helper markup inside existing components over broad refactors.
- If a desktop assertion breaks while implementing mobile-only changes, fix the regression rather than weakening coverage.
