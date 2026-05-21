# Gateways Mobile Density Design

Date: 2026-04-23
Surface: `apps/gateway-admin/app/(admin)/gateways/page.tsx`
Scope: Mobile-only information architecture and density pass for `/gateways`

## Goal

Make `/gateways` materially denser and more operator-friendly on mobile while staying inside the Aurora design-system contract.

The current mobile layout spends too much vertical space on summary cards and always-visible control chrome. The target state is a compact, glanceable operator surface where search and the gateway table dominate the viewport.

## Constraints

- Follow `docs/design-system-contract.md` strictly.
- Preserve the Gateways page language:
  - Aurora summary strip
  - Aurora filters and pills
  - tiered panel hierarchy
  - calmer status accents
- Use Aurora semantic tokens only in product code.
- Do not invent a separate mobile theme or alternate component language.
- Keep desktop workflow and desktop density intact unless a shared component change is unavoidable.

## Approved Mobile Information Architecture

### 1. Header and primary actions

On mobile, the page keeps:

- page title: `Gateways`
- compact tools-entry action
- compact add action

The mobile header should not spend space on full text controls for density toggles, filter drawers, or summary cards.

### 2. Summary strip

Replace the four large summary cards with one compact strip directly under the title.

Rules:

- four items remain visible by default
- each item is icon + number only
- no labels in the strip on mobile
- strip behaves like compact telemetry, not hero cards
- active state is restrained and Aurora-aligned

Metrics retained:

- configured gateways
- healthy gateways
- disconnected gateways
- discovered tools

### 3. Search-first control row

Place search immediately under the summary strip.

Rules:

- search is the primary control
- filter access is embedded into the search control itself
- no separate full-width filter button row
- any density or secondary utility control should be icon-sized and co-located with search, not elevated into its own row

### 4. Hidden-by-default filters

Secondary filters are hidden by default on mobile.

When revealed:

- show compact filter pills/chips directly below the search control
- do not render the desktop left-rail filter panel on mobile
- do not keep a sheet or drawer open by default

Supported filter categories remain the same; only the mobile presentation changes.

### 5. Table-first viewport use

The gateway table becomes the dominant mobile surface.

Rules:

- table starts as high on the page as practical
- summary and controls should consume minimal height
- mobile layout should prioritize visible rows over decorative framing

## Approved Mobile Table Structure

### Visible columns

Mobile should not keep the current wider desktop column model.

Approved mobile structure:

- `Gateway`
- `State`
- overflow menu

Remove the separate mobile tools column.

### Gateway cell content

The gateway cell carries all glanceable metadata by default.

Order:

1. status dot + gateway name
2. endpoint / transport line
3. inline metrics row

Inline metrics shown by default:

- tools
- resources
- prompts
- runtime

These must be inline and compact, not rendered as separate stacked cards or lower detail panels.

### State column

The right-side state block remains compact and glanceable.

It should communicate:

- primary state label such as `Live`, `Stale`, `Down`
- secondary warning/health context

Keep this inline and dense, not vertically elaborate.

### Removed mobile affordances

The following mobile patterns are explicitly not part of the approved design:

- four large summary cards
- separate tools column
- resource/prompt/runtime detail cards
- permanently visible filter panel
- dedicated full-width filter button row

## Density Rules

### Rows

Rows should be tighter than the current mobile rendering.

Approved density direction:

- reduced vertical padding
- inline status and metric treatment
- avoid stacked mini-panels inside rows
- preserve readable text sizing and touch targets

### Readability

Density should not come from abbreviations that reduce comprehension.

Rules:

- avoid cryptic headers like `EXP`
- if information can be made clearer by folding it into the main gateway cell, prefer that over inventing shorthand
- maintain strong contrast and non-color cues for status

## Interaction Model

Mobile remains interactive and realistic rather than static.

Expected interaction behavior:

- summary strip can indicate active lens
- search is a real input
- filter control reveals compact filters
- rows remain actionable
- overflow menu remains present

The mobile design does not require separate detail cards for resource/prompt/runtime data because those metrics are already visible inline by default.

## Implementation Scope

Primary implementation targets are expected to be:

- `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
- `apps/gateway-admin/components/gateway/gateway-filters.tsx`
- `apps/gateway-admin/components/gateway/gateway-table.tsx`

Potential shared token or recipe adjustments are acceptable only if they strengthen the existing Aurora patterns and do not create one-off styling.

## Non-Goals

- redesigning desktop `/gateways`
- changing backend data shape
- changing filtering semantics
- redesigning `/gateway` detail pages
- redesigning `/docs`, `/settings`, or other admin surfaces in the same pass

## Acceptance Criteria

The pass is successful when, on mobile:

- the 4-card summary block is replaced by a compact icon-and-number strip
- search appears directly under the strip
- filter access is embedded into the search control area
- secondary filters are hidden by default
- the gateway table occupies most of the viewport
- the mobile table has no separate tools column
- tools, resources, prompts, and runtime are visible inline in each gateway row by default
- there are no separate resource/prompt/runtime cards
- the result remains clearly Aurora-aligned and operator-readable

## Notes

This design intentionally optimizes for dense operational scanning on mobile rather than preserving desktop hierarchy. The page should feel like a compressed control surface, not a card dashboard squeezed onto a phone.
