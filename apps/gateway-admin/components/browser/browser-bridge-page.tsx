'use client'

import * as React from 'react'
import { toast } from 'sonner'
import { Check, Globe2, Loader2, MonitorSmartphone, RefreshCw, ShieldCheck, Unplug } from 'lucide-react'

import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { AURORA_DENSE_META, AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { browserApi } from '@/lib/api/browser-client'
import { formatUiDateTime, formatUiRelativeTime } from '@/lib/format-ui-time'
import type { BrowserIdentity, BrowserPairing, BrowserSession } from '@/lib/types/browser'
import { cn } from '@/lib/utils'

type BrowserData = {
  browsers: BrowserIdentity[]
  pairings: BrowserPairing[]
  sessions: BrowserSession[]
  sessionNextCursor: string | null
}

const POLL_INTERVAL_MS = 5_000
const PAIRING_FINGERPRINT_LENGTH = 12

function normalizePairingFingerprint(value: string): string {
  return value.toUpperCase().replace(/[^0-9A-F]/g, '').slice(0, PAIRING_FINGERPRINT_LENGTH)
}

function validPairingFingerprint(value: string): boolean {
  return new RegExp(`^[0-9A-F]{${PAIRING_FINGERPRINT_LENGTH}}$`).test(value)
}

function browserName(browsers: BrowserIdentity[], id: string): string {
  return browsers.find((browser) => browser.id === id)?.display_name ?? 'Unknown browser'
}

function pageLabel(session: BrowserSession): string {
  return session.page_title.trim() || `${session.origin}${session.sanitized_path}`
}

export function BrowserBridgePage() {
  const [data, setData] = React.useState<BrowserData>({ browsers: [], pairings: [], sessions: [], sessionNextCursor: null })
  const [sessionCursors, setSessionCursors] = React.useState<Array<string | undefined>>([undefined])
  const sessionCursor = sessionCursors.at(-1)
  const [loading, setLoading] = React.useState(true)
  const [refreshing, setRefreshing] = React.useState(false)
  const [error, setError] = React.useState<string>()
  const [warnings, setWarnings] = React.useState<string[]>([])
  const [availability, setAvailability] = React.useState({ browsers: false, pairings: false, sessions: false })
  const [busyKey, setBusyKey] = React.useState<string>()
  const [pairingFingerprints, setPairingFingerprints] = React.useState<Record<string, string>>({})
  const [revokeTarget, setRevokeTarget] = React.useState<BrowserIdentity>()
  const loadGeneration = React.useRef(0)
  const mutationPending = React.useRef(false)

  const load = React.useCallback(async (signal?: AbortSignal, announce = false) => {
    const generation = ++loadGeneration.current
    if (announce) setRefreshing(true)
    try {
      const [browserResult, pairingResult, sessionResult] = await Promise.allSettled([
        browserApi.list(signal), browserApi.pairings(signal), browserApi.sessions(signal, sessionCursor),
      ])
      if (signal?.aborted || generation !== loadGeneration.current) return false

      const issue = (label: string, result: PromiseSettledResult<unknown>) =>
        result.status === 'rejected'
          ? `${label}: ${result.reason instanceof Error ? result.reason.message : 'request failed'}`
          : undefined
      const nextWarnings = [
        issue('paired browsers unavailable', browserResult),
        issue('pairing requests unavailable', pairingResult),
        issue('browser sessions unavailable', sessionResult),
        ...(sessionResult.status === 'fulfilled' ? sessionResult.value.detail_warnings : []),
      ].filter((message): message is string => Boolean(message))
      const allFailed = nextWarnings.length === 3

      setData((current) => ({
        browsers: browserResult.status === 'fulfilled' ? browserResult.value : current.browsers,
        pairings: pairingResult.status === 'fulfilled' ? pairingResult.value : current.pairings,
        sessions: sessionResult.status === 'fulfilled' ? sessionResult.value.sessions : current.sessions,
        sessionNextCursor: sessionResult.status === 'fulfilled' ? sessionResult.value.next_cursor : current.sessionNextCursor,
      }))
      setAvailability((current) => ({
        browsers: browserResult.status === 'fulfilled' || current.browsers,
        pairings: pairingResult.status === 'fulfilled' || current.pairings,
        sessions: sessionResult.status === 'fulfilled' || current.sessions,
      }))
      setWarnings(allFailed ? [] : nextWarnings)
      setError(allFailed ? `Browser bridge state could not be loaded. ${nextWarnings.join('; ')}` : undefined)
      return !allFailed
    } finally {
      if (!signal?.aborted && generation === loadGeneration.current) {
        setLoading(false)
        setRefreshing(false)
      }
    }
  }, [sessionCursor])

  React.useEffect(() => {
    const controller = new AbortController()
    let timer: number | undefined
    const poll = async () => {
      await load(controller.signal)
      if (!controller.signal.aborted) timer = window.setTimeout(() => void poll(), POLL_INTERVAL_MS)
    }
    void poll()
    return () => { controller.abort(); if (timer) window.clearTimeout(timer) }
  }, [load])

  async function mutate(key: string, operation: () => Promise<unknown>, success: string) {
    if (mutationPending.current) return false
    mutationPending.current = true
    setBusyKey(key)
    try {
      await operation()
      toast.success(success)
      const refreshed = await load()
      if (!refreshed) toast.warning('The operation succeeded, but refreshed browser state could not be loaded.')
      return true
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : 'The browser operation failed.')
      return false
    } finally {
      setBusyKey(undefined)
      mutationPending.current = false
    }
  }

  function showPreviousSessionPage() {
    if (sessionCursors.length <= 1) return
    setLoading(true)
    setSessionCursors((current) => current.slice(0, -1))
  }

  function showNextSessionPage() {
    if (!data.sessionNextCursor) return
    setLoading(true)
    const cursor = data.sessionNextCursor
    setSessionCursors((current) => [...current, cursor])
  }

  const activeSessions = data.sessions.filter((session) => session.status === 'active')
  const connected = data.browsers.filter((browser) => browser.connected && !browser.revoked_at)
  const enabled = activeSessions.filter((session) => session.enabled)

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Control Plane' }, { label: 'Browsers' }]} />
      <div className={cn(AURORA_PAGE_SHELL, AURORA_PAGE_FRAME)} style={{ gap: 14, lineHeight: 'normal' }}>
      <ConsoleHero
        eyebrow="Browser-native WebMCP"
        title="Browser bridges"
        description="Paired extension identities and the WebMCP pages they observe. Discovery is metadata-only; execution stays disabled until you enable the exact active document."
        pulse={{ color: availability.browsers && connected.length > 0 ? 'var(--aurora-success)' : 'var(--aurora-warn)', label: availability.browsers ? `${connected.length} connected` : 'browser state unavailable' }}
        actions={<Button variant="outline" size="icon" aria-label="Refresh browser bridge" title="Refresh browser bridge" className="size-9 rounded-[10px] text-aurora-text-muted" onClick={() => void load(undefined, true)} disabled={refreshing}><RefreshCw className={cn('size-[15px]', refreshing && 'animate-spin')} /></Button>}
        stats={[
          { label: 'Paired', value: availability.browsers ? data.browsers.filter((browser) => !browser.revoked_at).length : '—' },
          { label: 'Pending', value: availability.pairings ? data.pairings.length : '—', tone: availability.pairings && data.pairings.length ? 'var(--aurora-warn)' : undefined },
          { label: 'Pages shown', value: availability.sessions ? activeSessions.length : '—' },
          { label: 'Enabled shown', value: availability.sessions ? enabled.length : '—', tone: availability.sessions && enabled.length ? 'var(--aurora-success)' : undefined },
        ]}
      />

      {error ? <Alert variant="error"><Unplug /><AlertTitle>Browser bridge unavailable</AlertTitle><AlertDescription>{error}<Button variant="outline" size="sm" onClick={() => void load(undefined, true)}>Try again</Button></AlertDescription></Alert> : null}
      {warnings.length > 0 ? (
        <Alert>
          <Unplug />
          <AlertTitle>Browser bridge is partially degraded</AlertTitle>
          <AlertDescription>Available browser data remains usable. {warnings.join('; ')}</AlertDescription>
        </Alert>
      ) : null}

      {availability.pairings && data.pairings.length > 0 ? (
        <BrowserPanel title="Pending pairing requests" description="Approve only identities you initiated from a browser you control.">
          <div className="grid gap-2.5 p-3 md:grid-cols-2">
            {data.pairings.map((pairing) => {
              const fingerprint = pairingFingerprints[pairing.id] ?? ''
              const canApprove = validPairingFingerprint(fingerprint)
              return (
                <div key={pairing.id} className="grid min-w-0 gap-[11px] rounded-xl border border-aurora-warn/30 bg-aurora-warn/8 px-3.5 py-3 sm:grid-cols-[auto_minmax(0,1fr)]">
                  <ShieldCheck className="size-5 shrink-0 text-aurora-warn" />
                  <div className="min-w-0">
                    <div className="text-[12.5px] font-semibold text-aurora-text-primary">{pairing.display_name}</div>
                    <div className={cn(AURORA_DENSE_META, 'truncate font-mono text-aurora-text-muted')} title={pairing.extension_id}>{pairing.extension_id}</div>
                    <div className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>Expires {formatUiRelativeTime(pairing.expires_at * 1000)}</div>
                    <div className="mt-2 flex flex-col gap-2 sm:flex-row sm:items-end">
                      <label className="min-w-0 flex-1 text-[10.5px] font-medium text-aurora-text-primary">
                        <span>Fingerprint from extension</span>
                        <Input
                          aria-label={`Pairing fingerprint for ${pairing.display_name}`}
                          autoComplete="off"
                          className="mt-1 h-[30px] rounded-lg font-mono text-xs uppercase"
                          inputMode="text"
                          maxLength={PAIRING_FINGERPRINT_LENGTH}
                          placeholder="A1B2C3D4E5F6"
                          spellCheck={false}
                          value={fingerprint}
                          onChange={(event) => setPairingFingerprints((current) => ({
                            ...current,
                            [pairing.id]: normalizePairingFingerprint(event.target.value),
                          }))}
                        />
                      </label>
                      <Button size="sm" className="h-[30px] rounded-lg px-3 text-xs" onClick={() => void mutate(`pair:${pairing.id}`, () => browserApi.approvePairing(pairing.id, fingerprint), `${pairing.display_name} paired`)} disabled={Boolean(busyKey) || !canApprove}>
                        {busyKey === `pair:${pairing.id}` ? <Loader2 className="animate-spin" /> : <Check />}Approve
                      </Button>
                    </div>
                  </div>
                </div>
              )
            })}
          </div>
        </BrowserPanel>
      ) : null}

      <BrowserPanel title="Paired browsers" description="Durable extension identities and their connection state">
        {loading ? <LoadingPanel label="Loading paired browsers" /> : !availability.browsers ? <EmptyPanel icon={<Unplug />} title="Paired browser state unavailable" description="This part of Browser Bridge could not be loaded. Other bridge capabilities remain usable; retry when the browser identity service recovers." /> : data.browsers.length === 0 ? <EmptyPanel icon={<MonitorSmartphone />} title="No paired browsers" description="Open the Labby Browser Bridge extension and send a pairing request. It will appear here for approval." /> : (
          <div className="grid gap-2.5 p-3" style={{ gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 320px), 1fr))' }}>
            {data.browsers.map((browser) => (
              <Card key={browser.id} className={cn('gap-2.5 rounded-xl px-3.5 py-[13px] shadow-none', browser.revoked_at && 'opacity-65')} style={{ background: 'var(--gw0-0_40)' }}>
                <CardHeader className="p-0">
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0"><CardTitle className="font-display text-sm font-[760]">{browser.display_name}</CardTitle><CardDescription className="mt-0.5 truncate text-[10.5px]" title={browser.extension_id}>{browser.extension_id}</CardDescription></div>
                    <Badge className="h-[18px] rounded px-[7px] text-[9px] uppercase tracking-widest" variant="pill" status={browser.revoked_at ? 'error' : browser.connected ? 'success' : 'warn'}><span aria-hidden="true" data-browser-connection-dot className="size-[5px] rounded-full bg-current" style={{ boxShadow: '0 0 4px currentColor' }} />{browser.revoked_at ? 'Revoked' : browser.connected ? 'Connected' : 'Offline'}</Badge>
                  </div>
                </CardHeader>
                <CardContent className="grid gap-2.5 p-0 text-[11.5px] sm:grid-cols-[1fr_auto] sm:items-end">
                  <dl className="grid gap-1 text-aurora-text-muted"><div><dt className="inline font-medium text-aurora-text-primary">Paired: </dt><dd className="inline">{formatUiDateTime(browser.paired_at * 1000)}</dd></div><div><dt className="inline font-medium text-aurora-text-primary">Last seen: </dt><dd className="inline">{browser.last_seen_at ? formatUiRelativeTime(browser.last_seen_at * 1000) : 'Never'}</dd></div></dl>
                  {!browser.revoked_at ? <Button variant="outline" size="sm" className="h-[26px] rounded-lg px-[11px] text-[11px]" disabled={Boolean(busyKey)} onClick={() => setRevokeTarget(browser)}>Revoke</Button> : null}
                </CardContent>
              </Card>
            ))}
          </div>
        )}
      </BrowserPanel>

      <BrowserPanel title="Observed pages and tools" description="Discovery is metadata-only until execution is enabled per document">
        {loading ? <LoadingPanel label="Loading observed browser pages" /> : !availability.sessions ? <EmptyPanel icon={<Unplug />} title="Browser session state unavailable" description="Observed pages could not be loaded. Paired-browser and pairing controls remain usable if their services are available." /> : activeSessions.length === 0 ? <EmptyPanel icon={<Globe2 />} title={sessionCursors.length > 1 || data.sessionNextCursor ? "No active WebMCP pages on this session page" : "No WebMCP pages observed"} description={sessionCursors.length > 1 || data.sessionNextCursor ? "Use the session-page controls to review older or newer observed pages." : "Grant the extension access to a WebMCP-enabled page. Catalog metadata will appear after the next scan."} /> : (
          <div>
            {activeSessions.map((session) => (
              <Card key={session.id} className="gap-2.5 rounded-none border-0 border-t px-[15px] py-[13px] shadow-none" style={{ background: 'transparent' }}>
                <CardHeader className="p-0">
                  <div className="flex flex-wrap items-start justify-between gap-4">
                    <div className="min-w-0"><CardTitle className="font-display text-sm font-[760]">{pageLabel(session)}</CardTitle><CardDescription className="mt-0.5 break-all text-[11px]">{session.origin}{session.sanitized_path} · {browserName(data.browsers, session.browser_id)}</CardDescription></div>
                    <label className="flex items-center gap-[9px] text-xs font-semibold text-aurora-text-primary"><span>{session.enabled ? 'Execution enabled' : 'Execution disabled'}</span><Switch aria-label={`Enable tool execution for ${pageLabel(session)}`} checked={session.enabled} disabled={Boolean(busyKey)} onCheckedChange={(checked) => void mutate(`session:${session.id}`, () => browserApi.setSessionEnabled(session.id, checked, session.catalog_digest), `${pageLabel(session)} execution ${checked ? 'enabled' : 'disabled'}`)} /></label>
                  </div>
                </CardHeader>
                <CardContent className="p-0">
                  <div className="mb-2.5 flex flex-wrap gap-1.5 [&_[data-slot=badge]]:h-[18px] [&_[data-slot=badge]]:rounded [&_[data-slot=badge]]:px-[7px] [&_[data-slot=badge]]:text-[9px]"><Badge variant="outline">Tab {session.tab_id}</Badge><Badge variant="outline">Revision {session.catalog_revision}</Badge><Badge variant="outline" status={session.enabled ? 'success' : 'default'}>{session.tools.length} tool{session.tools.length === 1 ? '' : 's'}</Badge><span className={cn(AURORA_DENSE_META, 'self-center text-aurora-text-muted')}>Seen {formatUiRelativeTime(session.last_seen_at * 1000)}</span></div>
                  <div className="grid gap-2" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(min(100%, 220px), 1fr))' }}>{session.tools.map((tool) => <div key={tool.name} className="rounded-aurora-2 border border-aurora-border-default bg-aurora-control-surface px-3 py-2.5"><div className="truncate font-mono text-[11.5px] font-semibold text-aurora-accent-strong">{tool.name}</div><p className="mt-1 text-[11px] leading-normal text-aurora-text-muted">{tool.description || 'No description provided by the page.'}</p></div>)}</div>
                </CardContent>
              </Card>
            ))}
          </div>
        )}
        {!loading && availability.sessions && (sessionCursors.length > 1 || data.sessionNextCursor) ? (
          <div className="flex items-center justify-between gap-3 px-[15px] py-3">
            <Button variant="outline" size="sm" onClick={showPreviousSessionPage} disabled={sessionCursors.length <= 1 || Boolean(busyKey)}>Previous pages</Button>
            <span className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>Session page {sessionCursors.length}</span>
            <Button variant="outline" size="sm" onClick={showNextSessionPage} disabled={!data.sessionNextCursor || Boolean(busyKey)}>Next pages</Button>
          </div>
        ) : null}
      </BrowserPanel>

      <ActionConfirmationDialog open={Boolean(revokeTarget)} title="Revoke browser identity?" description={`This disconnects ${revokeTarget?.display_name ?? 'the browser'}, disables its active page sessions, and requires a new pairing before it can reconnect.`} confirmLabel="Revoke browser" busy={Boolean(busyKey)} onOpenChange={(open) => { if (!open) setRevokeTarget(undefined) }} onConfirm={() => { if (!revokeTarget) return; const target = revokeTarget; void mutate(`revoke:${target.id}`, () => browserApi.revoke(target.id), `${target.display_name} revoked`).then((succeeded) => { if (succeeded) setRevokeTarget(undefined) }) }} />
    </div>
    </>
  )
}

function LoadingPanel({ label }: { label: string }) {
  return <div className="flex min-h-24 items-center justify-center text-xs text-aurora-text-muted"><Loader2 className="mr-2 size-4 animate-spin" />{label}</div>
}

function EmptyPanel({ icon, title, description }: { icon: React.ReactNode; title: string; description: string }) {
  return <div className="flex items-center gap-3 p-5 text-aurora-text-muted"><span className="shrink-0 [&>svg]:size-5">{icon}</span><div><h3 className="text-sm font-semibold text-aurora-text-primary">{title}</h3><p className="mt-1 text-xs leading-normal">{description}</p></div></div>
}

function BrowserPanel({ title, description, children }: { title: string; description: string; children: React.ReactNode }) {
  return <section aria-label={title} style={{ borderRadius: 'var(--radius-2)', border: '1px solid color-mix(in srgb, var(--aurora-border-default) 45%, var(--aurora-page-bg))', background: 'linear-gradient(180deg, var(--aurora-panel-strong-top), var(--aurora-panel-strong))', boxShadow: 'var(--aurora-shadow-medium), inset 0 1px 0 rgba(255,255,255,0.04)', overflow: 'hidden' }}>
    <div className="flex flex-wrap items-center gap-2 px-[15px] py-2.5" style={{ borderBottom: '1px solid color-mix(in srgb, var(--aurora-border-default) 55%, var(--aurora-page-bg))', background: 'var(--gw0-0_38)' }}><h2 className="text-[9.5px] font-bold uppercase tracking-[0.13em] text-aurora-text-muted">{title}</h2><span className="ml-auto text-[11px] text-aurora-text-muted">{description}</span></div>
    {children}
  </section>
}
