'use client'
import { useEffect, useRef, useState } from 'react'
import { X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { getBrowserSessionEpoch } from '@/lib/auth/session-store'
import type { Gateway } from '@/lib/types/gateway'
import { runGatewayBatch, type GatewayBatchCallbacks, type GatewayBatchAction, type GatewayBatchTarget, type GatewayBatchReport } from './gateway-selection-model'

export function GatewaySelectionToolbar({ gateways, selectedIds, onClear, ...callbacks }: GatewayBatchCallbacks & { gateways: Gateway[]; selectedIds: string[]; onClear: () => void }) {
  const current = useRef(gateways)
  const mounted = useRef(false)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  useEffect(() => { current.current = gateways }, [gateways])
  const busyLock = useRef(false)
  const [busy, setBusy] = useState(false)
  const [pending, setPending] = useState<{ action: GatewayBatchAction; targets: GatewayBatchTarget[]; epoch: number } | null>(null)
  const [reports, setReports] = useState<GatewayBatchReport[]>([])
  const selected = gateways.filter(gateway => selectedIds.includes(gateway.id))
  const request = (action: GatewayBatchAction) => {
    if (busyLock.current) return
    setReports([])
    setPending({ action, targets: selected.map(({ id, name }) => ({ id, name })), epoch: getBrowserSessionEpoch() })
  }
  const confirm = async () => {
    if (!pending || busyLock.current) return
    busyLock.current = true
    setBusy(true)
    try {
      const result = await runGatewayBatch(pending.action, pending.targets, () => current.current, callbacks, () => mounted.current && pending.epoch === getBrowserSessionEpoch())
      if (mounted.current && pending.epoch === getBrowserSessionEpoch()) setReports(result)
      if (mounted.current) setPending(null)
    } finally { busyLock.current = false; if (mounted.current) setBusy(false) }
  }
  if (!selected.length && !pending && !reports.length) return null
  return <div className="border-b border-aurora-accent-primary/25 bg-aurora-accent-primary/5 px-5 py-[7px]">
    <div className="flex flex-wrap items-center gap-2"><span className="mr-auto text-[11px] font-semibold tabular-nums text-aurora-accent-strong">{selected.length} selected</span>
      {(['enable', 'disable', 'reload'] as const).map(action => {
        const available = action === 'reload' ? callbacks.onBatchReload : callbacks.onBatchSetEnabled
        const tone = action === 'enable' ? 'border-aurora-success/35 bg-aurora-success/10 text-aurora-success' : action === 'reload' ? 'border-aurora-accent-primary/40 bg-aurora-accent-primary/10 text-aurora-accent-strong' : 'text-aurora-text-muted'
        return <Button key={action} data-visible-label="1" variant="outline" size="sm" disabled={busy || !selected.length || !available} title={!available ? 'Batch operation is unavailable' : undefined} className={`h-[26px] rounded-[7px] px-[11px] text-[11px] capitalize ${tone}`} onClick={() => request(action)}>{action}</Button>
      })}
      <Button variant="ghost" size="icon-sm" className="size-[26px]" aria-label="Clear selection" disabled={busy} onClick={() => { onClear(); setReports([]) }}><X className="size-3"/></Button>
    </div>
    {reports.length ? <ul role="status" aria-label="Batch operation results" className="mt-2 space-y-1 text-xs text-aurora-text-muted">{reports.map(report => <li key={report.id}>{report.name}: {report.outcome}{report.detail ? ` — ${report.detail}` : ''}</li>)}</ul> : null}
    <ActionConfirmationDialog open={Boolean(pending)} busy={busy} title={pending ? `${pending.action[0].toUpperCase()}${pending.action.slice(1)} selected servers?` : 'Confirm selected servers'} description={pending ? `Apply ${pending.action} sequentially to these confirmed servers: ${pending.targets.map(target => `${target.name} (${target.id})`).join(', ')}. Servers that disappear or already match the requested state will be skipped.` : ''} confirmLabel="Confirm batch" onOpenChange={open => { if (!open && !busyLock.current) setPending(null) }} onConfirm={() => void confirm()}/>
  </div>
}
