# Gateways Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rework the gateway-admin gateways page so summary cards set the primary lens, dropdown filters become checkbox-driven filters, discovered tools open an in-place tools inventory, density toggles move into the header, and row warnings/errors stop rendering inline.

**Architecture:** Keep `GatewayListContent` as the stateful coordinator for page lens, density, and filter state. Split presentational concerns into focused gateway-specific components: a filter rail/sheet component, a tools inventory component, and a gateway table that supports both comfortable and condensed row specifications without owning page state.

**Tech Stack:** Next.js app router, React client components, existing Aurora tokens and shadcn primitives, `node:test` + `react-dom/server` markup tests.

---

## File structure and responsibilities

- Modify: `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
  - Own page-level state for primary lens, density mode, gateways filters, tools filters, and derived aggregated tools rows.
  - Move density controls into `AppHeader` actions.
  - Swap content area between gateways table and tools inventory.
- Modify: `apps/gateway-admin/components/gateway/gateway-filters.tsx`
  - Replace dropdown filters with checkbox-group filtering UI.
  - Support both desktop rail rendering and mobile sheet rendering from the same filter taxonomy.
- Modify: `apps/gateway-admin/components/gateway/gateway-table.tsx`
  - Add `comfortable` and `condensed` row rendering behavior.
  - Remove inline `last_error` rendering from both desktop and mobile list views.
  - Keep warning disclosure behind the warning affordance only.
- Create: `apps/gateway-admin/components/gateway/gateway-tools-table.tsx`
  - Render the in-place tools inventory view using Aurora dense-data language.
- Create: `apps/gateway-admin/components/gateway/gateway-list-state.ts`
  - Hold local types and pure helpers for primary lens selection, gateway filtering, tools aggregation, and tools filtering.
- Modify: `apps/gateway-admin/components/gateway/index.ts`
  - Export the new tools inventory component if needed by the page surface.
- Modify: `apps/gateway-admin/components/gateway/gateway-filters.test.tsx`
  - Update tests to reflect checkbox groups and mobile/desktop filter entrypoints.
- Modify: `apps/gateway-admin/components/gateway/gateway-table.test.tsx`
  - Update tests for condensed layout and removal of inline warning/error text.
- Create: `apps/gateway-admin/components/gateway/gateway-list-content.test.tsx`
  - Cover page-level header actions, primary lens switching, and tools-view swap behavior with mocked hooks.
- Create: `apps/gateway-admin/components/gateway/gateway-list-state.test.ts`
  - Cover pure filtering and aggregation behavior.
- Create: `apps/gateway-admin/components/gateway/gateway-tools-table.test.tsx`
  - Cover tools inventory rendering and Aurora dense-data styling.
- Modify: `apps/gateway-admin/components/design-system/design-system-shell.tsx`
  - Add or update a sandbox section if the new checkbox filter rail / header density controls are treated as shared product patterns.

## Task 1: Add pure page-state helpers for gateway and tools lenses

**Files:**
- Create: `apps/gateway-admin/components/gateway/gateway-list-state.ts`
- Test: `apps/gateway-admin/components/gateway/gateway-list-state.test.ts`
- Reference: `apps/gateway-admin/lib/types/gateway.ts`

- [ ] **Step 1: Confirm the existing gateway fields are sufficient for the new facets before writing logic**

Check in `apps/gateway-admin/lib/types/gateway.ts` and current gateway payload usage that the following fields exist and are already populated on the page model:

- `source`
- `configured`
- `enabled`
- `transport`
- `status.connected`
- `status.healthy`

If any of these are missing or not trustworthy for the intended facet, stop and add a small prerequisite task to adapt the page model before continuing with filter implementation.

- [ ] **Step 2: Write the failing pure-state tests**

```ts
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  aggregateToolsFromGateways,
  filterGateways,
  filterTools,
} from './gateway-list-state'

test('configured primary lens returns configured gateways before secondary filters', () => {
  const result = filterGateways(fixtures, {
    primaryLens: 'configured',
    search: '',
    status: [],
    source: [],
    transport: [],
  })

  assert.deepEqual(result.map((gateway) => gateway.id), ['gw_lab', 'gw_http'])
})

test('tools aggregation produces one row per tool per gateway', () => {
  const rows = aggregateToolsFromGateways(fixtures)

  assert.equal(rows.length, 3)
  assert.deepEqual(rows.map((row) => [row.gatewayId, row.toolName]), [
    ['gw_lab', 'gateway'],
    ['gw_lab', 'unifi'],
    ['gw_http', 'search'],
  ])
})

test('tools filters combine search with gateway and exposure filters', () => {
  const rows = filterTools(toolFixtures, {
    search: 'uni',
    gatewayIds: ['gw_lab'],
    exposure: 'exposed',
    source: [],
    transport: [],
  })

  assert.deepEqual(rows.map((row) => row.toolName), ['unifi'])
})

test('gateway status facets map configured healthy disconnected enabled and disabled correctly', () => {
  assert.equal(matchesGatewayStatusFacet(configuredHealthyGateway, ['configured']), true)
  assert.equal(matchesGatewayStatusFacet(configuredHealthyGateway, ['healthy']), true)
  assert.equal(matchesGatewayStatusFacet(disconnectedGateway, ['disconnected']), true)
  assert.equal(matchesGatewayStatusFacet(disabledGateway, ['disabled']), true)
  assert.equal(matchesGatewayStatusFacet(disabledGateway, ['enabled']), false)
})
```

- [ ] **Step 3: Run test to verify it fails**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-list-state.test.ts`
Expected: FAIL because `gateway-list-state.ts` does not exist yet.

- [ ] **Step 4: Write the minimal pure helpers and local types**

```ts
import type { Gateway } from '@/lib/types/gateway'

export type GatewayPrimaryLens = 'configured' | 'healthy' | 'disconnected'
export type ToolsExposureFilter = 'all' | 'exposed' | 'hidden'

export interface GatewayFilterState {
  primaryLens: GatewayPrimaryLens
  search: string
  status: string[]
  source: string[]
  transport: string[]
}

export interface ToolInventoryRow {
  gatewayId: string
  gatewayName: string
  source: string
  transport: Gateway['transport']
  toolName: string
  description: string
  exposed: boolean
}

export function aggregateToolsFromGateways(gateways: Gateway[]): ToolInventoryRow[] {
  return gateways.flatMap((gateway) =>
    gateway.discovery.tools.map((tool) => ({
      gatewayId: gateway.id,
      gatewayName: gateway.name,
      source: gateway.source ?? 'custom',
      transport: gateway.transport,
      toolName: tool.name,
      description: tool.description ?? '',
      exposed: tool.exposed,
    })),
  )
}
```

- [ ] **Step 5: Implement gateway and tools filter semantics exactly as specified**

```ts
function matchesOrFacet(selected: string[], actual: string) {
  return selected.length === 0 || selected.includes(actual)
}

export function filterGateways(gateways: Gateway[], state: GatewayFilterState) {
  return gateways.filter((gateway) => {
    if (state.primaryLens === 'healthy' && !(gateway.status.healthy && gateway.status.connected)) return false
    if (state.primaryLens === 'disconnected' && gateway.status.connected) return false
    if (state.primaryLens === 'configured' && !(gateway.configured ?? true)) return false

    const haystack = [
      gateway.name,
      gateway.config.url ?? '',
      gateway.config.command ?? '',
      gateway.source ?? '',
      gateway.transport,
      ...gateway.discovery.tools.map((tool) => tool.name),
    ].join(' ').toLowerCase()

    if (state.search && !haystack.includes(state.search.toLowerCase())) return false
    if (!matchesGatewayStatusFacet(gateway, state.status)) return false
    if (!matchesOrFacet(state.source, gateway.source === 'lab_service' ? 'lab' : 'custom')) return false
    if (!matchesOrFacet(state.transport, gateway.transport)) return false
    return true
  })
}

export function matchesGatewayStatusFacet(gateway: Gateway, selected: string[]) {
  if (selected.length === 0) return true

  const actual = new Set<string>()
  if (gateway.configured ?? true) actual.add('configured')
  if (gateway.status.healthy && gateway.status.connected) actual.add('healthy')
  if (!gateway.status.connected) actual.add('disconnected')
  if (gateway.enabled ?? true) actual.add('enabled')
  if (!(gateway.enabled ?? true)) actual.add('disabled')

  return selected.some((value) => actual.has(value))
}

export function filterTools(
  rows: ToolInventoryRow[],
  state: {
    search: string
    gatewayIds: string[]
    exposure: 'all' | 'exposed' | 'hidden'
    source: string[]
    transport: string[]
  },
) {
  return rows.filter((row) => {
    const haystack = [row.toolName, row.description, row.gatewayName].join(' ').toLowerCase()

    if (state.search && !haystack.includes(state.search.toLowerCase())) return false
    if (state.gatewayIds.length > 0 && !state.gatewayIds.includes(row.gatewayId)) return false
    if (state.exposure === 'exposed' && !row.exposed) return false
    if (state.exposure === 'hidden' && row.exposed) return false
    if (!matchesOrFacet(state.source, row.source === 'lab_service' ? 'lab' : 'custom')) return false
    if (!matchesOrFacet(state.transport, row.transport)) return false
    return true
  })
}
```

- [ ] **Step 6: Run the pure-state tests**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-list-state.test.ts`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-list-state.ts apps/gateway-admin/components/gateway/gateway-list-state.test.ts
git commit -m "feat: add gateway list lens and filtering state helpers"
```

## Task 2: Replace dropdown filters with checkbox filter groups and mobile sheet parity

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-filters.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-filters.test.tsx`
- Reference: `docs/design-system-contract.md`

- [ ] **Step 1: Write failing markup tests for checkbox groups and mobile parity hooks**

```ts
test('gateway filters render checkbox groups instead of selects', () => {
  const markup = renderToStaticMarkup(
    <GatewayFilters
      mode="gateways"
      search="plex"
      gatewayFilters={{ status: ['configured'], source: ['lab'], transport: ['stdio'] }}
      toolFilters={emptyToolFilters}
      onSearchChange={() => {}}
      onGatewayFilterToggle={() => {}}
      onToolFilterToggle={() => {}}
      onExposureChange={() => {}}
      onClearFilters={() => {}}
    />,
  )

  assert.match(markup, /type="checkbox"/)
  assert.match(markup, /Configured/)
  assert.match(markup, /Transport/)
  assert.doesNotMatch(markup, /role="combobox"/)
})
```

- [ ] **Step 2: Run test to verify it fails**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-filters.test.tsx`
Expected: FAIL because the component still renders `Select` controls.

- [ ] **Step 3: Refactor `GatewayFilters` props to be facet-driven instead of dropdown-driven**

```ts
export interface GatewayFiltersProps {
  mode: 'gateways' | 'tools'
  search: string
  gatewayFilters: {
    status: string[]
    source: string[]
    transport: string[]
  }
  toolFilters: {
    gatewayIds: string[]
    exposure: 'all' | 'exposed' | 'hidden'
    source: string[]
    transport: string[]
  }
  gatewayOptions: Array<{ value: string; label: string }>
  onSearchChange: (value: string) => void
  onGatewayFilterToggle: (group: 'status' | 'source' | 'transport', value: string) => void
  onToolFilterToggle: (group: 'gatewayIds' | 'source' | 'transport', value: string) => void
  onExposureChange: (value: 'all' | 'exposed' | 'hidden') => void
  onClearFilters: () => void
}
```

- [ ] **Step 4: Implement Aurora checkbox groups and segmented exposure control**

```tsx
function FilterCheckbox({ checked, label, onChange }: FilterCheckboxProps) {
  return (
    <label className="flex items-center gap-2 text-[13px] font-medium text-aurora-text-primary">
      <Checkbox checked={checked} onCheckedChange={onChange} />
      <span>{label}</span>
    </label>
  )
}
```

- [ ] **Step 5: Preserve desktop and mobile parity through layout only, not different filter semantics**

```tsx
<div className="hidden lg:block">{filterGroups}</div>
<Sheet>
  <SheetTrigger asChild>
    <Button className="lg:hidden" variant="outline">Filters</Button>
  </SheetTrigger>
  <SheetContent side="bottom">{filterGroups}</SheetContent>
</Sheet>
```

- [ ] **Step 6: Re-run the filter tests**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-filters.test.tsx`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-filters.tsx apps/gateway-admin/components/gateway/gateway-filters.test.tsx
git commit -m "feat: replace gateway dropdown filters with checkbox groups"
```

## Task 3: Move page state and header controls into `GatewayListContent`

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
- Create: `apps/gateway-admin/components/gateway/gateway-list-content.test.tsx`
- Reference: `apps/gateway-admin/components/app-header.tsx`
- Reference: `apps/gateway-admin/lib/types/gateway.ts`
- Reference: `apps/gateway-admin/components/gateway/gateway-list-state.ts`

- [ ] **Step 1: Add a page-level test harness with a defined fallback if `mock.module` is unreliable under `tsx --test`**

Primary approach:

- use `mock.module('@/lib/hooks/use-gateways', ...)` to provide stable gateway fixtures and no-op mutations

Fallback if the environment does not honor `mock.module` reliably:

- extract a small presentational child from `GatewayListContent`, such as `GatewayListView`, that receives already-derived props
- keep `GatewayListContent` thin and test the extracted child for header actions, lens switching, and content swaps

Do not proceed with brittle page-level assertions against live hooks. Pick one of these two harnesses and make it stable first.

- [ ] **Step 2: Add failing state-focused render test or targeted assertions through existing component tests**

```ts
mock.module('@/lib/hooks/use-gateways', () => ({
  useGateways: () => ({ data: fixtures, isLoading: false, error: null }),
  useGatewayMutations: () => ({
    testGateway: async () => ({ success: true, message: 'ok' }),
    reloadGateway: async () => ({ success: true, message: 'ok', previous_tool_count: 1, new_tool_count: 1 }),
    removeGateway: async () => {},
    createGateway: async () => {},
    updateGateway: async () => {},
    disableVirtualServer: async () => {},
  }),
}))

test('gateway list header exposes icon-only density toggles and contextual tools back action', async () => {
  const { GatewayListContent } = await import('./gateway-list-content')
  const markup = renderToStaticMarkup(<GatewayListContent />)

  assert.match(markup, /aria-label="Comfortable view"/)
  assert.match(markup, /aria-label="Condensed view"/)
})
```

- [ ] **Step 3: Run the test to verify it fails**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-list-content.test.tsx`
Expected: FAIL because the page does not yet expose these controls.

- [ ] **Step 4: Introduce page-level lens, density, and filter state**

```ts
const [primaryLens, setPrimaryLens] = useState<GatewayPrimaryLens | 'tools'>('configured')
const [density, setDensity] = useState<'comfortable' | 'condensed'>('comfortable')
const [gatewayFilters, setGatewayFilters] = useState({ status: [], source: [], transport: [] })
const [toolFilters, setToolFilters] = useState({ gatewayIds: [], exposure: 'all', source: [], transport: [] })
const [lastGatewayLens, setLastGatewayLens] = useState<GatewayPrimaryLens>('configured')
```

- [ ] **Step 5: Derive filtered gateways and aggregated tools rows with `useMemo`**

```ts
const toolRows = useMemo(() => aggregateToolsFromGateways(gateways ?? []), [gateways])
const filteredGateways = useMemo(
  () => filterGateways(gateways ?? [], { primaryLens: lastGatewayLens, search, ...gatewayFilters }),
  [gateways, lastGatewayLens, search, gatewayFilters],
)
const filteredToolRows = useMemo(
  () => filterTools(toolRows, { search, ...toolFilters }),
  [toolRows, search, toolFilters],
)
```

- [ ] **Step 6: Move density controls into `AppHeader` actions as icon-only buttons**

```tsx
<Button
  variant="outline"
  size="icon"
  aria-label="Comfortable view"
  aria-pressed={density === 'comfortable'}
  onClick={() => setDensity('comfortable')}
>
  <Rows3 className="size-4" />
</Button>
```

- [ ] **Step 7: Add the required sticky-header entrypoints for mobile filters and tools back-navigation**

```tsx
<>
  <Button className="lg:hidden" variant="outline" onClick={() => setFiltersSheetOpen(true)}>
    <SlidersHorizontal className="size-4" />
    <span className="sr-only">Open filters</span>
  </Button>
  <Button
    variant="outline"
    size="icon"
    aria-label="Comfortable view"
    aria-pressed={density === 'comfortable'}
    onClick={() => setDensity('comfortable')}
  >
    <Rows3 className="size-4" />
  </Button>
  <Button
    variant="outline"
    size="icon"
    aria-label="Condensed view"
    aria-pressed={density === 'condensed'}
    onClick={() => setDensity('condensed')}
  >
    <Rows2 className="size-4" />
  </Button>
  {primaryLens === 'tools' ? (
    <Button variant="outline" onClick={() => setPrimaryLens(lastGatewayLens)}>
      Back to gateways
    </Button>
  ) : null}
  <Button>Add Gateway</Button>
</>
```

- [ ] **Step 8: Wire summary cards so they reset the primary lens first**

```ts
function activatePrimaryLens(next: GatewayPrimaryLens | 'tools') {
  if (next === 'tools') {
    setPrimaryLens('tools')
    return
  }
  setLastGatewayLens(next)
  setPrimaryLens(next)
}
```

- [ ] **Step 9: Re-run the page-level test**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-list-content.test.tsx`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-list-content.tsx apps/gateway-admin/components/gateway/gateway-list-content.test.tsx

git commit -m "feat: add gateway page lens and density state"
```

## Task 4: Implement comfortable and condensed row specifications in `GatewayTable`

**Files:**
- Modify: `apps/gateway-admin/components/gateway/gateway-table.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-table.test.tsx`
- Reference: `apps/gateway-admin/components/gateway/warnings-pill.tsx`

- [ ] **Step 1: Extend the table test with condensed-mode and no-inline-error assertions**

```ts
test('gateway table condensed mode pulls launcher metadata into the identity row and removes inline errors', () => {
  const markup = renderToStaticMarkup(
    <GatewayTable
      gateways={[gateway]}
      density="condensed"
      onEdit={() => {}}
      onTest={() => {}}
      onReload={() => {}}
      onDelete={() => {}}
    />,
  )

  assert.match(markup, /https:\/\/plex\.example\.com\/mcp/)
  assert.doesNotMatch(markup, /Reload required to apply policy changes/)
})
```

- [ ] **Step 2: Run the table test to verify it fails**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-table.test.tsx`
Expected: FAIL because `density` does not exist yet and `last_error` still renders inline.

- [ ] **Step 3: Add a `density` prop and branch the row layout intentionally**

```ts
interface GatewayTableProps {
  gateways: Gateway[]
  density: 'comfortable' | 'condensed'
  onEdit: (gateway: Gateway) => void
  onTest: (gateway: Gateway) => void
  onReload: (gateway: Gateway) => void
  onDelete: (gateway: Gateway) => void
}
```

- [ ] **Step 4: Remove inline `last_error` rendering from mobile and desktop rows**

```tsx
{/* Do not render gateway.status.last_error in the list table. Warning detail stays behind the warning affordance. */}
```

- [ ] **Step 5: Render launcher metadata inline in condensed mode**

```tsx
const endpointPreview = buildGatewayEndpointPreview(gateway)
const launcherState = isDisabled ? 'deactivated' : 'active'

<div className="flex min-w-0 flex-wrap items-center gap-2">
  <Link href={gatewayDetailHref(gateway.id)}>{gateway.name}</Link>
  <TransportBadge transport={gateway.transport} />
  <WarningsPill warnings={gateway.warnings} />
  {density === 'condensed' ? (
    <span className="truncate text-[13px] text-aurora-text-muted">{endpointPreview} • {launcherState}</span>
  ) : null}
</div>
```

- [ ] **Step 6: Keep comfortable mode on a two-line hierarchy**

```tsx
{density === 'comfortable' ? (
  <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-aurora-text-muted">
    <span className="truncate" title={endpointPreview}>{endpointPreview}</span>
    <span>{launcherState}</span>
  </div>
) : null}
```

- [ ] **Step 7: Re-run the table test**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-table.test.tsx`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-table.tsx apps/gateway-admin/components/gateway/gateway-table.test.tsx
git commit -m "feat: add gateway table density modes"
```

## Task 5: Add the in-place tools inventory component

**Files:**
- Create: `apps/gateway-admin/components/gateway/gateway-tools-table.tsx`
- Create: `apps/gateway-admin/components/gateway/gateway-tools-table.test.tsx`
- Modify: `apps/gateway-admin/components/gateway/index.ts`
- Modify: `apps/gateway-admin/components/gateway/gateway-list-content.tsx`

- [ ] **Step 1: Write the failing tools inventory test**

```ts
test('gateway tools table renders dense operational rows and exposure state', () => {
  const markup = renderToStaticMarkup(
    <GatewayToolsTable
      rows={[
        {
          gatewayId: 'gw_1',
          gatewayName: 'Lab Core',
          source: 'lab_service',
          transport: 'stdio',
          toolName: 'unifi',
          description: 'UniFi Network Application local API',
          exposed: true,
        },
      ]}
    />,
  )

  assert.match(markup, /UniFi Network Application local API/)
  assert.match(markup, /Exposed/)
  assert.match(markup, /bg-aurora-panel-strong/)
})
```

- [ ] **Step 2: Run the tools inventory test to verify it fails**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-tools-table.test.tsx`
Expected: FAIL because the component does not exist yet.

- [ ] **Step 3: Create the tools inventory component with Aurora dense-data styling**

```tsx
export function GatewayToolsTable({ rows }: { rows: ToolInventoryRow[] }) {
  return (
    <div className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
      <Table className="table-fixed">
        <TableHeader>...</TableHeader>
        <TableBody>
          {rows.map((row) => (
            <TableRow key={`${row.gatewayId}:${row.toolName}`}>
              <TableCell>
                <div className="min-w-0">
                  <p className="text-sm font-medium text-aurora-text-primary">{row.toolName}</p>
                  <p className="text-xs text-aurora-text-muted">{row.description}</p>
                </div>
              </TableCell>
              <TableCell>{row.gatewayName}</TableCell>
              <TableCell>{row.exposed ? 'Exposed' : 'Hidden'}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  )
}
```

- [ ] **Step 4: Sort the filtered tools rows alphabetically before rendering**

```ts
const filteredToolRows = useMemo(
  () =>
    filterTools(toolRows, { search, ...toolFilters }).sort((a, b) =>
      a.toolName.localeCompare(b.toolName, undefined, { sensitivity: 'base' }),
    ),
  [toolRows, search, toolFilters],
)
```

- [ ] **Step 5: Swap the content area in `GatewayListContent` when the tools lens is active and split the empty-state branch**

```tsx
{primaryLens === 'tools' ? (
  filteredToolRows.length > 0 ? (
    <GatewayToolsTable rows={filteredToolRows} />
  ) : (
    <EmptyState
      title="No tools match the current filters"
      description="Adjust tools search or filters to broaden the inventory view."
    />
  )
) : (
  filteredGateways.length > 0 ? (
    <GatewayTable gateways={filteredGateways} density={density} ... />
  ) : (
    <EmptyState
      title="No gateways match the current filters"
      description="Adjust gateway filters or reset the primary lens."
    />
  )
)}
```

- [ ] **Step 6: Export the component and re-run the tools inventory test**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-tools-table.test.tsx`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add apps/gateway-admin/components/gateway/gateway-tools-table.tsx apps/gateway-admin/components/gateway/gateway-tools-table.test.tsx apps/gateway-admin/components/gateway/index.ts apps/gateway-admin/components/gateway/gateway-list-content.tsx
git commit -m "feat: add in-place gateway tools inventory"
```

## Task 6: Wire mobile warning disclosure and complete accessibility polish

**Files:**
- Modify: `apps/gateway-admin/components/gateway/warnings-pill.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-table.tsx`
- Modify: `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
- Reference: `docs/design-system-contract.md`

- [ ] **Step 1: Write a failing markup test for warning disclosure affordance and icon-only density control accessibility**

```ts
test('warnings pill remains compact and density controls expose accessible names', async () => {
  const tableMarkup = renderToStaticMarkup(<GatewayTable gateways={[gateway]} density="comfortable" ... />)
  const { GatewayListContent } = await import('./gateway-list-content')
  const pageMarkup = renderToStaticMarkup(<GatewayListContent />)

  assert.match(tableMarkup, /AlertTriangle/)
  assert.doesNotMatch(tableMarkup, /Tool exposure differs from the last successful sync/)
  assert.match(pageMarkup, /aria-label="Comfortable view"/)
  assert.match(pageMarkup, /aria-label="Condensed view"/)
})
```

- [ ] **Step 2: Run the accessibility-focused test to verify it fails**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-list-content.test.tsx apps/gateway-admin/components/gateway/gateway-table.test.tsx`
Expected: FAIL until the header controls and disclosure behavior are fully wired.

- [ ] **Step 3: Update `WarningsPill` so desktop and mobile share the same compact trigger but use touch-safe disclosure**

```tsx
<Tooltip>
  <TooltipTrigger asChild>
    <button type="button" className={warningPillClass} aria-label={`${warnings.length} warnings`}>
      <AlertTriangle className="size-3" />
      {warnings.length}
    </button>
  </TooltipTrigger>
  <TooltipContent>...</TooltipContent>
</Tooltip>
```

If the existing tooltip primitive is not touch-safe enough, replace it with a compact `Popover` trigger while preserving the same visual trigger.

- [ ] **Step 4: Ensure the header density controls expose programmatic selected state and Aurora focus-visible treatment**

```tsx
<Button
  aria-label="Condensed view"
  aria-pressed={density === 'condensed'}
  className="focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/34"
>
  <PanelTopClose className="size-4" />
</Button>
```

- [ ] **Step 5: Re-run the accessibility-focused tests**

Run: `pnpm exec tsx --test apps/gateway-admin/components/gateway/gateway-list-content.test.tsx apps/gateway-admin/components/gateway/gateway-table.test.tsx`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add apps/gateway-admin/components/gateway/warnings-pill.tsx apps/gateway-admin/components/gateway/gateway-table.tsx apps/gateway-admin/components/gateway/gateway-list-content.tsx

git commit -m "feat: complete gateway accessibility and warning disclosure polish"
```

## Task 7: Update the design-system sandbox if the new patterns are shared references

**Files:**
- Modify: `apps/gateway-admin/components/design-system/design-system-shell.tsx`
- Reference: `docs/design-system-contract.md`

- [ ] **Step 1: Add a failing design-system shell test if needed**

```ts
assert.match(markup, /Gateway filter rail/)
assert.match(markup, /Density toggles/)
```

- [ ] **Step 2: Run the shell test to verify it fails**

Run: `pnpm exec tsx --test apps/gateway-admin/components/design-system/design-system-shell.test.tsx`
Expected: FAIL if the sandbox does not yet show these patterns.

- [ ] **Step 3: Add a compact sandbox section for checkbox filter rails and icon-only density toggles**

```tsx
<section>
  <h2>Gateway filter rail</h2>
  <p>Reference interaction for Aurora checkbox filters and header density toggles.</p>
  ...
</section>
```

- [ ] **Step 4: Re-run the sandbox test**

Run: `pnpm exec tsx --test apps/gateway-admin/components/design-system/design-system-shell.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add apps/gateway-admin/components/design-system/design-system-shell.tsx apps/gateway-admin/components/design-system/design-system-shell.test.tsx
git commit -m "docs: add gateway filter and density patterns to design system sandbox"
```

## Final verification task

**Files:**
- Verify only the files touched above.

- [ ] **Step 1: Run focused gateway component tests**

Run:

```bash
pnpm exec tsx --test \
  apps/gateway-admin/components/gateway/gateway-list-state.test.ts \
  apps/gateway-admin/components/gateway/gateway-filters.test.tsx \
  apps/gateway-admin/components/gateway/gateway-table.test.tsx \
  apps/gateway-admin/components/gateway/gateway-tools-table.test.tsx \
  apps/gateway-admin/components/gateway/gateway-list-content.test.tsx
```

Expected: PASS.

- [ ] **Step 2: Run the design-system sandbox test if Task 7 changed the sandbox**

Run: `pnpm exec tsx --test apps/gateway-admin/components/design-system/design-system-shell.test.tsx`
Expected: PASS.

- [ ] **Step 3: Run the smallest relevant lint/typecheck slice for gateway-admin**

Run: `pnpm --dir apps/gateway-admin lint`
Expected: PASS.

- [ ] **Step 4: Commit the final verification or cleanup changes**

```bash
git add apps/gateway-admin/components/gateway apps/gateway-admin/components/design-system

git commit -m "test: verify gateways redesign"
```
