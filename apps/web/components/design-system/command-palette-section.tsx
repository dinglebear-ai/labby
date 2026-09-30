import { cn } from '@/lib/utils'
import {
  AURORA_DISPLAY_2,
  AURORA_MUTED_LABEL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import { CommandPaletteDemo } from './command-palette-demo'

export function CommandPaletteSection() {
  return (
    <section className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
      <div className="border-b border-aurora-border-strong px-5 py-4">
        <p className={AURORA_MUTED_LABEL}>Command Palette</p>
        <h2 className={cn(AURORA_DISPLAY_2, 'mt-2 text-aurora-text-primary')}>
          Hybrid spotlight navigation and actions
        </h2>
        <p className="mt-2 max-w-2xl text-sm text-aurora-text-muted">
          Canonical reference for the global `cmd+k` interaction: one ranked stack that mixes
          destinations, actions, and recent context without leaving the Aurora control-plane language.
        </p>
      </div>

      <div className="px-5 py-5">
        <CommandPaletteDemo />
      </div>
    </section>
  )
}
