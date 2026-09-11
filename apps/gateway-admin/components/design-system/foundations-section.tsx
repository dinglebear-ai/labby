import { cn } from '@/lib/utils'
import { discoverKindPresentation } from '@/components/depot/discover-kind-presentation'
import {
  AURORA_DISPLAY_2,
  AURORA_DISPLAY_NUMBER,
  AURORA_MEDIUM_PANEL,
  AURORA_MUTED_LABEL,
  AURORA_STRONG_PANEL,
} from '@/components/aurora/tokens'
import {
  auroraColorTokens,
  elevationTiers,
  radiusScale,
  spacingScale,
  typeRamp,
} from './demo-data'

export function FoundationsSection() {
  return (
    <section className={cn(AURORA_STRONG_PANEL, 'overflow-hidden')}>
      <div className="border-b border-aurora-border-strong px-5 py-4">
        <p className={AURORA_MUTED_LABEL}>Theme Foundations</p>
        <h2 className={cn(AURORA_DISPLAY_2, 'mt-2 text-aurora-text-primary')}>
          Aurora page language
        </h2>
        <p className="mt-2 max-w-2xl text-sm text-aurora-text-muted">
          Reference the palette, type ramp, spacing cadence, radius decisions, and elevation tiers
          that distinguish the current admin shell.
        </p>
      </div>

      <div className="space-y-6 px-5 py-5">
        <section aria-label="Artifact identity palette" className="space-y-3">
          <p className={AURORA_MUTED_LABEL}>Artifact identity palette</p>
          <p className="text-sm text-aurora-text-muted">Protocol orange identifies MCP-family kinds. Warning gold remains reserved for guards and operational warnings.</p>
          <div className="flex flex-wrap gap-3">
            {['MCP', 'ACP', 'Skill', 'Agent', 'Prompt', 'Command', 'Hook', 'Plugin', 'Extension', 'Loadout', 'Snippet'].map(kind => {
              const { icon: Icon, iconStyle, color } = discoverKindPresentation(kind)
              return <div key={kind} className="flex items-center gap-2 rounded-aurora-1 border border-aurora-border-default bg-aurora-panel-strong p-3">
                <span style={iconStyle} className="grid size-8 place-items-center rounded-[9px] border"><Icon aria-hidden="true" className="size-4" /></span>
                <span style={{ color }} className="text-xs font-semibold">{kind}</span>
              </div>
            })}
          </div>
        </section>
        <section className="space-y-3">
          <p className={AURORA_MUTED_LABEL}>Color Tokens</p>
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            {auroraColorTokens.map((token) => (
              <article key={token.label} className={cn(AURORA_MEDIUM_PANEL, 'overflow-hidden')}>
                <div className={cn('h-20 border-b border-aurora-border-strong', token.className)} />
                <div className="space-y-1 px-4 py-4">
                  <p className="text-sm font-medium text-aurora-text-primary">{token.label}</p>
                  <p className={cn('font-mono text-xs', token.textClassName)}>{token.value}</p>
                </div>
              </article>
            ))}
          </div>
        </section>

        <section className="space-y-3">
          <p className={AURORA_MUTED_LABEL}>Type Ramp</p>
          <div className="grid gap-3">
            {typeRamp.map((sample) => (
              <div key={sample.label} className={cn(AURORA_MEDIUM_PANEL, 'grid gap-3 px-4 py-4 md:grid-cols-[180px_minmax(0,1fr)]')}>
                <p className="text-sm font-medium text-aurora-text-muted">{sample.label}</p>
                <p className={sample.className}>{sample.preview}</p>
              </div>
            ))}
          </div>
        </section>

        <div className="grid gap-4 xl:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)]">
          <section className="space-y-3">
            <p className={AURORA_MUTED_LABEL}>Spacing And Radius</p>
            <div className={cn(AURORA_MEDIUM_PANEL, 'space-y-5 px-4 py-4')}>
              <div className="space-y-3">
                {spacingScale.map((space) => (
                  <div key={space.label} className="flex items-center gap-3">
                    <span className="w-20 text-sm text-aurora-text-muted">{space.label}</span>
                    <div className="h-3 rounded-full bg-aurora-accent-primary/80" style={{ width: space.value, maxWidth: '100%' }} />
                    <span className="font-mono text-xs text-aurora-text-muted">{space.value}</span>
                  </div>
                ))}
              </div>
              <div className="grid gap-3 sm:grid-cols-3">
                {radiusScale.map((radius) => (
                  <div key={radius.label} className="space-y-2">
                    <div className={cn('h-16 border border-aurora-border-strong bg-aurora-control-surface', radius.className)} />
                    <p className="text-sm font-medium text-aurora-text-primary">{radius.label}</p>
                    <p className="font-mono text-xs text-aurora-text-muted">{radius.value}</p>
                  </div>
                ))}
              </div>
            </div>
          </section>

          <section className="space-y-3">
            <p className={AURORA_MUTED_LABEL}>Elevation</p>
            <div className="grid gap-3 md:grid-cols-2">
              {elevationTiers.map((tier) => (
                <div
                  key={tier.label}
                  className={cn(
                    'rounded-aurora-3 border border-aurora-border-strong bg-aurora-panel-medium px-4 py-5',
                    tier.className,
                    tier.emphasized ? 'bg-aurora-panel-strong' : '',
                  )}
                >
                  <p className={cn(AURORA_DISPLAY_NUMBER, 'text-xl text-aurora-text-primary')}>
                    {tier.label}
                  </p>
                  <p className="mt-2 text-sm text-aurora-text-muted">{tier.description}</p>
                </div>
              ))}
            </div>
          </section>
        </div>
      </div>
    </section>
  )
}
