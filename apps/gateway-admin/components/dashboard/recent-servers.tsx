'use client'

import Link from 'next/link'
import { ArrowRight } from 'lucide-react'
import { Skeleton } from '@/components/ui/skeleton'
import { gatewayDetailHref } from '@/lib/api/gateway-config'
import type { Gateway } from '@/lib/types/gateway'

export type RecentServer = Pick<Gateway, 'id' | 'name' | 'transport'> & {
  status: Pick<Gateway['status'], 'healthy' | 'connected' | 'exposed_tool_count'>
}

export function RecentServers({ gateways, loading = false, error = false }: { gateways: RecentServer[]; loading?: boolean; error?: boolean }) {
  return <section aria-label="Recent servers" className="overflow-hidden rounded-aurora-2 border border-aurora-border-subtle bg-aurora-panel-medium shadow-[var(--aurora-shadow-medium)]">
    <header className="flex items-center justify-between border-b border-aurora-border-subtle px-[14px] py-[11px]">
      <h2 className="text-[10px] font-bold uppercase tracking-[.14em] text-aurora-text-muted">Recent Servers</h2>
      <Link href="/gateways" className="inline-flex items-center gap-1 rounded-lg px-1.5 py-0.5 text-[11px] font-semibold text-aurora-text-muted hover:bg-aurora-hover-bg hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">View All<ArrowRight aria-hidden="true" className="size-[11px]"/></Link>
    </header>
    {loading ? <div role="status" aria-label="Loading recent servers" className="space-y-2 px-[14px] py-2">{[0, 1, 2].map(index => <Skeleton key={index} className="h-5 w-full"/>)}</div>
      : error ? <p role="status" className="px-[14px] py-3 text-xs text-aurora-text-muted">Recent servers are unavailable.</p>
      : gateways.length === 0 ? <div className="px-[14px] py-3 text-xs text-aurora-text-muted"><p>No servers configured.</p><Link href="/gateways" className="mt-2 inline-block text-aurora-accent-strong underline">Add server</Link></div>
      : <ul>{gateways.slice(0, 5).map(gateway => {
        const status = !gateway.status.connected ? 'Disconnected' : gateway.status.healthy ? 'Healthy' : 'Needs Attention'
        const color = !gateway.status.connected ? 'bg-aurora-error' : gateway.status.healthy ? 'bg-aurora-success' : 'bg-aurora-warn'
        return <li key={gateway.id} className="border-t border-aurora-border-subtle/70 first:border-t-0"><Link href={gatewayDetailHref(gateway.id)} className="flex min-w-0 items-center gap-[9px] px-[14px] py-2 hover:bg-aurora-hover-bg focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-aurora-accent-primary">
          <span title={status} aria-label={status} className={`size-1.5 shrink-0 rounded-full ${color}`}/>
          <span className="min-w-0 flex-1 truncate font-display text-[12.5px] font-bold text-aurora-text-primary">{gateway.name}</span>
          <span className="shrink-0 text-[9.5px] font-semibold uppercase tracking-[.1em] text-aurora-text-muted">{gateway.transport === 'in_process' ? 'Lab' : gateway.transport}</span>
          <span title="Exposed downstream tools" aria-label={`${gateway.status.exposed_tool_count} exposed downstream tools`} className="w-[30px] shrink-0 text-right text-[11.5px] font-semibold tabular-nums text-aurora-text-primary">{gateway.status.exposed_tool_count}</span>
        </Link></li>
      })}</ul>}
  </section>
}
