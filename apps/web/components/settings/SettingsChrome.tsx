'use client'

import type { CSSProperties, ReactNode } from 'react'


const CARD_STYLE: CSSProperties = {
  borderRadius: 'var(--radius-2)',
  border:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
  background:
    'var(--aurora-panel-medium)',
  boxShadow: 'var(--aurora-shadow-medium), var(--aurora-highlight-medium)',
  overflow: 'hidden',
}

const CARD_HEADER_STYLE: CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 8,
  padding: '16px 20px',
  borderBottom:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
  background: 'var(--aurora-panel-medium)',
  fontSize: 16,
  lineHeight: 'normal',
  fontWeight: 700,
  fontFamily: 'var(--font-display)',
  textTransform: 'none',
  color: 'var(--aurora-text-muted)',
}

const ROW_STYLE: CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 14,
  padding: '16px 20px',
  borderTop:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 35%, var(--aurora-page-bg))',
}

export const SETTINGS_LABEL_STYLE: CSSProperties = {
  fontSize: 13,
  lineHeight: 'normal',
  fontWeight: 600,
  color: 'var(--aurora-text-primary)',
}

export const SETTINGS_DESCRIPTION_STYLE: CSSProperties = {
  margin: '6px 0 0',
  fontSize: 14,
  lineHeight: 1.55,
  color: 'var(--aurora-text-muted)',
}

export const SETTINGS_CONTROL_STYLE: CSSProperties = {
  height: 36,
  minHeight: 36,
  padding: '0 10px',
  borderRadius: 8,
  border:
    '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))',
  background: 'var(--aurora-control-surface)',
  fontFamily: 'inherit',
  fontSize: 13,
  color: 'var(--aurora-text-primary)',
}

export const SETTINGS_MULTILINE_CONTROL_STYLE: CSSProperties = {
  ...SETTINGS_CONTROL_STYLE,
  height: undefined,
  minHeight: 72,
  padding: '8px 10px',
}

export const SETTINGS_VALUE_STYLE: CSSProperties = {
  flexShrink: 0,
  fontFamily: 'inherit',
  fontSize: 11,
  lineHeight: 'normal',
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
          lineHeight: 'normal',
          fontWeight: 800,
          color: 'var(--aurora-text-primary)',
        }}
      >
        {title}
      </h1>
      {description ? (
        <p style={{ margin: '5px 0 0', fontSize: 14, lineHeight: 'normal', color: 'var(--aurora-text-muted)' }}>
          {description}
        </p>
      ) : null}
    </div>
  )
}

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
        <h2 style={{ minWidth: 0, margin: 0, color: 'var(--aurora-text-primary)' }}>{title}</h2>
        {action ? (
          <>
            <div style={{ flex: 1 }} />
            <div
              style={{
                flexShrink: 0,
                display: 'flex',
                alignItems: 'center',
                gap: 6,
                textTransform: 'none',
                letterSpacing: 'normal',
                fontWeight: 400,
                fontSize: 13,
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
              padding: '12px 20px 8px',
              fontSize: 13,
              lineHeight: 1.55,
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
    <div className="flex-col items-stretch sm:flex-row sm:items-center" style={{ ...ROW_STYLE, alignItems: undefined }}>
      {labelBlock}
      {control ? <div style={{ flexShrink: 0, display: 'flex', alignItems: 'center' }}>{control}</div> : null}
      {children}
    </div>
  )
}

export function SettingsRowStrip({
  children,
  style,
}: {
  children: ReactNode
  style?: CSSProperties
}) {
  return <div style={{ ...ROW_STYLE, ...style }}>{children}</div>
}

export function SettingsValue({ children }: { children: ReactNode }) {
  return <code style={SETTINGS_VALUE_STYLE}>{children}</code>
}

export function SettingsToggle({
  checked,
  onChange,
  disabled,
  readOnly = false,
  id,
  label,
  describedBy,
  invalid,
}: {
  checked: boolean
  onChange?: (checked: boolean) => void
  disabled?: boolean
  readOnly?: boolean
  id?: string
  label: string
  describedBy?: string
  invalid?: boolean
}) {
  return (
    <button
      id={id}
      type="button"
      className="focus-visible:outline-2 focus-visible:outline-offset-4 focus-visible:outline-aurora-accent-primary"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      aria-describedby={describedBy}
      aria-invalid={invalid || undefined}
      aria-readonly={readOnly || undefined}
      disabled={disabled}
      onClick={readOnly ? undefined : () => onChange?.(!checked)}
      style={{
        flexShrink: 0,
        width: 34,
        height: 19,
        borderRadius: 999,
        border: 'none',
        position: 'relative',
        cursor: disabled ? 'not-allowed' : readOnly ? 'default' : 'pointer',
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
          boxShadow: 'var(--aurora-shadow-medium)',
        }}
      />
    </button>
  )
}

export function settingsSegmentStyle(active: boolean): CSSProperties {
  return {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 6,
    height: 36,
    padding: '0 12px',
    borderRadius: 8,
    fontFamily: 'inherit',
    fontSize: 13,
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
        fontSize: 11,
        fontWeight: 500,
        letterSpacing: 'normal',
        textTransform: 'none',
        color: tone === 'warn' ? 'var(--aurora-warn)' : 'var(--aurora-text-muted)',
        whiteSpace: 'nowrap',
      }}
    >
      {children}
    </span>
  )
}
