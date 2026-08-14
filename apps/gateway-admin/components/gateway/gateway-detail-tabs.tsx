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
 * (in the mock) a capability-icon cluster on the right.
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
