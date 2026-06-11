import { Globe, Layers, SquareTerminal } from 'lucide-react'
import { cn } from '@/lib/utils'
import type { TransportType } from '@/lib/types/gateway'

interface TransportBadgeProps {
  transport: TransportType
  className?: string
  iconOnly?: boolean
}

export function TransportBadge({ transport, className, iconOnly = false }: TransportBadgeProps) {
  const config = (() => {
    switch (transport) {
      case 'http':
        return {
          label: 'HTTP',
          className:
            'border-aurora-accent-primary/28 bg-[linear-gradient(180deg,rgba(16,35,48,0.96),rgba(11,25,35,0.98))] text-aurora-accent-strong shadow-[var(--aurora-active-glow)]',
          icon: Globe,
        }
      case 'stdio':
        return {
          label: 'stdio',
          className:
            'border-aurora-border-strong bg-[linear-gradient(180deg,rgba(17,32,44,0.98),rgba(11,22,30,0.98))] text-aurora-text-muted shadow-[0_8px_16px_rgba(0,0,0,0.14),var(--aurora-highlight-medium)]',
          icon: SquareTerminal,
        }
      case 'in_process':
        return {
          label: 'Lab',
          className:
            'border-aurora-border-strong bg-[linear-gradient(180deg,rgba(18,40,56,0.96),rgba(14,31,44,0.98))] text-aurora-text-muted shadow-[0_8px_16px_rgba(0,0,0,0.14),var(--aurora-highlight-medium)]',
          icon: Layers,
        }
      default: {
        const exhaustive: never = transport
        return exhaustive
      }
    }
  })()

  const Icon = config.icon
  
  return (
    <span
      className={cn(
        iconOnly
          ? 'inline-flex h-8 w-8 items-center justify-center rounded-aurora-1 border p-0'
          : 'inline-flex h-8 min-w-[74px] items-center justify-center gap-1.5 rounded-aurora-1 border px-2.5 text-[11px] font-semibold uppercase tracking-[0.12em]',
        config.className,
        className
      )}
      title={config.label}
      aria-label={config.label}
    >
      <Icon className={iconOnly ? 'size-3.5' : 'size-3'} />
      {!iconOnly ? config.label : null}
    </span>
  )
}
