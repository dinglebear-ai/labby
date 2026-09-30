import { cn } from '@/lib/utils'

interface StatusBadgeProps {
  healthy: boolean
  connected: boolean
  className?: string
}

export function StatusBadge({ healthy, connected, className }: StatusBadgeProps) {
  const status = healthy && connected ? 'healthy' : !connected ? 'disconnected' : 'unhealthy'
  
  const statusConfig = {
    healthy: {
      label: 'Healthy',
      className: 'border-success/30 bg-success/10 text-success shadow-sm shadow-success/10',
      dotClassName: 'bg-success animate-pulse shadow-sm shadow-success/40',
    },
    unhealthy: {
      label: 'Needs Attention',
      className: 'border-warning/30 bg-warning/10 text-warning shadow-sm shadow-warning/10',
      dotClassName: 'bg-warning animate-pulse',
    },
    disconnected: {
      label: 'Disconnected',
      className: 'border-destructive/30 bg-destructive/10 text-destructive shadow-sm shadow-destructive/10',
      dotClassName: 'bg-destructive',
    },
  }

  const config = statusConfig[status]

  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs font-medium',
        config.className,
        className
      )}
    >
      <span className={cn('size-1.5 rounded-full', config.dotClassName)} />
      {config.label}
    </span>
  )
}
