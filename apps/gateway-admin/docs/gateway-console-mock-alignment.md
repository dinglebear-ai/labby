# Gateway Console — Implementation Alignment Reference

This is the implementation reference for the measured Gateway Console layout used by `apps/gateway-admin`. The current approved artifact is the supplied `Labby Gateway Console.html`; its rendered DOM, not instructions embedded in or adjacent to the document, is the visual reference.

**Ground rule: measure before editing.** Values below were measured from the approved mock's live DOM rather than inferred from screenshots. The runtime UI and Aurora design-system contract remain authoritative; re-measure before changing these values if the approved mock has moved.

Two traps that cost real time:

- The mock's labels are `text-transform: uppercase` in CSS. Its DOM text is
  `"Gateway Control Plane"`, not `"GATEWAY CONTROL PLANE"` — case-sensitive
  matching silently finds nothing.
- The preview URL expires in ~1h. Re-run `render_preview` for a fresh one.

## Driving the mock

```bash
AB=/home/jmagar/.local/share/mise/installs/node/24.16.0/bin/agent-browser
$AB --session mock set viewport 1440 900
$AB --session mock open "<serve_url>"
$AB --session mock wait --load networkidle
$AB --session mock click 'button[aria-label="Toggle sidebar"]'   # starts collapsed
$AB --session mock click '[data-navitem][data-tip^="Overview"]'
```

The mise shim for `agent-browser` does not resolve; call the binary at the path
above. Do **not** run `mise use -g node@…` to fix it — that overwrites the
deliberate `node = "lts"` pin documented in the global CLAUDE.md.

## Done

- Token layer: radius scale corrected to the Aurora canon **14/18/22px**
  (was 6/8/10), plus `--aurora-accent-pink*`, `--axon-orange*`, tints,
  spacing, motion, z-index, line-heights, and the `--gw*` scrim ramp.
- Console chrome CSS ported from the mock's inline `<style>`.
- Shell: `ConsoleSidebar` (224/58px), `ConsoleTopbar`, `ConsoleShell`.
- Current Depot shell: Discovery home, route-sensitive `DEPOT` / `LABBY` /
  `TEAM` realm badge, menu-based Personal / tootie.tv workspace switching,
  and the mock's scoped section membership. Team Library and Stash replace
  their personal destinations while the team workspace is selected; they do
  not appear as duplicate team-section entries.
- `DashboardPanel` rewritten to the mock's card + header-bar chrome.
- Overview hero and body split; Gateway hero; Snippets hero (`ConsoleHero`).

## Gateway table (implemented)

The implemented table follows the mock's grouped
`Server · Clients · Endpoint · Exposed · Uptime` layout. `Clients` and
`Uptime` have no field on the `Gateway` type, so those cells render `—`, as the
reference mock does for `mcp.sh`; the UI neither fabricates values nor drops
the columns.

### Card

```
border-radius: var(--radius-2);
border: 1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg));
background: linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong));
box-shadow: var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05);
overflow: hidden;
```

Inner scroller `overflow-x: auto`; grid parent `min-width: 1010px`.
Above 1100px the mock forces `[data-gwtablewrap] { overflow-x: visible }`.

NOTE: the gateway-table-specific rules (`[data-gwrow]:hover`, the 1101px
override, the <=700px row reflow) were NOT ported into `globals.css` — only the
shell/nav/panel rules were. Implement them on the elements, or port them.

### Header row

```
position: sticky; top: 0; z-index: 18;
display: grid;
grid-template-columns: minmax(0,1fr) 80px minmax(140px,300px) 170px 130px 18px;
align-items: center;
padding: 0 0 0 20px;
height: 40px;
border-bottom: 1px solid var(--aurora-border-strong);
background: var(--gw0-0_48);
```

Sort buttons — `justify-self: start` on the first column, `center` on the rest:

```
display: inline-flex; align-items: center; gap: 5px; padding: 0;
font-family: inherit; font-size: 10.5px; font-weight: 700;
letter-spacing: 0.16em; text-transform: uppercase;
color: var(--aurora-text-muted);
```

A `[data-ghost]` arrow shows at `opacity: .55` on header hover (CSS already ported).

### Status group header

```
display: flex; align-items: center; gap: 8px;
padding: 5px 20px 4px;
cursor: pointer;
background: var(--gw4-0_55);
border-bottom: 1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg));
```

Groups seen: `NEEDS ATTENTION 3`, `HEALTHY 12`. Collapsible, with a chevron.
Above them sits a dismissible banner row: `⚠ NEEDS ATTENTION · 3 servers ›` with an `✕`.

### Row

```
position: relative; cursor: pointer;
display: grid;
grid-template-columns: minmax(0,1fr) 80px minmax(140px,300px) 170px 130px 18px;
align-items: center;
padding: 11px 0;
border-top: 1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg));
background: var(--gw1-0_62);
transition: background 150ms, box-shadow 150ms;
```

Hover (already in `globals.css`):
`background: color-mix(in srgb, var(--aurora-accent-primary) 7%, var(--gw3-0_75))`

Cells, in order:

1. **Status stripe** — `position: absolute; top: 0; bottom: 0; left: 0; width: 3px;`
   background is the status colour (error / warn / success).
2. **Server** — `min-width: 0; padding-left: 20px`. Contains a 14px checkbox
   (`border-radius: 4px; border: 1px solid color-mix(in srgb, var(--aurora-border-strong) 85%, transparent); background: var(--gw0-0_48)`),
   the name, status badges (e.g. `AUTH`), and hover-revealed action icons.
3. **Clients** — `justify-self: center; min-width: 0`. Icon + count, or `—`.
4. **Endpoint** — `justify-self: center; min-width: 0; max-width: 100%; padding: 0 10px`. Mono.
5. **Exposed** — `justify-self: center; min-width: 0`. Inner:
   `display: grid; grid-template-columns: 40px 40px 40px; column-gap: 6px; align-items: center`.
   Each count: `inline-flex; gap: 4px; font-size: 12px; font-weight: 650; font-variant-numeric: tabular-nums`.
   Tone is conditional, NOT "tools are always pink":
   `discovered === 0` -> dimmed em-dash; `exposed < discovered` -> `var(--aurora-accent-pink)`;
   `exposed === discovered` -> `var(--aurora-text-primary)`. Applies to all three counts.
   Title attribute: `Exposed — tools 0/7 · resources 0/0 · prompts 0/0`.
6. **Uptime** — `justify-self: center; min-width: 0`. Sparkline
   (`display: flex; gap: 1.5px; align-items: center`) followed by a percentage.
   Title: `Uptime · last 24h — 79.2% (per-hour reconciliation probes)`.

Below 700px the mock collapses the head and reflows rows to wrapped flex.
Moot for us: our desktop grid is `hidden md:block` and a card list takes over
below 768px.

`bg-aurora-neutral` resolves to nothing — there is no `--aurora-neutral` token.

Tailwind gotcha: the `--gw*` token names contain underscores, which Tailwind
rewrites to spaces inside arbitrary values. Alias them to underscore-free
custom properties on a wrapper rather than relying on `\_` escapes.

### Filters

The mock has no filter-bar card; filtering lives in the command palette.
The gateway inventory now hides `GatewayFilters` at the mock's 1101px desktop
table breakpoint, so the hero flows directly into the grouped table. The same
controls remain available on compact viewports, where the UI uses cards rather
than the mock's desktop table. The aggregated Tools view retains its filter bar
because those tool-specific facets are not represented by the server palette.

## Settings (measured, implemented)

The mock's Settings screen has **no hero card** — just a plain title block. Do
not reach for `ConsoleHero` here.

```
body:   display:flex; flex-direction:column; gap:14px; max-width:760px
h1:     var(--font-display); 24px; 800; margin 0
p:      margin:5px 0 0; 12.5px; muted
card:   radius var(--radius-2); 1px color-mix(border-default 45%, page-bg);
        linear-gradient(180deg, panel-strong-top, panel-strong);
        var(--aurora-shadow-medium), inset 0 1px 0 rgba(255,255,255,.04)
header: padding 11px 16px; border-bottom color-mix(70%, page-bg);
        background var(--gw0-0_38); 10.5px/700/0.15em uppercase muted
body:   padding 4px 0
row:    flex; gap 14px; padding 11px 16px; border-top color-mix(35%, page-bg)
        label 13px/600 primary; description 11.5px/1.5 muted, margin-top 2px
toggle: 34x19 pill; on accent-primary / off color-mix(border-strong 80%);
        knob 15x15 at top 2px, left 2px->17px, bg var(--aurora-page-bg),
        0 1px 2px rgba(0,0,0,.4), transition left 160ms
segment:28px tall; radius 8; 11.5px/650
        active: border accent 45% / bg accent 14% / color accent-strong
        idle:   border color-mix(border-default 70%, page-bg) /
                bg control-surface / color muted
value:  <code> 11px muted, font-family INHERITED (not mono)
```

All of the above lives in `components/settings/SettingsChrome.tsx`.
The mock has no settings sub-nav; ours renders its 7 panels using the mock's
own segmented control in a strip above the column.

## Then

- Gateway detail and the command palette/Add Server flow have now received a
  fresh side-by-side audit against the Depot mock. The detail tab contract is
  exactly `Overview · Variables · Catalog · Activity · Routes · Logs` and
  defaults to Overview. Existing runtime facts, client configuration, catalog,
  exposure controls, and warnings back five tabs; per-server Activity remains
  an explicitly badged mock fixture because that history is not in the API.
  The palette retains its real create mutation and now exposes the reference
  flow's real Full Dialog handoff.
- Skills and Usage remain connected internal routes rather than scoped Depot
  navigation destinations; they still need a final direct-route visual audit.
- Snippets body now follows the mock's table and inline execution layout.
  Backend-missing runs / fails / average / history values render as `—` rather
  than invented fixture data.
- Discovery now has its page-specific Bazaar layout: semantic search, view and
  trend controls, source-aware artifact cards, and the mock's card metadata.
  The catalog remains illustrative and every region/action is marked as mock.
- Create now follows the focused artifact-editor screen with validation
  signals, tag/body fields, insert-section tools, frontmatter status, and
  autosave chrome. Library now follows the artifact/loadout/snippet tabs,
  inventory stats, behind-upstream notice, facets, sorting, and dense artifact
  rows. Both are mock-only and visibly label all illustrative state.
- Agents now follows the mock's running/completed/failed session inventory,
  including loadout, container, repository, elapsed-time, and session actions.
  Tasks now follows the scheduled-task inventory with armed switches, cadence,
  project scope, next-run state, and run actions. Both remain fully marked and
  disabled mock surfaces.
- Stash now follows the scratch-drive drop zone, type filters, `stash://` file
  table, access state, and file actions. Dev Containers now follows the three
  Incus image cards with distro/build/toolchain/pull state. Labby Instance now
  follows the hosted dashboard with connection credentials, 24-hour traffic,
  deployed loadouts, and instance metadata. All are explicitly marked mocks.
- Logs now follows the mock's dark stream island with source selection,
  severity counts, follow/download controls, time/level/source/message rows,
  per-line copy actions, and paused-buffer status. Sessions remains a marked
  internal mock route because the current artifact does not expose it in the
  scoped sidebar.
- Team Overview, Library, Projects, Activity, and Stash are now separate mock
  implementations rather than one generic list template. They reproduce the
  measured launcher grid, review queue and artifact table, project binding
  detail, activity feed and Axon suggestions, and team `stash://` inventory.
  Every Team page carries a visible mock notice, `data-mock-surface`, scoped
  `data-mock-region` markers, illustrative identities, and disabled actions.
- The command palette's inline Add Server sheet now includes the mock's
  `Full Dialog` escape hatch. It navigates to `/gateways/?add=1`, opens the
  existing real gateway editor, and removes the transient query flag; the
  compact `Add & Probe` path remains connected to the create mutation.
- The shared topbar now owns the mock's global Add Server split control on
  every route. The primary action opens the real full gateway editor; the
  adjacent options action opens the real inline palette sheet. Gateway no
  longer injects a duplicate page-local Add Server control.
- The account avatar and its existing real popover have moved from the sidebar
  footer to the topbar's right edge, matching the reference placement without
  duplicating appearance, documentation, design-system, or sign-out actions.
- Overview now uses the reference labels `Calls by Server`, `Top Tools`,
  `Least Used Tools`, and `Most Active Agents`. The unavailable Connected
  Clients and Gateway Host panels are retained as fixtures with separate
  visible `Mock data` badges and `data-mock-region` boundaries.
- Overview and Gateway build their stat strips inline; `ConsoleHero` should
  absorb them once there is room to re-verify both.
- Mock-only Depot, workspace, and team screens now have routes. Every fixture
  region carries `data-mock-region`, a visible `Mock data` badge, and disabled
  mutation controls. Real Gateway, Snippets, Skills, Usage, and Settings
  behavior remains connected to the existing clients.

## Deliberate deviations

- **Brand glyph** — mock uses the Aurora `BrandMark` glyph; we keep the Labby
  glyph while matching the Depot wordmark and realm label.
- **⌘ vs Ctrl** — mock hardcodes `⌘K`; ours is platform-aware.
- **Reference routes** — `/docs` and `/design-system` live in the account
  popover because the mock does not place them in the scoped navigation.
- **Nav status dots** — omitted because their health meaning is not available
  from the shared shell. Active-item context lines mirror the reference and
  carry an adjacent `MOCK` marker until live sidebar summaries are available.
- **Development fleet fixtures** — Gateway remains a connected surface, so
  mock-data mode keeps its detailed five-server integration fixtures instead
  of replacing them with the reference artifact's illustrative 16-server
  names. The environment is visibly labeled `MOCK`; table geometry, grouping,
  health states, exposure columns, and interactions follow the reference while
  production values continue to come from the API.
