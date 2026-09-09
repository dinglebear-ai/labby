'use client'

import * as React from 'react'
import { toast } from 'sonner'
import { Check, Globe2, Loader2, MonitorSmartphone, RefreshCw, ShieldCheck, Unplug, Wrench } from 'lucide-react'

import { ActionConfirmationDialog } from '@/components/action-confirmation-dialog'
import { AppHeader } from '@/components/app-header'
import { ConsoleHero } from '@/components/console/console-hero'
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { Switch } from '@/components/ui/switch'
import { AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
import { browserApi } from '@/lib/api/browser-client'
import { formatUiDateTime, formatUiRelativeTime } from '@/lib/format-ui-time'
import type { BrowserIdentity, BrowserPairing, BrowserSession } from '@/lib/types/browser'
import { cn } from '@/lib/utils'

type BrowserData = {
  browsers: BrowserIdentity[]
  pairings: BrowserPairing[]
  sessions: BrowserSession[]
}

const POLL_INTERVAL_MS = 5_000

function browserName(browsers: BrowserIdentity[], id: string): string {
  return browsers.find((browser) => browser.id === id)?.display_name ?? 'Unknown browser'
}

function pageLabel(session: BrowserSession): string {
  return session.page_title.trim() || `${session.origin}${session.sanitized_path}`
}

export function BrowserBridgePage() {
  const [data, setData] = React.useState<BrowserData>({ browsers: [], pairings: [], sessions: [] })
  const [loading, setLoading] = React.useState(true)
  const [refreshing, setRefreshing] = React.useState(false)
  const [error, setError] = React.useState<string>()
  const [busyKey, setBusyKey] = React.useState<string>()
  const [revokeTarget, setRevokeTarget] = React.useState<BrowserIdentity>()
  const loadGeneration = React.useRef(0)
  const mutationPending = React.useRef(false)

  const load = React.useCallback(async (signal?: AbortSignal, announce = false) => {
    const generation = ++loadGeneration.current
    if (announce) setRefreshing(true)
    try {
      const [browsers, pairings, sessions] = await Promise.all([
        browserApi.list(signal), browserApi.pairings(signal), browserApi.sessions(signal),
      ])
      if (generation === loadGeneration.current) {
        setData({ browsers, pairings, sessions })
        setError(undefined)
      }
      return true
    } catch (cause) {
      if (signal?.aborted || generation !== loadGeneration.current) return false
      setError(cause instanceof Error ? cause.message : 'Browser bridge state could not be loaded.')
      return false
    } finally {
      if (!signal?.aborted && generation === loadGeneration.current) {
        setLoading(false)
        setRefreshing(false)
      }
    }
  }, [])

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

  const activeSessions = data.sessions.filter((session) => session.status === 'active')
  const connected = data.browsers.filter((browser) => browser.connected && !browser.revoked_at)
  const enabled = activeSessions.filter((session) => session.enabled)

  return (
    <>
      <AppHeader breadcrumbs={[{ label: 'Control Plane' }, { label: 'Browsers' }]} />
      <main className={cn(AURORA_PAGE_SHELL, AURORA_PAGE_FRAME)}>
      <ConsoleHero
        eyebrow="Browser-native WebMCP"
        title="Browser bridges"
        description="Paired extension identities and the WebMCP pages they observe. Discovery is metadata-only; execution stays disabled until you enable the exact active document."
        pulse={connected.length > 0 ? { color: 'var(--aurora-success)', label: `${connected.length} connected` } : undefined}
        actions={<Button variant="outline" size="sm" onClick={() => void load(undefined, true)} disabled={refreshing}><RefreshCw className={cn(refreshing && 'animate-spin')} />Refresh</Button>}
        stats={[
          { label: 'Paired', value: data.browsers.filter((browser) => !browser.revoked_at).length, icon: <MonitorSmartphone size={14} /> },
          { label: 'Pending', value: data.pairings.length, icon: <ShieldCheck size={14} />, tone: data.pairings.length ? 'var(--aurora-warn)' : undefined },
          { label: 'Pages', value: activeSessions.length, icon: <Globe2 size={14} /> },
          { label: 'Enabled', value: enabled.length, icon: <Wrench size={14} />, tone: enabled.length ? 'var(--aurora-success)' : undefined },
        ]}
      />

      {error ? <Alert variant="error"><Unplug /><AlertTitle>Browser bridge unavailable</AlertTitle><AlertDescription>{error}<Button variant="outline" size="sm" onClick={() => void load(undefined, true)}>Try again</Button></AlertDescription></Alert> : null}

      {data.pairings.length > 0 ? (
        <section aria-labelledby="pending-pairings-heading" className="overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-strong shadow-aurora-medium">
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-aurora-border-default bg-aurora-page-bg/30 px-[15px] py-2.5">
            <h2 id="pending-pairings-heading" className="text-[9.5px] font-bold uppercase tracking-[0.13em] text-aurora-text-muted">Pending pairing requests</h2>
            <p className="text-[11px] text-aurora-text-muted">Approve only extension identities you initiated from a browser you control.</p>
          </div>
          <div className="grid grid-cols-[repeat(auto-fit,minmax(min(100%,300px),1fr))] gap-2.5 p-3">
            {data.pairings.map((pairing) => (
              <div key={pairing.id} className="flex min-w-0 items-center gap-[11px] rounded-xl border border-aurora-warn/30 bg-aurora-warn/8 px-3.5 py-3">
                <ShieldCheck className="size-[18px] shrink-0 text-aurora-warn" />
                <div className="min-w-0 flex-1">
                  <div className="text-[12.5px] font-[650] text-aurora-text-primary">{pairing.display_name}</div>
                  <div className="mt-0.5 truncate text-[10.5px] text-aurora-text-muted" title={pairing.extension_id}>{pairing.extension_id}</div>
                  <div className="mt-0.5 text-[10.5px] text-aurora-text-muted">Expires {formatUiRelativeTime(pairing.expires_at * 1000)}</div>
                </div>
                <Button data-visible-label="1" variant="outline" size="sm" className="h-[30px] shrink-0 gap-1.5 rounded-lg border-aurora-accent-primary/55 bg-aurora-accent-primary/10 px-3 text-xs font-[650] text-aurora-accent-strong [&>svg]:size-3" onClick={() => void mutate(`pair:${pairing.id}`, () => browserApi.approvePairing(pairing.id), `${pairing.display_name} paired`)} disabled={Boolean(busyKey)}>
                  {busyKey === `pair:${pairing.id}` ? <Loader2 className="animate-spin" /> : <Check />}Approve
                </Button>
              </div>
            ))}
          </div>
        </section>
      ) : null}

      <section aria-labelledby="paired-browsers-heading" className="overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-strong shadow-aurora-medium">
        <div className="border-b border-aurora-border-default bg-aurora-page-bg/30 px-[15px] py-2.5">
          <div className="flex flex-wrap items-center justify-between gap-2"><h2 id="paired-browsers-heading" className="text-[9.5px] font-bold uppercase tracking-[0.13em] text-aurora-text-muted">Paired browsers</h2><p className="text-[11px] text-aurora-text-muted">Durable extension identities and their connection state</p></div>
        </div>
        {loading ? <LoadingPanel label="Loading paired browsers" /> : data.browsers.length === 0 ? <EmptyPanel icon={<MonitorSmartphone />} title="No paired browsers" description="Open the Labby Browser Bridge extension and send a pairing request. It will appear here for approval." /> : (
          <div className="grid grid-cols-[repeat(auto-fit,minmax(min(100%,320px),1fr))] gap-2.5 p-3">
            {data.browsers.map((browser) => (
              <Card key={browser.id} className={cn('gap-2.5 rounded-xl bg-aurora-page-bg/40 py-[13px]', browser.revoked_at && 'opacity-65')}>
                <CardHeader className="px-3.5 py-0">
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0"><CardTitle className="text-sm font-[760]">{browser.display_name}</CardTitle><CardDescription className="mt-0.5 truncate text-[10.5px]" title={browser.extension_id}>{browser.extension_id}</CardDescription></div>
                    <Badge variant="pill" status={browser.revoked_at ? 'error' : browser.connected ? 'success' : 'warn'}>{browser.revoked_at ? 'Revoked' : browser.connected ? 'Connected' : 'Offline'}</Badge>
                  </div>
                </CardHeader>
                <CardContent className="grid gap-2.5 px-3.5 py-0 text-[11.5px] sm:grid-cols-[1fr_auto] sm:items-end">
                  <dl className="grid gap-1 text-aurora-text-muted"><div><dt className="inline font-medium text-aurora-text-primary">Paired: </dt><dd className="inline">{formatUiDateTime(browser.paired_at * 1000)}</dd></div><div><dt className="inline font-medium text-aurora-text-primary">Last seen: </dt><dd className="inline">{browser.last_seen_at ? formatUiRelativeTime(browser.last_seen_at * 1000) : 'Never'}</dd></div></dl>
                  {!browser.revoked_at ? <Button variant="outline" size="sm" disabled={Boolean(busyKey)} onClick={() => setRevokeTarget(browser)}>Revoke</Button> : null}
                </CardContent>
              </Card>
            ))}
          </div>
        )}
      </section>

      <section aria-labelledby="browser-pages-heading" className="overflow-hidden rounded-aurora-2 border border-aurora-border-default bg-aurora-panel-strong shadow-aurora-medium">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-aurora-border-default bg-aurora-page-bg/30 px-[15px] py-2.5"><h2 id="browser-pages-heading" className="text-[9.5px] font-bold uppercase tracking-[0.13em] text-aurora-text-muted">Observed pages and tools</h2><p className="text-[11px] text-aurora-text-muted">Discovery is metadata-only until execution is enabled per document</p></div>
        {loading ? <LoadingPanel label="Loading observed browser pages" /> : activeSessions.length === 0 ? <EmptyPanel icon={<Globe2 />} title="No WebMCP pages observed" description="Grant the extension access to a WebMCP-enabled page. Catalog metadata will appear after the next scan." /> : (
          <div className="grid divide-y divide-aurora-border-subtle">
            {activeSessions.map((session) => (
              <Card key={session.id} className="gap-2.5 rounded-none border-0 bg-transparent py-[13px] shadow-none">
                <CardHeader className="px-[15px] py-0">
                  <div className="flex flex-wrap items-start justify-between gap-4">
                    <div className="min-w-0"><CardTitle className="text-sm font-[760]">{pageLabel(session)}</CardTitle><CardDescription className="mt-0.5 break-all text-[11px]">{session.origin}{session.sanitized_path} · {browserName(data.browsers, session.browser_id)}</CardDescription></div>
                    <label className="flex items-center gap-2 text-sm font-medium text-aurora-text-primary"><span>{session.enabled ? 'Execution enabled' : 'Execution disabled'}</span><Switch aria-label={`Enable tool execution for ${pageLabel(session)}`} checked={session.enabled} disabled={Boolean(busyKey)} onCheckedChange={(checked) => void mutate(`session:${session.id}`, () => browserApi.setSessionEnabled(session.id, checked), `${pageLabel(session)} execution ${checked ? 'enabled' : 'disabled'}`)} /></label>
                  </div>
                </CardHeader>
                <CardContent className="px-[15px] py-0">
                  <div className="mb-3 flex flex-wrap items-center gap-2"><BrowserMetadataBadge>Tab {session.tab_id}</BrowserMetadataBadge><BrowserMetadataBadge>Revision {session.catalog_revision}</BrowserMetadataBadge><BrowserMetadataBadge enabled={session.enabled}>{session.tools.length} tool{session.tools.length === 1 ? '' : 's'}</BrowserMetadataBadge><span className="text-[10.5px] text-aurora-text-muted">Seen {formatUiRelativeTime(session.last_seen_at * 1000)}</span></div>
                  <div className="grid grid-cols-[repeat(auto-fill,minmax(min(100%,220px),1fr))] gap-2">{session.tools.map((tool) => <div key={tool.name} className="min-w-0 rounded-[10px] border border-aurora-border-default bg-aurora-control-surface px-3 py-2.5"><div className="truncate font-mono text-[11.5px] font-semibold text-aurora-accent-strong" title={tool.name}>{tool.name}</div><p className="mt-1 text-pretty text-[11px] leading-[1.5] text-aurora-text-muted">{tool.description || 'No description provided by the page.'}</p></div>)}</div>
                </CardContent>
              </Card>
            ))}
          </div>
        )}
      </section>

      <ActionConfirmationDialog open={Boolean(revokeTarget)} title="Revoke browser identity?" description={`This disconnects ${revokeTarget?.display_name ?? 'the browser'}, disables its active page sessions, and requires a new pairing before it can reconnect.`} confirmLabel="Revoke browser" busy={Boolean(busyKey)} onOpenChange={(open) => { if (!open) setRevokeTarget(undefined) }} onConfirm={() => { if (!revokeTarget) return; const target = revokeTarget; void mutate(`revoke:${target.id}`, () => browserApi.revoke(target.id), `${target.display_name} revoked`).then((succeeded) => { if (succeeded) setRevokeTarget(undefined) }) }} />
      </main>
    </>
  )
}

function BrowserMetadataBadge({ children, enabled = false }: { children: React.ReactNode; enabled?: boolean }) {
  const tone = enabled ? 'var(--aurora-success)' : 'var(--aurora-text-muted)'
  return <span className="inline-flex h-[18px] items-center whitespace-nowrap rounded px-[7px] text-[9px] font-bold uppercase tracking-[.1em]" style={{ color: tone, background: `color-mix(in srgb, ${tone} 11%, transparent)`, border: `1px solid color-mix(in srgb, ${tone} 30%, transparent)` }}>{children}</span>
}

function LoadingPanel({ label }: { label: string }) {
  return <div className="flex min-h-36 items-center justify-center rounded-aurora-3 border border-aurora-border-default bg-aurora-panel-medium text-sm text-aurora-text-muted"><Loader2 className="mr-2 size-4 animate-spin" />{label}</div>
}

function EmptyPanel({ icon, title, description }: { icon: React.ReactNode; title: string; description: string }) {
  return <Empty className="border border-aurora-border-default bg-aurora-panel-medium"><EmptyHeader><EmptyMedia variant="icon">{icon}</EmptyMedia><EmptyTitle>{title}</EmptyTitle><EmptyDescription>{description}</EmptyDescription></EmptyHeader></Empty>
}
