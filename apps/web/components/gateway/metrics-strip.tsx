import { Wrench, Eye, Filter } from 'lucide-react'
import { cn } from '@/lib/utils'
import { buildGatewayMetricItems } from '@/lib/api/gateway-metrics'
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip'

interface MetricsStripProps {
  discoveredCount: number
  exposedCount: number
  showFiltered?: boolean
  layout?: 'inline' | 'stacked'
  className?: string
}

export function MetricsStrip({ 
  discoveredCount, 
  exposedCount, 
  showFiltered = true,
  layout = 'inline',
  className 
}: MetricsStripProps) {
  const items = buildGatewayMetricItems(discoveredCount, exposedCount, showFiltered)

  if (layout === 'stacked') {
    return (
      <div className={cn('space-y-1 text-xs', className)}>
        {items.map((item) => (
          <div
            key={item.key}
            className={cn(
              'flex items-center justify-between gap-3',
              item.tone === 'success'
                ? 'text-aurora-success'
                : 'text-aurora-text-muted',
            )}
          >
            <span className="uppercase tracking-[0.18em]">{item.label}</span>
            <span className="tabular-nums font-medium">{item.value}</span>
          </div>
        ))}
      </div>
    )
  }

  return (
    <TooltipProvider>
      <div className={cn('flex items-center gap-3 text-sm', className)}>
        {items.map((item) => {
          const Icon = item.key === 'discovered' ? Wrench : item.key === 'exposed' ? Eye : Filter
          const tooltip =
            item.key === 'discovered'
              ? 'Discovered MCP tools'
              : item.key === 'exposed'
                ? 'Exposed downstream'
                : 'Filtered by allowlist'

          return (
          <Tooltip>
            <TooltipTrigger asChild>
              <div
                className={cn(
                  'flex items-center gap-1.5',
                  item.tone === 'success'
                    ? 'text-aurora-success'
                    : 'text-aurora-text-muted',
                )}
              >
                <Icon className="size-3.5" />
                <span className="tabular-nums">{item.value}</span>
              </div>
            </TooltipTrigger>
            <TooltipContent>{tooltip}</TooltipContent>
          </Tooltip>
          )
        })}
      </div>
    </TooltipProvider>
  )
}
