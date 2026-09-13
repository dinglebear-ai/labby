'use client'

import { Button } from '@/components/ui/button'

const CARDS = [
  { workspace: 'catalog', title: 'Catalog lifecycle', description: 'Discovery, canonical artifacts, sources, durable jobs, uploads, bundles, and publication share one workspace.', action: 'Open Catalog' },
  { workspace: 'access', title: 'Access & governance', description: 'Token administration and publication policy use Depot’s canonical schemas and Labby’s admin guard.', action: 'Open Access' },
  { workspace: 'operations', title: 'System operations', description: 'Status, CAS audits, maintenance, and migrations remain explicit, reviewable operations.', action: 'Open Operations' },
] as const

export function AdministrationOverview({ onOpen }: { onOpen: (workspace: 'catalog' | 'access' | 'operations') => void }) {
  return <div className="grid grid-cols-[repeat(auto-fit,minmax(min(260px,100%),1fr))] gap-3.5">
    {CARDS.map(card => <section key={card.workspace} aria-label={card.title} className="min-w-0 overflow-hidden rounded-aurora-2 border border-[color-mix(in_srgb,var(--aurora-border-default)_45%,var(--aurora-page-bg))] bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-aurora-medium">
      <h2 style={{ background: 'var(--gw0-0_38)' }} className="border-b border-[color-mix(in_srgb,var(--aurora-border-default)_55%,var(--aurora-page-bg))] px-[15px] py-2.5 text-[9.5px] font-bold uppercase tracking-[.13em] text-aurora-text-muted">{card.title}</h2>
      <div className="flex flex-col gap-3.5 px-[15px] py-3.5">
        <p className="text-pretty text-[12.5px] leading-[1.6] text-aurora-text-muted">{card.description}</p>
        <Button variant="outline" size="sm" onClick={() => onOpen(card.workspace)} style={{ borderColor: 'color-mix(in srgb, var(--aurora-accent-primary) 55%, var(--aurora-border-strong))', background: 'color-mix(in srgb, var(--aurora-accent-primary) 10%, var(--aurora-panel-strong))' }} className="h-[30px] self-start rounded-lg px-[13px] text-xs font-[650] text-aurora-accent-strong">{card.action}</Button>
      </div>
    </section>)}
  </div>
}
