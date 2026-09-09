'use client'

import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { Loader2, Rocket } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { AURORA_DISPLAY_2, AURORA_STRONG_PANEL } from '@/components/aurora/tokens'
import { createDepotSkillActivationAttempt, prepareDepotSkillActivation, type DepotSkillActivationAttempt, type DepotSkillActivationPreparation } from '@/lib/api/depot-activation-client'
import type { DepotMembershipSource } from '@/lib/api/depot-membership-client'
import { getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'

export type DiscoverSendTarget = { title: string; kind: string; source?: DepotMembershipSource }
export type DiscoverSendDialogProps = {
  target: DiscoverSendTarget | null
  onOpenChange: (open: boolean) => void
  onActivated?: () => void
}
type State = { key: string; preparation?: DepotSkillActivationPreparation; error?: string; success?: boolean }

/** Separate from Add to Library: only an explicit confirmation can activate an already stored Skill. */
export function DiscoverSendDialog({ target, onOpenChange, onActivated }: DiscoverSendDialogProps) {
  const epoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => -1)
  const requestKey = target ? JSON.stringify([epoch, target.kind, target.source ?? null]) : ''
  const [state, setState] = useState<State>()
  const [busyKey, setBusyKey] = useState<string>()
  const activeKey = useRef(requestKey)
  const attempt = useRef<{ key: string; operation: DepotSkillActivationAttempt } | undefined>(undefined)
  const inFlight = useRef(false)
  useEffect(() => {
    activeKey.current = requestKey
    // A preflight-only failure has no mutation to reconcile. A sent attempt keeps its key.
    if (attempt.current && !attempt.current.operation.hasBeenSent) attempt.current = undefined
    if (!requestKey) return
    const [, kind, source] = JSON.parse(requestKey) as [number, string, DepotMembershipSource | null]
    const controller = new AbortController()
    void prepareDepotSkillActivation(kind, source ?? undefined, controller.signal).then(preparation => {
      if (!controller.signal.aborted) setState({ key: requestKey, preparation })
    }).catch(error => {
      if (!controller.signal.aborted) setState({ key: requestKey, error: error instanceof Error ? error.message : 'Unable to check activation readiness.' })
    })
    return () => controller.abort()
  }, [requestKey])
  const current = state?.key === requestKey ? state : undefined
  const busy = busyKey === requestKey
  const ready = current?.preparation?.status === 'ready' ? current.preparation : undefined

  async function confirm() {
    if (!ready || inFlight.current || current?.success) return
    const key = requestKey
    if (!attempt.current || attempt.current.key !== key) {
      attempt.current = { key, operation: createDepotSkillActivationAttempt(ready) }
    }
    inFlight.current = true
    setBusyKey(key)
    try {
      await attempt.current.operation.run()
      if (activeKey.current === key && getBrowserSessionEpoch() === ready.epoch) {
        setState({ key, preparation: ready, success: true })
        onActivated?.()
      }
    } catch (error) {
      if (activeKey.current === key && getBrowserSessionEpoch() === ready.epoch) {
        setState({ key, preparation: ready, error: error instanceof Error ? error.message : 'Activation could not be confirmed.' })
      }
    } finally {
      inFlight.current = false
      setBusyKey(undefined)
    }
  }

  return <Dialog open={target !== null} onOpenChange={open => { if (!inFlight.current) onOpenChange(open) }}>
    <DialogContent className={AURORA_STRONG_PANEL} showCloseButton={!busy}>
      <DialogHeader>
        <DialogTitle className={AURORA_DISPLAY_2}>Send to Labby</DialogTitle>
        <DialogDescription className="text-aurora-text-muted">
          Activate this exact Skill revision in the currently connected gateway. This does not deploy to another machine or automatically add anything to your library.
        </DialogDescription>
      </DialogHeader>
      <div className="space-y-3 overflow-y-auto aurora-scrollbar">
        <p className="break-words font-medium text-aurora-text-primary">{target?.title}</p>
        {target?.source ? <dl className="grid gap-2 text-sm text-aurora-text-muted">
          <div><dt>Depot connection</dt><dd className="break-all font-mono">{target.source.connection_id}</dd></div>
          <div><dt>Exact revision</dt><dd className="break-all font-mono">{target.source.revision_id}</dd></div>
        </dl> : null}
        {!current ? <p role="status" className="flex items-center gap-2 text-sm text-aurora-text-muted"><Loader2 aria-hidden="true" className="size-4 animate-spin motion-reduce:animate-none" />Checking the current library and activation permissions…</p> : null}
        {current?.preparation && current.preparation.status !== 'ready' ? <p role="status" className="text-sm text-aurora-text-muted">{current.preparation.message}</p> : null}
        {current?.error ? <Alert variant="error"><AlertDescription className="text-aurora-error">{current.error}{ready ? ' Retry uses the same operation key; it does not create a second activation request.' : ''}</AlertDescription></Alert> : null}
        {current?.success ? <Alert variant="success"><AlertDescription className="text-aurora-success">This exact Skill revision is active in the connected gateway.</AlertDescription></Alert> : null}
      </div>
      <DialogFooter>
        <Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>{current?.success ? 'Close' : 'Cancel'}</Button>
        {ready && !current?.success ? <Button data-visible-label disabled={busy} onClick={() => void confirm()}>
          {busy ? <Loader2 aria-hidden="true" className="size-4 animate-spin motion-reduce:animate-none" /> : <Rocket aria-hidden="true" className="size-4" />}
          {busy ? 'Activating…' : current?.error ? 'Retry same activation' : 'Activate in this gateway'}
        </Button> : null}
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
