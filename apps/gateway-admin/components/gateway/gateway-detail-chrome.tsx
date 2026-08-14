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
 *    `page: 'detail'`. This page has a sticky header card, an attached stat
 *    strip, and an underline tab bar. Its topbar cluster is 32px bordered
 *    control-surface buttons (Test / View in Logs / Reload / Generate skill /
 *    Edit / More), NOT the row expansion's 26px ghosts.
 *
 *    Re-verified live on 2026-08-14 by enumerating `button[aria-pressed]`
 *    inside the header card rather than testing a guessed list of names. The
 *    tab set is exactly: Overview · Variables · Catalog 7 · Activity 1K ·
 *    Routes 3 · Logs. Its chrome lives in `gateway-detail-tabs.tsx`.
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
 * The detail page's topbar action button. Unlike the row expansion's 26px
 * borderless ghosts, the mock's `isDetailPage` cluster is 32px squares with a
 * 70%-blended border on `--aurora-control-surface`, spaced 5px apart, and
 * hovering to primary text on `--aurora-hover-bg`.
 */
export function DetailTopbarButton({
  className,
  style,
  type = 'button',
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      type={type}
      className={[
        'grid place-items-center shrink-0 cursor-pointer',
        'border border-[color-mix(in_srgb,var(--aurora-border-default)_70%,var(--aurora-page-bg))]',
        'bg-aurora-control-surface text-aurora-text-muted',
        'hover:bg-[var(--aurora-hover-bg)] hover:text-aurora-text-primary',
        'disabled:cursor-not-allowed disabled:opacity-45 disabled:hover:bg-aurora-control-surface',
        'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--aurora-accent-primary)]/40',
        className,
      ]
        .filter(Boolean)
        .join(' ')}
      style={{ width: 32, height: 32, borderRadius: 'var(--radius-1)', ...style }}
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
