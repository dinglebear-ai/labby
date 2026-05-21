# Gateways redesign design

## Scope

Redesign the `apps/gateway-admin` gateways page to support:

- clickable summary cards that set the primary page lens
- an in-place tools inventory view triggered by `Discovered tools`
- a desktop checkbox-based filter rail with a mobile sheet equivalent
- a quickly toggleable `comfortable` / `condensed` density mode
- condensed rows that move command or URL metadata onto the same line as the gateway identity
- warning and error details hidden behind the warning affordance rather than rendered inline in table rows

This redesign is constrained by `docs/design-system-contract.md` and must preserve mobile usability as a first-class requirement.

## Goals

- Increase operator scan speed on the gateways page
- Preserve the current dense operational workflow while improving information architecture
- Make summary metrics actionable, not decorative
- Replace dropdown-heavy filters with a more browsable checkbox system
- Keep the page aligned with the Aurora design system contract
- Ensure the same capabilities remain available on mobile, not a reduced mobile variant

## Non-goals

- No routing split for the tools inventory view in this pass
- No grouped or deduplicated tools inventory in this pass
- No gateway detail redesign in this pass
- No new warning model or new backend capability model in this pass

## Interaction model

### Primary lenses

The summary strip at the top of the page becomes the primary lens selector.

Available primary lenses:

- `Configured`
- `Healthy`
- `Disconnected`
- `Discovered tools`

Rules:

- Only one primary lens is active at a time
- Clicking a summary card immediately replaces the current list lens
- This primary lens reset happens before any secondary filters are applied
- Secondary filters can narrow results after the primary lens is chosen
- The active summary card remains visibly selected until replaced or cleared

### Gateways lenses

`Configured`, `Healthy`, and `Disconnected` all keep the page in the gateways results view.

Expected semantics:

- `Configured`: default operator lens for configured gateways
- `Healthy`: only gateways currently healthy and connected
- `Disconnected`: only gateways currently disconnected

### Tools lens

Clicking `Discovered tools` switches the content area from gateways results to an in-place tools inventory view.

Rules:

- This does not navigate away from the page
- The page header and shell remain intact
- A contextual `Back to gateways` control appears in the header while the tools lens is active
- Leaving the tools lens restores the last non-tools gateway lens and its secondary filters

## Filter model

### Desktop

Desktop uses a persistent left-side filter rail.

Structure:

- search input at the top
- grouped checkbox sections underneath
- `Clear filters` action resets only the secondary filters

Secondary filters for the gateways lens:

- `Search`
  - matches gateway name
  - command
  - URL
  - source label
  - transport
  - surfaced capability names when already present in the page model
- `Status`
  - `Configured`
  - `Healthy`
  - `Disconnected`
  - `Enabled`
  - `Disabled`
- `Source`
  - `Lab`
  - `Custom`
- `Transport`
  - `stdio`
  - `HTTP`

Selection semantics for gateways filters:

- `Search` is free text and combines with all other active filters using `AND`
- `Status`, `Source`, and `Transport` are faceted multi-select groups
- multiple checked options within the same group use `OR`
- different groups combine with each other using `AND`
- if no options are selected in a group, that group is treated as unfiltered

Secondary filters for the tools lens:

- `Search`
  - matches tool name
  - description
  - gateway name
- `Gateway`
  - multi-select gateway names
- `Exposure`
  - `Exposed only`
  - `Hidden only`
  - `All`
- `Source`
  - `Lab`
  - `Custom`
- `Transport`
  - `stdio`
  - `HTTP`

Selection semantics for tools filters:

- `Search` is free text and combines with all other active filters using `AND`
- `Gateway`, `Source`, and `Transport` are faceted multi-select groups
- multiple checked options within the same group use `OR`
- different groups combine with each other using `AND`
- `Exposure` is a single-select segmented filter even if it is visually grouped with other pills
- `Exposure = All` means no exposure filtering
- `Exposure = Exposed only` means show only exposed tools
- `Exposure = Hidden only` means show only non-exposed tools

### Mobile

Mobile preserves the same filter taxonomy and selection model, but presents it inside a sheet or drawer.

Rules:

- the `Filters` entrypoint lives in the sticky page header
- the sheet uses the same checkbox groups as desktop
- search remains available in the filter sheet
- filter state is shared with desktop behavior, not a separate mobile-only model

## Density model

The gateways page supports two display densities.

### Control placement

The density toggle lives in the sticky page header, not in a large intermediary content row.

Header control order:

- mobile `Filters` button when applicable
- icon-only `Comfortable` toggle
- icon-only `Condensed` toggle
- contextual `Back to gateways` action when the tools lens is active
- `Add Gateway`

Rules:

- density toggles are icon-only
- density toggles require tooltip and `aria-label`
- selected state must be understandable without relying only on color
- these controls are part of the Tier 1 header strip, consistent with the design system contract

### Comfortable mode

Purpose:

- primary day-to-day operational view
- preserves more row context without sending the operator into the detail page

Visible fields per gateway row:

- status dot
- gateway name
- source chip
- transport chip
- warning icon or count pill
- secondary metadata line containing command or URL, separator, and active or enabled state
- surfaces group showing tools, resources, and prompts ratios
- actions: test, reload, overflow menu

Rules:

- warnings never expand inline
- `last_error` is not rendered in the table body
- secondary metadata remains on its own line
- surface pills may wrap on narrow layouts when necessary
- mobile uses stacked cards, but preserves the same information hierarchy

### Condensed mode

Purpose:

- faster row scanning with less vertical height
- optimized for operators moving through many gateways quickly

Visible fields per gateway row:

- status dot
- gateway name
- source chip
- transport chip
- warning icon or count pill
- command or URL plus active state moved onto the same row as the gateway identity
- surfaces group showing tools, resources, and prompts ratios
- actions minimized to reduce row width pressure

Rules:

- row height is reduced compared with comfortable mode
- command or URL truncates aggressively
- no second descriptive line
- no explanatory copy beneath the identity row
- truncation is preferred over wrapping for scanability
- touch targets remain full-size even when text density increases
- direct actions may be minimized in favor of the overflow action, especially on smaller screens

## Tools inventory view

The tools inventory is an operational lens on the same page, not a separate page type.

Visible fields per row:

- tool name
- short description
- owning gateway
- exposure state
- optional source or transport chips where useful

Rules:

- the tools view uses the same Aurora dense-data language as the gateways table
- default sort is alphabetical by tool name
- duplicates across gateways remain separate rows in this pass
- the tools view is not a replacement for gateway detail; it is an inventory and discovery lens

## Warning and error presentation

Warnings and errors must not render as inline row copy on the gateways list.

Rules:

- warning details are hidden behind the warning icon or pill affordance
- inline `last_error` paragraphs are removed from list rows
- warning and error badges remain muted and compact per the design system contract
- desktop detail disclosure happens through the warning affordance via tooltip or hover card, not by increasing baseline row height
- mobile detail disclosure happens through the same warning affordance using tap to open a compact popover, sheet, or dialog anchored to that row context
- mobile must not rely on hover-only disclosure for warning detail

## Layout and page structure

### Shell

The redesign keeps the existing gateway-admin shell intact:

- app sidebar remains unchanged
- sticky app header remains unchanged except for the addition of density and contextual controls
- gateways page content remains within the existing page shell and spacing system

### Surface hierarchy

Per `docs/design-system-contract.md`:

- sticky page header is Tier 1
- filter rail or filter sheet is Tier 1
- gateways table and tools inventory are Tier 2 primary data surfaces
- summary cards remain the Aurora summary strip used for top-level metrics and quick filters

This means the page should not introduce an extra large explanatory banner or mode row between the summary strip and the primary data surface.

## Design system contract alignment

The implementation must align with the following rules from `docs/design-system-contract.md`:

- `Manrope` only for summary metrics and approved display moments
- `Inter` for controls, tables, metadata, and dense rows
- Aurora token system only; no raw hex or ad hoc color styling in product code
- Tiered panel hierarchy: calmer Tier 1 support surfaces, stronger Tier 2 primary data surfaces
- restrained selected states using border emphasis and subtle glow
- compact operator-first spacing and density
- checkbox and pill filters use calm default state and restrained selected state
- mobile preserves the same system language rather than inventing alternate styling

## Responsive behavior

Mobile is a first-class consumer of this page.

Rules:

- summary cards remain prominent and tappable at the top of the page
- filters collapse into a mobile sheet instead of being removed
- density toggle remains accessible in the header
- gateways remain usable in both comfortable and condensed stacked-card presentations
- tools inventory remains available in-place on mobile using the same lens switch model
- the page should prioritize operational readability rather than attempting to preserve every desktop layout proportion exactly

## Accessibility requirements for density controls

The icon-only density toggles are page-critical controls and must remain fully operable without relying on visual inference.

Rules:

- each toggle must expose an explicit accessible name via `aria-label`
- each toggle must expose its selected state programmatically
- keyboard users must be able to reach and activate the controls from the header in normal tab order
- focus-visible treatment must use the shared Aurora focus language
- selection must be understandable through more than color alone, such as active border treatment, icon state, or another persistent non-color indicator

## Data and state requirements

The redesign can be implemented against the existing client-side page state with additional local UI state for:

- active primary lens
- active secondary filters per lens
- density mode
- current content lens (`gateways` or `tools`)
- derived aggregated tools inventory built from gateway discovery data

Recommended derived state model:

- keep primary lens separate from secondary filters
- keep a gateway filter state and a tools filter state
- derive aggregated tools rows via memoization from the gateway list payload
- preserve the last non-tools gateway lens when entering the tools view

## Implementation notes

Likely edit surface:

- `apps/gateway-admin/components/gateway/gateway-list-content.tsx`
- `apps/gateway-admin/components/gateway/gateway-filters.tsx`
- `apps/gateway-admin/components/gateway/gateway-table.tsx`
- shared gateway presentation helpers or new gateway page-local components as needed
- corresponding tests for filters and table behavior
- `/design-system` sandbox updates if the new checkbox filter rail or density toggle pattern becomes a shared reference interaction

Preferred architectural direction:

- keep the page container responsible for lens and filter state
- keep row rendering components presentational
- extract tools inventory rendering into a dedicated component rather than overloading the existing gateway table
- reuse Aurora tokens and existing component primitives instead of introducing page-local visual languages

## Acceptance criteria

The redesign is complete when all of the following are true:

- summary cards are clickable and set the primary page lens
- `Discovered tools` swaps the content area to an in-place tools inventory
- desktop uses checkbox-based filter groups instead of dropdowns
- mobile exposes the same filter system via a sheet or drawer
- density controls are icon-only, in the header, beside `Add Gateway`
- condensed mode pulls command or URL plus active state onto the same row as the gateway identity
- gateway rows do not render inline warning or error detail text
- the gateways and tools views both use the Aurora design system language and panel hierarchy
- mobile remains a full-featured operational surface, not a reduced fallback
