'use client'

import { useEffect, useRef, useState, useSyncExternalStore } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Field, FieldLabel } from '@/components/ui/field'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Alert, AlertDescription } from '@/components/ui/alert'
import { AURORA_DISPLAY_2, AURORA_STRONG_PANEL } from '@/components/aurora/tokens'
import { createDepotForkAttempt, prepareDepotFork, type DepotForkPreparation } from '@/lib/api/depot-fork-client'
import type { DepotMembershipSource } from '@/lib/api/depot-membership-client'
import { getBrowserSessionEpoch, subscribeToBrowserSession } from '@/lib/auth/session-store'

export type DiscoverForkTarget = { title: string; source?: DepotMembershipSource }
export type DiscoverForkDialogProps = { target: DiscoverForkTarget | null; onOpenChange: (open: boolean) => void; onForked?: () => void }
type State = { key: string; preparation?: DepotForkPreparation; error?: string; created?: string }

export function DiscoverForkDialog({ target, onOpenChange, onForked }: DiscoverForkDialogProps) {
  const epoch = useSyncExternalStore(subscribeToBrowserSession, getBrowserSessionEpoch, () => -1)
  const key = target ? JSON.stringify([epoch, target.source ?? null]) : ''
  const [state, setState] = useState<State>()
  const [namespace, setNamespace] = useState('')
  const [name, setName] = useState('')
  const [busy, setBusy] = useState(false)
  const [submittedKey, setSubmittedKey] = useState<string>()
  const activeKey = useRef(key)
  const running = useRef(false)
  useEffect(() => {
    activeKey.current = key
    if (!key) return
    const controller = new AbortController()
    const [, source] = JSON.parse(key) as [number, DepotMembershipSource | null]
    void prepareDepotFork(source ?? undefined, controller.signal).then(preparation => {
      if (!controller.signal.aborted) setState({ key, preparation })
    }).catch(error => {
      if (!controller.signal.aborted) setState({ key, error: error instanceof Error ? error.message : 'Unable to check Fork permissions.' })
    })
    return () => { controller.abort(); activeKey.current = '' }
  }, [key])
  const current = state?.key === key ? state : undefined
  const ready = current?.preparation?.status === 'ready' ? current.preparation : undefined

  async function confirm() {
    if (!ready || running.current || submittedKey === key) return
    let attempt: ReturnType<typeof createDepotForkAttempt>
    try { attempt = createDepotForkAttempt(ready, namespace, name) }
    catch { setState({ key, preparation: ready, error: 'Use letters or digits separated by single dots, underscores or hyphens, up to 128 characters, for namespace and name.' }); return }
    running.current = true
    setBusy(true)
    setSubmittedKey(key)
    try {
      const created = await attempt()
      if (activeKey.current === key && getBrowserSessionEpoch() === ready.epoch) {
        setState({ key, preparation: ready, created })
        onForked?.()
      }
    } catch (error) {
      if (activeKey.current === key && getBrowserSessionEpoch() === ready.epoch) setState({ key, preparation: ready, error: error instanceof Error ? error.message : 'Fork outcome is unknown.' })
    } finally { running.current = false; setBusy(false) }
  }

  return <Dialog open={target !== null} onOpenChange={open => { if (!running.current) onOpenChange(open) }}>
    <DialogContent className={AURORA_STRONG_PANEL} showCloseButton={!busy}>
      <DialogHeader>
        <DialogTitle className={AURORA_DISPLAY_2}>Fork Artifact</DialogTitle>
        <DialogDescription>Create a hosted fork of this exact revision in the selected Depot connection. Nothing is imported or activated in Labby.</DialogDescription>
      </DialogHeader>
      <p className="break-words text-sm text-aurora-text-primary">{target?.title}</p>
      {target?.source ? <dl className="grid gap-2 text-sm text-aurora-text-muted">
        <div><dt>Depot connection</dt><dd className="break-all font-mono">{target.source.connection_id}</dd></div>
        <div><dt>Source Artifact</dt><dd className="break-all font-mono">{target.source.artifact_id}</dd></div>
        <div><dt>Exact revision</dt><dd className="break-all font-mono">{target.source.revision_id}</dd></div>
      </dl> : null}
      {!current ? <p role="status" className="text-sm text-aurora-text-muted">Checking current Fork permissions…</p> : null}
      {current?.preparation?.status === 'blocked' ? <p role="status" className="text-sm text-aurora-text-muted">{current.preparation.message}</p> : null}
      {ready && !current?.created ? <div className="grid gap-3">
        <Field><FieldLabel htmlFor="discover-fork-namespace">Namespace</FieldLabel><Input id="discover-fork-namespace" name="fork-namespace" value={namespace} maxLength={128} disabled={busy || submittedKey === key} onChange={event => setNamespace(event.target.value)} /></Field>
        <Field><FieldLabel htmlFor="discover-fork-name">Name</FieldLabel><Input id="discover-fork-name" name="fork-name" value={name} maxLength={128} disabled={busy || submittedKey === key} onChange={event => setName(event.target.value)} /></Field>
      </div> : null}
      {current?.error ? <Alert variant="error"><AlertDescription>{current.error}{submittedKey === key ? ' No retry was sent. Check the selected hosted catalog before starting another fork.' : ''}</AlertDescription></Alert> : null}
      {current?.created ? <Alert variant="success"><AlertDescription>Hosted fork created: {current.created}</AlertDescription></Alert> : null}
      <DialogFooter>
        <Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>Close</Button>
        {ready && !current?.created ? <Button disabled={busy || submittedKey === key || !namespace || !name} onClick={() => void confirm()}>{busy ? 'Creating fork…' : 'Create hosted fork'}</Button> : null}
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
