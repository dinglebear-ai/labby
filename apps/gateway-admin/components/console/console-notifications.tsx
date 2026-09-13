'use client'

import { useState } from 'react'
import Link from 'next/link'
import { ArrowRight, Bell, CheckCheck } from 'lucide-react'
import { gatewayDetailHref } from '@/lib/api/gateway-config'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Button } from '@/components/ui/button'
import { useGatewayNotifications } from '@/lib/notification-acknowledgements'
import type { ConsoleStatusState } from './console-status-strip'

/** Current upstream attention, derived from the same snapshot as the status rail. */
export function ConsoleNotifications({ state }: { state: ConsoleStatusState }) {
  const [open, setOpen] = useState(false)
  const { notifications, clearAll } = useGatewayNotifications('gateway-runtime', state.kind === 'ready' ? (state.attention ?? []).map(name => ({ key: `gateway:${name}:disconnected`, fingerprint: state.disconnectedOccurrences?.[name] ?? 'disconnected', gatewayName: name, message: 'Enabled upstream is disconnected.' })) : undefined)
  useGatewayNotifications('gateway-alerts', state.kind === 'ready' ? state.alerts : undefined)
  const attention = notifications
  const notificationTitle = attention.length === 0
    ? 'Notifications — all clear'
    : attention.length === 1
      ? 'Notifications — 1 needs attention'
      : `Notifications — ${attention.length} need attention`
  return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger asChild>
    <Button data-visible-label variant="ghost" size="icon" aria-label="Notifications" aria-expanded={open} title={notificationTitle} style={{ position: 'relative', width: 32, minWidth: 32, height: 32, borderRadius: 'var(--radius-1)', border: '1px solid color-mix(in srgb, var(--aurora-border-default) 70%, var(--aurora-page-bg))', background: 'var(--aurora-control-surface)', color: 'var(--aurora-text-muted)' }}>
      <Bell size={14} strokeWidth={1.7} />
      {attention.length > 0 ? <span aria-hidden style={{ position: 'absolute', top: 4, right: 3.5, width: 6, height: 6, borderRadius: 999, background: 'var(--aurora-warn)', boxShadow: '0 0 4px var(--aurora-warn)', border: '1.5px solid var(--aurora-page-bg)', boxSizing: 'content-box' }} /> : null}
    </Button>
    </PopoverTrigger>
    <PopoverContent align="end" sideOffset={8} className="p-0" data-anim="menu" style={{ width: 292, borderRadius: 'var(--radius-2)', border: '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))', background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))', boxShadow: 'var(--aurora-shadow-strong), inset 0 1px 0 rgba(255,255,255,.05)', overflow: 'hidden' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '10px 12px', borderBottom: '1px solid var(--aurora-border-default)', background: 'var(--gw0-0_36)', fontSize: 9.5, fontWeight: 700, letterSpacing: '.1em', color: 'var(--console-workspace-label)' }}>NOTIFICATIONS<Button data-visible-label variant="ghost" size="sm" className="ml-auto h-6 gap-1 px-1.5 text-[10px] normal-case tracking-normal" disabled={attention.length === 0} onClick={clearAll}><CheckCheck size={12}/>Clear all</Button></div>
      {state.kind === 'ready' && attention.length > 0 ? <div className="aurora-scrollbar" style={{ padding: 5, maxHeight: 268, overflowY: 'auto' }}>{attention.map(notification => <Link key={notification.key} href={gatewayDetailHref(notification.gatewayName)} data-menurow="1" onClick={() => setOpen(false)} style={{ display: 'flex', gap: 9, padding: '8px 9px', borderRadius: 8 }}><span aria-hidden style={{ marginTop: 4, width: 6, height: 6, flexShrink: 0, borderRadius: 999, background: 'var(--aurora-warn)' }} /><span style={{ minWidth: 0, fontSize: 12, fontWeight: 620 }}>{notification.gatewayName}<span style={{ display: 'block', fontSize: 10.5, fontWeight: 400, color: 'var(--aurora-text-muted)' }}>{notification.message}</span></span></Link>)}</div> : <p role="status" style={{ margin: 0, padding: '18px 14px', fontSize: 11.5, color: 'var(--aurora-text-muted)' }}>{state.kind === 'loading' ? 'Loading gateway status…' : state.kind === 'unauthorized' ? 'Gateway status requires administrator access.' : state.kind === 'unavailable' ? `Gateway status unavailable: ${state.reason}` : 'No new notifications.'}</p>}
      <div style={{ padding: 5, borderTop: '1px solid var(--aurora-border-default)' }}><Link href="/gateways" data-menurow="1" onClick={() => setOpen(false)} style={{ display: 'flex', alignItems: 'center', gap: 9, height: 32, padding: '0 9px', borderRadius: 8, fontSize: 12.5, fontWeight: 560, color: 'var(--aurora-accent-strong)' }}><ArrowRight size={14} strokeWidth={1.7} />Open Gateway</Link></div>
    </PopoverContent>
  </Popover>
}
