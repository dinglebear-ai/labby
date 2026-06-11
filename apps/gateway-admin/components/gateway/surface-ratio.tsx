import type { LucideIcon } from 'lucide-react'

import { cn } from '@/lib/utils'
import { AURORA_GATEWAY_SUBTLE_SURFACE } from './gateway-theme'

interface SurfaceRatioProps {
  icon: LucideIcon
  label: string
  exposed: number
  total: number
  className?: string
}

export function SurfaceRatio({ icon: Icon, label, exposed, total, className }: SurfaceRatioProps) {
  const denominator = Math.max(total, 0)
  const ratio = denominator > 0 ? Math.max(0, Math.min(100, (exposed / denominator) * 100)) : 0

  return (
    <div
      className={cn(
        AURORA_GATEWAY_SUBTLE_SURFACE,
        'relative inline-flex h-8 min-w-[74px] items-center justify-center gap-1.5 overflow-hidden px-2.5 text-[13px] font-semibold text-aurora-text-primary',
        className,
      )}
      title={`${label}: ${exposed}/${total}`}
      aria-label={`${label}: ${exposed} of ${total}`}
    >
      <span
        className="absolute inset-y-0 left-0 bg-aurora-accent-primary/16 transition-[width]"
        style={{ width: `${ratio}%` }}
        aria-hidden="true"
      />
      <span className="absolute inset-x-2 bottom-1 h-px rounded-full bg-aurora-border-strong/70" aria-hidden="true">
        <span
          className="block h-full rounded-full bg-aurora-accent-strong/80 transition-[width]"
          style={{ width: `${ratio}%` }}
        />
      </span>
      <Icon className="size-3.5 text-aurora-accent-strong" aria-hidden="true" />
      <span className="relative tabular-nums">{exposed}/{total}</span>
    </div>
  )
}
