# Labby Design System Contract

**Status:** Active  
**Scope:** `apps/gateway-admin` web UI  
**Mode:** Dark (primary) and light  
**Primary reference sandbox:** `/design-system`

## Purpose

This document is the root-level source of truth for the Labby web UI design system.

It defines the visual and interaction contract that new pages, refactors, and component work must follow. The goal is to keep Labby consistent as a premium control plane: clean, modern, and operator-friendly without feeling glossy, intimidating, or over-styled.

This contract is implementation-facing. It exists so product pages do not drift into one-off styling decisions.

Related documents:

- [Aurora Dark Theme Spec](../superpowers/specs/2026-04-17-aurora-dark-theme-spec.md)
- [Design System Page Plan](../superpowers/plans/2026-04-17-design-system-page.md)

## Theme Modes

Aurora is designed dark-first. A light remap of the same token system is also supported so users can switch via the theme toggle in the sidebar footer (`next-themes` system/light/dark).

Rules:

- the canonical visual reference is dark; design and review work happens against dark first
- the light remap lives in `app/globals.css` under `.light` and overrides only the raw Aurora variables; semantic tokens in `:root` already resolve correctly
- both modes must stay on the same semantic-token surface — do not branch on `dark:` selectors with one-off colors when the Aurora system already provides a token
- any new Aurora token added to dark mode must be paired with a light-mode value in the same commit
- light mode must be verified in `/design-system` before a shared surface, status color, chart, or form primitive is considered complete; dark-first does not mean dark-only
- light hover treatments must preserve visible delta from adjacent surfaces. The current light `--aurora-hover-bg` value must be checked against `--aurora-control-surface` whenever either token changes.
- every status color used in light mode must pass WCAG contrast in its real combinations, including panel-medium, panel-strong, and control-surface contexts; do not only check status text on white

## Product Direction

Labby uses the `Aurora` theme language.

Aurora should feel:

- premium, but not glossy
- modern, but not flashy
- calm, but not dull
- dense enough for operator workflows, but not daunting

Aurora should avoid:

- bright neon dark-mode styling
- heavy gradient fills for standard controls
- high-alert warning and error colors used by default
- a field of identical flat cards
- decorative motion that competes with operational data

## Typography

Labby uses a split typography system.

- Display family: `Manrope`
- Working UI family: `Inter`

### `Manrope` Usage

Use `Manrope` for:

- page titles
- section headers
- top-level metric numbers

### `Inter` Usage

Use `Inter` for:

- navigation labels
- buttons and controls
- forms
- tables
- logs and dense operational rows
- metadata and inspectors
- body copy

### Typography Ramp

When a call site uses an `AURORA_DISPLAY_*` token, the token is the **complete** specification. Do not override its size (`text-2xl`, `text-3xl`), weight (`font-semibold`, `font-bold`), or tracking (`tracking-tight`) via additional classes — Tailwind's merge order will silently win and your "token-based" typography will render as something else entirely. Color is the only permitted companion class (e.g. `text-aurora-text-primary`, `text-aurora-warn`). If you need a size the ramp doesn't cover, add a new ramp slot in this document and in `components/aurora/tokens.ts`; do not patch it per call site.

Line-height is also tokenized. Typography constants and Tailwind utilities should reference these values rather than hardcoding local line-height numbers:

| Token | Value | Use |
|---|---:|---|
| `--lh-display-tight` | `1.04` | Display 1 and other intentionally tight hero-scale text |
| `--lh-display` | `1.16` | Display 2, card titles, compact headings |
| `--lh-body` | `1.55` | standard body copy and helper text |
| `--lh-dense` | `1.4` | dense rows, metadata, inspectors, compact controls |

Allowed exceptions are metric numerals and badge/eyebrow labels where the line box intentionally collapses to `1`.

#### Display 1

- family: `Manrope`
- size: `34px`
- line-height: `--lh-display-tight` (`1.04`)
- weight: `800`
- tracking: `-0.045em`
- use: major page titles

#### Display 2

- family: `Manrope`
- size: `19px`
- line-height: `--lh-display` (`1.16`)
- weight: `700`
- tracking: `-0.02em`
- use: section headers

#### Compact Title

- token: `AURORA_COMPACT_TITLE`
- family: `Manrope`
- size: `17px`
- line-height: `--lh-display` (`1.16`)
- weight: `800`
- tracking: `-0.02em`
- use: empty-state titles and compact panel headings that sit below Display 2

#### Card Title

- token: `AURORA_CARD_TITLE`
- family: `Manrope`
- size: `15px`
- line-height: `--lh-display` (`1.16`)
- weight: `800`
- tracking: `-0.02em`
- use: dense catalog/list card titles

#### Metric Display

- family: `Manrope`
- size: `28px`
- line-height: `1`
- weight: `800`
- tracking: `-0.04em`
- use: top-level metric numbers
- use tabular numerals when possible

#### Body

- family: `Inter`
- size: `14px`
- line-height: `--lh-body` (`1.55`)
- weight: `400`
- use: standard copy, helper text

#### Control

- family: `Inter`
- size: `13px`
- line-height: `1.2`
- weight: `500` to `600`
- use: buttons, inputs, selects, pills

#### Dense Data

- family: `Inter` or mono where required
- size: `12px` to `13px`
- line-height: `--lh-dense` (`1.4`)
- use: tables, logs, inspectors

#### Dense Metadata

- token: `AURORA_DENSE_META`
- family: `Inter`
- size: `11px`
- line-height: `--lh-dense` (`1.4`)
- weight: `500`
- use: card subtitles, package identifiers, versions, compact helper metadata, and table support text

#### Eyebrow

- family: `Inter`
- size: `10px` to `11px`
- line-height: `1`
- weight: `700`
- tracking: `0.14em` to `0.18em`
- use uppercase

The single sanctioned Eyebrow implementation is `AURORA_MUTED_LABEL`. Do **not** hand-roll eyebrow styling with `text-xs uppercase tracking-*` — the 1px size delta, the 500-vs-700 weight gap, and the arbitrary tracking value all compound into visible inconsistency. A `text-xs uppercase tracking-[...]` combination in product code is a drift signal and must be migrated.

#### Badge Label

- token: `AURORA_BADGE_LABEL`
- family: `Inter`
- size: `10px`
- line-height: `1`
- weight: `700`
- tracking: `0.14em`
- use uppercase
- use: compact status, category, active-filter, and count badges

`AURORA_BADGE_LABEL` is for small badges and chips only. Do not use it for section labels; section labels must use `AURORA_MUTED_LABEL`.

## Color Contract

All new page and component styling should use semantic Aurora tokens rather than embedded one-off hex values.

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
- `--aurora-text-muted: #a7bcc9` — lifted from the original `#90a9b9` to clear WCAG AA (≥4.5:1) on `--aurora-panel-medium`. The lighter value is the source of truth.

### Accent Family

- `--aurora-accent-primary: #29b6f6`
- `--aurora-accent-strong: #67cbfa`
- `--aurora-accent-deep: #1c7fac`

### Opacity Scale

Aurora uses a named tint scale instead of ad-hoc alpha suffixes. Existing `/12`, `/18`, `/22`, `/28`, `/30`, `/35`, and `/40` usages should migrate to semantic tint tokens.

| Token | Value | Use |
|---|---:|---|
| `--aurora-tint-subtle` | `12%` | quiet icon boxes, inactive accents, chart ramp floor |
| `--aurora-tint-soft` | `18%` | default accent tint, soft selected state |
| `--aurora-tint-medium` | `30%` | active badges, stronger control tint |
| `--aurora-tint-strong` | `40%` | high-emphasis tint that still stops short of a filled state |

Use these through `color-mix()` against the semantic color token, for example `color-mix(in srgb, var(--aurora-accent-primary) var(--aurora-tint-soft), transparent)`. Do not introduce new `bg-*/15`, `bg-*/22`, or `rgba(..., 0.28)` variants in product code; add a named tint when a new alpha level is genuinely needed.

### State Colors

- `--aurora-warn: #c6a36b`
- `--aurora-error: #c78490`
- `--aurora-success: #7dd3c7` — muted teal in the accent family; used for success/connected/authorized states. Reads calm, not neon.
- `--aurora-focus-ring: rgba(41, 182, 246, 0.34)`
- `--aurora-active-glow: 0 0 0 1px rgba(41, 182, 246, 0.18), 0 0 16px rgba(41, 182, 246, 0.08)`

### Data Visualization Tokens

Charts must use chart-specific tokens, not badge/status tokens by habit.

- `--aurora-chart-positive` — positive deltas and up-trends; distinct from `--aurora-success`
- `--aurora-chart-negative` — negative deltas and down-trends; distinct from `--aurora-error`
- `--aurora-chart-grid` — grid lines; lighter and quieter than `--aurora-border-default`
- `--aurora-chart-axis-label` — axis and tick labels; quieter than `--aurora-text-muted` where needed

Sequential chart ramps should use a five-step Aurora ramp from control surface to accent strong:

| Token | Use |
|---|---|
| `--aurora-chart-ramp-1` | lowest value / background bin, near `--aurora-control-surface` |
| `--aurora-chart-ramp-2` | low value |
| `--aurora-chart-ramp-3` | middle value |
| `--aurora-chart-ramp-4` | high value |
| `--aurora-chart-ramp-5` | highest value, near `--aurora-accent-strong` |

Do not use `--aurora-success` / `--aurora-error` for chart fills unless the chart is explicitly a status indicator rather than a data visualization.

### Interaction Helper Tokens

- `--aurora-hover-bg: #17364b` — single source for hover backgrounds on outline/ghost controls. Use the `bg-aurora-hover-bg` utility, never the raw hex.

### Live-Preview Tokens (restricted scope)

These tokens intentionally sit brighter than the Aurora muted family. They exist so a real-time visualization surface can signal `allowed` / `unmatched` / `highlight` states at a glance. They are **not** general-purpose status colors.

- `--aurora-preview-allowed: #00e676`
- `--aurora-preview-unmatched: #ff9100`
- `--aurora-preview-highlight: #ffea00`

Sanctioned use:

- the exposure-policy editor's live-preview surface (`components/gateway/exposure-policy-editor.tsx`)

These are the only place in Labby where saturated/neon color and decorative gradients are allowed. Any new use must be added to this list — do not reach for these tokens for routine status display.

### Status Guidance

Status accents must stay muted and integrated into the Aurora family.

- success/live should read clear, not neon
- warning should read informational before alarming
- error should read serious without becoming the loudest surface on the page by default

## Elevation And Surface Contract

Aurora uses noticeable lift with tiers.

### Tier 0

- the page canvas
- deepest navy surface
- should clearly sit behind all working UI
- intentionally flat: no Tier 0 shadow token

### Tier 1

Use for:

- toolbars
- header control strips
- secondary support panels

Supporting tokens:

- `--aurora-shadow-subtle: 0 8px 16px rgba(0, 0, 0, 0.16)` — subtle lift for cards or stats nested inside a panel
- `--aurora-shadow-medium: 0 12px 24px rgba(0, 0, 0, 0.18)`
- `--aurora-highlight-medium: inset 0 1px 0 rgba(255, 255, 255, 0.035)`

### Tier 2

Use for:

- primary data panels
- inspectors
- strong metric surfaces
- main stream containers

Supporting tokens:

- `--aurora-shadow-strong: 0 20px 38px rgba(0, 0, 0, 0.26)`
- `--aurora-highlight-strong: inset 0 1px 0 rgba(255, 255, 255, 0.05)`

### Rule

Do not flatten major surfaces into a single visual plane. Toolbar/header layers should be calmer than main content layers.

## Radius Contract

Aurora uses a token-backed radius scale exposed as both CSS variables and Tailwind utilities.

| Token | Value | Tailwind utility | Use |
|-------|-------|------------------|-----|
| `--radius-1` | `14px` | `rounded-aurora-1` | dense controls, small buttons |
| `--radius-2` | `18px` | `rounded-aurora-2` | inline cards, metadata blocks |
| `--radius-3` | `22px` | `rounded-aurora-3` | major panels, toolbars, inspectors |
| `--radius-pill` | `999px` | `rounded-full` | filter chips, compact state badges |

Rules:

- new code must reach for the tokens (`rounded-aurora-1/2/3`) before any arbitrary value
- one remaining arbitrary radius is tolerated as a **legacy tuned variant**: `rounded-[1rem]` (16px) sits between Radius 1 and 2 with no clean home and may be migrated to `rounded-aurora-2` incrementally
- `Button` defaults to `rounded-md` (~8px) for compact dense controls — this is below Radius 1 by design and stays as-is
- do not introduce new arbitrary radii outside the table above without updating this contract

## Spacing Contract

Aurora uses a compact operator spacing system.

Spacing is tokenized as CSS variables and Tailwind utilities:

| Token | Value | Tailwind utility | Use |
|---|---:|---|---|
| `--space-1` | `4px` | `gap-space-1`, `p-space-1`, etc. | micro alignment and icon gaps |
| `--space-2` | `8px` | `gap-space-2`, `p-space-2`, etc. | tight internal spacing |
| `--space-3` | `10px` | `gap-space-3`, `p-space-3`, etc. | compact control grouping |
| `--space-4` | `12px` | `gap-space-4`, `p-space-4`, etc. | default control padding and short stacks |
| `--space-5` | `16px` | `gap-space-5`, `p-space-5`, etc. | standard card/panel padding |
| `--space-6` | `20px` | `gap-space-6`, `p-space-6`, etc. | major group spacing |
| `--space-7` | `24px` | `gap-space-7`, `p-space-7`, etc. | section separation |

### Rule

Favor compact clarity over large breathable marketing spacing. Labby is an operational UI.

Do not introduce new arbitrary spacing values in product code for common gaps and padding. If a spacing value appears repeatedly, add it to this table and the Tailwind spacing scale before using it broadly.

## Layering And Z-Index Contract

Aurora layers use named z-index tokens so overlays compose predictably.

| Token | Value | Use |
|---|---:|---|
| `--z-sidebar` | `30` | persistent app sidebar and navigation shell |
| `--z-popover` | `40` | menus, selects, comboboxes, hover cards, command suggestions |
| `--z-modal` | `50` | dialogs, sheets, blocking overlays, command palette |
| `--z-toast` | `60` | toasts and urgent non-blocking feedback |

Product code should not invent page-local `z-[999]` values. If a new layer type is needed, define its relationship to these four layers first.

## Active, Hover, And Focus Rules

Aurora uses restrained active states.

State should be communicated through:

- border change
- text emphasis
- indicator dots or check marks
- slight surface deepening
- subtle outer glow

Aurora should avoid:

- glossy selected fills
- bright radial sheen
- full-accent flooded control backgrounds for standard selection

### Focus

Interactive elements must present a clear focus-visible treatment using the shared Aurora focus ring language.

### Motion

Motion should be minimal, functional, and tokenized.

| Token | Value | Use |
|---|---:|---|
| `--motion-duration-instant` | `100ms` | toggles, taps, checkbox/radio state |
| `--motion-duration-fast` | `160ms` | hover and color transitions |
| `--motion-duration-medium` | `240ms` | expand/collapse, popover and sheet entry |
| `--motion-duration-slow` | `360ms` | route or major panel transitions |
| `--motion-ease-out` | `cubic-bezier(0.2, 0.8, 0.2, 1)` | default entrance and hover easing |
| `--motion-ease-in-out` | `cubic-bezier(0.4, 0, 0.2, 1)` | panels, sheets, expand/collapse |

`prefers-reduced-motion: reduce` must zero or near-zero these durations and disable shimmer, transform, and route-motion effects.

Allow:

- hover transitions
- expand/collapse transitions
- loading and progress indicators

Avoid:

- decorative ambient motion
- large animated glows
- animated gradients in normal control states

Loading states use one shared skeleton treatment: Aurora muted/control-surface colors, subdued contrast, left-to-right shimmer, and `--motion-duration-slow` timing. Do not use bright white shimmer defaults from generic component libraries.

## Component Contract

### Buttons

Labby uses a button hierarchy:

- primary
- secondary
- outline
- ghost
- destructive

Rules:

- primary actions should use the Aurora accent family with restrained fill and clear contrast
- secondary and outline actions should rely on surface, border, and text hierarchy rather than louder color
- destructive actions should remain clearly distinct, but still muted within Aurora
- buttons should not introduce one-off shadows, radii, or typography

### Pills And Multi-Select Filters

Pill-style checkbox controls are part of the system.

Rules:

- default state should stay flat and calm
- selected state should use border emphasis, a subtle glow, and a clear internal indicator
- selected pills should not use bright sheen or heavy filled gradients

Dense filter rails are the approved exception to pill-style multi-select controls. Gateway-style sidebar filter rails may use compact checkbox rows when a page has many filter groups or many values per group. Checkbox-row filters must still use Aurora control surfaces, semantic borders, visible focus treatment, and compact Inter control typography.

### Inputs And Selects

Rules:

- use Aurora control surfaces
- keep borders and focus states consistent with the shared token system
- validation states must layer on top of the control contract rather than replace it with unrelated styling
- validation must be exposed through composable variants: `status="error" | "warn" | "success"`; call sites should not assemble bespoke status borders, icons, or helper text styles
- input sizes must match button density: `size="sm" | "default" | "lg"`
- placeholders are supportive copy only; search and filter controls still require a real accessible name via label, `aria-label`, or both, and product forms should provide a stable `name` attribute so browser tooling and audits do not flag them as anonymous
- on narrow screens, search-driven list pages should prefer a single full-width search field with embedded secondary actions (for example filter or sort access) instead of parallel search/select/stat rows that compress the primary input
- mobile filter state should collapse into an attached sheet, popover, or inline panel launched from the search field action rather than consuming permanent horizontal space beside the field

Required primitives:

- `Field` owns label, helper text, error text, validation status, and the input slot. Helper text and error text occupy the same semantic region; error replaces helper while invalid.
- `SearchInput` owns the embedded action slot used by gateway-style mobile filter patterns (`<SearchInput action={...} />`). Product code should reuse it before inventing a new search/action shell.

### Status Badges

Status badges should be compact and quiet.

Rules:

- badges should reinforce meaning without becoming the main visual event
- keep warning and error badges muted
- do not use highly saturated fills for routine status display

### Tables And Dense Data

Labby supports dense operational views.

Rules:

- favor scanability over decorative row styling
- keep row heights compact and consistent
- use truncation by default where scanability matters
- allow wrapping only when the interaction clearly calls for expansion
- use mono only where value alignment or log readability benefits from it
- if alternating row tones are used in dense operational tables, keep them restrained, keep them inside the Aurora blue surface family, and derive them from Aurora tokens only; do not ship hardcoded RGB or hex striping in product code
- tables, lists, and inspectors should expose `data-density="compact | default | comfortable"` when density can vary by page, viewport, or user preference
- density tiers define row height, vertical padding, and font ramp together; do not tune compactness by guessing independent pixel values per call site

Density tier defaults:

| Density | Row height | Vertical padding | Font ramp |
|---|---:|---:|---|
| `compact` | `32px` to `36px` | `--space-1` to `--space-2` | Dense Data / Dense Metadata |
| `default` | `40px` to `44px` | `--space-2` to `--space-4` | Control / Body as appropriate |
| `comfortable` | `48px` to `56px` | `--space-4` to `--space-5` | Body with clearer metadata separation |

### Panels And Inspectors

Rules:

- inspectors should use Tier 2 lift
- secondary support panels should use Tier 1 lift unless they are the primary interaction surface
- avoid mixing many panel languages on a single page

### Cards (`components/ui/card.tsx`)

`CardTitle` defaults to `font-display tracking-[-0.02em]` so card headers automatically pick up Manrope and the Aurora display tracking. Sizing stays the caller's responsibility — compose `AURORA_DISPLAY_2` (or another ramp entry) when an explicit ramp slot is required.

`Card` exposes `variant="medium" | "strong"` mapped to Tier 1 / Tier 2. Use `variant="strong"` for the primary surface on a page; everything else stays on the medium default.

### Service Brand Identity

External-service brand colors (Plex orange, Sonarr cyan, Radarr yellow, etc.) are kept in `lib/branding/service-brands.ts`, not inlined in components. They are the single sanctioned exception to the "no raw hex" rule because they encode third-party brand identity rather than Aurora UI tone.

### Empty, Loading, Success, Warning, Error States

Rules:

- these states must feel like part of the same product, not separate illustration cards
- empty states should stay concise and operational
- loading states should be understated
- success/warning/error states should communicate clearly without breaking palette discipline
- backend-unavailable or environment-unavailable states should render as calm Aurora support panels with direct explanatory copy; do not surface raw transport or HTTP error strings as the primary UI copy on product pages

### Feedback, Alerts, And Notifications

Aurora feedback primitives:

- `Toast` / sonner surface: Tier 2 panel language, `--aurora-panel-strong`, `--aurora-border-strong`, `--aurora-shadow-strong`, compact Body/Control typography, and muted status accents.
- `Banner`: page-level strip for persistent page or route state such as degraded backend, read-only mode, missing environment, or migration required. Use Tier 1 surface, status-tinted border, and direct action when recovery exists.
- `InlineAlert`: local field, card, or section feedback. Use it when the message only affects the adjacent control or panel.

Decision rule: use a toast for transient completion or failure feedback after an action; use a banner when the page is currently constrained; use an inline alert when the problem belongs to one form field, card, or section.

The Overview warning banner is not a standalone pattern; it must migrate to the shared `Banner` recipe.

### Shared Aurora Primitives

Product pages should reach for shared Aurora primitives before assembling long class strings.

- `StatCard` owns dashboard-style metrics, number typography, label treatment, helper text, icon treatment, and icon-tone variants (`default | success | warning | info`). The inline Overview implementation is the source candidate and should move to `components/aurora/stat-card.tsx`.
- `TonedIconBox` owns the recurring centered icon container: tone (`accent | success | warn | error | neutral`), size (`sm | md | lg`), radius, tint, and icon color. Use it in stat cards, recent gateway rows, empty states, and command-palette rows.
- `Eyebrow` renders the canonical `AURORA_MUTED_LABEL`; product code should not hand-roll `text-xs uppercase tracking-*`.
- `MutedLabel` is the non-heading muted metadata label companion for dense surfaces.
- `PageHeader` owns the route title, eyebrow, supporting body copy, status cluster, and actions.
- `Section` owns the common strong-panel section recipe: optional eyebrow, Display 1/2 heading slot, body copy, action slot, and Tier 1/Tier 2 surface selection.

When these primitives exist, ad-hoc equivalents in `app/**` are contract violations unless the call site documents a short-lived migration reason.

### Iconography

Labby uses `lucide-react` for product UI icons.

Rules:

- default dense-row icon size: `16px`
- default panel/header icon size: `20px`
- default metadata-strip icon size: `14px` (`size-3.5`)
- keep Lucide's default `2px` stroke; do not tune stroke width per page
- icon-only buttons require a tooltip with an accessible label
- do not use emoji as UI icons, status markers, or empty-state illustrations

## Display Slot Assignments

The Typography ramp defines sizes; this section defines which component types own which ramp slot.

| Component | Required token | Notes |
|---|---|---|
| Route-level `<h1>` (top of an admin page, login screen, etc.) | `AURORA_DISPLAY_1` composed with `text-aurora-text-primary` | One Display 1 per route, always. |
| Section / panel headings inside a page | `AURORA_DISPLAY_2` (or `AURORA_DISPLAY_TITLE` legacy alias) | Panel inspectors, sidebar section titles. |
| Large metric / stat card numbers | `AURORA_DISPLAY_NUMBER` | Dashboard stats, summary strips. Use tabular numerals. |
| Category / metadata label above a title | `AURORA_MUTED_LABEL` | See Eyebrow spec. |
| Card titles rendered through `CardTitle` | Default from the primitive (`font-display tracking-[-0.02em]`); compose an explicit ramp slot when you need a specific size | See Component Contract → Cards. |

A route-level `<h1>` without a Display token is a contract violation. A metric number without `AURORA_DISPLAY_NUMBER` is a contract violation. These two rules are the most common drift source in the current codebase.

## Page-Level Patterns

### Gateways

The Gateways page should align with the same Aurora principles while preserving its dense table workflow:

- Aurora summary strip
- Aurora filters and pills
- Tiered panel hierarchy
- calmer status accents
- the mobile `GatewayFilters` search-with-embedded-filter-action pattern is the reference implementation for search-driven operator lists and should be reused by similar catalog pages before introducing new mobile filter shells
- toggle-heavy mutable controls do not belong in the route header status cluster; they should live in a dedicated settings surface or tab so headers remain status-first

### Authentication Surfaces

Login, re-auth, and auth-error screens are product surfaces, not a separate marketing shell. They must:

- use `AURORA_PAGE_SHELL` for the backdrop — no bespoke gradients, no `bg-gradient-to-*` with shadcn tokens
- use `AURORA_STRONG_PANEL` for the centered card (Tier 2 lift — same language as inspectors)
- follow the Typography ramp: Display 1 for the title, Body for intro copy, `AURORA_MUTED_LABEL` for the category eyebrow
- route the primary action through the `Button` primitive with `size="lg"`

If the auth surface lives under a dedicated auth path such as `components/auth/**` or `app/**/auth/**` and is imported by a non-Vite test runner (`node:test` + `react-dom/server`) that cannot resolve the `@/` alias, duplicate the `Button` primitive's default+lg class string inline rather than inventing styling. The inline copy must stay on pure Aurora tokens, must preserve the `AURORA_PAGE_SHELL`, `AURORA_STRONG_PANEL`, and `AURORA_MUTED_LABEL` contract around it, and must be flagged with a one-line comment explaining why it bypasses the primitive.

### Design System Sandbox

`/design-system` is the interactive implementation reference for the current web UI.

It should remain:

- reachable by direct URL only
- excluded from the primary sidebar
- safe to interact with using local fake state
- broad enough to exercise the UI patterns currently used in the app

Its sections should cover:

- foundations
- controls
- feedback
- navigation
- command palette
- data display
- application patterns
- anti-pattern gallery

The command-palette section is the canonical reference for Labby's global `cmd+k` interaction:

- one ranked stack for destinations, actions, and recent context
- Aurora-aligned focus, density, tags, and preview treatment
- local fake state only, with no requirement for live backend search or execution

Sandbox verification requirements:

- shared primitives must be visible in both dark and light mode before rollout
- every new or materially changed surface, status color, form state, chart token, and feedback primitive requires a dark screenshot and a light screenshot
- the light screenshot must include hover/focus examples when the change touches controls or interactive rows
- the feedback section must cover toast, banner, inline alert, loading skeleton, empty, success, warning, and error examples
- the data-display section must cover chart grid/axis colors, positive/negative deltas, and the sequential ramp

The anti-pattern gallery is required documentation, not optional polish. It should show real "do not do" examples beside the corrected Aurora version:

- raw hex/rgba one-off surface versus semantic token surface
- hand-rolled `text-xs uppercase tracking-*` label versus `Eyebrow`
- arbitrary alpha tint versus named tint token through `color-mix()`
- page-local warning strip versus `Banner`
- ad-hoc stat panel versus `StatCard`
- bright generic skeleton shimmer versus Aurora skeleton treatment
- dense list with guessed row padding versus `data-density` tiers

## Accessibility And Interaction

The design system must preserve usability under keyboard and assistive interaction.

Minimum rules:

- all interactive controls require visible focus treatment
- contrast must remain readable on dark surfaces
- contrast must remain readable in light mode across panel, control, status, and chart contexts
- pill filters and custom controls must remain understandable without relying on color alone
- destructive actions must stay explicit in both label and style
- dense views must preserve clear hit targets and readable text sizing
- icon-only buttons require a tooltip and accessible name

## Responsive Rules

Labby is desktop-first but must remain usable on narrower screens.

Rules:

- preserve the same system language on mobile rather than inventing alternate styling
- allow inspectors and secondary panels to collapse into sheets or drawers
- prioritize operational readability over perfectly preserving desktop density
- if a desktop toolbar forces horizontal competition on mobile, move secondary controls into a filter/settings surface rather than letting primary inputs or rows get clipped

## Engineering Rules

When implementing or refactoring UI:

- use shared semantic tokens before adding new hardcoded values
- prefer extending shared component recipes over page-local one-offs
- keep display typography to the approved display moments
- do not introduce alternate dark themes on a per-page basis
- update `/design-system` when adding or materially changing a shared interaction pattern
- update the `/design-system` anti-pattern gallery when a recurring drift pattern is found during review
- treat this document as the stable contract and dated exploration docs as supporting material
- product code should be linted or reviewed for banned drift signals: `text-xs uppercase tracking-*`, arbitrary alpha suffixes where tint tokens exist, page-local `z-[...]`, icon-only buttons without tooltips, emoji used as UI icons, and raw color values
- all `overflow-auto`, `overflow-scroll`, `overflow-y-auto`, and `overflow-x-auto` containers must include the `aurora-scrollbar` utility class — this applies the token-backed thin scrollbar style (defined in `app/globals.css`) consistently across all scroll surfaces
- **shadcn-generic tokens are reserved for `components/ui/` primitives.** Product code — anything under `app/**`, `components/**` except `components/ui/**` — must use the Aurora semantic equivalents:

  | Banned in product code | Use instead |
  |---|---|
  | `text-muted-foreground` | `text-aurora-text-muted` |
  | `text-foreground` | `text-aurora-text-primary` |
  | `bg-card`, `bg-card/NN` | `bg-aurora-panel-medium` or `bg-aurora-panel-strong` (choose by elevation tier) |
  | `bg-background` | `bg-aurora-page-bg` |
  | `bg-muted` | `bg-aurora-control-surface` |
  | `border-border`, `border-border/NN` | `border-aurora-border-strong` |

  The shadcn tokens exist so the forked primitives stay syncable with upstream; product code must not inherit them. The light-mode remap in `.light` pairs Aurora variables only — shadcn tokens drift in light mode even when they look right in dark.

- **primitive imports must use the `@/` alias.** Relative imports into `components/ui/**` from product code are banned so the import path doesn't encode directory structure. The one exception is auth surfaces under `components/auth/**` or `app/**/auth/**` when imported by `node:test` harnesses; see the Authentication Surfaces section for the exact escape hatch, Aurora-token-only requirement, and required inline comment.

- **no raw hex, rgba, or hsl values in className or inline styles.** All colors come from Aurora tokens. The sole sanctioned exceptions are listed in the Live-Preview Tokens section and the Service Brand Identity section.

## Approval Rule

A page is not considered aligned with the design system unless it satisfies all of the following:

- uses the Aurora token system
- follows the typography split correctly
- uses the approved elevation hierarchy
- uses restrained active states
- preserves operator-first density and readability
- does not introduce an incompatible component language
- has dark and light `/design-system` evidence for shared components, status colors, charts, forms, and feedback primitives it adds or changes
- uses shared primitives (`StatCard`, `TonedIconBox`, `Eyebrow`, `Field`, `SearchInput`, `Banner`, etc.) where the contract defines them

When in doubt, update the contract and the `/design-system` sandbox together rather than letting implementation drift.
