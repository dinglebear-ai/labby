'use client'

import { AlertTriangle } from 'lucide-react'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'
import type { GatewayWarning } from '@/lib/types/gateway'

interface WarningsPillProps {
  warnings: GatewayWarning[]
  className?: string
}

export function WarningsPill({ warnings, className }: WarningsPillProps) {
  if (warnings.length === 0) return null

  const leadWarning = warnings[0]

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={`${warnings.length} gateway warnings`}
          className={cn(
            'inline-flex h-6 min-w-8 items-center justify-center gap-1 rounded-full border border-aurora-warn/28 bg-[color-mix(in_srgb,var(--aurora-warn)_12%,transparent)] px-2 text-[10px] font-semibold uppercase tracking-[0.12em] text-aurora-warn shadow-[inset_0_1px_0_rgba(255,255,255,0.02)] transition-colors hover:bg-[color-mix(in_srgb,var(--aurora-warn)_18%,transparent)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-warn/35',
            className,
          )}
        >
          <AlertTriangle className="size-3" />
          {warnings.length}
        </button>
      </PopoverTrigger>
      <PopoverContent
        side="top"
        align="start"
        className="w-72 border-aurora-border-strong bg-aurora-panel-strong p-3 text-aurora-text-primary"
      >
        <div className="space-y-1.5">
          {leadWarning ? (
            <p className="text-xs font-semibold uppercase tracking-[0.16em] text-aurora-warn">
              {leadWarning.code}
            </p>
          ) : null}
          {warnings.slice(0, 3).map((warning, index) => (
            <p key={`${warning.code}:${index}`} className="text-xs leading-5 text-aurora-text-muted">
              {warning.message}
            </p>
          ))}
          {warnings.length > 3 ? (
            <p className="text-xs text-aurora-text-muted">+{warnings.length - 3} more warnings</p>
          ) : null}
        </div>
      </PopoverContent>
    </Popover>
  )
}
