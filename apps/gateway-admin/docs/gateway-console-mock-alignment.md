# Gateway Console — mock alignment spec

Working notes for aligning `apps/gateway-admin` to the Claude Design mock
`Gateway Console.dc.html` (project `d80fe050-1bc9-44b0-aa68-6e873344c619`).

**Ground rule: measure before editing.** Every value below was read off the
mock's live DOM via `agent-browser eval`, not inferred from screenshots or
source. Re-measure rather than trusting this file if the mock has moved.

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
- `DashboardPanel` rewritten to the mock's card + header-bar chrome.
- Overview hero and body split; Gateway hero; Snippets hero (`ConsoleHero`).

## Next: Gateway table

Currently `Server · Transport · Tools · Resources · Prompts · Actions`, flat.
The mock is `Server · Clients · Endpoint · Exposed · Uptime`, grouped by status.

`Clients` and `Uptime` have no field on the `Gateway` type. **Render the
columns and show `—`** — the mock does exactly this for `mcp.sh`, which has no
client data. Do not invent values; do not omit the columns.

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
Ours renders `GatewayFilters` as a separate card between hero and table.

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

- Gateway detail (tabbed), Skills, Usage, Settings, command palette,
  Add Server dialog — none yet driven side by side.
- Snippets **body** still diverges: the mock has a snippet table with
  runs / fails / avg / history sparklines and an inline execution panel.
  Only the hero is aligned.
- Overview and Gateway build their stat strips inline; `ConsoleHero` should
  absorb them once there is room to re-verify both.
- Mock screens with no route or API — Loadouts, Registry, Sessions, Tasks,
  Files, Logs, Terminal. A build, not a restyle.

## Deliberate deviations

- **Brand mark** — mock uses the Aurora `BrandMark` glyph; we keep `LabbyIcon`.
- **⌘ vs Ctrl** — mock hardcodes `⌘K`; ours is platform-aware.
- **Nav** — mock's `defs` map is Control Plane / Catalog / Agents / Observe.
  We ship only routes that exist, and `/docs` + `/design-system` live in the
  account popover because the mock has no nav entry for them.
- Nav badge dots and the active item's context line ("16 servers · 127
  calls/min") are unwired — both need data plumbed into the sidebar.
