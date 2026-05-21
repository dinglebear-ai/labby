# Aurora Dark Theme Spec

**Date:** 2026-04-17
**Status:** Approved
**Scope:** Labby dark-mode visual system, using `/logs/` as the first reference implementation

## Summary

Aurora is the chosen dark-mode direction for Labby.

The target is a premium control-plane visual language that feels clean, modern, and welcoming rather than severe. Aurora should feel more refined than a generic dark dashboard, but it must not drift into glossy sci-fi styling, security-console intimidation, or decorative startup-dark-mode excess.

This spec defines the concrete visual system for the first implementation pass:

- dark mode only
- deep navy page surfaces
- noticeable but tiered panel elevation
- restrained active states
- cyan-blue primary accent family
- muted warning and error accents
- Manrope display typography with Inter as the working UI sans
- pill-style checkbox filters for multi-select controls
- shared global tokens instead of page-local color picks

## Design Principles

### Premium, Not Glossy

Aurora should use depth, contrast, and material separation to feel refined. It should not rely on bright radial sheen, glassy overlays, or heavy filled active states.

### Operator-First

Dense logs, filters, metrics, and metadata must stay easy to scan. Visual character should support hierarchy, not compete with the data.

### Calm Hierarchy

Hierarchy should come from elevation, typography, spacing, and measured accent use. The UI should feel composed rather than hyperactive.

## Typography

Aurora uses a split typography system:

- **Display family:** `Manrope`
- **Working UI family:** `Inter`

### Manrope Usage

Use `Manrope` for:

- page titles
- section headers
- top-level metric numbers

### Inter Usage

Use `Inter` for:

- nav labels
- buttons and controls
- tables
- dense log rows
- inspector metadata
- general body copy

### Rule

Do not use `Manrope` across the full operational UI. The split system keeps the product refined while preserving scanability.

## Typography Ramp

Aurora should use a fixed type ramp rather than page-by-page font sizing.

### Display Styles

- **Display 1**
  - use: major page titles
  - family: `Manrope`
  - size: `34px`
  - line-height: `1.04`
  - weight: `800`
  - tracking: `-0.045em`

- **Display 2**
  - use: major section headers
  - family: `Manrope`
  - size: `19px`
  - line-height: `1.12`
  - weight: `700`
  - tracking: `-0.02em`

- **Metric Display**
  - use: top-level metric numbers
  - family: `Manrope`
  - size: `28px`
  - line-height: `1`
  - weight: `800`
  - tracking: `-0.04em`
  - use tabular numerals when possible

### Working UI Styles

- **Body**
  - use: standard copy, helper text
  - family: `Inter`
  - size: `14px`
  - line-height: `1.55`
  - weight: `400`

- **Control**
  - use: buttons, inputs, selects, pill labels
  - family: `Inter`
  - size: `13px`
  - line-height: `1.2`
  - weight: `500` to `600`

- **Dense Data**
  - use: log rows, tables, inspector fields
  - family: `Inter` or mono where needed
  - size: `12px` to `13px`
  - line-height: `1.35` to `1.5`

- **Eyebrow**
  - use: small labels above metrics, sections, groups
  - family: `Inter`
  - size: `10px` to `11px`
  - line-height: `1`
  - weight: `700`
  - uppercase
  - tracking: `0.14em` to `0.18em`

## Color Tokens

Aurora should be implemented through named global tokens. The initial token contract is:

### Base Surfaces

- `--aurora-page-bg: #07131c`
- `--aurora-nav-bg: #07111a`
- `--aurora-panel-medium: #102330`
- `--aurora-panel-medium-top: rgba(20, 44, 60, 0.18)`
- `--aurora-panel-strong: #13293a`
- `--aurora-panel-strong-top: #173245`
- `--aurora-control-surface: #0c1a24`
- `--aurora-control-surface-top: rgba(18, 40, 56, 0.96)`

### Borders And Text

- `--aurora-border-default: #1d3d4e`
- `--aurora-border-strong: #24536c`
- `--aurora-text-primary: #e6f4fb`
- `--aurora-text-muted: #90a9b9`

### Accent Family

- `--aurora-accent-primary: #29b6f6`
- `--aurora-accent-strong: #67cbfa`
- `--aurora-accent-deep: #1c7fac`

### State Colors

- `--aurora-warn: #c6a36b`
- `--aurora-error: #c78490`
- `--aurora-focus-ring: rgba(41, 182, 246, 0.34)`

These warning and error values are intentionally muted down. They should remain readable, but they should not feel like bright alert banners.

## Elevation Tokens

Aurora uses **noticeable lift** with **tiered elevation**.

### Tier 0

The page base is the deepest navy surface and should remain clearly behind all working surfaces.

### Tier 1

Use medium lift for:

- toolbars
- header/control panels
- support containers

Supporting tokens:

- `--aurora-shadow-medium: 0 12px 24px rgba(0, 0, 0, 0.18)`
- `--aurora-highlight-medium: inset 0 1px 0 rgba(255, 255, 255, 0.035)`

### Tier 2

Use strong lift for:

- primary streams
- inspectors
- major content panels
- strong metrics surfaces

Supporting tokens:

- `--aurora-shadow-strong: 0 20px 38px rgba(0, 0, 0, 0.26)`
- `--aurora-highlight-strong: inset 0 1px 0 rgba(255, 255, 255, 0.05)`

### Rule

Aurora should avoid the “field of equal cards” problem. Toolbar/header layers should be calmer than main content layers.

## Active State Rules

Aurora uses restrained active states.

Selected and active UI should communicate state primarily through:

- border change
- text emphasis
- indicator dots
- subtle glow
- slight surface deepening

Aurora should avoid:

- glossy selected fills
- bright gradient buttons
- heavy accent washes

Supporting token:

- `--aurora-active-glow: 0 0 0 1px rgba(41, 182, 246, 0.18), 0 0 16px rgba(41, 182, 246, 0.08)`

## Radius Scale

Aurora should use a small, fixed radius system.

- **Radius 1**
  - use: dense controls, inputs, small buttons
  - value: `14px`

- **Radius 2**
  - use: inline cards, expanded metadata blocks, secondary panes
  - value: `18px`

- **Radius 3**
  - use: major panels, toolbars, stream containers, inspectors
  - value: `22px`

- **Radius pill**
  - use: pill filters, rounded badges, compact state chips
  - value: `999px`

### Rule

Do not improvise a different radius for every component. Aurora should feel intentionally tooled, not inconsistently softened.

## Spacing Scale

Aurora should use a consistent spacing system anchored around compact operator layouts.

### Core Spacing

- `4px` for micro alignment and icon gaps
- `8px` for tight internal spacing
- `10px` for compact control grouping
- `12px` for dense section gaps
- `14px` for row and chip breathing room
- `16px` for standard inner padding
- `18px` for toolbar and pane header padding
- `20px` for strong panel content padding
- `24px` for page-level grouping inside a section

### Layout Rules

- major panels should usually use `18px` to `20px` padding
- toolbars should usually use `16px` to `18px` padding
- chip groups should use `10px` gaps
- stacked panel sections should generally separate by `16px` to `24px`

Aurora should avoid oversized empty areas. It should feel premium through structure, not through wasted space.

## Data Density Rules

Aurora is a control-plane system, so dense operational surfaces need explicit consistency rules.

### Rows And Tables

- default dense row height: `42px`
- compact expanded rows may grow vertically, but default rows should stay scan-first
- table or stream headers should use eyebrow styling rather than large title styling

### Monospace Usage

Use mono only for:

- timestamps
- levels when helpful
- subsystem keys
- JSON
- identifiers, codes, and command-like values

Do not use mono for general explanatory copy.

### Truncation

- default stream rows should truncate long messages
- expanded or selected views may wrap
- inspector values may wrap when needed

### Rule

The main stream should always optimize for fast scanning first, detail second.

## Filter Pattern

Aurora uses pill-style checkbox filters for multi-select filter groups.

### Pill Checkbox Contract

Each pill should include:

- rounded capsule shape
- integrated circular state indicator
- quiet off state
- brighter but still flat on state
- restrained outer glow only when selected

The selected indicator carries most of the “on” energy. The fill should stay flatter and calmer than the earlier glossy prototype.

## `/logs/` Reference Application

The `/logs/` page is the first Aurora reference implementation.

That page should reflect:

- regular Labby shell
- Tier 1 toolbar lift
- Tier 2 stream and inspector lift
- `Manrope` section headers and metrics
- `Inter` working UI and dense rows
- restrained active states
- pill-style filter controls

## Implementation Rule

Aurora should roll out as a system, not as one-off component decoration.

New pages should consume named tokens for:

- page background
- panel lift
- borders
- text
- accent
- warn/error
- glow

Hardcoded page-local color choices should be treated as temporary and phased out in favor of token-backed values.

## Next Theme Locks

After this spec, the next system-level pieces to lock should be:

- button variant taxonomy
- status badge treatment
- focus-visible treatment across all controls
- tab and nav selected-state recipes
