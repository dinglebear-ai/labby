'use client'

import { Button } from '@/components/ui/button'

const CARDS = [
  { workspace: 'sources', title: 'Sources & ingestion', description: 'Repository sources, refresh schedules, revision state, failures, and durable ingest jobs have a dedicated operator workspace.', action: 'Open Sources' },
  { workspace: 'artifacts', title: 'Artifacts & publication', description: 'Discover and ingest Artifacts, upload archives, govern publication and license state, and curate bundles through Labby’s Artifact control plane.', action: 'Open Artifacts' },
  { workspace: 'catalog', title: 'Canonical operation catalog', description: 'Every Depot API-capable operation remains schema-driven here as the complete escape hatch when no dedicated workflow exists yet.', action: 'Open Catalog' },
  { workspace: 'access', title: 'Access & governance', description: 'Token administration and publication policy use Depot’s authority requirements with Labby’s admin and CSRF guard.', action: 'Open Access' },
  { workspace: 'operations', title: 'System operations', description: 'Maintenance, CAS audits and migrations use a separate delegated operator authority bound to the exact operation and parameters.', action: 'Open Operations' },
] as const

export function AdministrationOverview({ onOpen }: { onOpen: (workspace: 'sources' | 'artifacts' | 'catalog' | 'access' | 'operations') => void }) {
  return <div className="grid grid-cols-[repeat(auto-fit,minmax(min(260px,100%),1fr))] gap-3.5">
    {CARDS.map(card => <section key={card.workspace} aria-label={card.title} className="min-w-0 overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-aurora-medium">
      <h2 className="border-b border-aurora-border-default bg-aurora-control-surface px-[15px] py-2.5 text-[9.5px] font-bold uppercase tracking-[.13em] text-aurora-text-muted">{card.title}</h2>
      <div className="flex flex-col gap-3.5 px-[15px] py-3.5">
        <p className="text-pretty text-[12.5px] leading-[1.6] text-aurora-text-muted">{card.description}</p>
        <Button variant="outline" size="sm" onClick={() => onOpen(card.workspace)} className="h-[30px] self-start rounded-lg border-aurora-border-strong bg-aurora-selected-bg px-[13px] text-xs font-[650] text-aurora-accent-strong">{card.action}</Button>
      </div>
    </section>)}
  </div>
}
