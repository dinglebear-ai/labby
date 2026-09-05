import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'

/**
 * Shared panel shell for dashboard insight/analysis cards.
 *
 * Chrome measured off the rendered Gateway Console mock: a `--radius-2` card
 * on the panel-strong gradient, with a tinted header bar separated by a rule
 * rather than the inline heading this used to draw. Every dashboard panel goes
 * through here, so the mock's card treatment lands everywhere at once.
 */
export function DashboardPanel({
  title,
  icon,
  meta,
  action,
  className,
  bodyClassName,
  children,
}: {
  title: string
  icon?: ReactNode
  meta?: ReactNode
  action?: ReactNode
  className?: string
  bodyClassName?: string
  children: ReactNode
}) {
  return (
    <div
      data-hovercard="1"
      className={cn('min-w-0 overflow-hidden', className)}
      style={{
        borderRadius: 'var(--radius-2)',
        border:
          '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
        background:
          'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
        boxShadow: 'var(--aurora-shadow-medium), inset 0 1px 0 rgba(255,255,255,0.04)',
      }}
    >
      <div
        data-panel-header="1"
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          padding: '10px 14px',
          borderBottom:
            '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
          background: 'var(--gw0-0_38)',
        }}
      >
        {icon ? (
          <span className="grid shrink-0 place-items-center text-aurora-accent-primary">
            {icon}
          </span>
        ) : null}
        <span
          style={{
            fontSize: 10,
            fontWeight: 700,
            letterSpacing: '0.14em',
            textTransform: 'uppercase',
            color: 'var(--aurora-text-muted)',
          }}
        >
          {title}
        </span>
        <div style={{ flex: 1 }} />
        {meta ? (
          <span
            style={{
              fontSize: 10,
              color: 'color-mix(in srgb, var(--aurora-text-muted) 80%, transparent)',
              fontVariantNumeric: 'tabular-nums',
              flexShrink: 0,
            }}
          >
            {meta}
          </span>
        ) : null}
        {action ? <div data-panel-action="1">{action}</div> : null}
      </div>

      <div
        data-panel-body="1"
        className={bodyClassName}
        style={{ display: 'flex', flexDirection: 'column', gap: 9, padding: '12px 14px' }}
      >
        {children}
      </div>
    </div>
  )
}
