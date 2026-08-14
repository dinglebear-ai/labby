'use client'

/**
 * Presentational primitives for the command palette, matched 1:1 against the
 * Claude Design `Gateway Console` mock. Layout/typography live in
 * `palette-styles.tsx`; this module only supplies the markup shape.
 */

import type { ReactNode } from 'react'

import type { PaletteCount, PaletteTone } from '@/lib/app-command-palette'

/** Maps a palette tone to the Aurora colour variable the mock uses. */
export function paletteToneVar(tone: PaletteTone): string {
  switch (tone) {
    case 'success':
      return 'var(--aurora-success)'
    case 'warn':
      return 'var(--aurora-warn)'
    case 'error':
      return 'var(--aurora-error)'
    case 'muted':
      return 'var(--aurora-text-muted)'
  }
}

export function PaletteSectionHeader({
  label,
  children,
}: {
  label: string
  children?: ReactNode
}) {
  return (
    <div className="pal-section">
      <span>{label}</span>
      {children ? (
        <>
          <span className="pal-grow" />
          {children}
        </>
      ) : null}
    </div>
  )
}

export function PaletteSplit() {
  return <div className="pal-split" />
}

export function PaletteChip({
  tone = 'accent',
  children,
}: {
  tone?: 'accent' | 'muted' | 'bare'
  children: ReactNode
}) {
  return (
    <span className="pal-chip" data-tone={tone}>
      {children}
    </span>
  )
}

/** 7px status dot. `glow` reproduces the alert-row halo; `halo` the server-row ring. */
export function PaletteDot({
  tone,
  variant = 'glow',
}: {
  tone: PaletteTone
  variant?: 'glow' | 'halo'
}) {
  const color = paletteToneVar(tone)
  return (
    <span
      className="pal-dot"
      style={{
        background: color,
        boxShadow:
          variant === 'glow'
            ? `0 0 4px ${color}`
            : `0 0 0 3px color-mix(in srgb, ${color} 12%, transparent)`,
      }}
    />
  )
}

export function PaletteCountsStrip({
  counts,
  scopeLabel,
  onClearScope,
  hint,
}: {
  counts: PaletteCount[]
  scopeLabel: string | null
  onClearScope: () => void
  hint: string
}) {
  return (
    <div className="pal-counts">
      {scopeLabel ? (
        <button type="button" className="pal-scope" title="Clear scope" onClick={onClearScope}>
          {scopeLabel}
        </button>
      ) : null}
      {counts.map((count) => (
        <span key={count.key} className="pal-count">
          {count.key}
          <b>{count.value}</b>
        </span>
      ))}
      <span className="pal-grow" />
      <span className="pal-hint">{hint}</span>
    </div>
  )
}

export function PaletteFooter({ label, children }: { label: string; children?: ReactNode }) {
  return (
    <div className="pal-foot">
      <span>{label}</span>
      {children}
    </div>
  )
}
