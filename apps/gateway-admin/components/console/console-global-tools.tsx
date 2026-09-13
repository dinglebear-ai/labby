'use client'

import { useEffect, useState } from 'react'
import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { ArrowUpRight, X } from 'lucide-react'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Button } from '@/components/ui/button'
import { useBrowserSession } from '@/lib/auth/session'
import { authorityIdentity } from '@/lib/auth/authority'
import { skillLibrary } from '@/lib/api/skill-library-client'
import { gatewayApi, gatewayAction } from '@/lib/api/gateway-client'
import { snippetsApi } from '@/lib/api/snippets-client'
import type { BackendGatewayMcpRuntimeView } from '@/lib/server/gateway-adapter'
import { deriveConsoleStatus } from './console-status-strip'

const TRAY_DESTINATIONS = [
  ['artifacts', '/library', 'Artifacts'], ['loadouts', '/loadouts', 'Loadouts'],
  ['snippets', '/snippets', 'Snippets'], ['tools', '/tools', 'Tools'],
] as const

type TrayCounts = Partial<Record<(typeof TRAY_DESTINATIONS)[number][0], number>>

function useTrayCounts() {
  const session = useBrowserSession()
  const identity = session.status === 'authenticated' ? authorityIdentity(session.authority) : session.status
  const [result, setResult] = useState<{ identity: string; counts: TrayCounts }>({ identity, counts: {} })
  useEffect(() => {
    const controller = new AbortController()
    let timer: ReturnType<typeof setTimeout> | undefined
    if (session.status !== 'authenticated') return () => controller.abort()
    const refresh = async () => {
      const signal = controller.signal
      const results = await Promise.allSettled([
        skillLibrary.list('', signal),
        gatewayApi.listLoadouts(signal), snippetsApi.list(signal),
        gatewayAction<BackendGatewayMcpRuntimeView[]>('gateway.mcp.list', {}, signal),
      ])
      if (signal.aborted) return
      const counts: TrayCounts = {}
      const [artifacts, loadouts, snippets, tools] = results
      if (artifacts.status === 'fulfilled' && !artifacts.value.next_cursor) counts.artifacts = artifacts.value.items.length
      if (loadouts.status === 'fulfilled') counts.loadouts = loadouts.value.length
      if (snippets.status === 'fulfilled') counts.snippets = snippets.value.length
      if (tools.status === 'fulfilled') counts.tools = deriveConsoleStatus(tools.value).tools
      setResult({ identity, counts })
      timer = setTimeout(() => { void refresh() }, 30_000)
    }
    void refresh()
    return () => { controller.abort(); clearTimeout(timer) }
  }, [identity, session.status])
  return result.identity === identity && session.status === 'authenticated' ? result.counts : {}
}

export function ConsoleLibraryTray({ counts }: { counts: TrayCounts }) {
  const pathname = usePathname() ?? ''
  return <nav aria-label="Quick library navigation" data-console-library-tray="1" className="aurora-scrollbar flex shrink-0 gap-0.5 overflow-x-auto border-t border-aurora-border-subtle bg-[var(--gw0-0_30)] px-5 pr-20">
    {TRAY_DESTINATIONS.map(([key, href, label]) => <Link key={key} href={href} aria-current={pathname === href || pathname.startsWith(`${href}/`) ? 'page' : undefined} className="inline-flex h-[38px] shrink-0 items-center gap-2 whitespace-nowrap border-b-2 border-transparent px-3.5 text-[12.5px] font-[650] text-aurora-text-muted hover:text-aurora-text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-aurora-accent-primary aria-[current=page]:border-aurora-accent-primary aria-[current=page]:text-aurora-accent-strong">
      {label}<span title={counts[key] === undefined ? 'Count unavailable for the current authority' : undefined} className="inline-flex h-[19px] min-w-5 items-center justify-center rounded-[5px] border border-aurora-border-strong/60 bg-[var(--gw0-0_45)] px-[5px] text-[10.5px] font-bold tabular-nums">{counts[key] ?? '—'}</span>
    </Link>)}
  </nav>
}

function PhoenixMark() {
  return <svg aria-hidden="true" width="25" height="25" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"><circle cx="12" cy="3.6" r="1.05"/><path d="M12.9 4.5c.5.7.6 1.5.3 2.4M11.7 7.2C9.7 4.9 7 3.8 3.7 4.1c1.5 2.4 3.5 4.1 6 5M12.3 7.2c2-2.3 4.7-3.4 8-3.1-1.5 2.4-3.5 4.1-6 5M12 7.6c-1.4 1.3-2.2 3.2-2.5 5.6-.3 2.5.5 4.8 2.5 7 2-2.2 2.8-4.5 2.5-7-.3-2.4-1.1-4.3-2.5-5.6zM9.6 15.4 7.1 18.7M14.4 15.4l2.5 3.3M12 20.5v1.4"/></svg>
}

export function PhoenixAvailability() {
  const [open, setOpen] = useState(false)
  return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger asChild><button type="button" aria-label={open ? 'Close Phoenix' : 'Ask Phoenix'} title={open ? 'Close Phoenix' : 'Ask Phoenix'} className="fixed bottom-[26px] right-[14px] z-40 grid size-[52px] place-items-center rounded-full border border-aurora-border-strong bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] text-aurora-accent-pink shadow-[0_14px_34px_-12px_rgba(2,10,16,.66),inset_0_1px_0_rgba(255,255,255,.05)] transition-transform hover:-translate-y-0.5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-aurora-accent-pink data-[state=open]:bg-aurora-accent-pink data-[state=open]:text-aurora-page-bg data-[state=open]:bg-none"><PhoenixMark/></button></PopoverTrigger>
    <PopoverContent side="top" align="end" sideOffset={14} collisionPadding={22} aria-label="Phoenix session" className="aurora-scrollbar flex h-[min(560px,calc(100vh-128px))] w-[min(420px,calc(100vw-44px))] flex-col overflow-hidden rounded-[16px] border-aurora-border-strong bg-[linear-gradient(180deg,var(--aurora-panel-strong-top),var(--aurora-panel-strong))] p-0 text-aurora-text-primary shadow-aurora-strong">
      <div className="flex items-center gap-[9px] border-b border-aurora-border-default bg-[var(--gw0-0_38)] px-3 py-2.5"><span className="text-aurora-accent-pink"><PhoenixMark/></span><div className="min-w-0 flex-1"><h2 className="font-display text-[13.5px] font-extrabold">Phoenix</h2><p className="text-[10.5px] text-aurora-text-muted">Session execution unavailable</p></div><Button data-visible-label variant="ghost" size="icon" className="size-6 min-w-6" aria-label="Close Phoenix panel" onClick={() => setOpen(false)}><X size={13}/></Button></div>
      <div className="flex flex-1 flex-col justify-center gap-3 px-6 py-8"><h3 className="font-display text-lg font-bold">Session execution is unavailable</h3><p className="text-[12.5px] leading-relaxed text-aurora-text-muted">This console does not expose an interactive agent session API. Phoenix cannot send messages or run tasks from this panel.</p><Link href="/agents" onClick={() => setOpen(false)} className="inline-flex items-center gap-1 self-start rounded-md py-1 text-xs font-semibold text-aurora-accent-strong hover:underline focus-visible:ring-2 focus-visible:ring-aurora-accent-primary">View agents<ArrowUpRight size={13}/></Link></div>
      <div className="border-t border-aurora-border-subtle p-3"><p className="rounded-[12px] border border-aurora-border-default bg-[var(--gw0-0_40)] px-3 py-3 text-xs text-aurora-text-muted">Messaging becomes available when a session execution service is connected.</p></div>
    </PopoverContent>
  </Popover>
}

export function ConsoleGlobalTools() {
  const counts = useTrayCounts()
  return <><ConsoleLibraryTray counts={counts}/><PhoenixAvailability/></>
}
