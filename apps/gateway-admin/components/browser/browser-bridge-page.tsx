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
import { AURORA_CARD_TITLE, AURORA_DENSE_META, AURORA_PAGE_FRAME, AURORA_PAGE_SHELL } from '@/components/aurora/tokens'
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

  const load = React.useCallback(async (signal?: AbortSignal, announce = false) => {
    if (announce) setRefreshing(true)
    try {
      const [browsers, pairings, sessions] = await Promise.all([
        browserApi.list(signal), browserApi.pairings(signal), browserApi.sessions(signal),
      ])
      setData({ browsers, pairings, sessions })
      setError(undefined)
    } catch (cause) {
      if (signal?.aborted) return
      setError(cause instanceof Error ? cause.message : 'Browser bridge state could not be loaded.')
    } finally {
      if (!signal?.aborted) {
        setLoading(false)
        setRefreshing(false)
      }
    }
  }, [])

  React.useEffect(() => {
    const controller = new AbortController()
    void load(controller.signal)
    const timer = window.setInterval(() => void load(controller.signal), POLL_INTERVAL_MS)
    return () => { controller.abort(); window.clearInterval(timer) }
  }, [load])

  async function mutate(key: string, operation: () => Promise<unknown>, success: string) {
    setBusyKey(key)
    try {
      await operation()
      toast.success(success)
      await load()
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : 'The browser operation failed.')
    } finally {
      setBusyKey(undefined)
    }
  }

  const activeSessions = data.sessions.filter((session) => session.status === 'active')
  const connected = data.browsers.filter((browser) => browser.connected && !browser.revoked_at)
  const enabled = activeSessions.filter((session) => session.enabled)

  return (
    <div className={cn(AURORA_PAGE_SHELL, AURORA_PAGE_FRAME)}>
      <AppHeader breadcrumbs={[{ label: 'Control Plane' }, { label: 'Browsers' }]} />
      <ConsoleHero
        eyebrow="Browser-native WebMCP"
        title="Browser bridges"
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
        <Card variant="strong">
          <CardHeader className="border-b border-aurora-border-default/70">
            <CardTitle className={AURORA_CARD_TITLE}>Pending pairing requests</CardTitle>
            <CardDescription>Approve only extension identities you initiated from a browser you control.</CardDescription>
          </CardHeader>
          <CardContent className="grid gap-3 pb-6 md:grid-cols-2">
            {data.pairings.map((pairing) => (
              <div key={pairing.id} className="flex min-w-0 items-center gap-3 rounded-aurora-2 border border-aurora-warn/30 bg-aurora-warn/8 p-4">
                <ShieldCheck className="size-5 shrink-0 text-aurora-warn" />
                <div className="min-w-0 flex-1">
                  <div className="font-medium text-aurora-text-primary">{pairing.display_name}</div>
                  <div className={cn(AURORA_DENSE_META, 'truncate font-mono text-aurora-text-muted')} title={pairing.extension_id}>{pairing.extension_id}</div>
                  <div className={cn(AURORA_DENSE_META, 'text-aurora-text-muted')}>Expires {formatUiRelativeTime(pairing.expires_at * 1000)}</div>
                </div>
                <Button size="sm" onClick={() => void mutate(`pair:${pairing.id}`, () => browserApi.approvePairing(pairing.id), `${pairing.display_name} paired`)} disabled={Boolean(busyKey)}>
                  {busyKey === `pair:${pairing.id}` ? <Loader2 className="animate-spin" /> : <Check />}Approve
                </Button>
              </div>
            ))}
          </CardContent>
        </Card>
      ) : null}

      <section aria-labelledby="paired-browsers-heading">
        <div className="mb-3 flex items-end justify-between gap-3">
          <div><h2 id="paired-browsers-heading" className="font-display text-[19px] leading-[1.12] font-bold text-aurora-text-primary">Paired browsers</h2><p className="mt-1 text-sm text-aurora-text-muted">Durable extension identities and their current connection state.</p></div>
        </div>
        {loading ? <LoadingPanel label="Loading paired browsers" /> : data.browsers.length === 0 ? <EmptyPanel icon={<MonitorSmartphone />} title="No paired browsers" description="Open the Labby Browser Bridge extension and send a pairing request. It will appear here for approval." /> : (
          <div className="grid gap-3 lg:grid-cols-2">
            {data.browsers.map((browser) => (
              <Card key={browser.id} className={cn(browser.revoked_at && 'opacity-65')}>
                <CardHeader className="border-b border-aurora-border-default/60">
                  <div className="flex items-start justify-between gap-3">
                    <div className="min-w-0"><CardTitle className={AURORA_CARD_TITLE}>{browser.display_name}</CardTitle><CardDescription className="mt-1 truncate font-mono" title={browser.extension_id}>{browser.extension_id}</CardDescription></div>
                    <Badge variant="pill" status={browser.revoked_at ? 'error' : browser.connected ? 'success' : 'warn'}>{browser.revoked_at ? 'Revoked' : browser.connected ? 'Connected' : 'Offline'}</Badge>
                  </div>
                </CardHeader>
                <CardContent className="grid gap-3 pb-6 text-sm sm:grid-cols-[1fr_auto] sm:items-end">
                  <dl className="grid gap-1 text-aurora-text-muted"><div><dt className="inline font-medium text-aurora-text-primary">Paired: </dt><dd className="inline">{formatUiDateTime(browser.paired_at * 1000)}</dd></div><div><dt className="inline font-medium text-aurora-text-primary">Last seen: </dt><dd className="inline">{browser.last_seen_at ? formatUiRelativeTime(browser.last_seen_at * 1000) : 'Never'}</dd></div></dl>
                  {!browser.revoked_at ? <Button variant="outline" size="sm" onClick={() => setRevokeTarget(browser)}>Revoke</Button> : null}
                </CardContent>
              </Card>
            ))}
          </div>
        )}
      </section>

      <section aria-labelledby="browser-pages-heading">
        <div className="mb-3"><h2 id="browser-pages-heading" className="font-display text-[19px] leading-[1.12] font-bold text-aurora-text-primary">Observed pages and tools</h2><p className="mt-1 text-sm text-aurora-text-muted">Discovery is metadata-only. Execution remains disabled until you enable the exact active document below.</p></div>
        {loading ? <LoadingPanel label="Loading observed browser pages" /> : activeSessions.length === 0 ? <EmptyPanel icon={<Globe2 />} title="No WebMCP pages observed" description="Grant the extension access to a WebMCP-enabled page. Catalog metadata will appear after the next scan." /> : (
          <div className="grid gap-3">
            {activeSessions.map((session) => (
              <Card key={session.id}>
                <CardHeader className="border-b border-aurora-border-default/60">
                  <div className="flex flex-wrap items-start justify-between gap-4">
                    <div className="min-w-0"><CardTitle className={AURORA_CARD_TITLE}>{pageLabel(session)}</CardTitle><CardDescription className="mt-1 break-all">{session.origin}{session.sanitized_path} · {browserName(data.browsers, session.browser_id)}</CardDescription></div>
                    <label className="flex items-center gap-2 text-sm font-medium text-aurora-text-primary"><span>{session.enabled ? 'Execution enabled' : 'Execution disabled'}</span><Switch aria-label={`Enable tool execution for ${pageLabel(session)}`} checked={session.enabled} disabled={Boolean(busyKey)} onCheckedChange={(checked) => void mutate(`session:${session.id}`, () => browserApi.setSessionEnabled(session.id, checked), `${pageLabel(session)} execution ${checked ? 'enabled' : 'disabled'}`)} /></label>
                  </div>
                </CardHeader>
                <CardContent className="pb-6">
                  <div className="mb-3 flex flex-wrap gap-2"><Badge variant="outline">Tab {session.tab_id}</Badge><Badge variant="outline">Revision {session.catalog_revision}</Badge><Badge variant="outline" status={session.enabled ? 'success' : 'default'}>{session.tools.length} tool{session.tools.length === 1 ? '' : 's'}</Badge><span className={cn(AURORA_DENSE_META, 'self-center text-aurora-text-muted')}>Seen {formatUiRelativeTime(session.last_seen_at * 1000)}</span></div>
                  <div className="grid gap-2 md:grid-cols-2 xl:grid-cols-3">{session.tools.map((tool) => <div key={tool.name} className="rounded-aurora-2 border border-aurora-border-default bg-aurora-control-surface p-3"><div className="font-mono text-xs font-semibold text-aurora-accent-strong">{tool.name}</div><p className="mt-1 line-clamp-2 text-xs leading-relaxed text-aurora-text-muted">{tool.description || 'No description provided by the page.'}</p></div>)}</div>
                </CardContent>
              </Card>
            ))}
          </div>
        )}
      </section>

      <ActionConfirmationDialog open={Boolean(revokeTarget)} title="Revoke browser identity?" description={`This disconnects ${revokeTarget?.display_name ?? 'the browser'}, disables its active page sessions, and requires a new pairing before it can reconnect.`} confirmLabel="Revoke browser" busy={busyKey?.startsWith('revoke:')} onOpenChange={(open) => { if (!open) setRevokeTarget(undefined) }} onConfirm={() => { if (!revokeTarget) return; const target = revokeTarget; void mutate(`revoke:${target.id}`, () => browserApi.revoke(target.id), `${target.display_name} revoked`).then(() => setRevokeTarget(undefined)) }} />
    </div>
  )
}

function LoadingPanel({ label }: { label: string }) {
  return <div className="flex min-h-36 items-center justify-center rounded-aurora-3 border border-aurora-border-default bg-aurora-panel-medium text-sm text-aurora-text-muted"><Loader2 className="mr-2 size-4 animate-spin" />{label}</div>
}

function EmptyPanel({ icon, title, description }: { icon: React.ReactNode; title: string; description: string }) {
  return <Empty className="border border-aurora-border-default bg-aurora-panel-medium"><EmptyHeader><EmptyMedia variant="icon">{icon}</EmptyMedia><EmptyTitle>{title}</EmptyTitle><EmptyDescription>{description}</EmptyDescription></EmptyHeader></Empty>
}
