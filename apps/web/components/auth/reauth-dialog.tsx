'use client'

import { useCallback, useEffect, useRef, useState } from 'react'
import { AlertCircle, Loader2, ShieldCheck } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from '@/components/ui/dialog'
import { getBrowserSessionEpoch, getSessionCsrfToken } from '@/lib/auth/session-store'
import { cancelReauth, startReauth, waitForReauthProof, type ReauthPurpose } from '@/lib/auth/reauth'
import { openIsolatedOauthPopup } from '@/lib/oauth-popup'

type Phase =
  | { kind: 'idle' }
  | { kind: 'starting' }
  | { kind: 'waiting'; interaction: string }
  | { kind: 'error'; message: string }

export function ReauthDialog({
  open,
  purpose,
  onOpenChange,
  onProof,
  openPopup = openIsolatedOauthPopup,
}: {
  open: boolean
  purpose: ReauthPurpose
  onOpenChange: (open: boolean) => void
  onProof: (proof: string) => void | Promise<void>
  openPopup?: typeof openIsolatedOauthPopup
}) {
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })
  const activeRun = useRef<{
    controller: AbortController
    popup: Window
    csrf: string
    interaction?: string
  } | null>(null)

  const cancelCurrent = useCallback(() => {
    const run = activeRun.current
    activeRun.current = null
    if (!run) return
    run.controller.abort()
    run.popup.close()
    if (run.interaction) void cancelReauth(run.interaction, run.csrf).catch(() => {})
  }, [])

  useEffect(() => {
    if (!open) {
      cancelCurrent()
      setPhase({ kind: 'idle' })
    }
    return cancelCurrent
  }, [open, cancelCurrent])

  async function begin() {
    const csrf = getSessionCsrfToken()
    if (!csrf) {
      setPhase({ kind: 'error', message: 'Your browser session is unavailable. Sign in again.' })
      return
    }
    cancelCurrent()
    const popup = openPopup()
    if (!popup) {
      setPhase({ kind: 'error', message: 'Allow popups for this site, then try again.' })
      return
    }
    const run = { controller: new AbortController(), popup, csrf, interaction: undefined as string | undefined }
    activeRun.current = run
    const epoch = getBrowserSessionEpoch()
    const current = () => activeRun.current === run && !run.controller.signal.aborted && epoch === getBrowserSessionEpoch()
    setPhase({ kind: 'starting' })
    try {
      // Retain a late start response so its server-owned interaction can be cancelled.
      const started = await startReauth(purpose, csrf)
      run.interaction = started.interaction
      if (!current() || popup.closed) {
        if (activeRun.current === run) cancelCurrent()
        else {
          popup.close()
          await cancelReauth(started.interaction, csrf).catch(() => {})
        }
        return
      }
      setPhase({ kind: 'waiting', interaction: started.interaction })
      popup.location.href = started.authorizationUrl
      const proof = await waitForReauthProof(started.interaction, epoch, { signal: run.controller.signal })
      if (!current()) return
      popup.close()
      await onProof(proof)
      if (!current()) return
      activeRun.current = null
      onOpenChange(false)
    } catch (error) {
      const showError = current()
      if (activeRun.current === run) cancelCurrent()
      else popup.close()
      if (showError) {
        setPhase({
          kind: 'error',
          message: error instanceof Error ? error.message : 'Reauthentication failed. Try again.',
        })
      }
    }
  }

  function changeOpen(next: boolean) {
    if (!next) { cancelCurrent(); setPhase({ kind: 'idle' }) }
    onOpenChange(next)
  }

  const busy = phase.kind === 'starting' || phase.kind === 'waiting'
  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Confirm it&apos;s you</DialogTitle>
          <DialogDescription>
            Reauthenticate before changing shared provider credentials. This approval is limited to this one save.
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-space-4" aria-live="polite">
          {busy && (
            <div className="flex items-center gap-space-2 text-sm text-aurora-text-muted">
              <Loader2 className="size-4 animate-spin" aria-hidden="true" />
              {phase.kind === 'starting' ? 'Starting secure sign-in…' : 'Complete sign-in in the new tab…'}
            </div>
          )}
          {phase.kind === 'idle' && (
            <div className="flex items-start gap-space-3 rounded-aurora-1 border border-aurora-border-default bg-aurora-control-surface p-space-4 text-sm text-aurora-text-muted">
              <ShieldCheck className="mt-0.5 size-4 shrink-0 text-aurora-success" aria-hidden="true" />
              Your secret remains in this form. You may need to enter it again after returning.
            </div>
          )}
          {phase.kind === 'error' && (
            <div role="alert" className="flex items-start gap-space-2 text-sm text-aurora-error">
              <AlertCircle className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
              {phase.message}
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => changeOpen(false)}>Cancel</Button>
          <Button disabled={busy} onClick={() => void begin()}>
            {phase.kind === 'error' ? 'Try again' : 'Continue with Google'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
