---
title: "Labby Aurora Design System"
created: "2026-08-18"
updated: "2026-09-20"
version: alpha
name: Labby Aurora
description: "Dark-first, operator-focused design system for Labby's Gateway Admin control plane and its thin native shells."
colors:
  primary: "#29B6F6"
  primary-strong: "#67CBFA"
  primary-deep: "#1C7FAC"
  primary-foreground: "#07131C"
  primary-tint-subtle: "color-mix(in srgb, #29B6F6 12%, transparent)"
  primary-tint-soft: "color-mix(in srgb, #29B6F6 18%, #0C1A24)"
  primary-tint-medium: "color-mix(in srgb, #29B6F6 30%, transparent)"
  primary-tint-strong: "color-mix(in srgb, #29B6F6 40%, transparent)"
  secondary: "#F9A8C4"
  secondary-strong: "#FBC4D6"
  secondary-deep: "#C46B88"
  tertiary: "#FF9645"
  tertiary-strong: "#FFB066"
  page-bg: "#07131C"
  nav-bg: "#07111A"
  panel-medium: "#102330"
  panel-strong: "#13293A"
  control-surface: "#0C1A24"
  hover-bg: "#17364B"
  border-default: "#1D3D4E"
  border-strong: "#24536C"
  text-primary: "#E6F4FB"
  text-muted: "#A9C6D8"
  success: "#7DD3C7"
  warning: "#C6A36B"
  error: "#C78490"
  marketplace: "#86D68F"
  preview-allowed: "#00E676"
  preview-unmatched: "#FF9100"
  preview-highlight: "#FFEA00"
  chart-1: "#29B6F6"
  chart-2: "#67CBFA"
  chart-3: "#C6A36B"
  chart-4: "#C78490"
  chart-5: "#1C7FAC"
  light-primary: "#0288D1"
  light-primary-strong: "#0277BD"
  light-primary-deep: "#01579B"
  light-secondary: "#B84372"
  light-secondary-strong: "#A13A64"
  light-secondary-deep: "#9C3560"
  light-tertiary: "#A8540E"
  light-tertiary-strong: "#934908"
  light-page-bg: "#F0F6F8"
  light-nav-bg: "#E4EFF3"
  light-panel-medium: "#FFFFFF"
  light-panel-strong: "#EDF4F7"
  light-control-surface: "#E8F2F5"
  light-hover-bg: "#DCEDF2"
  light-border-default: "#C5DAE2"
  light-border-strong: "#9FBFCC"
  light-text-primary: "#162126"
  light-text-muted: "#4A6872"
  light-success: "#2D7D6E"
  light-warning: "#8A6914"
  light-error: "#9C3545"
  light-marketplace: "#337A3D"
typography:
  display-1:
    fontFamily: Manrope
    fontSize: 34px
    fontWeight: 800
    lineHeight: 1.04
    letterSpacing: -0.045em
  display-2:
    fontFamily: Manrope
    fontSize: 19px
    fontWeight: 700
    lineHeight: 1.16
    letterSpacing: -0.02em
  compact-title:
    fontFamily: Manrope
    fontSize: 17px
    fontWeight: 800
    lineHeight: 1.16
    letterSpacing: -0.02em
  card-title:
    fontFamily: Manrope
    fontSize: 15px
    fontWeight: 800
    lineHeight: 1.16
    letterSpacing: -0.02em
  metric-display:
    fontFamily: Manrope
    fontSize: 28px
    fontWeight: 800
    lineHeight: 1
    letterSpacing: -0.04em
  body:
    fontFamily: Inter
    fontSize: 14px
    fontWeight: 400
    lineHeight: 1.55
  control:
    fontFamily: Inter
    fontSize: 13px
    fontWeight: 600
    lineHeight: 1.2
  dense-data:
    fontFamily: Inter
    fontSize: 12px
    fontWeight: 400
    lineHeight: 1.4
  dense-metadata:
    fontFamily: Inter
    fontSize: 11px
    fontWeight: 500
    lineHeight: 1.4
  eyebrow:
    fontFamily: Inter
    fontSize: 11px
    fontWeight: 700
    lineHeight: 1
    letterSpacing: 0.18em
  badge:
    fontFamily: Inter
    fontSize: 10px
    fontWeight: 700
    lineHeight: 1
    letterSpacing: 0.14em
  mono-data:
    fontFamily: "SFMono-Regular, SF Mono, Consolas, Liberation Mono, Menlo, monospace"
    fontSize: 12px
    fontWeight: 400
    lineHeight: 1.4
rounded:
  none: 0px
  button: 8px
  sm: 14px
  md: 18px
  lg: 22px
  full: 999px
spacing:
  xs: 4px
  sm: 8px
  compact: 10px
  md: 12px
  lg: 16px
  xl: 20px
  2xl: 24px
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.primary-foreground}"
    typography: "{typography.control}"
    rounded: "{rounded.button}"
    height: 36px
  button-primary-hover:
    backgroundColor: "{colors.primary-strong}"
    textColor: "{colors.primary-foreground}"
    typography: "{typography.control}"
    rounded: "{rounded.button}"
    height: 36px
  button-outline:
    backgroundColor: "{colors.control-surface}"
    textColor: "{colors.text-primary}"
    typography: "{typography.control}"
    rounded: "{rounded.button}"
    height: 36px
  input:
    backgroundColor: "{colors.control-surface}"
    textColor: "{colors.text-primary}"
    typography: "{typography.control}"
    rounded: "{rounded.button}"
    height: 36px
  panel-medium:
    backgroundColor: "{colors.panel-medium}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.lg}"
    padding: 16px
  panel-strong:
    backgroundColor: "{colors.panel-strong}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.lg}"
    padding: 16px
  badge-default:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.primary-foreground}"
    typography: "{typography.badge}"
    rounded: "{rounded.button}"
    height: 20px
  pill-selected:
    backgroundColor: "{colors.primary-tint-soft}"
    textColor: "{colors.text-primary}"
    typography: "{typography.control}"
    rounded: "{rounded.full}"
    height: 32px
  tabs-list:
    backgroundColor: "{colors.control-surface}"
    textColor: "{colors.text-muted}"
    typography: "{typography.control}"
    rounded: "{rounded.button}"
    height: 36px
---

# Labby Aurora

## Overview

Aurora is Labby's visual language for the Gateway Admin control plane. It is designed for operators who spend long sessions managing gateways, MCP capabilities, authentication, Artifacts, logs, jobs, and other dense technical state. The interface should feel **premium, calm, precise, and capable** without turning operational data into a glossy dashboard or a neon developer toy.

The cross-product Aurora authority is the standalone `dinglebear-ai/aurora` repository and its shadcn registry. This file is Labby's self-contained DESIGN.md consumer profile: it records the visual contract agents should follow in this repository plus the current adoption/migration state. Where an existing Labby token or primitive differs from canonical Aurora, treat that as consumer drift to reconcile deliberately, not as a competing shared definition.

The design is dark-first. Dark mode is the canonical review target and the unprefixed color tokens in this file are its normative values. Light mode is a semantic remap of the same system, using the `light-*` color tokens. Components, hierarchy, typography, density, shapes, and interaction patterns do not change between modes.

Aurora should feel:

- premium, not glossy;
- modern, not flashy;
- calm, not dull;
- information-dense, not cramped;
- technical, but approachable enough for long operator workflows.

The product is a control plane, not a marketing surface. Functional hierarchy must beat decoration. State should be discoverable at a glance, but routine status should not shout.

Implementation mapping today:

- `apps/gateway-admin/app/globals.css` maps Labby's current Aurora values into CSS and Tailwind v4 variables;
- `apps/gateway-admin/components/aurora/tokens.ts` contains local reusable typography and surface recipes;
- `apps/gateway-admin/components/ui/` is the pre-registry compatibility layer still used by existing pages;
- canonical registry components install under `apps/gateway-admin/components/ui/aurora/` as they are adopted;
- `/design-system` is Labby's interactive visual reference and anti-pattern gallery;
- `docs/design/design-system-contract.md` is the longer Labby consumer/implementation contract.

For a shared Aurora change, update the standalone Aurora DESIGN.md, registry source, gallery evidence, and generated artifacts first, then sync the Labby consumer. For Labby-specific composition, update this file and the local implementation/evidence together.

## Colors

Aurora is built on deep blue-black surfaces, cool cyan interaction color, and muted semantic accents. The palette deliberately avoids pure black, pure white, and high-saturation status fills in routine product UI.

### Dark foundation

- **Page (`page-bg`, #07131C):** deepest working canvas. It should visually recede behind every operator surface.
- **Navigation (`nav-bg`, #07111A):** slightly distinct shell tone for persistent navigation.
- **Medium panel (`panel-medium`, #102330):** normal cards, support panels, and standard grouped content.
- **Strong panel (`panel-strong`, #13293A):** primary panels, inspectors, dialogs, and featured working surfaces.
- **Control (`control-surface`, #0C1A24):** inputs, filter rails, toolbars, compact controls, and inset utility surfaces.
- **Borders (`border-default`, `border-strong`):** blue-gray structure. Prefer borders over gratuitous fill changes when separating dense content.
- **Primary text (`text-primary`, #E6F4FB):** high-legibility text without stark white glare.
- **Muted text (`text-muted`, #A9C6D8):** secondary copy and metadata. This value is intentionally light enough to meet WCAG AA on `panel-medium`.

### Interaction family

- **Primary (`primary`, #29B6F6):** the central interaction accent. Use for primary actions, active indicators, focus, links, and selected-state edges.
- **Primary strong (`primary-strong`, #67CBFA):** brighter accent used for hover, icon emphasis, and high-confidence highlights.
- **Primary deep (`primary-deep`, #1C7FAC):** darker accent used when the base cyan needs more weight.
- Use the named tint tokens instead of inventing arbitrary alpha values. The preferred steps are 12%, 18%, 30%, and 40%.

A standard selected state is not a solid cyan block. Prefer a subtle cyan tint, stronger border, clearer text, an indicator, and at most a restrained glow.

### Secondary and identity colors

- **Secondary rose (`secondary`, #F9A8C4):** authored content, agent/prompt identity, selected code or key-label moments. Use sparingly.
- **Tertiary protocol orange (`tertiary`, #FF9645):** protocol identity for MCP-family concepts and async/heavy-operation identity. It is not the warning color.
- **Marketplace green (`marketplace`, #86D68F):** stable Marketplace identity.

Third-party service brand colors are allowed only when they encode the identity of that external service. Keep them centralized in the service-brand mapping rather than sprinkling raw brand hex values through product code.

### Status colors

- **Success (`success`, #7DD3C7):** muted teal. Connected, authorized, healthy, or completed should read clearly without turning neon.
- **Warning (`warning`, #C6A36B):** restrained gold. Warnings should read informational before alarming.
- **Error (`error`, #C78490):** muted rose-red. Errors must be unmistakable, but should not become the loudest element on the page by default.

Never use the protocol orange as warning gold. Never use success or error colors as generic chart colors just because a value went up or down.

### Restricted live-preview colors

`preview-allowed`, `preview-unmatched`, and `preview-highlight` are intentionally saturated exceptions used only by the exposure-policy editor's real-time visualization. They are not general status tokens and must not spread into routine UI.

### Light mode

Light mode keeps identical semantic roles with the `light-*` values in frontmatter. It uses cool blue-gray surfaces instead of plain white application chrome. The main panel may be white, but navigation, controls, strong panels, borders, and hover states remain visibly structured.

Do not implement light mode with page-local `dark:` branches and one-off colors. Change semantic variables, not component language. Every new dark color requires an intentional light counterpart.

## Typography

Aurora uses a split family system:

- **Manrope** provides display identity for page titles, section headings, card titles, and large metrics.
- **Inter** is the working UI face for navigation, controls, forms, tables, metadata, body copy, logs, and inspectors.
- **Monospace** is functional only. Use it where alignment, code, identifiers, or log readability benefits from it.

The typography tokens in frontmatter are complete recipes. Do not apply a token and then override its size, weight, tracking, or line height at the call site.

### Display hierarchy

- **Display 1:** one route-level title per page. Manrope 34px / 800 with tight tracking.
- **Display 2:** section and inspector headings. Manrope 19px / 700.
- **Compact title:** empty-state and compact-panel heading. Manrope 17px / 800.
- **Card title:** dense catalog/list card title. Manrope 15px / 800.
- **Metric display:** dashboard numbers and summary values. Manrope 28px / 800 with tabular numerals where possible.

A route-level `<h1>` without the Display 1 recipe is drift. A large metric without Metric Display is drift.

### Working hierarchy

- **Body:** 14px Inter at 1.55 line height for standard copy and helper text.
- **Control:** 13px Inter, normally 500 to 600. The token uses 600 as the portable default.
- **Dense data:** 12px to 13px at 1.4 for tables, logs, and inspectors.
- **Dense metadata:** 11px / 500 for versions, package IDs, subtitles, and support text.
- **Eyebrow:** 11px / 700, uppercase, 0.18em tracking. Use for category and section metadata above titles.
- **Badge:** 10px / 700, uppercase, 0.14em tracking. Use only in compact badges and chips.

Do not hand-roll `text-xs uppercase tracking-*` labels. Eyebrow and badge typography are deliberately distinct.

## Layout

Aurora uses a compact operator layout rather than spacious marketing composition.

The shell owns the global content measure and page padding. Product pages should not declare a second max-width or duplicate shell padding. Within a page, use a simple vertical frame with a 16px default gap and the spacing scale from frontmatter.

### Spacing rhythm

The core spacing steps are 4, 8, 10, 12, 16, 20, and 24 pixels. They cover micro alignment through section separation. Repeated values outside this scale should become a deliberate token rather than recurring arbitrary CSS.

Typical use:

- 4px for icon/text micro-alignment;
- 8px for tight control interiors;
- 10px for compact control groups;
- 12px for normal control padding and short stacks;
- 16px for card/panel padding and common page gaps;
- 20px for major groups;
- 24px for clear section separation and roomy card headers.

### Density

Dense operational data is a feature, not a compromise. Tables, lists, and inspectors should choose a coherent density tier rather than independently shrinking text and padding.

- **Compact:** 32 to 36px rows, 4 to 8px vertical padding, Dense Data / Dense Metadata typography.
- **Default:** 40 to 44px rows, 8 to 12px vertical padding, Control or Body typography.
- **Comfortable:** 48 to 56px rows, 12 to 16px vertical padding, Body typography with stronger metadata separation.

Favor scanability and stable column rhythm. Truncate where scanning matters and allow wrapping only where the interaction explicitly expands content.

### Responsive behavior

Labby is desktop-first, not desktop-only.

- Preserve the same Aurora language on narrow screens.
- Collapse inspectors and secondary panels into sheets or drawers when necessary.
- Protect the primary search/input from horizontal competition.
- Search-driven lists should use one full-width search field with embedded filter/sort actions on narrow screens.
- Move secondary controls into attached sheets, popovers, or inline panels rather than clipping rows or compressing the main input.
- Prefer operational readability over preserving desktop density pixel-for-pixel.

## Elevation & Depth

Aurora uses tonal layering, borders, modest shadows, and a restrained inset highlight. Depth is noticeable but never glassy.

### Canvas

The page canvas is flat. It should be the deepest tone and must not compete with content surfaces.

### Medium lift

Use normal lifted surfaces for cards, toolbars, support panels, and grouped content:

- dark shadow: `0 12px 24px rgba(0, 0, 0, 0.18)`;
- subtle nested lift: `0 8px 16px rgba(0, 0, 0, 0.16)`;
- inset highlight: `inset 0 1px 0 rgba(255, 255, 255, 0.035)`.

### Strong lift

Use strong lift for primary panels, inspectors, dialogs, and other top-level working surfaces:

- dark shadow: `0 20px 38px rgba(0, 0, 0, 0.26)`;
- inset highlight: `inset 0 1px 0 rgba(255, 255, 255, 0.05)`.

Light mode softens the shadows to cool gray, lower-opacity values while keeping the same hierarchy. It must still look layered, not flat.

Active controls may use a restrained cyan outer glow. Do not use large ambient glows, glossy sheen, frosted-glass blur as a default card treatment, or heavy gradient fills for ordinary controls.

Layering order for application overlays is: sidebar/navigation, popovers, modal/sheet surfaces, then toast/urgent non-blocking feedback. Do not solve layering with page-local `z-[999]` values.

Motion is functional and short. Use approximately 100ms for immediate state, 160ms for hover/color, 240ms for popovers or expand/collapse, and 360ms only for larger panel transitions. Reduced-motion preferences must collapse these durations and disable shimmer or transform-heavy effects.

## Shapes

Aurora is soft-edged but engineered. Large surfaces have confident rounded corners while dense controls stay tighter.

The normative radius scale is:

- **Button, 8px:** compact shadcn-derived buttons, tabs, badges, and basic inputs.
- **Small, 14px:** dense branded controls and compact Aurora surfaces.
- **Medium, 18px:** inline cards and metadata blocks.
- **Large, 22px:** major panels, toolbars, inspectors, and featured surfaces.
- **Full, 999px:** filter chips, state pills, compact selection controls.

Do not sprinkle arbitrary radii through pages. The legacy 16px tuned surface may remain while migrating toward the medium token, but it is not a new scale level.

Use pills only where the interaction is truly pill-like. Do not make every button or card fully rounded.

## Components

Components should encode brand-safe defaults while leaving page-specific layout to composition. The `components/ui/` primitives are intentionally shallow so they can remain syncable with upstream shadcn. Brand identity may be baked into a primitive; page layout, bespoke sizing, and behavior should not be.

### Source registry

Aurora is already a standalone **shadcn-compatible source registry** in `dinglebear-ai/aurora`. Labby is a consumer of that registry, not a second publication authority for Aurora components.

The source-of-truth split is:

- **Aurora `DESIGN.md`** defines the cross-product visual and interaction system.
- **Aurora `registry.json` and `registry/aurora/**`** define the installable component, token, theme, block, file-theme, and bundle catalog.
- **Labby `DESIGN.md`** is a self-contained product profile for agents working in this repository. It may document Labby-specific composition rules, but shared Aurora tokens and primitives should stay aligned with the standalone system.
- **Labby `apps/gateway-admin/components/**`** contains the source installed or synchronized into this product. Those files are editable source, but a change intended to become shared Aurora behavior belongs upstream in Aurora first and then flows back into Labby.

Labby's `components.json` registers `@aurora` against the generated `public/r` artifacts from the canonical `dinglebear-ai/aurora` repository. Typical installs are:

```bash
npx shadcn@latest add @aurora/aurora-base
npx shadcn@latest add @aurora/aurora-button
npx shadcn@latest add @aurora/aurora-banner
```

The public Aurora repository is also a shadcn GitHub source registry, so an item can be addressed directly as `dinglebear-ai/aurora/<item>`. Pin a tag or commit SHA when a build needs reproducible registry input.

The canonical registry is layered rather than monolithic:

- **`aurora-base` (`registry:base`)** is the complete starting bundle and preserves Aurora's shadcn/Radix architecture.
- **`aurora-tokens` and `aurora-components`** carry the shared token and component-style substrate.
- **`aurora-theme-dark` / `aurora-theme-light` (`registry:theme`)** expose theme variants.
- **`aurora-*` (`registry:ui`)** items expose individual primitives such as Button, Badge, Input, Card, Tabs, Banner, and StatCard.
- **`registry:block` items** package heavier product patterns such as prompt input, command surfaces, file tooling, and code editor experiences.
- Editor, terminal, browser, Android, and other theme outputs remain part of the same Aurora design infrastructure rather than becoming Labby-specific copies.

Registry rules for Labby:

- Use an existing `@aurora` item before hand-rolling a lookalike component.
- Canonical Aurora registry components install under `components/ui/aurora/*`. Labby's existing `components/ui/*` tree predates that registry extraction and remains a product-local compatibility layer until each primitive is deliberately migrated.
- Do not silently swap an existing Labby primitive for the registry variant when their public APIs differ. Install or inspect the canonical item, reconcile behavior and call sites, then migrate explicitly.
- Keep Labby-specific composition in Labby, but move reusable primitive/token behavior to Aurora and sync the consumer afterward.
- Do not create a second Labby-owned Aurora registry or parallel registry-only copy of a component.
- Treat a local edit to a shared Aurora primitive as a fork that needs reconciliation, not as a new source of truth.
- When shared visual semantics change, the canonical Aurora `DESIGN.md`, registry source, gallery evidence, generated registry output, and consuming Labby implementation should move together.

### Buttons

The hierarchy is primary, secondary, outline, ghost, link, and destructive.

- **Primary:** cyan action fill with dark high-contrast text. Reserve for the main action in a local decision area.
- **Outline:** control-surface background, strong border, normal primary text. This is the default toolbar/action workhorse.
- **Secondary:** strong-panel family for supporting actions.
- **Ghost:** transparent until hover, then reveal a quiet panel/control treatment.
- **Destructive:** use only for consequence-bearing actions such as delete, remove, or irreversible changes. Do not reuse destructive styling for error status.

Default button height is 36px; small is 32px and large is 40px. Icon-only buttons follow the same height scale and require both an accessible name and a tooltip.

Selected state is an interaction state, not a new visual species. Use border emphasis, an indicator, text emphasis, and a subtle glow instead of a flooded accent fill.

### Inputs and selects

Inputs live on the control surface, use the compact button radius, and share the same focus language as buttons. Default height is 36px.

Validation augments the control rather than replacing it with unrelated styling. Error, warning, and success treatments should use semantic borders/icons/helper text around the same base control.

Every search/filter field needs a real accessible name. Placeholders are supporting copy, not labels. Product forms should also provide stable `name` attributes.

### Cards, panels, and inspectors

Use `panel-medium` for normal cards and `panel-strong` for the primary or elevated working surface. Major surfaces normally use the 22px radius and strong blue-gray border.

Card titles inherit Manrope display identity. When a specific title slot is needed, choose one of the typography tokens instead of creating a new arbitrary size.

Avoid a page made from a field of identical flat cards. Use hierarchy: canvas, support surface, primary surface.

### Pills and filters

Pills are appropriate for short filters, compact toggles, and selected-state chips. Selected pills use subtle cyan tint, border emphasis, and a visible internal indicator.

When filters become numerous, switch to a dense checkbox/filter rail rather than forcing dozens of pills. Mobile filter state should move to a sheet, popover, or attached panel.

### Tabs

Tabs use a quiet control-surface list. Active tabs communicate state with accent text, a bottom border/indicator, and a small glow. They do not use a large filled active pill.

### Badges and status

Badges are compact and quiet. Status should reinforce meaning without becoming the visual centerpiece.

- Use teal for success/live/healthy.
- Use gold for warnings and caution.
- Use muted red for errors.
- Use cyan for neutral emphasis or generic active state.
- Use pill shape for compact status/filter badges when appropriate.

Do not use error/destructive interchangeably. Error is state; destructive is action consequence.

### Tables, lists, and logs

Operational data favors scanability:

- compact, stable row heights;
- muted separators and restrained alternate rows if needed;
- truncation by default in scan-heavy columns;
- monospace only for code, IDs, logs, and aligned technical values;
- a coherent density tier for text and padding together;
- Aurora-styled scrollbars on every scrollable product surface.

### Feedback

Use the smallest feedback surface that matches the scope:

- **Toast:** transient completion/failure after an action.
- **Banner:** page-wide degraded, read-only, migration, or environment state.
- **Inline alert:** message belonging to one field, card, or section.
- **Skeleton:** muted control/panel colors with subdued shimmer; never generic bright-white shimmer.

Backend or environment failures should be translated into calm, direct operator copy. Raw transport errors are supporting detail, not the page headline.

### Shared Aurora patterns

Prefer shared recipes for recurring product patterns:

- route/page header with eyebrow, Display 1 title, supporting body text, status, and actions;
- section container with optional eyebrow, Display 2 heading, body, action slot, and medium/strong surface choice;
- stat card with Metric Display number and toned icon box;
- toned icon box with semantic accent/status tone;
- canonical eyebrow and muted metadata labels;
- field wrapper for label, helper/error copy, validation, and control slot;
- search input with embedded mobile secondary action;
- banner, toast, inline alert, empty state, and loading state.

When a shared recipe exists, a page-local copy is design drift unless it is an explicitly temporary migration.

### Iconography

Use `lucide-react` for product UI icons.

- Dense rows: 16px default.
- Panel/header icons: 20px default.
- Metadata-strip icons: 14px default.
- Keep Lucide's normal 2px stroke.
- Icon-only controls require a tooltip and accessible name.
- Do not use emoji as product icons, status glyphs, or empty-state illustrations.

Artifact-kind glyphs may use reviewed vendored paths when the product requires exact identity rather than an approximate Lucide metaphor.

### Authentication surfaces

Login, re-authentication, and auth-error screens are part of the application, not a separate marketing theme. Use the same page shell, strong panel, Display 1 title, Body copy, eyebrow treatment, and button hierarchy.

### Accessibility

Every interactive element requires a visible focus treatment. Do not rely on color alone for selected, warning, destructive, or status meaning. Maintain WCAG AA contrast in both modes on the real panel/control combinations where a token is used.

## Do's and Don'ts

- **Do** treat this as an operational control plane. Keep hierarchy clear and density useful.
- **Do** use semantic Aurora tokens before adding a raw color, radius, spacing value, or shadow.
- **Do** use one Display 1 route title and the defined title ramp below it.
- **Do** use Manrope for display moments and Inter for working UI.
- **Do** keep active states restrained: border, indicator, text, subtle tint, subtle glow.
- **Do** use the named 12%, 18%, 30%, and 40% tint steps instead of arbitrary alpha suffixes.
- **Do** preserve the same semantic component system in dark and light modes.
- **Do** show new or materially changed shared patterns in `/design-system` in both modes.
- **Do** check hover and focus states in light mode, where surface deltas can disappear more easily.
- **Do** use strong panels selectively so the visual hierarchy has somewhere to go.
- **Do** use calm status colors and reserve neon preview colors for the exposure-policy visualization.
- **Do** make icon-only controls accessible and tooltipped.
- **Do** collapse secondary tooling on mobile instead of squeezing the primary workflow.
- **Do** use shared density tiers for tables and inspectors.
- **Do** keep third-party brand colors centralized and scoped to actual brand identity.

- **Don't** create a page-specific dark theme or alternate visual language.
- **Don't** use bright neon cyan/green for routine status.
- **Don't** use large decorative glows, glossy selected fills, or animated gradients on standard controls.
- **Don't** flood selected pills or tabs with solid accent color.
- **Don't** make warning or error the loudest visual treatment unless the situation is genuinely critical.
- **Don't** use protocol orange as operational warning gold.
- **Don't** hand-roll `text-xs uppercase tracking-*` instead of the eyebrow/badge recipes.
- **Don't** use raw hex, `rgb()`, `rgba()`, or `hsl()` values in product component styling when an Aurora token exists.
- **Don't** use generic shadcn semantic colors directly in product pages when an Aurora semantic token exists; keep those compatibility tokens inside the primitive layer.
- **Don't** invent arbitrary `z-index`, radii, repeated gaps, or status tints at the page level.
- **Don't** use emoji for icons or status markers.
- **Don't** place large mutable toggle clusters in route headers. Keep headers status-first and move configuration into the appropriate working surface.
- **Don't** treat placeholder text as an accessible label.
- **Don't** ship a new shared pattern without updating this file and the `/design-system` implementation reference together.
