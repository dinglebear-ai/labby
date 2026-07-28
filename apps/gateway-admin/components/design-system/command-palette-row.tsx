'use client'
import {
  ArrowUpRight,
  Cable,
  Clock3,
  KeyRound,
  Logs,
  RefreshCcw,
  Settings2,
  type LucideIcon,
} from 'lucide-react'

import { Badge } from '@/components/ui/badge'
import { CommandItem, CommandShortcut } from '@/components/ui/command'
import { formatUiDateTime, formatUiRelativeTime } from '@/lib/format-ui-time'
import { cn } from '@/lib/utils'
import type { CommandPaletteItem } from './command-palette-data'

const ICONS: Record<CommandPaletteItem['icon'], LucideIcon> = {
  gateway: Cable,
  logs: Logs,
  settings: Settings2,
  reload: RefreshCcw,
  token: KeyRound,
  setup: ArrowUpRight,
  recent: Clock3,
}

const TYPE_LABELS: Record<CommandPaletteItem['type'], string> = {
  destination: 'Destination',
  action: 'Action',
  recent: 'Recent',
}

type CommandPaletteRowProps = {
  item: CommandPaletteItem
  active: boolean
  onActivate: (itemId: string) => void
  onSelect: (itemId: string) => void
}

export function CommandPaletteRow({
  item,
  active,
  onActivate,
  onSelect,
}: CommandPaletteRowProps) {
  const Icon = ICONS[item.icon]
  const recentLabel = item.recentTimestamp
    ? item.type === 'recent'
      ? formatUiRelativeTime(item.recentTimestamp)
      : formatUiDateTime(item.recentTimestamp)
    : null

  return (
    <CommandItem
      value={item.title}
      keywords={item.keywords}
      onMouseEnter={() => onActivate(item.id)}
      onSelect={() => onSelect(item.id)}
      className={cn(
        'rounded-aurora-2 border border-aurora-border-strong/80 bg-aurora-control-surface px-3 py-3 text-aurora-text-primary transition-[border-color,background-color,box-shadow] hover:bg-aurora-hover-bg',
        active
          ? 'border-aurora-accent-primary/40 bg-aurora-panel-medium shadow-[var(--aurora-active-glow)]'
          : '',
      )}
    >
      <div className="flex size-9 items-center justify-center rounded-aurora-1 border border-aurora-border-strong bg-aurora-panel-medium text-aurora-accent-strong">
        <Icon className="size-4" />
      </div>
      <div className="grid min-w-0 flex-1 gap-1">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-[13px] font-semibold leading-[1.2] text-aurora-text-primary">
            {item.title}
          </span>
          <Badge
            variant="pill"
            status="default"
            className="border-aurora-border-strong bg-aurora-panel-medium text-[11px] text-aurora-text-muted"
          >
            {TYPE_LABELS[item.type]}
          </Badge>
        </div>
        <span className="truncate text-[12px] leading-[1.45] text-aurora-text-muted">
          {item.description}
        </span>
      </div>
      <CommandShortcut className="text-[11px] tracking-[0.08em] text-aurora-text-muted">
        {recentLabel ?? item.actionHint}
      </CommandShortcut>
    </CommandItem>
  )
}
