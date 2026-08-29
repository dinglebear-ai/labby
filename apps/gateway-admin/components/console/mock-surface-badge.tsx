import { FlaskConical } from 'lucide-react'

import { cn } from '@/lib/utils'

export function MockSurfaceBadge({ className }: { className?: string }) {
  return (
    <span
      data-mock-surface="true"
      className={cn(
        'inline-flex h-[22px] shrink-0 items-center gap-1.5 whitespace-nowrap rounded-[7px] border border-aurora-warn/35 bg-[color-mix(in_srgb,var(--aurora-warn)_10%,transparent)] px-2 text-[10px] font-bold uppercase tracking-[0.12em] text-aurora-warn',
        className,
      )}
      title="This surface uses illustrative data and is not connected to a Labby runtime contract"
    >
      <FlaskConical className="size-3" aria-hidden="true" />
      Mock data
    </span>
  )
}
