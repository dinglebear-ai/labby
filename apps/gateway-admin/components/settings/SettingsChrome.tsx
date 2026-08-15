'use client'

import type { CSSProperties, ReactNode } from 'react'

/**
 * Settings chrome, measured off the rendered Gateway Console mock's Settings
 * screen (`section[data-screen-label="Settings"]`).
 *
 * The mock's settings body is a plain flex column — no hero card. It opens
 * with a 24px display title plus a 12.5px muted lede, then stacks
 * `--radius-2` cards. Each card is an uppercase header bar over a body of
 * label/description rows separated by hairlines, with the control parked on
 * the row's right edge.
 *
 * Every literal in this file was read off the mock's live DOM, not inferred
 * from a screenshot. Re-measure before changing one.
 */

/** The mock caps its settings column at 760px. */
export const SETTINGS_MEASURE = 760

const CARD_STYLE: CSSProperties = {
  borderRadius: 'var(--radius-2)',
  border:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
  background:
    'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
  boxShadow: 'var(--aurora-shadow-medium), inset 0 1px 0 rgba(255,255,255,0.04)',
  overflow: 'hidden',
}

const CARD_HEADER_STYLE: CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 8,
  padding: '11px 16px',
  borderBottom:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
  background: 'var(--gw0-0_38)',
  fontSize: 10.5,
  fontWeight: 700,
  letterSpacing: '0.15em',
  textTransform: 'uppercase',
  color: 'var(--aurora-text-muted)',
}

const ROW_STYLE: CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 14,
  padding: '11px 16px',
  borderTop:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 35%, var(--aurora-page-bg))',
}

/** Row label: 13px/600 on primary text. */
export const SETTINGS_LABEL_STYLE: CSSProperties = {
  fontSize: 13,
  fontWeight: 600,
  color: 'var(--aurora-text-primary)',
}

/** Row description: 11.5px/1.5 muted, 2px below the label. */
export const SETTINGS_DESCRIPTION_STYLE: CSSProperties = {
  margin: '2px 0 0',
  fontSize: 11.5,
  lineHeight: 1.5,
  color: 'var(--aurora-text-muted)',
}

/**
 * Control chrome shared by the mock's segmented buttons and, by extension,
 * every input/select we park on a row's right edge.
 */
export const SETTINGS_CONTROL_STYLE: CSSProperties = {
  height: 28,
  minHeight: 28,
  padding: '0 10px',
  borderRadius: 8,
  border:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
  background: 'var(--aurora-control-surface)',
  fontFamily: 'inherit',
  fontSize: 11.5,
  color: 'var(--aurora-text-primary)',
}

/** Multi-line controls keep the chrome but drop the fixed height. */
export const SETTINGS_MULTILINE_CONTROL_STYLE: CSSProperties = {
  ...SETTINGS_CONTROL_STYLE,
  height: undefined,
  minHeight: 72,
  padding: '8px 10px',
}

/**
 * Read-only scalar values render as an 11px muted `code`. The mock uses a
 * `code` element but leaves the family inherited, so this does too.
 */
export const SETTINGS_VALUE_STYLE: CSSProperties = {
  flexShrink: 0,
  fontFamily: 'inherit',
  fontSize: 11,
  color: 'var(--aurora-text-muted)',
}

export function SettingsPageHeader({
  title,
  description,
}: {
  title: string
  description?: ReactNode
}) {
  return (
    <div>
      <h1
        style={{
          margin: 0,
          fontFamily: 'var(--font-display)',
          fontSize: 24,
          fontWeight: 800,
          color: 'var(--aurora-text-primary)',
        }}
      >
        {title}
      </h1>
      {description ? (
        <p style={{ margin: '5px 0 0', fontSize: 12.5, color: 'var(--aurora-text-muted)' }}>
          {description}
        </p>
      ) : null}
    </div>
  )
}

/**
 * One settings card: uppercase header bar plus a hairline-separated body.
 *
 * `action` is our addition — the mock's settings cards carry no header
 * affordances, but Extract and the per-service editor need one, and the
 * dashboard panels already establish the pattern.
 */
export function SettingsCard({
  title,
  action,
  description,
  children,
  bodyStyle,
}: {
  title: ReactNode
  action?: ReactNode
  description?: ReactNode
  children: ReactNode
  bodyStyle?: CSSProperties
}) {
  return (
    <section data-hovercard="1" style={CARD_STYLE}>
      <div style={CARD_HEADER_STYLE}>
        <div style={{ minWidth: 0 }}>{title}</div>
        {action ? (
          <>
            <div style={{ flex: 1 }} />
            <div
              style={{
                flexShrink: 0,
                display: 'flex',
                alignItems: 'center',
                gap: 6,
                // The header bar is uppercase/tracked label type; controls
                // parked in it must not inherit that.
                textTransform: 'none',
                letterSpacing: 'normal',
                fontWeight: 400,
                fontSize: 11.5,
                color: 'var(--aurora-text-primary)',
              }}
            >
              {action}
            </div>
          </>
        ) : null}
      </div>
      <div style={{ padding: '4px 0', ...bodyStyle }}>
        {description ? (
          <p
            style={{
              margin: 0,
              padding: '11px 16px 3px',
              fontSize: 11.5,
              lineHeight: 1.5,
              color: 'var(--aurora-text-muted)',
            }}
          >
            {description}
          </p>
        ) : null}
        {children}
      </div>
    </section>
  )
}

/**
 * A card body row. `layout="inline"` is the mock's shape — label block on the
 * left, compact control on the right. `layout="stacked"` keeps the same
 * hairline and padding but drops the control onto its own full-width line,
 * which is what wide text and list editors need.
 */
export function SettingsRow({
  label,
  description,
  meta,
  control,
  layout = 'inline',
  htmlFor,
  children,
}: {
  label?: ReactNode
  description?: ReactNode
  meta?: ReactNode
  control?: ReactNode
  layout?: 'inline' | 'stacked'
  htmlFor?: string
  children?: ReactNode
}) {
  const labelBlock = (
    <div style={{ flex: '1 1 0%', minWidth: 0 }}>
      {label ? (
        htmlFor ? (
          <label htmlFor={htmlFor} style={{ ...SETTINGS_LABEL_STYLE, display: 'block' }}>
            {label}
          </label>
        ) : (
          <div style={SETTINGS_LABEL_STYLE}>{label}</div>
        )
      ) : null}
      {description ? <div style={SETTINGS_DESCRIPTION_STYLE}>{description}</div> : null}
      {meta ? <div style={{ marginTop: 6 }}>{meta}</div> : null}
    </div>
  )

  if (layout === 'stacked') {
    return (
      <div style={{ ...ROW_STYLE, display: 'block' }}>
        {labelBlock}
        {control ? <div style={{ marginTop: 8 }}>{control}</div> : null}
        {children}
      </div>
    )
  }

  return (
    <div style={ROW_STYLE}>
      {labelBlock}
      {control ? <div style={{ flexShrink: 0 }}>{control}</div> : null}
      {children}
    </div>
  )
}

/** A hairline-separated strip for card-level actions or notices. */
export function SettingsRowStrip({
  children,
  style,
}: {
  children: ReactNode
  style?: CSSProperties
}) {
  return <div style={{ ...ROW_STYLE, ...style }}>{children}</div>
}

/** The mock's right-aligned read-only value. */
export function SettingsValue({ children }: { children: ReactNode }) {
  return <code style={SETTINGS_VALUE_STYLE}>{children}</code>
}

/**
 * The mock's toggle: a 34x19 pill with a 15px knob that slides 2px → 17px.
 * The knob colour is the page background, which is what the mock hardcodes.
 */
export function SettingsToggle({
  checked,
  onChange,
  disabled,
  id,
  label,
  describedBy,
  invalid,
}: {
  checked: boolean
  onChange: (checked: boolean) => void
  disabled?: boolean
  id?: string
  label: string
  describedBy?: string
  invalid?: boolean
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      aria-describedby={describedBy}
      aria-invalid={invalid || undefined}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      style={{
        flexShrink: 0,
        width: 34,
        height: 19,
        borderRadius: 999,
        border: 'none',
        position: 'relative',
        cursor: disabled ? 'not-allowed' : 'pointer',
        opacity: disabled ? 0.5 : 1,
        transition: 'background 160ms',
        background: checked
          ? 'var(--aurora-accent-primary)'
          : 'color-mix(in srgb, var(--aurora-border-strong) 80%, transparent)',
      }}
    >
      <span
        style={{
          position: 'absolute',
          top: 2,
          left: checked ? 17 : 2,
          width: 15,
          height: 15,
          borderRadius: 999,
          background: 'var(--aurora-page-bg)',
          transition: 'left 160ms',
          boxShadow: '0 1px 2px rgba(0,0,0,0.4)',
        }}
      />
    </button>
  )
}

/** Segmented-button chrome: 28px tall, 8px radius, 11.5px/650. */
export function settingsSegmentStyle(active: boolean): CSSProperties {
  return {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    height: 28,
    padding: '0 12px',
    borderRadius: 8,
    fontFamily: 'inherit',
    fontSize: 11.5,
    fontWeight: 650,
    cursor: 'pointer',
    whiteSpace: 'nowrap',
    textDecoration: 'none',
    border: active
      ? '1px solid color-mix(in srgb, var(--aurora-accent-primary) 45%, transparent)'
      : '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
    background: active
      ? 'color-mix(in srgb, var(--aurora-accent-primary) 14%, transparent)'
      : 'var(--aurora-control-surface)',
    color: active ? 'var(--aurora-accent-strong)' : 'var(--aurora-text-muted)',
  }
}

/** Small uppercase metadata pill — the mock's 9.5px/700/0.08em label scale. */
export function SettingsMetaPill({
  children,
  tone = 'default',
}: {
  children: ReactNode
  tone?: 'default' | 'warn'
}) {
  return (
    <span
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        padding: '1px 6px',
        borderRadius: 6,
        border:
          tone === 'warn'
            ? '1px solid color-mix(in srgb, var(--aurora-warn) 40%, transparent)'
            : '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
        background:
          tone === 'warn'
            ? 'color-mix(in srgb, var(--aurora-warn) 12%, transparent)'
            : 'var(--aurora-control-surface)',
        fontSize: 9.5,
        fontWeight: 700,
        letterSpacing: '0.08em',
        textTransform: 'uppercase',
        color: tone === 'warn' ? 'var(--aurora-warn)' : 'var(--aurora-text-muted)',
        whiteSpace: 'nowrap',
      }}
    >
      {children}
    </span>
  )
}
