'use client'

import { Grid2X2, List, Table2, type LucideIcon } from 'lucide-react'

import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'

export type CollectionViewMode = 'table' | 'list' | 'cards'

const VIEW_OPTIONS: Record<CollectionViewMode, { label: string; icon: LucideIcon }> = {
  table: { label: 'Table view', icon: Table2 },
  list: { label: 'List view', icon: List },
  cards: { label: 'Card view', icon: Grid2X2 },
}

export function CollectionViewToggle({
  value,
  onChange,
  modes = ['table', 'list', 'cards'],
  ariaLabel = 'Collection view',
  className,
}: {
  value: CollectionViewMode
  onChange: (value: CollectionViewMode) => void
  modes?: readonly CollectionViewMode[]
  ariaLabel?: string
  className?: string
}) {
  return (
    <div
      data-collection-view-toggle="1"
      className={cn(
        'inline-flex shrink-0 rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-0.5',
        className,
      )}
      role="group"
      aria-label={ariaLabel}
    >
      {modes.map((mode) => {
        const { label, icon: Icon } = VIEW_OPTIONS[mode]
        return (
          <Button
            key={mode}
            type="button"
            variant="ghost"
            size="icon-sm"
            aria-label={label}
            title={label}
            aria-pressed={value === mode}
            className={cn(
              'size-11 rounded-[7px] text-aurora-text-muted sm:size-8',
              value === mode && 'bg-aurora-selected-bg text-aurora-accent-strong',
            )}
            onClick={() => onChange(mode)}
          >
            <Icon aria-hidden="true" className="size-3.5" />
          </Button>
        )
      })}
    </div>
  )
}
