'use client'

import { useEffect, useRef, useState } from 'react'
import { Loader2, Link2, ShieldCheck, AlertCircle } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Badge } from '@/components/ui/badge'
import { upstreamOauthApi } from '@/lib/api/upstream-oauth-client'
import { openIsolatedOauthPopup } from '@/lib/oauth-popup'
import { isAbortError } from '@/lib/api/service-action-client'
import { useUpstreamOauthStatus } from '@/lib/hooks/use-upstream-oauth'
import type { ProbeResponse } from '@/lib/types/upstream-oauth'

interface ConnectUpstreamDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onConnected: () => void
}

type Phase =
  | { kind: 'idle' }
  | { kind: 'probing' }
  | { kind: 'discovered'; probe: ProbeResponse }
  | { kind: 'no_oauth'; probe: ProbeResponse }
  | { kind: 'authorizing'; upstream: string }
  | { kind: 'error'; message: string }

export function ConnectUpstreamDialog({ open, onOpenChange, onConnected }: ConnectUpstreamDialogProps) {
  const [url, setUrl] = useState('')
  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })
  const abortRef = useRef<AbortController | null>(null)

  const authorizingUpstream = phase.kind === 'authorizing' ? phase.upstream : null
  const { data: status } = useUpstreamOauthStatus(authorizingUpstream, {
    pollWhilePending: phase.kind === 'authorizing',
  })

  useEffect(() => {
    if (phase.kind === 'authorizing' && status?.authenticated) {
      onConnected()
      onOpenChange(false)
    }
  }, [phase.kind, status?.authenticated, onConnected, onOpenChange])

  // Reset state when the dialog is closed externally (e.g. parent sets open={false}
  // after onConnected fires). handleOpenChange is only called when Radix fires
  // onOpenChange, so direct prop changes would otherwise leave stale phase state.
  useEffect(() => {
    if (!open) {
      abortRef.current?.abort()
      setUrl('')
      setPhase({ kind: 'idle' })
    }
  }, [open])

  function reset() {
    abortRef.current?.abort()
    setUrl('')
    setPhase({ kind: 'idle' })
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset()
    onOpenChange(next)
  }

  async function handleProbe() {
    if (!url.trim()) return
    abortRef.current?.abort()
    const ac = new AbortController()
    abortRef.current = ac
    setPhase({ kind: 'probing' })
    try {
      const probe = await upstreamOauthApi.probe(url.trim(), ac.signal)
      setPhase(probe.oauth_discovered ? { kind: 'discovered', probe } : { kind: 'no_oauth', probe })
    } catch (err: unknown) {
      if (isAbortError(err)) return
      setPhase({ kind: 'error', message: err instanceof Error ? err.message : 'Probe failed' })
    }
  }

  async function handleAuthorize(upstream: string) {
    setPhase({ kind: 'authorizing', upstream })

    // Open a blank tab synchronously — must happen directly in the click handler
    // before any await, otherwise browsers treat it as an unsolicited popup and block it.
    const authTab = openIsolatedOauthPopup()
    if (!authTab) {
      setPhase({ kind: 'error', message: 'Popup blocked — allow popups for this site and try again' })
      return
    }

    try {
      const { authorization_url } = await upstreamOauthApi.start(upstream)
      if (authTab.closed) {
        setPhase({ kind: 'error', message: 'Authorization tab was closed. Please try again.' })
        return
      }
      authTab.location.href = authorization_url
    } catch (err: unknown) {
      authTab.close()
      setPhase({ kind: 'error', message: err instanceof Error ? err.message : 'Failed to start authorization' })
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Connect upstream MCP server</DialogTitle>
          <DialogDescription>
            Enter the server URL. The gateway will probe it for OAuth support and guide you through authorization.
          </DialogDescription>
        </DialogHeader>

        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="upstream-url">Server URL</Label>
            <div className="flex gap-2">
              <Input
                id="upstream-url"
                type="url"
                placeholder="https://mcp.example.com"
                value={url}
                onChange={e => setUrl(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter') void handleProbe() }}
                disabled={phase.kind === 'probing' || phase.kind === 'authorizing'}
              />
              <Button
                onClick={() => void handleProbe()}
                disabled={!url.trim() || phase.kind === 'probing' || phase.kind === 'authorizing'}
                variant="secondary"
              >
                {phase.kind === 'probing' ? <Loader2 className="size-4 animate-spin" /> : 'Probe'}
              </Button>
            </div>
          </div>

          {phase.kind === 'discovered' && (
            <div className="rounded-lg border border-aurora-success/30 bg-aurora-success/10 p-4 flex flex-col gap-3">
              <div className="flex items-center gap-2 text-sm font-medium text-aurora-success">
                <ShieldCheck className="size-4" />
                OAuth discovered
              </div>
              {phase.probe.issuer && (
                <p className="text-xs text-aurora-text-muted break-all">
                  Issuer: <span className="font-mono">{phase.probe.issuer}</span>
                </p>
              )}
              {phase.probe.scopes && phase.probe.scopes.length > 0 && (
                <div className="flex flex-wrap gap-1">
                  {phase.probe.scopes.map(s => (
                    <Badge key={s} variant="outline" className="text-xs font-mono">{s}</Badge>
                  ))}
                </div>
              )}
              <Button onClick={() => void handleAuthorize(phase.probe.upstream)} className="w-full">
                Authorize
              </Button>
            </div>
          )}

          {phase.kind === 'no_oauth' && (
            <div className="rounded-lg border p-4 flex flex-col gap-2">
              <div className="flex items-center gap-2 text-sm text-aurora-text-muted">
                <Link2 className="size-4" />
                No OAuth required
              </div>
              <p className="text-xs text-aurora-text-muted">
                This server is reachable but does not advertise OAuth. Configure a bearer token in the gateway settings if it requires authentication.
              </p>
            </div>
          )}

          {phase.kind === 'authorizing' && (
            <div className="flex items-center gap-2 text-sm text-aurora-text-muted">
              <Loader2 className="size-4 animate-spin" />
              Complete authorization in the new tab…
            </div>
          )}

          {phase.kind === 'error' && phase.message && (
            <div className="flex items-start gap-2 text-sm text-destructive">
              <AlertCircle className="size-4 mt-0.5 shrink-0" />
              {phase.message}
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}
