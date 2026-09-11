'use client'

import { METRICS_WINDOWS, type MetricsWindow } from '@/lib/types/metrics'

const WINDOW_SHORT: Record<MetricsWindow, string> = {
  '1h': '1h',
  '24h': '24h',
  '7d': '7d',
}

/** Rolling-window pill toggle measured from the Activity mock. */
export function WindowSelector({
  value,
  onChange,
}: {
  value: MetricsWindow
  onChange: (window: MetricsWindow) => void
}) {
  return (
    <div
      role="tablist"
      aria-label="Activity window"
      style={{
        flexShrink: 0,
        display: 'inline-flex',
        gap: 3,
        padding: 3,
        borderRadius: 999,
        background: 'var(--gw0-0_40)',
        border: '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
      }}
    >
      {METRICS_WINDOWS.map((window) => {
        const active = window === value
        return (
          <button
            key={window}
            type="button"
            role="tab"
            aria-selected={active}
            onClick={() => onChange(window)}
            className="focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary/40"
            style={{
              height: 28,
              padding: '0 13px',
              borderRadius: 999,
              fontFamily: 'inherit',
              fontSize: 11.5,
              fontWeight: 650,
              cursor: 'pointer',
              whiteSpace: 'nowrap',
              border: active
                ? '1px solid color-mix(in srgb, var(--aurora-accent-primary) 45%, transparent)'
                : '1px solid transparent',
              background: active
                ? 'color-mix(in srgb, var(--aurora-accent-primary) 14%, transparent)'
                : 'transparent',
              color: active ? 'var(--aurora-accent-strong)' : 'var(--aurora-text-muted)',
            }}
          >
            {WINDOW_SHORT[window]}
          </button>
        )
      })}
    </div>
  )
}
