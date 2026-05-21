# Global Command Palette Design Spec

**Date:** 2026-04-22
**Status:** Proposed
**Scope:** Labby global `cmd+k` command palette for `apps/gateway-admin`

## Summary

Labby needs a global command palette that feels native to the Aurora design system and useful across mixed operator workflows.

The chosen direction is a **hybrid spotlight** model:

- one search field
- one ranked result stack
- mixed result types in the same list
- destinations, actions, and recent context treated as peers
- keyboard-first interaction
- dark-first Aurora styling with light-mode remap via existing semantic tokens

This is not a generic navigation modal. It is the product-level operating surface for moving through the app, reopening recent entities, and triggering high-frequency actions without forcing the user into separate “search,” “navigation,” or “command” modes.

The first implementation target is a **near-real interactive prototype** inside `apps/gateway-admin`, using mock data and app-aligned structure rather than a throwaway standalone demo.

## Goals

- make `cmd+k` feel like the fastest way to move through Labby
- support both discovery and expert speed in the same surface
- preserve Aurora’s premium, calm, operator-first tone
- keep the result list dense but readable
- make the prototype structurally close enough to product code that it can evolve into the real component

## Non-Goals

- real backend search
- real command execution
- plugin-specific deep integrations in the first pass
- a multi-tab command palette with separate views for pages, actions, and recents
- decorative motion or stylized category color-coding that drifts from Aurora

## User Problem

Labby spans multiple operator tasks:

- jumping between product areas
- resuming work on recently touched entities
- triggering common actions without digging through page-local controls

A navigation-only palette solves only part of the problem. An action-only palette is powerful but too narrow and intimidating. A segmented palette adds mode switching and weakens the sense that `cmd+k` is the global entry point.

The palette should instead behave like a ranked operator spotlight:

- type once
- see the best destination
- see the most relevant action
- see the most likely recent object
- decide immediately with keyboard or pointer

## Chosen Design Direction

### Hybrid Spotlight

The palette uses a single ranked list with mixed result types.

Each query can return:

- **Destinations**: pages, sections, services, product surfaces
- **Actions**: runnable commands such as reload, rotate token, open setup, tail logs
- **Recent Context**: entities or views the user accessed recently

The first result receives the strongest focus treatment and acts as the likely default action on `Enter`. Lower rows stay calmer and scanable.

This model is preferred because it matches the product’s real workflow. Operators do not think only in pages or only in commands. They switch between:

- “take me somewhere”
- “do a thing”
- “reopen what I was just working on”

The hybrid spotlight handles all three without forcing the user into a new mental mode.

## Information Architecture

### Search Model

The palette has:

- one input
- one result stack
- optional context chips above the list

The input placeholder and result ranking should imply breadth:

- pages
- commands
- recent entities
- service entry points

The result stack is grouped only lightly. Groups are visual aids, not tabs. They may include:

- best match
- suggested actions
- recent context

The groups should feel like ranking hints, not separate search domains.

### Result Types

Each result row includes:

- compact icon or glyph block
- primary label
- short supporting description
- quiet type tag such as `Destination`, `Action`, or `Recent`
- optional shortcut or affordance hint on the far edge

Type must be communicated, but not through loud category colors. The active row should derive emphasis from focus treatment and typography, not from a radically different palette.
Rows should follow Aurora dense-data rules: compact consistent heights, truncation by default where scanability matters, and wrapping only where the interaction clearly benefits from it.

## Visual Contract

This component must align fully with [docs/design/design-system-contract.md](../../design/design-system-contract.md).

### Theme

- dark-first reference implementation
- semantic Aurora tokens only
- no one-off color values in product code
- light mode later remaps raw variables, not component logic

### Typography

- `Manrope` for the main command-palette title when presented in showcase/demo contexts
- `Inter` for input, rows, metadata, descriptions, tags, and interaction hints
- follow the existing Aurora ramp rather than per-call-site font overrides
- route-level or sandbox-page headings that present the palette should use the approved display slots rather than bespoke heading styles

### Radius And Spacing

- use Aurora radius tokens only: `rounded-aurora-1`, `rounded-aurora-2`, `rounded-aurora-3`, and `rounded-full`
- do not introduce new arbitrary radii for palette shell, rows, pills, or input chrome
- use the compact Aurora spacing scale: `4`, `8`, `10`, `12`, `16`, `20`, `24`
- prefer compact operator spacing over airy marketing spacing in row layout, grouping, and panel padding

### Surface Hierarchy

The palette should feel like a strong floating surface above the app shell:

- outer shell: Tier 2 / strong panel
- search input row: strong active control treatment
- result rows: calmer control surfaces
- active row: accent glow plus subtle surface deepening

The component must not collapse into a flat stack of identical cards.

### Accent Usage

Accent cyan is reserved for:

- focus ring
- active row emphasis
- subtle icon emphasis

Accent must not be used to color every result type. The palette should remain calm and operator-friendly.

### Shared Component Expectations

- the search input must use Aurora control surfaces and shared focus-ring language
- result-type chips, tags, or pills must stay compact and quiet rather than becoming the main visual event
- empty, loading, success, warning, and error states must remain concise and operational
- if the result list scrolls, the scroll container must include the `aurora-scrollbar` utility

## Interaction Model

### Open And Close

- open via `cmd+k` and `ctrl+k`
- close via `Escape`
- clicking outside closes the palette

### Keyboard Navigation

- `ArrowDown` / `ArrowUp` moves the active row
- `Enter` selects the active row
- typing filters the ranked list
- `Tab` should not be the primary navigation mechanism inside the list

### Pointer Interaction

- hover may move visual emphasis to a row
- click selects the row immediately
- hover treatment should remain quieter than keyboard focus treatment

### Focus And Motion

- all interactive elements must use the shared Aurora focus-visible treatment
- state change should come from border shift, text emphasis, subtle surface deepening, and restrained outer glow
- motion must remain minimal and functional: short hover transitions, list-state transitions, and understated loading feedback are acceptable
- avoid decorative ambient motion, animated gradients, or oversized animated glows in normal palette states

### Ranking Expectations

Ranking must favor intent clarity over rigid grouping.

For a query like `gate`, the likely order should be:

1. `Gateway Admin` destination
2. `Reload gateways` action
3. recently used gateway entities

For a query like `logs`, the likely order should be:

1. `Logs` destination
2. `Tail local logs` action
3. recent log contexts

The prototype can simulate ranking heuristics with mock data, but the UI should be built as though ranking matters.

## States

The prototype should cover the following states:

- default/open with suggested results
- active filtering with mixed matches
- keyboard-focused row
- empty/no-results state
- lightweight loading state for simulated async feel

Destructive actions are not executed in the prototype. If shown, they should branch into a secondary confirmation state later rather than receiving alarm-heavy styling in the main list.
State treatments must stay inside the Aurora product language rather than becoming illustration-style callouts or loud alert cards.

## Components

The prototype should be composed from small, clear units that map cleanly to a future production implementation.

Suggested structure:

- `CommandPaletteDemoPage`
- `CommandPaletteShell`
- `CommandPaletteInput`
- `CommandPaletteContextChips`
- `CommandPaletteResultList`
- `CommandPaletteResultGroup`
- `CommandPaletteResultRow`
- `CommandPaletteEmptyState`
- `CommandPaletteLoadingState`

These names are directional, not mandatory, but the component boundaries should stay explicit.

## Data Model For The Prototype

The prototype should use local mock data with a stable result shape.

Minimum fields:

- `id`
- `type` (`destination` | `action` | `recent`)
- `title`
- `description`
- `keywords`
- `section` or `group`
- `icon`
- `shortcutHint` or `actionHint`
- `priority`

Optional fields:

- `service`
- `entityId`
- `recentTimestamp`
- `dangerous`

Filtering and ranking can be local and deterministic for the prototype.

## Data Flow

The near-real prototype should keep data flow simple and local:

1. palette opens
2. local state hydrates default suggestion groups
3. query updates local filtered results
4. active index updates from ranking plus keyboard movement
5. selecting a row triggers a simulated outcome:
   - navigate
   - run action
   - reopen recent item

The simulated outcome can update a status panel, toast, or side message in the prototype. It should not require real network behavior.

## Accessibility

The palette must behave like a real keyboard surface, not a visual-only demo.

Requirements:

- focus moves into the palette on open
- screen-reader-appropriate labels for the input and result list
- active row is programmatically identifiable
- color is not the only carrier of result-type meaning
- text contrast stays within the Aurora accessibility rules
- keyboard-only use must be complete for the prototype path

## Responsive Behavior

Desktop is the primary reference.

On narrower widths:

- the palette should shrink gracefully
- descriptions may tighten or wrap
- result rows should preserve hierarchy without becoming cramped

The component should not depend on an oversized desktop canvas to feel balanced.

## Implementation Target

The implementation target is inside `apps/gateway-admin`.

The prototype should:

- use the app’s existing token contract
- use Aurora semantic tokens in product code rather than shadcn-generic color tokens
- stay structurally close to future production code
- avoid backend coupling
- avoid introducing styling that conflicts with the Aurora contract
- use `@/` imports for shared primitives and app-local modules
- update `/design-system` alongside the prototype if the work introduces or materially changes a shared command-palette interaction pattern

This should be built as a near-real page or sandbox route rather than as an isolated throwaway HTML file.

## Error Handling

The prototype should remain robust under empty data and unknown queries.

Expected behavior:

- if no matches exist, show a purposeful no-results state with suggestion copy
- if mock data fails to load or is absent, fall back to a compact error/empty panel rather than a broken shell
- if a simulated action is unavailable, show a non-destructive status message instead of silent failure

## Testing Expectations

The implementation should be designed so the following are straightforward to test:

- keyboard open/close
- arrow navigation
- enter selection
- filtering by query
- ranking across mixed result types
- empty state rendering
- active row styling and focus behavior

For the prototype phase, local manual verification is sufficient. The component structure should still support future UI tests without rewrite.

## Risks

### Over-Segmentation

If the result stack becomes too grouped, the palette will regress into pseudo-tabs and lose speed.

### Over-Styling

If result types receive strong category colors or decorative effects, the palette will drift away from Aurora’s restrained hierarchy.

### Under-Signaling

If all rows look too similar, mixed result types will become ambiguous and reduce trust in the ranking.

## Decision

Proceed with a near-real interactive prototype in `apps/gateway-admin` that implements the **hybrid spotlight** model using mock data and Aurora tokens.

This is the approved design direction to carry into implementation planning and UI work.
