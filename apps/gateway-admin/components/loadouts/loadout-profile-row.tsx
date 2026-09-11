'use client'

import { Children, useState, type ReactNode } from 'react'
import { ChevronDown } from 'lucide-react'
import type { GatewayLoadout, ProtectedMcpRoute } from '@/lib/types/gateway'
import { cn } from '@/lib/utils'

export const LOADOUT_PROFILE_COLUMNS = 'minmax(180px,1fr) 100px minmax(200px,1.2fr) 150px 96px 64px'

export function LoadoutProfileLayout({ view, loadouts, routes, routeStateUnavailable, children }: { view: 'table' | 'list' | 'cards'; loadouts: GatewayLoadout[]; routes: ProtectedMcpRoute[]; routeStateUnavailable: boolean; children: ReactNode }) {
  if (view !== 'table') return <div className={`grid gap-3 ${view === 'cards' ? 'xl:grid-cols-2' : ''}`}>{children}</div>
  const details = Children.toArray(children)
  return <section aria-label="Loadout profiles" className="aurora-scrollbar overflow-x-auto rounded-aurora-2 border border-aurora-border-subtle bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] shadow-aurora-strong"><div className="min-w-[862px]">
    <LoadoutProfileHeader />
    {loadouts.map((loadout, index) => <LoadoutProfileRow key={loadout.name} loadout={loadout} routes={routes} routeStateUnavailable={routeStateUnavailable} index={index}>{details[index]}</LoadoutProfileRow>)}
  </div></section>
}

export function LoadoutProfileHeader() {
  return <div aria-hidden="true" className="grid h-[34px] items-center gap-2 border-b border-aurora-border-strong bg-[var(--gw0-0_48)] px-4 text-[10px] font-bold uppercase tracking-[.14em] text-aurora-text-muted" style={{ gridTemplateColumns: LOADOUT_PROFILE_COLUMNS }}>
    {['Loadout', 'Auth', 'Endpoint', 'Scope', 'Calls 24h', ''].map((label, index) => <span key={index} className={index > 3 ? 'text-center' : undefined}>{label}</span>)}
  </div>
}

export function LoadoutProfileRow({ loadout, routes, routeStateUnavailable, index, children }: { loadout: GatewayLoadout; routes: ProtectedMcpRoute[]; routeStateUnavailable: boolean; index: number; children: ReactNode }) {
  const [expanded, setExpanded] = useState(false)
  const mounted = routes.filter(route => route.target?.kind === 'gateway_subset' && route.target.loadout === loadout.name)
  const endpoint = routeStateUnavailable ? 'Unavailable' : mounted.length ? mounted.map(route => `${route.public_host}${route.public_path}`).join(', ') : 'Not hosted'
  return <div>
    <button type="button" aria-expanded={expanded} aria-label={`${expanded ? 'Collapse' : 'Expand'} ${loadout.name}`} onClick={() => setExpanded(value => !value)} className={cn('grid w-full items-center gap-2 border-t border-[color-mix(in_srgb,var(--aurora-border-default)_55%,var(--aurora-page-bg))] px-4 py-2.5 text-left transition-colors duration-150 hover:bg-[var(--gw3-0_75)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-aurora-accent-primary', expanded ? 'bg-[color-mix(in_srgb,var(--aurora-accent-primary)_6%,var(--gw1-0_62))] shadow-[inset_3px_0_0_color-mix(in_srgb,var(--aurora-accent-primary)_42%,transparent)]' : index % 2 ? 'bg-[var(--gw2-0_55)]' : 'bg-[var(--gw1-0_62)]')} style={{ gridTemplateColumns: LOADOUT_PROFILE_COLUMNS }}>
      <span className="flex min-w-0 items-center gap-2"><span aria-hidden="true" className={`size-1.5 shrink-0 rounded-full ${loadout.restart_required ? 'bg-aurora-warn' : 'bg-aurora-text-subtle'}`} /><span className="truncate font-display text-[12.5px] font-[760] text-aurora-text-primary">{loadout.name}</span></span>
      <span className="justify-self-start text-[10px] text-aurora-text-muted">{!routeStateUnavailable && mounted.length ? <span className="inline-flex h-[19px] shrink-0 items-center rounded-full border border-[color-mix(in_srgb,var(--aurora-accent-primary)_26%,transparent)] bg-[color-mix(in_srgb,var(--aurora-accent-primary)_9%,transparent)] px-2 text-[9.5px] font-[650] uppercase tracking-[.08em] text-aurora-accent-strong">Protected</span> : '—'}</span>
      <span title={endpoint} className="truncate text-[10.5px] text-aurora-text-muted">{endpoint}</span>
      <span className="truncate text-[11px] text-aurora-text-muted">{loadout.upstreams.length} servers · {loadout.services.length} plugins</span>
      <span title="Call metrics unavailable" className="text-center text-[11.5px] text-aurora-text-muted">—</span>
      <ChevronDown aria-hidden="true" className={`size-3.5 justify-self-center text-aurora-text-muted transition-transform ${expanded ? 'rotate-180' : ''}`} />
    </button>
    {expanded ? <div className="relative border-t border-[color-mix(in_srgb,var(--aurora-accent-primary)_20%,var(--aurora-border-default))] after:pointer-events-none after:absolute after:inset-y-0 after:left-0 after:w-[3px] after:bg-[color-mix(in_srgb,var(--aurora-accent-primary)_42%,transparent)]">{children}</div> : null}
  </div>
}
