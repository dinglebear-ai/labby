# Design System Page Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a hidden in-app `/design-system` route that acts as a full interactive UI/UX testing ground for the current Labby web UI.

**Architecture:** Build `/design-system` as a real admin route under `app/(admin)` so it renders inside the existing shell but remains reachable by direct URL only. Reuse existing UI primitives from `components/ui` and compose them through a small set of demo-only `components/design-system/*` sections with local state, sample data, and fake async flows.

**Tech Stack:** Next.js App Router, React 19 client/server components, Tailwind utility classes, Node test runner with `renderToStaticMarkup`

---

## File Structure

### New Route

- `apps/gateway-admin/app/(admin)/design-system/page.tsx`
  - Route entry for the hidden design-system page.
  - Uses `AppHeader` with breadcrumbs, but is not linked from `AppSidebar`.

### New Demo Components

- `apps/gateway-admin/components/design-system/design-system-shell.tsx`
  - Top-level page layout and section ordering.
- `apps/gateway-admin/components/design-system/foundations-section.tsx`
  - Color tokens, type ramp, spacing, radius, elevation.
- `apps/gateway-admin/components/design-system/controls-section.tsx`
  - Buttons, pills, inputs, selects, checkboxes, switches, radios, textareas.
- `apps/gateway-admin/components/design-system/feedback-section.tsx`
  - Alerts, badges, loading, empty, success, warning, error, fake async actions.
- `apps/gateway-admin/components/design-system/navigation-section.tsx`
  - Breadcrumbs, tabs, sidebar item states, toolbar actions, pagination.
- `apps/gateway-admin/components/design-system/data-display-section.tsx`
  - Metric cards, tables, dense rows, key/value blocks.
- `apps/gateway-admin/components/design-system/patterns-section.tsx`
  - Logs stream rows, inspector panes, gateway-style toolbar, setup/auth states.
- `apps/gateway-admin/components/design-system/demo-data.ts`
  - Shared fake data and option lists.
- `apps/gateway-admin/components/design-system/demo-state.ts`
  - Local-state helpers for fake async and interaction demos.

### New Tests

- `apps/gateway-admin/components/design-system/design-system-shell.test.tsx`
  - Verifies the route content is composed from the expected sections.
- `apps/gateway-admin/components/design-system/controls-section.test.tsx`
  - Verifies interactive control section renders the approved Aurora primitives and destructive/loading demos.
- `apps/gateway-admin/components/design-system/feedback-section.test.tsx`
  - Verifies feedback section renders loading, empty, success, warning, and error examples.
- `apps/gateway-admin/components/design-system/navigation-section.test.tsx`
  - Verifies navigation section renders breadcrumbs, tabs, and sidebar-state samples.
- `apps/gateway-admin/components/design-system/data-display-section.test.tsx`
  - Verifies data display section renders metric cards, dense rows, and key/value blocks.
- `apps/gateway-admin/components/design-system/patterns-section.test.tsx`
  - Verifies application-pattern demos render fake logs, inspector, gateway toolbar, and auth/setup states.
- `apps/gateway-admin/components/app-sidebar.test.tsx`
  - Verifies the primary sidebar navigation still excludes `Design System`.

### Existing Files To Modify

- `apps/gateway-admin/components/app-sidebar.tsx`
  - No behavior change expected; verify `/design-system` is not added here.

---

### Task 1: Add The Hidden Route And Page Shell

**Files:**
- Create: `apps/gateway-admin/app/(admin)/design-system/page.tsx`
- Create: `apps/gateway-admin/components/design-system/design-system-shell.tsx`
- Test: `apps/gateway-admin/components/design-system/design-system-shell.test.tsx`

- [ ] **Step 1: Write the failing test**

```tsx
import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { DesignSystemShell } from './design-system-shell'

test('design system shell renders all primary section headings', () => {
  const markup = renderToStaticMarkup(React.createElement(DesignSystemShell))
  assert.match(markup, /Theme Foundations/)
  assert.match(markup, /Controls/)
  assert.match(markup, /Feedback/)
  assert.match(markup, /Navigation/)
  assert.match(markup, /Data Display/)
  assert.match(markup, /Application Patterns/)
})
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/design-system-shell.test.tsx
```

Expected: FAIL because `DesignSystemShell` does not exist yet.

- [ ] **Step 3: Write minimal implementation**

Create `page.tsx` and `design-system-shell.tsx` with:

- an `AppHeader` breadcrumb for `Design System`
- a top-level explanatory block stating it is an internal component/testing ground
- placeholder sections for all six major areas

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/design-system-shell.test.tsx
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/app/\(admin\)/design-system/page.tsx apps/gateway-admin/components/design-system/design-system-shell.tsx apps/gateway-admin/components/design-system/design-system-shell.test.tsx
git commit -m "feat(design-system): add hidden route and shell"
```

### Task 2: Add Theme Foundations Section

**Files:**
- Create: `apps/gateway-admin/components/design-system/foundations-section.tsx`
- Create: `apps/gateway-admin/components/design-system/demo-data.ts`
- Modify: `apps/gateway-admin/components/design-system/design-system-shell.tsx`

- [ ] **Step 1: Write the failing test**

Add to `design-system-shell.test.tsx`:

```tsx
assert.match(markup, /Aurora page background/)
assert.match(markup, /Display 1/)
assert.match(markup, /Radius 3/)
assert.match(markup, /Tier 2 lift/)
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/design-system-shell.test.tsx
```

Expected: FAIL because foundations content is still placeholder-only.

- [ ] **Step 3: Write minimal implementation**

Implement `FoundationsSection` with:

- token swatches for Aurora page/panel/control/accent/warn/error colors
- typography samples for Display 1, Display 2, Metric Display, Body, Control, Dense Data, Eyebrow
- spacing/radius reference blocks
- Tier 1 vs Tier 2 lift cards

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/design-system-shell.test.tsx
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/design-system/foundations-section.tsx apps/gateway-admin/components/design-system/demo-data.ts apps/gateway-admin/components/design-system/design-system-shell.tsx apps/gateway-admin/components/design-system/design-system-shell.test.tsx
git commit -m "feat(design-system): add Aurora foundations section"
```

### Task 3: Add Controls And Feedback Sandbox

**Files:**
- Create: `apps/gateway-admin/components/design-system/controls-section.tsx`
- Create: `apps/gateway-admin/components/design-system/feedback-section.tsx`
- Create: `apps/gateway-admin/components/design-system/demo-state.ts`
- Test: `apps/gateway-admin/components/design-system/controls-section.test.tsx`
- Test: `apps/gateway-admin/components/design-system/feedback-section.test.tsx`
- Modify: `apps/gateway-admin/components/design-system/design-system-shell.tsx`

- [ ] **Step 1: Write the failing tests**

Controls:

```tsx
import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { ControlsSection } from './controls-section'

test('controls section renders primary controls plus fake async and destructive states', () => {
  const markup = renderToStaticMarkup(React.createElement(ControlsSection))
  assert.match(markup, /Primary button/)
  assert.match(markup, /Pill filters/)
  assert.match(markup, /Loading state/)
  assert.match(markup, /Destructive action/)
})
```

Feedback:

```tsx
import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { FeedbackSection } from './feedback-section'

test('feedback section renders empty loading success warning and error states', () => {
  const markup = renderToStaticMarkup(React.createElement(FeedbackSection))
  assert.match(markup, /Loading/)
  assert.match(markup, /Empty state/)
  assert.match(markup, /Success/)
  assert.match(markup, /Warning/)
  assert.match(markup, /Error/)
})
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/controls-section.test.tsx components/design-system/feedback-section.test.tsx
```

Expected: FAIL because the sections do not exist yet.

- [ ] **Step 3: Write minimal implementation**

Implement `ControlsSection` and `FeedbackSection` using existing real primitives:

- buttons: primary, secondary, outline, ghost, destructive
- inputs, select, textarea, checkbox, radio, switch, toggle
- pill-style filter examples
- fake loading/success/error controls using local state
- alerts, badges, empty blocks, spinner/skeleton, status combinations

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/controls-section.test.tsx components/design-system/feedback-section.test.tsx
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/design-system/controls-section.tsx apps/gateway-admin/components/design-system/feedback-section.tsx apps/gateway-admin/components/design-system/demo-state.ts apps/gateway-admin/components/design-system/controls-section.test.tsx apps/gateway-admin/components/design-system/feedback-section.test.tsx apps/gateway-admin/components/design-system/design-system-shell.tsx
git commit -m "feat(design-system): add controls and feedback sandbox"
```

### Task 4: Add Navigation And Data Display Sections

**Files:**
- Create: `apps/gateway-admin/components/design-system/navigation-section.tsx`
- Create: `apps/gateway-admin/components/design-system/data-display-section.tsx`
- Test: `apps/gateway-admin/components/design-system/navigation-section.test.tsx`
- Test: `apps/gateway-admin/components/design-system/data-display-section.test.tsx`
- Modify: `apps/gateway-admin/components/design-system/design-system-shell.tsx`

- [ ] **Step 1: Write the failing tests**

Navigation:

```tsx
assert.match(markup, /Sidebar item states/)
assert.match(markup, /Tabs/)
assert.match(markup, /Breadcrumbs/)
```

Data display:

```tsx
assert.match(markup, /Metric cards/)
assert.match(markup, /Dense table rows/)
assert.match(markup, /Key\/value blocks/)
```

- [ ] **Step 2: Run tests to verify they fail**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/navigation-section.test.tsx components/design-system/data-display-section.test.tsx
```

Expected: FAIL because navigation and data-display content is missing.

- [ ] **Step 3: Write minimal implementation**

Implement:

- breadcrumb examples
- tab states
- sidebar item state samples without adding route links to `AppSidebar`
- toolbar action group examples
- metric card demos
- table/dense row demos
- key-value inspector blocks

- [ ] **Step 4: Run tests to verify they pass**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/navigation-section.test.tsx components/design-system/data-display-section.test.tsx
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/design-system/navigation-section.tsx apps/gateway-admin/components/design-system/data-display-section.tsx apps/gateway-admin/components/design-system/navigation-section.test.tsx apps/gateway-admin/components/design-system/data-display-section.test.tsx apps/gateway-admin/components/design-system/design-system-shell.tsx
git commit -m "feat(design-system): add navigation and data display demos"
```

### Task 5: Add Application Pattern Demos

**Files:**
- Create: `apps/gateway-admin/components/design-system/patterns-section.tsx`
- Test: `apps/gateway-admin/components/design-system/patterns-section.test.tsx`
- Modify: `apps/gateway-admin/components/design-system/design-system-shell.tsx`
- Reuse: presentational-only pieces and fake-data wrappers derived from `apps/gateway-admin/components/logs/*`

- [ ] **Step 1: Write the failing test**

```tsx
assert.match(markup, /Logs stream pattern/)
assert.match(markup, /Inspector pane/)
assert.match(markup, /Gateway toolbar pattern/)
assert.match(markup, /Auth and setup states/)
```

- [ ] **Step 2: Run test to verify it fails**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/patterns-section.test.tsx
```

Expected: FAIL because the patterns section is not implemented.

- [ ] **Step 3: Write minimal implementation**

Implement `PatternsSection` with interactive demos for:

- logs row + inspector pairing
- gateway-style toolbar with search/filter/actions
- auth/session signed-in vs signed-out state block
- setup/empty/error/result state examples

Prefer reusing presentational-only compositions. Do not pull in `LogConsole` or any real API/stream wiring; the page must remain backend-independent.

- [ ] **Step 4: Run test to verify it passes**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/patterns-section.test.tsx
```

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/design-system/patterns-section.tsx apps/gateway-admin/components/design-system/patterns-section.test.tsx apps/gateway-admin/components/design-system/design-system-shell.tsx
git commit -m "feat(design-system): add application pattern demos"
```

### Task 6: Verify Route Behavior And Hidden Navigation

**Files:**
- Verify only: `apps/gateway-admin/app/(admin)/design-system/page.tsx`
- Verify only: `apps/gateway-admin/components/app-sidebar.tsx`
- Test: `apps/gateway-admin/components/app-sidebar.test.tsx`

- [ ] **Step 1: Write the failing sidebar test**

```tsx
import test from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { AppSidebar } from './app-sidebar'

test('app sidebar navigation excludes design system route', () => {
  const markup = renderToStaticMarkup(React.createElement(AppSidebar))
  assert.doesNotMatch(markup, /Design System/)
})
```

- [ ] **Step 2: Run focused design-system and sidebar tests**

Run:

```bash
cd apps/gateway-admin && pnpm exec tsx --test components/design-system/*.test.tsx components/app-sidebar.test.tsx
```

Expected: PASS

- [ ] **Step 3: Run production build**

Run:

```bash
cd apps/gateway-admin && pnpm build
```

Expected: build succeeds and `/design-system` is included in the static route output.

- [ ] **Step 4: Verify route is reachable but not linked in sidebar**

Run:

```bash
cd apps/gateway-admin && pnpm dev --hostname 0.0.0.0 --port 4101
```

Then verify in browser:

- `http://10.1.0.6:4101/design-system` loads correctly
- the page renders inside the admin shell
- `AppSidebar` does not show a `Design System` nav item
- local demo controls respond without backend dependency

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/app/\(admin\)/design-system/page.tsx apps/gateway-admin/components/design-system apps/gateway-admin/components/app-sidebar.test.tsx
git commit -m "feat(design-system): add hidden in-app UI sandbox"
```
