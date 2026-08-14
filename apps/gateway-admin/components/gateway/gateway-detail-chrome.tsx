'use client'

import * as React from 'react'

/**
 * Chrome primitives for the Gateway detail screen, measured off the rendered
 * Gateway Console mock (`Gateway Console.dc.html`, project
 * d80fe050-1bc9-44b0-aa68-6e873344c619) via `agent-browser eval`.
 *
 * Where the mock's detail lives — TWO different surfaces
 * ------------------------------------------------------
 * The mock's gateway table gives a row two distinct affordances, and they
 * have different chrome. Do not conflate them:
 *
 * 1. **Row expansion** — clicking the row *body* expands an inline panel
 *    beneath it. Chrome: 26px borderless ghost icon buttons, a warn "Auth"
 *    pill, 24px metric pills, a full-bleed divider, a stat-card grid
 *    (auto-fit minmax(150px,1fr), gap 10) and a wider panel grid
 *    (auto-fit minmax(190px,1fr), gap 10).
 *
 * 2. **Detail page** — clicking the server *name* (an `<a>` in the first grid
 *    cell) is a real page change: the table unmounts, `h1` and
 *    `[data-crumbleaf]` become the server name, and the mock's source flips
 *    `page: 'detail'`. This page has a sticky header card, a stat strip, and
 *    an underline tab bar: Overview · Variables · Catalog · Activity ·
 *    Routes · Logs. Its topbar cluster is 32px bordered control-surface
 *    buttons (Test / View in Logs / Reload / Generate skill / Edit / More),
 *    NOT the row expansion's 26px ghosts.
 *
 * Our route is the detail page, so it drives surface 2. The surface-1
 * primitives are kept because the result sheets reuse that stat-card
 * vocabulary.
 *
 * Every literal below was read off the mock's live DOM. Re-measure rather than
 * adjusting by eye.
 */

// ---------------------------------------------------------------------------
// Card
// ---------------------------------------------------------------------------

/**
 * The console card chrome the mock uses for the gateway table and every
 * top-level panel: radius-2, a 45%-blended border, the panel-strong gradient,
 * and the strong shadow with a 5% inset top highlight.
 */
export const DETAIL_CARD_STYLE: React.CSSProperties = {
  borderRadius: 'var(--radius-2)',
  border:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
  background:
    'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
  boxShadow: 'var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05)',
}

export function DetailCard({
  children,
  padding = '16px 20px',
  style,
  ...rest
}: React.HTMLAttributes<HTMLDivElement> & { padding?: string }) {
  return (
    <div style={{ ...DETAIL_CARD_STYLE, padding, minWidth: 0, ...style }} {...rest}>
      {children}
    </div>
  )
}

/**
 * Inset surface used for every sub-card inside the mock's detail panel —
 * the CPU/RAM/CONNECTION/STORAGE/NETWORK stat cards and the
 * CLIENTS/TOP TOOLS/CALLS panels share one chrome.
 */
export const DETAIL_INSET_STYLE: React.CSSProperties = {
  padding: '10px 12px',
  borderRadius: 9,
  border:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 60%, var(--aurora-page-bg))',
  background: 'var(--gw0-0_42)',
  boxShadow: 'inset 0 1px 0 rgba(255,255,255,0.03)',
  minWidth: 0,
}

export function DetailInset({
  children,
  style,
  ...rest
}: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div style={{ ...DETAIL_INSET_STYLE, ...style }} {...rest}>
      {children}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Typography
// ---------------------------------------------------------------------------

/** Micro eyebrow above every stat value in the mock's detail panel. */
export const DETAIL_MICRO_LABEL_STYLE: React.CSSProperties = {
  fontSize: 9,
  fontWeight: 700,
  letterSpacing: '0.14em',
  textTransform: 'uppercase',
  color: 'color-mix(in srgb, var(--aurora-text-muted) 75%, transparent)',
  whiteSpace: 'nowrap',
}

/**
 * Stat value — Manrope display face, 14.5px, extrabold, tabular. Matches the
 * mock's inline `font-family: var(--font-display)`.
 *
 * `var(--font-display)` used to resolve to nothing here (Tailwind v4's
 * `@theme inline` substitutes font families into utilities without emitting
 * the custom properties), which silently downgraded every display value to
 * Inter. That was fixed in `app/layout.tsx` + `app/globals.css` on
 * 2026-08-14; the `font-display` utility class on the consuming element is
 * kept as a belt-and-braces guard.
 */
export const DETAIL_STAT_VALUE_STYLE: React.CSSProperties = {
  fontFamily: 'var(--font-display)',
  fontSize: 14.5,
  lineHeight: 1,
  fontWeight: 800,
  fontVariantNumeric: 'tabular-nums',
  color: 'var(--aurora-text-primary)',
  whiteSpace: 'nowrap',
  overflow: 'hidden',
  textOverflow: 'ellipsis',
}

/** Support line under a stat value. */
export const DETAIL_STAT_SUB_STYLE: React.CSSProperties = {
  marginTop: 5,
  fontSize: 10,
  color: 'var(--aurora-text-muted)',
  whiteSpace: 'nowrap',
  overflow: 'hidden',
  textOverflow: 'ellipsis',
}

export function DetailMicroLabel({
  children,
  style,
}: {
  children: React.ReactNode
  style?: React.CSSProperties
}) {
  return <div style={{ ...DETAIL_MICRO_LABEL_STYLE, ...style }}>{children}</div>
}

// ---------------------------------------------------------------------------
// Grids
// ---------------------------------------------------------------------------

/** Stat-card grid — the mock's first detail row. */
export const DETAIL_STAT_GRID_STYLE: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'repeat(auto-fit, minmax(150px, 1fr))',
  gap: 10,
  alignItems: 'stretch',
}

/** Panel grid — the mock's second, wider detail row. */
export const DETAIL_PANEL_GRID_STYLE: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'repeat(auto-fit, minmax(190px, 1fr))',
  gap: 10,
  alignItems: 'stretch',
}

// ---------------------------------------------------------------------------
// Stat card
// ---------------------------------------------------------------------------

export interface DetailStatCardProps {
  label: string
  /** Rendered verbatim. Pass `'—'` for a metric the API does not expose. */
  value: React.ReactNode
  sub?: React.ReactNode
  icon?: React.ReactNode
  /** 0–100. Renders the mock's 3px accent meter under the value. */
  meterPercent?: number
  title?: string
}

/**
 * The mock's detail stat card: micro eyebrow + display value, with an optional
 * 3px accent meter and a 10px support line.
 */
export function DetailStatCard({
  label,
  value,
  sub,
  icon,
  meterPercent,
  title,
}: DetailStatCardProps) {
  return (
    <DetailInset title={title}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 6 }}>
        {icon ? (
          <span
            style={{
              display: 'grid',
              placeItems: 'center',
              color: 'color-mix(in srgb, var(--aurora-text-muted) 80%, transparent)',
            }}
            aria-hidden="true"
          >
            {icon}
          </span>
        ) : null}
        <DetailMicroLabel>{label}</DetailMicroLabel>
      </div>
      <div className="font-display" style={DETAIL_STAT_VALUE_STYLE}>
        {value}
      </div>
      {typeof meterPercent === 'number' ? (
        <div
          style={{
            marginTop: 6,
            height: 3,
            borderRadius: 999,
            background: 'var(--gw0-0_6)',
            overflow: 'hidden',
          }}
          aria-hidden="true"
        >
          <span
            style={{
              display: 'block',
              height: '100%',
              borderRadius: 999,
              background: 'color-mix(in srgb, var(--aurora-accent-primary) 65%, transparent)',
              width: `${Math.max(0, Math.min(100, meterPercent))}%`,
            }}
          />
        </div>
      ) : null}
      {sub ? <div style={DETAIL_STAT_SUB_STYLE}>{sub}</div> : null}
    </DetailInset>
  )
}

// ---------------------------------------------------------------------------
// Mini list
// ---------------------------------------------------------------------------

/**
 * The mock's CLIENTS · 24H / TOP TOOLS · 24H list panel — a labelled inset
 * holding name/value rows.
 */
export function DetailMiniList({
  label,
  rows,
  emptyValue = '—',
  title,
}: {
  label: string
  rows: Array<{ name: string; value: React.ReactNode }>
  emptyValue?: React.ReactNode
  title?: string
}) {
  return (
    <DetailInset title={title}>
      <DetailMicroLabel style={{ marginBottom: 6 }}>{label}</DetailMicroLabel>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        {rows.length === 0 ? (
          <div style={{ fontSize: 10.5, color: 'var(--aurora-text-muted)' }}>{emptyValue}</div>
        ) : (
          rows.map((row) => (
            <div key={row.name} style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
              <span
                style={{
                  flex: '1 1 0%',
                  minWidth: 0,
                  fontSize: 10.5,
                  color: 'var(--aurora-text-primary)',
                  whiteSpace: 'nowrap',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                }}
              >
                {row.name}
              </span>
              <span
                style={{
                  flexShrink: 0,
                  fontSize: 10,
                  color: 'var(--aurora-text-muted)',
                  fontVariantNumeric: 'tabular-nums',
                }}
              >
                {row.value}
              </span>
            </div>
          ))
        )}
      </div>
    </DetailInset>
  )
}

// ---------------------------------------------------------------------------
// Action cluster
// ---------------------------------------------------------------------------

/** Vertical hairline between action-cluster groups. */
export function DetailClusterRule({ margin }: { margin?: string }) {
  return (
    <span
      aria-hidden="true"
      style={{
        width: 1,
        height: 14,
        flexShrink: 0,
        margin,
        background: 'color-mix(in srgb, var(--aurora-border-default) 70%, transparent)',
      }}
    />
  )
}

/** Full-bleed divider. Pass negative margins to bleed past the card padding. */
export function DetailDivider({ margin }: { margin?: string }) {
  return (
    <div
      aria-hidden="true"
      style={{
        height: 1,
        margin,
        background: 'color-mix(in srgb, var(--aurora-border-default) 50%, var(--aurora-page-bg))',
      }}
    />
  )
}

const GHOST_BUTTON_CLASS =
  'grid place-items-center shrink-0 cursor-pointer border-0 bg-transparent text-aurora-text-muted ' +
  'hover:bg-[var(--gw0-0_40)] hover:text-aurora-text-primary ' +
  'disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-transparent ' +
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--aurora-accent-primary)]/40'

const ACCENT_BUTTON_CLASS =
  'grid place-items-center shrink-0 cursor-pointer ' +
  'border border-[color-mix(in_srgb,var(--aurora-accent-primary)_55%,var(--aurora-border-strong))] ' +
  'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_9%,var(--aurora-panel-strong))] text-[rgb(191,231,251)] ' +
  'hover:bg-[color-mix(in_srgb,var(--aurora-accent-primary)_16%,var(--aurora-panel-strong))] ' +
  'disabled:cursor-not-allowed disabled:opacity-45 ' +
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--aurora-accent-primary)]/40'

/**
 * Action-cluster icon button. The mock uses 26px borderless ghosts for the
 * secondary lane and one 28px accent-bordered button for the primary action.
 */
export function DetailIconButton({
  tone = 'ghost',
  style,
  className,
  type = 'button',
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { tone?: 'ghost' | 'accent' }) {
  const size = tone === 'accent' ? 28 : 26
  return (
    <button
      type={type}
      className={[tone === 'accent' ? ACCENT_BUTTON_CLASS : GHOST_BUTTON_CLASS, className]
        .filter(Boolean)
        .join(' ')}
      style={{ width: size, height: size, borderRadius: 8, ...style }}
      {...rest}
    />
  )
}

/**
 * The mock's warn-toned cluster pill (its "Auth" affordance on a server that
 * needs OAuth).
 */
export function DetailWarnPill({
  className,
  style,
  type = 'button',
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      type={type}
      className={[
        'inline-flex shrink-0 items-center cursor-pointer',
        'border border-[color-mix(in_srgb,var(--aurora-warn)_34%,transparent)]',
        'bg-[color-mix(in_srgb,var(--aurora-warn)_12%,transparent)] text-aurora-warn',
        'hover:bg-[color-mix(in_srgb,var(--aurora-warn)_18%,transparent)]',
        'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--aurora-warn)]/40',
        className,
      ]
        .filter(Boolean)
        .join(' ')}
      style={{
        gap: 4,
        height: 26,
        padding: '0 9px',
        borderRadius: 8,
        fontFamily: 'inherit',
        fontSize: 10.5,
        fontWeight: 650,
        whiteSpace: 'nowrap',
        ...style,
      }}
      {...rest}
    />
  )
}

/** Cluster metric-pill chrome, exported for call sites that need their own element. */
export const DETAIL_METRIC_PILL_STYLE: React.CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 6,
  height: 24,
  padding: '0 9px',
  borderRadius: 7,
  border:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 50%, var(--aurora-page-bg))',
  background: 'var(--gw0-0_40)',
  fontFamily: 'inherit',
  flexShrink: 0,
}

/**
 * The mock's read-only cluster metric pill (transport, p50, p95).
 *
 * Renders as a `<button>` when `onClick` is supplied so it stays keyboard
 * reachable; otherwise a plain `<div>`, exactly as the mock does.
 */
export function DetailMetricPill({
  icon,
  children,
  title,
  onClick,
  ariaLabel,
}: {
  icon?: React.ReactNode
  children: React.ReactNode
  title?: string
  onClick?: () => void
  ariaLabel?: string
}) {
  const body = (
    <>
      {icon ? (
        <span
          aria-hidden="true"
          style={{
            display: 'grid',
            placeItems: 'center',
            color: 'color-mix(in srgb, var(--aurora-text-muted) 80%, transparent)',
          }}
        >
          {icon}
        </span>
      ) : null}
      <span
        style={{
          fontSize: 11.5,
          fontWeight: 650,
          fontVariantNumeric: 'tabular-nums',
          color: 'var(--aurora-text-primary)',
          whiteSpace: 'nowrap',
        }}
      >
        {children}
      </span>
    </>
  )

  if (onClick) {
    return (
      <button
        type="button"
        onClick={onClick}
        title={title}
        aria-label={ariaLabel}
        className="cursor-pointer hover:border-[color-mix(in_srgb,var(--aurora-accent-primary)_35%,var(--aurora-border-strong))] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--aurora-accent-primary)]/40"
        style={DETAIL_METRIC_PILL_STYLE}
      >
        {body}
      </button>
    )
  }

  return (
    <div title={title} aria-label={ariaLabel} style={DETAIL_METRIC_PILL_STYLE}>
      {body}
    </div>
  )
}

/** Em-dash used wherever the mock shows a metric the gateway API does not expose. */
export const DETAIL_NO_DATA = '—'

// ===========================================================================
// Detail PAGE chrome (surface 2 — see the file header)
// ===========================================================================

/**
 * Sticky header card. The mock uses `radius-3` here (the table card is
 * `radius-2`) and bleeds its inner rows to the card edge with `-18px`
 * margins, so the padding is asymmetric: `16px 18px 0`.
 */
export const DETAIL_HEADER_CARD_STYLE: React.CSSProperties = {
  position: 'sticky',
  top: -186,
  zIndex: 30,
  padding: '16px 18px 0',
  overflow: 'hidden',
  borderRadius: 'var(--radius-3)',
  border:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
  background:
    'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
  boxShadow: 'var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,0.05)',
}

/** Detail `h1` — Manrope 25px/1.1/800, -0.01em. */
export const DETAIL_TITLE_STYLE: React.CSSProperties = {
  margin: 0,
  fontFamily: 'var(--font-display)',
  fontSize: 25,
  lineHeight: 1.1,
  fontWeight: 800,
  letterSpacing: '-0.01em',
  color: 'var(--aurora-text-primary)',
  wordBreak: 'break-word',
}

/** Title-row meta rail (version, protocol, links) sitting right of the `h1`. */
export const DETAIL_TITLE_META_STYLE: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  justifyContent: 'flex-end',
  gap: 10,
  flexWrap: 'wrap',
  fontSize: 11,
  lineHeight: 1,
  color: 'var(--aurora-text-muted)',
}

/** 3px separator dot between meta-rail items. */
export function DetailMetaDot() {
  return (
    <span
      aria-hidden="true"
      style={{
        width: 3,
        height: 3,
        borderRadius: 999,
        background: 'color-mix(in srgb, var(--aurora-text-muted) 45%, transparent)',
      }}
    />
  )
}

/**
 * Header stat strip — `2fr` identity cell then four metric cells, bled to the
 * card edge with `-18px` side margins and separated by left borders.
 */
export const DETAIL_STAT_STRIP_STYLE: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: '2fr repeat(4, minmax(120px, 1fr))',
  margin: '10px -18px 0',
  borderTop:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
  background: 'var(--gw0-0_30)',
}

/** A metric cell in the header stat strip. */
export function DetailStripCell({
  label,
  value,
  sub,
  title,
  first,
}: {
  label: string
  value: React.ReactNode
  sub?: React.ReactNode
  title?: string
  /** The identity cell carries no left border and a wider left inset. */
  first?: boolean
}) {
  return (
    <div
      title={title}
      style={{
        minWidth: 0,
        padding: first ? '12px 16px 13px 18px' : '12px 16px 13px',
        borderLeft: first
          ? undefined
          : '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
      }}
    >
      <div
        style={{
          fontSize: 10,
          fontWeight: 700,
          letterSpacing: '0.13em',
          textTransform: 'uppercase',
          color: 'var(--aurora-text-muted)',
          whiteSpace: 'nowrap',
        }}
      >
        {label}
      </div>
      <div
        className="font-display"
        style={{
          marginTop: 6,
          fontFamily: 'var(--font-display)',
          fontSize: 17,
          lineHeight: 1,
          fontWeight: 800,
          fontVariantNumeric: 'tabular-nums',
          color: 'var(--aurora-text-primary)',
        }}
      >
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

// ---------------------------------------------------------------------------
// Underline tab bar
// ---------------------------------------------------------------------------

/**
 * Row that hosts the tab scroller. Bled to the card edge and topped with a
 * hairline, exactly as the mock does.
 */
export const DETAIL_TAB_ROW_STYLE: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 10,
  minWidth: 0,
  margin: '0 -18px',
  padding: '6px 18px 0',
  borderTop:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
}

/**
 * Overrides for `components/ui/tabs.tsx` `TabsList`. The shared primitive is a
 * filled pill rail (`bg-aurora-control-surface rounded-lg p-[3px] h-9`); the
 * mock's detail tab bar is a bare underline scroller, so the background,
 * radius, padding, height and centring are all unset here. Editing the shared
 * primitive would change every other screen, so the override lives at this
 * call site.
 */
export const DETAIL_TAB_LIST_CLASS =
  'h-auto w-full max-w-full justify-start gap-[2px] overflow-x-auto rounded-none ' +
  'bg-transparent p-0 md:w-full md:justify-start'

/**
 * Overrides for `TabsTrigger` — 34px tall, 13px side padding, 12.5px/650,
 * 2px bottom indicator. Inactive is `--aurora-text-muted`; active is
 * `--aurora-accent-strong` over an `--aurora-accent-primary` indicator, with
 * the primitive's default active glow removed (the mock has none).
 */
export const DETAIL_TAB_TRIGGER_CLASS =
  'h-[34px] shrink-0 gap-1.5 rounded-none border-0 border-b-2 border-transparent px-[13px] py-0 ' +
  'text-[12.5px] font-[650] text-aurora-text-muted transition-[color,border-color] duration-150 md:flex-none ' +
  'data-[state=active]:border-b-2 data-[state=active]:border-aurora-accent-primary ' +
  'data-[state=active]:text-aurora-accent-strong data-[state=active]:shadow-none'

/** The 17px count chip the mock puts inside a tab label. */
export function DetailTabBadge({ children }: { children: React.ReactNode }) {
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        height: 17,
        padding: '0 5px',
        borderRadius: 5,
        fontSize: 10,
        fontWeight: 650,
        fontVariantNumeric: 'tabular-nums',
        border: '1px solid color-mix(in srgb, var(--aurora-border-strong) 80%, transparent)',
        background: 'var(--gw0-0_48)',
        color: 'var(--aurora-text-muted)',
      }}
    >
      {children}
    </span>
  )
}

// ---------------------------------------------------------------------------
// Spec card (the Overview tab's vocabulary)
// ---------------------------------------------------------------------------

/**
 * Overview card. The mock emphasises the first card with the panel-strong
 * gradient and renders the rest on panel-medium; `emphasis` selects which.
 */
export function DetailSpecCard({
  label,
  action,
  emphasis,
  children,
}: {
  label: string
  action?: React.ReactNode
  emphasis?: boolean
  children: React.ReactNode
}) {
  return (
    <div
      style={{
        borderRadius: 'var(--radius-2)',
        border:
          '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
        background: emphasis
          ? 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))'
          : 'linear-gradient(180deg, var(--aurora-panel-medium-top), transparent), var(--aurora-panel-medium)',
        boxShadow: 'var(--aurora-shadow-medium), inset 0 1px 0 rgba(255,255,255,0.04)',
        overflow: 'hidden',
        minWidth: 0,
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: 10,
          padding: '11px 16px',
          borderBottom:
            '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
          background: 'var(--gw0-0_38)',
          fontSize: 10.5,
          fontWeight: 700,
          letterSpacing: '0.15em',
          textTransform: 'uppercase',
          color: 'var(--aurora-text-muted)',
        }}
      >
        <span>{label}</span>
        {action}
      </div>
      <div style={{ padding: '12px 16px', display: 'flex', flexDirection: 'column', gap: 8 }}>
        {children}
      </div>
    </div>
  )
}

/** Key/value row inside a `DetailSpecCard`. */
export function DetailSpecRow({
  label,
  value,
  tone = 'default',
  title,
}: {
  label: React.ReactNode
  /** Pass `DETAIL_NO_DATA` for anything the gateway API cannot back. */
  value: React.ReactNode
  tone?: 'default' | 'muted' | 'accent' | 'faint'
  title?: string
}) {
  const color =
    tone === 'accent'
      ? 'var(--aurora-accent-strong)'
      : tone === 'muted'
        ? 'var(--aurora-text-muted)'
        : tone === 'faint'
          ? 'color-mix(in srgb, var(--aurora-text-muted) 70%, transparent)'
          : 'var(--aurora-text-primary)'
  return (
    <div
      title={title}
      style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}
    >
      <span style={{ fontSize: 12.5, color: 'var(--aurora-text-muted)', flexShrink: 0 }}>
        {label}
      </span>
      <span
        style={{
          fontSize: 12.5,
          fontWeight: 650,
          fontVariantNumeric: 'tabular-nums',
          color,
          minWidth: 0,
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          whiteSpace: 'nowrap',
        }}
      >
        {value}
      </span>
    </div>
  )
}

/** Overview's card grid — `repeat(auto-fit, minmax(300px, 1fr))`, gap 12. */
export const DETAIL_SPEC_GRID_STYLE: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'repeat(auto-fit, minmax(300px, 1fr))',
  gap: 12,
}

// ---------------------------------------------------------------------------
// Topbar toolbar button (detail page)
// ---------------------------------------------------------------------------

const TOOLBAR_BUTTON_BASE =
  'grid place-items-center shrink-0 cursor-pointer ' +
  'disabled:cursor-not-allowed disabled:opacity-45 ' +
  'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--aurora-accent-primary)]/40'

/**
 * The detail page's topbar action button: 32px, `radius-1`, control-surface
 * fill with a 70%-blended border. This is deliberately different from
 * `DetailIconButton` — that one is the row-expansion cluster's 26px ghost.
 */
export function DetailToolbarButton({
  tone = 'default',
  className,
  style,
  type = 'button',
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { tone?: 'default' | 'pink' }) {
  const toneStyle: React.CSSProperties =
    tone === 'pink'
      ? {
          border: '1px solid color-mix(in srgb, var(--aurora-accent-pink-deep) 55%, transparent)',
          background: 'color-mix(in srgb, var(--aurora-accent-pink) 8%, var(--aurora-control-surface))',
          color: 'var(--aurora-accent-pink)',
        }
      : {
          border:
            '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
          background: 'var(--aurora-control-surface)',
          color: 'var(--aurora-text-muted)',
        }
  return (
    <button
      type={type}
      className={[TOOLBAR_BUTTON_BASE, className].filter(Boolean).join(' ')}
      style={{
        width: 32,
        height: 32,
        borderRadius: 'var(--radius-1)',
        ...toneStyle,
        ...style,
      }}
      {...rest}
    />
  )
}
