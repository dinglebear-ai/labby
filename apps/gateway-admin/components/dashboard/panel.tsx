import type { CSSProperties, ReactNode } from 'react'
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
  iconTone = 'accent',
  iconVariant = 'tile',
  meta,
  action,
  className,
  bodyClassName,
  headerStyle,
  titleStyle,
  metaStyle,
  bodyStyle,
  variant = 'card',
  children,
}: {
  title: string
  icon?: ReactNode
  iconTone?: 'accent' | 'pink' | 'warn' | 'error' | 'success'
  iconVariant?: 'tile' | 'plain'
  meta?: ReactNode
  action?: ReactNode
  className?: string
  bodyClassName?: string
  headerStyle?: CSSProperties
  titleStyle?: CSSProperties
  metaStyle?: CSSProperties
  bodyStyle?: CSSProperties
  variant?: 'card' | 'inline'
  children: ReactNode
}) {
  const tone = iconTone === 'accent' ? 'var(--aurora-accent-primary)' : iconTone === 'pink' ? 'var(--aurora-accent-pink)' : `var(--aurora-${iconTone})`
  return (
    <div
      data-hovercard={variant === 'card' ? '1' : undefined}
      data-panel-variant={variant}
      className={cn('min-w-0 overflow-hidden', className)}
      style={{
        borderRadius: variant === 'inline' ? 0 : 'var(--radius-2)',
        border:
          variant === 'inline' ? 'none' : '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))',
        background:
          'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))',
        boxShadow: variant === 'inline' ? 'none' : 'var(--aurora-shadow-medium), inset 0 1px 0 rgba(255,255,255,0.04)',
      }}
    >
      <div
        data-panel-header="1"
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          padding: variant === 'inline' ? '10px 16px' : '10px 14px',
          borderBottom:
            '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))',
          background: variant === 'inline' ? 'var(--gw0-0_30)' : 'var(--gw0-0_38)',
          ...headerStyle,
        }}
      >
        {icon ? (
          <span aria-hidden="true" data-panel-icon="1" data-panel-icon-variant={iconVariant} className={iconVariant === 'plain' ? 'grid shrink-0 place-items-center text-aurora-text-muted [&>svg]:size-[13px]' : 'grid size-[22px] shrink-0 place-items-center rounded-[7px] [&>svg]:size-3'} style={iconVariant === 'plain' ? undefined : { color: tone, border: `1px solid color-mix(in srgb, ${tone} 26%, transparent)`, background: `color-mix(in srgb, ${tone} 9%, transparent)` }}>
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
            ...titleStyle,
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
              ...metaStyle,
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
        style={{ display: 'flex', flexDirection: 'column', gap: variant === 'inline' ? 11 : 9, padding: variant === 'inline' ? '12px 16px 14px' : '12px 14px', ...bodyStyle }}
      >
        {children}
      </div>
    </div>
  )
}
