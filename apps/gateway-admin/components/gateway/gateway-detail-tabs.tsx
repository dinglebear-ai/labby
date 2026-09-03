'use client'

import * as React from 'react'
import * as TabsPrimitive from '@radix-ui/react-tabs'

/**
 * Header-card chrome for the Gateway detail *page* — tab bar, attached stat
 * strip, and the overview key/value card.
 *
 * Provenance
 * ----------
 * Measured off the rendered Gateway Console mock (`Gateway Console.dc.html`,
 * project d80fe050-1bc9-44b0-aa68-6e873344c619) on 2026-08-14, two ways:
 *
 * 1. Live DOM via `agent-browser eval` after navigating to the detail page —
 *    click the server *name* (`<a>` in the row's first grid cell), not the row
 *    body. The table unmounts (`[data-gwrow]` count 0), `h1` and
 *    `[data-crumbleaf]` become the server name.
 * 2. The mock's own inline style strings (`dTabDefs` / `dTabs`, the header-card
 *    template), which are the literals the computed values above came from.
 *
 * The live tab set, enumerated (not guessed) from `button[aria-pressed]`
 * inside the header card:
 *
 *     Overview · Variables · Catalog 7 · Activity 1K · Routes 3 · Logs
 *
 * `Overview`/`Variables`/`Logs` carry no count badge; the other three do. A
 * seventh `files` tab exists in the mock's state machine but is not in
 * `dTabDefs` — it is only reachable from the topbar's "Generate skill" action.
 *
 * We do not ship the mock's tab set: ours is driven by what the gateway API
 * actually returns (see `gateway-detail-content.tsx`). What is ported here is
 * the *chrome* — geometry, type, colour, and the count-badge treatment.
 *
 * Every literal below was read off the mock. Re-measure rather than adjusting
 * by eye.
 */

// ---------------------------------------------------------------------------
// Tab bar
// ---------------------------------------------------------------------------

/**
 * The tab-bar row. In the mock this is the last band of the header card: a
 * full-bleed strip separated by a hairline, holding the scrolling tab list and
 * a capability-icon cluster on the right (see {@link DetailCapabilityCluster}).
 *
 * `bleed` is the host card's horizontal padding — the row cancels it with
 * negative margins and re-applies it as padding, exactly as the mock does at
 * its own 18px card padding.
 */
export function DetailTabBar({
  children,
  bleed = 20,
  style,
  ...rest
}: React.HTMLAttributes<HTMLDivElement> & { bleed?: number }) {
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 10,
        minWidth: 0,
        margin: `0 -${bleed}px`,
        padding: `6px ${bleed}px 0`,
        borderTop:
          '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
        ...style,
      }}
      {...rest}
    >
      {children}
    </div>
  )
}

/** The horizontally scrolling tab list inside {@link DetailTabBar}. */
export function DetailTabsList({
  className,
  style,
  ...rest
}: React.ComponentProps<typeof TabsPrimitive.List>) {
  return (
    <TabsPrimitive.List
      className={className}
      style={{
        flex: '1 1 0%',
        minWidth: 0,
        overflowX: 'auto',
        display: 'flex',
        alignItems: 'center',
        gap: 2,
        ...style,
      }}
      {...rest}
    />
  )
}

/**
 * Mock tab-bar geometry: 34px tall, 13px side padding, 2px bottom indicator,
 * 12.5px/650 label. Idle is muted text with a transparent indicator; active is
 * `--aurora-accent-strong` text over an `--aurora-accent-primary` indicator.
 */
const DETAIL_TAB_BASE_STYLE: React.CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 6,
  height: 34,
  padding: '0 13px',
  border: 'none',
  borderBottomWidth: 2,
  borderBottomStyle: 'solid',
  borderBottomColor: 'transparent',
  background: 'none',
  fontFamily: 'inherit',
  fontSize: 12.5,
  fontWeight: 650,
  cursor: 'pointer',
  whiteSpace: 'nowrap',
  flexShrink: 0,
  transition: 'color 150ms, border-color 150ms',
}

/**
 * Count badge beside a tab label. The mock re-tones it with the tab: idle is a
 * `--gw0-0_48` chip on an 80%-blended strong border, active is a 12% accent
 * wash on a 30% accent border.
 *
 * These backgrounds are inline rather than Tailwind arbitrary values on
 * purpose: the `--gw*` token names contain underscores, which Tailwind rewrites
 * to spaces inside `bg-[var(--gw0-0_48)]`.
 */
function detailTabCountStyle(active: boolean): React.CSSProperties {
  return {
    display: 'inline-flex',
    alignItems: 'center',
    height: 17,
    padding: '0 5px',
    borderRadius: 5,
    fontSize: 10,
    fontWeight: 650,
    fontVariantNumeric: 'tabular-nums',
    border: active
      ? '1px solid color-mix(in srgb, var(--aurora-accent-primary) 30%, transparent)'
      : '1px solid color-mix(in srgb, var(--aurora-border-strong) 80%, transparent)',
    background: active
      ? 'color-mix(in srgb, var(--aurora-accent-primary) 12%, transparent)'
      : 'var(--gw0-0_48)',
    color: active ? 'var(--aurora-accent-strong)' : 'var(--aurora-text-muted)',
  }
}

export interface DetailTabTriggerProps
  extends Omit<React.ComponentProps<typeof TabsPrimitive.Trigger>, 'children'> {
  /** Whether this tab is the selected one. Drives the mock's active tone. */
  active: boolean
  label: React.ReactNode
  /** Omit for a label-only tab — the mock's Overview/Variables/Logs. */
  count?: React.ReactNode
  /**
   * The mock keeps a warn branch in its tab factory for a warnings-style tab:
   * idle text switches to `--aurora-warn` while the active tone is unchanged.
   */
  tone?: 'default' | 'warn'
}

export function DetailTabTrigger({
  active,
  label,
  count,
  tone = 'default',
  className,
  style,
  ...rest
}: DetailTabTriggerProps) {
  const idleColor = tone === 'warn' ? 'var(--aurora-warn)' : 'var(--aurora-text-muted)'
  return (
    <TabsPrimitive.Trigger
      className={[
        active ? '' : 'hover:text-[var(--aurora-text-primary)]',
        'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--aurora-accent-primary)]/40',
        className,
      ]
        .filter(Boolean)
        .join(' ')}
      style={{
        ...DETAIL_TAB_BASE_STYLE,
        color: active ? 'var(--aurora-accent-strong)' : idleColor,
        borderBottomColor: active ? 'var(--aurora-accent-primary)' : 'transparent',
        ...style,
      }}
      {...rest}
    >
      {label}
      {count === undefined || count === null ? null : (
        <span style={detailTabCountStyle(active)}>{count}</span>
      )}
    </TabsPrimitive.Trigger>
  )
}

// ---------------------------------------------------------------------------
// Capability cluster
// ---------------------------------------------------------------------------

/**
 * Per-capability tone in {@link DetailCapabilityCluster}.
 *
 * `supported` / `not_advertised` are the mock's own two states. `unknown` is
 * ours, and it exists because the two mock states are both *claims*: one says
 * the server advertised the capability in its `initialize` response, the other
 * says it did not. The gateway API returns no capability set at all, so making
 * either claim would be fabrication.
 */
export type DetailCapabilityState = 'supported' | 'not_advertised' | 'unknown'

export type DetailCapabilityKey =
  | 'tools'
  | 'prompts'
  | 'resources'
  | 'elicitation'
  | 'ui_resources'
  | 'sampling'
  | 'logging'
  | 'completions'
  | 'tasks'
  | 'ping'
  | 'progress'

/**
 * 12px/1.6-stroke glyph, matching the mock's inline `<svg>` attributes exactly
 * (`viewBox 0 0 24 24`, `fill none`, `stroke currentColor`, round caps/joins,
 * `display: block`). Paths below are the literal `d` strings read off the
 * mock's DOM rather than lucide-react components, so the cluster cannot drift
 * when the icon package is bumped.
 */
function CapabilityGlyph({ children }: { children: React.ReactNode }) {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ display: 'block' }}
      aria-hidden="true"
    >
      {children}
    </svg>
  )
}

/**
 * The mock's capability list, in its own order — enumerated from the live DOM
 * (`title` attribute per icon), not guessed from the MCP spec.
 */
export const DETAIL_CAPABILITIES: ReadonlyArray<{
  key: DetailCapabilityKey
  label: string
  icon: React.ReactNode
}> = [
  {
    key: 'tools',
    label: 'Tools',
    icon: (
      <CapabilityGlyph>
        <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z" />
      </CapabilityGlyph>
    ),
  },
  {
    key: 'prompts',
    label: 'Prompts',
    icon: (
      <CapabilityGlyph>
        <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
      </CapabilityGlyph>
    ),
  },
  {
    key: 'resources',
    label: 'Resources',
    icon: (
      <CapabilityGlyph>
        <path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z" />
        <path d="M14 2v4a2 2 0 0 0 2 2h4" />
        <path d="m9 18 3-3-3-3" />
      </CapabilityGlyph>
    ),
  },
  {
    key: 'elicitation',
    label: 'Elicitation',
    icon: (
      <CapabilityGlyph>
        <path d="M12 3v3M18.4 5.6l-2.1 2.1M21 12h-3M5.6 5.6l2.1 2.1M3 12h3M12 21v-3M7.7 16.3l-2.1 2.1M16.3 16.3l2.1 2.1" />
        <circle cx="12" cy="12" r="3" />
      </CapabilityGlyph>
    ),
  },
  {
    key: 'ui_resources',
    label: 'UI Resources',
    icon: (
      <CapabilityGlyph>
        <rect width="18" height="18" x="3" y="3" rx="2" />
        <path d="M3 9h18" />
        <path d="M9 21V9" />
      </CapabilityGlyph>
    ),
  },
  {
    key: 'sampling',
    label: 'Sampling',
    icon: (
      <CapabilityGlyph>
        <path d="m18 14 4 4-4 4" />
        <path d="m18 2 4 4-4 4" />
        <path d="M2 18h1.973a4 4 0 0 0 3.3-1.7l5.454-8.6a4 4 0 0 1 3.3-1.7H22" />
        <path d="M2 6h1.972a4 4 0 0 1 3.6 2.2" />
        <path d="M22 18h-6.041a4 4 0 0 1-3.3-1.8l-.359-.45" />
      </CapabilityGlyph>
    ),
  },
  {
    key: 'logging',
    label: 'Logging',
    icon: (
      <CapabilityGlyph>
        <path d="M3 5h.01M3 12h.01M3 19h.01M8 5h13M8 12h13M8 19h13" />
      </CapabilityGlyph>
    ),
  },
  {
    key: 'completions',
    label: 'Completions',
    icon: (
      <CapabilityGlyph>
        <path d="M17 22h-1a4 4 0 0 1-4-4V6a4 4 0 0 1 4-4h1" />
        <path d="M7 22h1a4 4 0 0 0 4-4v-1" />
        <path d="M7 2h1a4 4 0 0 1 4 4v1" />
      </CapabilityGlyph>
    ),
  },
  {
    key: 'tasks',
    label: 'Tasks',
    icon: (
      <CapabilityGlyph>
        <path d="m9 11 3 3L22 4" />
        <path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11" />
      </CapabilityGlyph>
    ),
  },
  {
    key: 'ping',
    label: 'Ping',
    icon: (
      <CapabilityGlyph>
        <path d="M22 12h-2.48a2 2 0 0 0-1.93 1.46l-2.35 8.36a.25.25 0 0 1-.48 0L9.24 2.18a.25.25 0 0 0-.48 0l-2.35 8.36A2 2 0 0 1 4.49 12H2" />
      </CapabilityGlyph>
    ),
  },
  {
    key: 'progress',
    label: 'Progress',
    icon: (
      <CapabilityGlyph>
        <circle cx="12" cy="12" r="10" />
        <path d="M12 6v6l4 2" />
      </CapabilityGlyph>
    ),
  },
]

/**
 * 24×24 icon box, 7px radius, grid-centred — measured off the mock. Only the
 * border/background/colour triple changes with state.
 *
 * `box-sizing: content-box` is load-bearing: the mock has no CSS reset, so its
 * `width: 24px` plus a 1px border renders a 26px box. Tailwind's preflight
 * makes everything `border-box` here, which would silently shrink the cluster
 * to 24px cells and 30px tall. Opting this one element out reproduces the
 * measured 26×26 cell and 32px cluster height.
 */
const CAPABILITY_BOX_STYLE: React.CSSProperties = {
  boxSizing: 'content-box',
  display: 'grid',
  placeItems: 'center',
  width: 24,
  height: 24,
  borderRadius: 7,
}

/**
 * The mock ships two tones; `unknown` is ours.
 *
 * Unknown retains the mock's muted box treatment. Its accessible label and
 * tooltip explicitly distinguish "not reported" from "not advertised".
 */
const CAPABILITY_TONE_STYLE: Record<DetailCapabilityState, React.CSSProperties> = {
  supported: {
    border: '1px solid color-mix(in srgb, var(--aurora-accent-primary) 28%, transparent)',
    background: 'color-mix(in srgb, var(--aurora-accent-primary) 10%, transparent)',
    color: 'var(--aurora-accent-strong)',
  },
  not_advertised: {
    border:
      '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
    background: 'var(--gw0-0_30)',
    color: 'color-mix(in srgb, var(--aurora-text-muted) 45%, transparent)',
  },
  unknown: {
    border: '1px dashed color-mix(in srgb, var(--aurora-border-strong) 70%, transparent)',
    background: 'transparent',
    color: 'color-mix(in srgb, var(--aurora-text-muted) 70%, transparent)',
  },
}

function capabilityTitle(label: string, state: DetailCapabilityState): string {
  if (state === 'supported') return `${label} — supported`
  if (state === 'not_advertised') return `${label} — not advertised`
  return (
    `${label} — not reported. The gateway API does not report this server's ` +
    `advertised capability set, so Labby cannot tell whether ${label} is ` +
    `supported. This is not the same as "not supported".`
  )
}

/**
 * The capability cluster parked at the right end of the detail tab bar.
 *
 * Mock geometry, measured live: `flex; align-items: center; gap: 3px;
 * padding-bottom: 6px`, eleven 24px boxes (26px border-box).
 * Its container `title` in the mock reads `6 of 12 capabilities advertised in
 * initialize.`
 *
 * **Why every icon is `unknown` here.** The mock's tones encode an answer to
 * "did this server advertise capability X in its `initialize` response?" The
 * gateway API exposes no capability set — `Gateway` carries `status` counts,
 * `discovery` lists, `config.proxy_*` (which are *our* proxy switches, not the
 * upstream's advertisement) and `surfaces` (Labby's own surfaces). None of
 * those answer the question. Deriving a capability from a tool/prompt/resource
 * count would be inference, so the affordance renders in the third state
 * instead of being dropped — matching how the rest of the console shows `—`
 * for fields the API cannot back.
 *
 * Pass `states` once the API grows a capability set; anything unlisted stays
 * `unknown`.
 *
 * One deliberate deviation from the mock: the mock pins the cluster at
 * `flex-shrink: 0`, which overflows the viewport below ~430px. Ours shrinks
 * and scrolls internally instead. Above that width the rendered box is
 * identical, because there is slack in the row and nothing shrinks.
 */
export function DetailCapabilityCluster({
  states,
  style,
  ...rest
}: Omit<React.HTMLAttributes<HTMLDivElement>, 'title' | 'children'> & {
  states?: Partial<Record<DetailCapabilityKey, DetailCapabilityState>>
}) {
  const resolved = DETAIL_CAPABILITIES.map((capability) => ({
    ...capability,
    state: states?.[capability.key] ?? ('unknown' as DetailCapabilityState),
  }))
  const allUnknown = resolved.every((capability) => capability.state === 'unknown')
  const advertised = resolved.filter((capability) => capability.state === 'supported').length

  const clusterTitle = allUnknown
    ? `Capabilities — not reported. The gateway API does not return the MCP ` +
      `initialize capability set for this server, so none of these ` +
      `${DETAIL_CAPABILITIES.length} capabilities can be shown as supported or ` +
      `unsupported.`
    : `${advertised} of ${DETAIL_CAPABILITIES.length} capabilities advertised in initialize.`

  return (
    <div
      role="img"
      aria-label={clusterTitle}
      title={clusterTitle}
      style={{
        flexShrink: 1,
        minWidth: 0,
        overflowX: 'auto',
        display: 'flex',
        alignItems: 'center',
        gap: 3,
        paddingBottom: 4,
        ...style,
      }}
      {...rest}
    >
      {resolved.map((capability) => (
        <span
          key={capability.key}
          title={capabilityTitle(capability.label, capability.state)}
          style={{
            ...CAPABILITY_BOX_STYLE,
            ...(capability.state === 'unknown'
              ? CAPABILITY_TONE_STYLE.not_advertised
              : CAPABILITY_TONE_STYLE[capability.state]),
            flexShrink: 0,
          }}
        >
          {capability.icon}
        </span>
      ))}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Stat strip
// ---------------------------------------------------------------------------

/**
 * The stat strip attached to the bottom of the header card, above the tab bar.
 * The mock lays it out as `2fr repeat(n, minmax(120px, 1fr))` — a wide Exposed
 * cell followed by equal health cards — full-bleed on a `--gw0-0_30` wash.
 */
export function DetailStatStrip({
  children,
  cardCount,
  bleed = 20,
  style,
  ...rest
}: React.HTMLAttributes<HTMLDivElement> & { cardCount: number; bleed?: number }) {
  return (
    <div
      style={{
        display: 'grid',
        gridTemplateColumns: `2fr repeat(${cardCount}, minmax(120px, 1fr))`,
        margin: `10px -${bleed}px 0`,
        borderTop:
          '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
        background: 'var(--gw0-0_30)',
        ...style,
      }}
      {...rest}
    >
      {children}
    </div>
  )
}

const STRIP_LABEL_STYLE: React.CSSProperties = {
  fontSize: 10,
  fontWeight: 700,
  letterSpacing: '0.13em',
  textTransform: 'uppercase',
  color: 'var(--aurora-text-muted)',
  whiteSpace: 'nowrap',
}

const STRIP_VALUE_STYLE: React.CSSProperties = {
  fontFamily: 'var(--font-display)',
  fontSize: 17,
  lineHeight: 1,
  fontWeight: 800,
  fontVariantNumeric: 'tabular-nums',
}

/** One health card in {@link DetailStatStrip}. Pass `'—'` for absent metrics. */
export function DetailStripCard({
  label,
  value,
  sub,
  valueColor = 'var(--aurora-text-primary)',
  title,
  bleed = 20,
  first = false,
}: {
  label: React.ReactNode
  value: React.ReactNode
  sub?: React.ReactNode
  valueColor?: string
  title?: string
  bleed?: number
  /** First cell of the strip — takes the card's own left padding, no rule. */
  first?: boolean
}) {
  return (
    <div
      title={title}
      style={{
        minWidth: 0,
        padding: first ? `12px 16px 13px ${bleed}px` : '12px 16px 13px',
        borderLeft: first
          ? undefined
          : '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
      }}
    >
      <div style={STRIP_LABEL_STYLE}>{label}</div>
      <div className="font-display" style={{ ...STRIP_VALUE_STYLE, marginTop: 6, color: valueColor }}>
        {value}
      </div>
      {sub ? (
        <div
          style={{
            marginTop: 4,
            fontSize: 10.5,
            color: 'var(--aurora-text-muted)',
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          {sub}
        </div>
      ) : null}
    </div>
  )
}

export interface DetailExposureStat {
  label: string
  icon: React.ReactNode
  exposed: number
  discovered: number
}

/**
 * The strip's wide leading cell: a button into the catalog holding one
 * icon + value + progress bar per primitive kind.
 *
 * The mock's tone rules, ported verbatim: `discovered === 0` dims the value to
 * a 55%-blended muted em-dash, and a bar is warn-toned whenever something is
 * discovered but not fully exposed.
 */
export function DetailExposureCell({
  stats,
  onClick,
  showEnable,
  bleed = 20,
  ariaLabel,
  title,
}: {
  stats: DetailExposureStat[]
  onClick: () => void
  /** The mock's pink "Enable" nudge — everything discovered, nothing exposed. */
  showEnable: boolean
  bleed?: number
  ariaLabel?: string
  title?: string
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={ariaLabel}
      title={title}
      className="hover:bg-[var(--aurora-hover-bg)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--aurora-accent-primary)]/40"
      style={{
        minWidth: 0,
        textAlign: 'left',
        fontFamily: 'inherit',
        cursor: 'pointer',
        padding: `12px 16px 13px ${bleed - 2}px`,
        border: 'none',
        background: 'none',
        transition: 'background 150ms',
      }}
    >
      <span style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <span style={STRIP_LABEL_STYLE}>Exposed</span>
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
          {showEnable ? (
            <span style={{ fontSize: 10, fontWeight: 700, color: 'var(--aurora-accent-pink)' }}>
              Enable
            </span>
          ) : null}
          <svg
            width="12"
            height="12"
            viewBox="0 0 24 24"
            fill="none"
            stroke="var(--aurora-text-muted)"
            strokeWidth="1.8"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true"
          >
            <path d="M5 12h14" />
            <path d="m12 5 7 7-7 7" />
          </svg>
        </span>
      </span>
      <span
        style={{
          display: 'grid',
          gridTemplateColumns: '1fr 1fr 1fr',
          gap: 12,
          marginTop: 6,
        }}
      >
        {stats.map((stat) => {
          const empty = stat.discovered === 0
          const pct = empty ? 0 : Math.round((stat.exposed / stat.discovered) * 100)
          return (
            <span
              key={stat.label}
              style={{ minWidth: 0 }}
              title={`${stat.label} — ${empty ? 'none discovered' : `${stat.exposed}/${stat.discovered} exposed`}`}
            >
              <span style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                <span
                  aria-hidden="true"
                  style={{ display: 'grid', placeItems: 'center', color: 'var(--aurora-text-muted)' }}
                >
                  {stat.icon}
                </span>
                <span
                  className="font-display"
                  style={{
                    ...STRIP_VALUE_STYLE,
                    color: empty
                      ? 'color-mix(in srgb, var(--aurora-text-muted) 55%, transparent)'
                      : 'var(--aurora-text-primary)',
                  }}
                >
                  {empty ? '—' : `${stat.exposed}/${stat.discovered}`}
                </span>
              </span>
              <span
                aria-hidden="true"
                style={{
                  display: 'block',
                  marginTop: 6,
                  height: 3,
                  borderRadius: 999,
                  background: 'var(--gw0-0_6)',
                  overflow: 'hidden',
                }}
              >
                <span
                  style={{
                    display: 'block',
                    height: '100%',
                    borderRadius: 999,
                    background:
                      !empty && stat.exposed < stat.discovered
                        ? 'var(--aurora-warn)'
                        : 'var(--aurora-accent-primary)',
                    width: `${pct}%`,
                  }}
                />
              </span>
            </span>
          )
        })}
      </span>
    </button>
  )
}

// ---------------------------------------------------------------------------
// Key/value card
// ---------------------------------------------------------------------------

export interface DetailKeyValueRow {
  label: string
  /** Rendered verbatim. Pass `'—'` for a field the API does not expose. */
  value: React.ReactNode
  valueColor?: string
}

/**
 * The mock's overview key/value panel — "Process & Storage" /
 * "Connection & Network" / "Server Metadata" all share this chrome: a
 * panel-medium card with an uppercase header band and baseline-aligned rows.
 */
export function DetailKeyValueCard({
  label,
  rows,
}: {
  label: React.ReactNode
  rows: DetailKeyValueRow[]
}) {
  return (
    <div
      style={{
        borderRadius: 'var(--radius-2)',
        border:
          '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
        background:
          'linear-gradient(180deg, var(--aurora-panel-medium-top), transparent), var(--aurora-panel-medium)',
        boxShadow: 'var(--aurora-shadow-medium), inset 0 1px 0 rgba(255,255,255,0.035)',
        overflow: 'hidden',
        minWidth: 0,
      }}
    >
      <div
        style={{
          padding: '11px 16px',
          borderBottom:
            '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
          background: 'var(--gw0-0_30)',
          fontSize: 10.5,
          fontWeight: 700,
          letterSpacing: '0.15em',
          textTransform: 'uppercase',
          color: 'var(--aurora-text-muted)',
        }}
      >
        {label}
      </div>
      <div style={{ padding: '6px 0' }}>
        {rows.map((row) => (
          <div
            key={row.label}
            style={{
              display: 'flex',
              alignItems: 'baseline',
              justifyContent: 'space-between',
              gap: 12,
              padding: '6px 16px',
            }}
          >
            <span
              style={{ fontSize: 11.5, color: 'var(--aurora-text-muted)', whiteSpace: 'nowrap' }}
            >
              {row.label}
            </span>
            <span
              style={{
                fontSize: 12,
                fontWeight: 560,
                color: row.valueColor ?? 'var(--aurora-text-primary)',
                textAlign: 'right',
                wordBreak: 'break-all',
              }}
            >
              {row.value}
            </span>
          </div>
        ))}
      </div>
    </div>
  )
}

/** Grid the mock uses for the overview's key/value card row. */
export const DETAIL_KV_GRID_STYLE: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'repeat(auto-fit, minmax(300px, 1fr))',
  gap: 12,
  alignItems: 'start',
}
