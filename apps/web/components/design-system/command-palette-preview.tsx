import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import {
  AURORA_DISPLAY_2,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
} from '@/components/aurora/tokens'
import type { CommandPaletteItem } from './command-palette-data'

type CommandPalettePreviewProps = {
  item: CommandPaletteItem | null
  lastActionLabel: string
}

function previewCopy(item: CommandPaletteItem | null): { title: string; description: string } {
  if (!item) {
    return {
      title: 'No active result',
      description: 'Type a query or move through the list to preview the currently highlighted result.',
    }
  }

  if (item.type === 'destination') {
    return {
      title: 'Destination preview',
      description: `Selecting ${item.title} opens the corresponding product surface and closes the palette.`,
    }
  }

  if (item.type === 'action') {
    return {
      title: 'Action preview',
      description: `${item.title} simulates a direct operator action while keeping the user anchored in the current flow.`,
    }
  }

  return {
    title: 'Recent context preview',
    description: `${item.title} restores a recent working context so the user can resume without navigating manually.`,
  }
}

export function CommandPalettePreview({
  item,
  lastActionLabel,
}: CommandPalettePreviewProps) {
  const copy = previewCopy(item)

  return (
    <div className={cn(AURORA_MEDIUM_PANEL, 'grid gap-4 px-4 py-4')}>
      <div>
        <p className={AURORA_MUTED_LABEL}>Selection preview</p>
        <h3 className={cn(AURORA_DISPLAY_2, 'mt-2 text-aurora-text-primary')}>{copy.title}</h3>
        <p className="mt-2 text-sm leading-[1.55] text-aurora-text-muted">{copy.description}</p>
      </div>

      <div className="grid gap-3 rounded-aurora-2 border border-aurora-border-strong bg-aurora-control-surface px-4 py-4">
        <div className="flex items-center justify-between gap-3">
          <span className="text-[13px] font-semibold text-aurora-text-primary">Current result</span>
          {item ? (
            <Badge
              variant="pill"
              status="default"
              className="border-aurora-border-strong bg-aurora-panel-medium text-aurora-text-muted"
            >
              {item.type}
            </Badge>
          ) : null}
        </div>
        <div className="text-sm text-aurora-text-muted">
          {item ? item.title : 'No result highlighted'}
        </div>
        <div className="rounded-aurora-1 border border-aurora-border-strong bg-aurora-panel-medium px-3 py-3 text-sm text-aurora-text-muted">
          {lastActionLabel}
        </div>
      </div>
    </div>
  )
}
